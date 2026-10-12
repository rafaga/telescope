//! macOS side of the game window lookup.
//!
//! Where the window is comes from CoreGraphics' window list (position and
//! size, plus the owner's name and process id), and whether the game is the
//! frontmost application from `NSWorkspace`. Neither needs a permission: the
//! window *titles* would need Screen Recording, so the game is recognised by
//! the name of its process instead.

use super::{GameWindow, GameWindowTracker, ScreenRect};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
    fn CGShieldingWindowLevel() -> i32;
}

type WindowInfo = NSDictionary<NSString, AnyObject>;

/// Reads the on-screen windows. The keys are the values of the CoreGraphics
/// `kCGWindow*` constants.
pub(super) struct MacWindowList;

impl MacWindowList {
    /// `kCGWindowListOptionOnScreenOnly`.
    const ON_SCREEN_ONLY: u32 = 1;
    /// `kCGWindowListExcludeDesktopElements`.
    const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

    fn text(entry: &WindowInfo, key: &str) -> Option<String> {
        let value = entry.objectForKey(&NSString::from_str(key))?;
        let text = value.downcast::<NSString>().ok()?;
        Some(text.to_string())
    }

    fn number(entry: &WindowInfo, key: &str) -> Option<f64> {
        let value = entry.objectForKey(&NSString::from_str(key))?;
        let number = value.downcast::<NSNumber>().ok()?;
        Some(number.doubleValue())
    }

    /// The window level of the overlay: above the "shielding" level the system
    /// uses for a game that captured the display, so the overlay is also
    /// visible there (a normal full screen app is a Space of its own, which
    /// the window's collection behaviour already covers).
    #[allow(unsafe_code)]
    pub(super) fn overlay_level() -> isize {
        // SAFETY: a plain query with no arguments.
        let shielding = unsafe { CGShieldingWindowLevel() };
        isize::try_from(shielding).unwrap_or(isize::MAX).saturating_add(1)
    }

    /// The on-screen windows, front to back.
    #[allow(unsafe_code)]
    fn windows() -> Option<Retained<NSArray<WindowInfo>>> {
        // SAFETY: the "Copy" function returns a +1 `CFArray`, which is
        // toll-free bridged to `NSArray`; `from_raw` takes that ownership and
        // returns `None` for null.
        unsafe {
            let list = CGWindowListCopyWindowInfo(
                Self::ON_SCREEN_ONLY | Self::EXCLUDE_DESKTOP_ELEMENTS,
                0,
            );
            Retained::from_raw(list.cast_mut().cast::<NSArray<WindowInfo>>())
        }
    }

    /// The process id of the frontmost application.
    fn frontmost_pid() -> Option<i32> {
        Some(
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()?
                .processIdentifier(),
        )
    }

    /// The frontmost window of the game, if it is on screen.
    pub(super) fn locate(tracker: &GameWindowTracker) -> Option<GameWindow> {
        let windows = Self::windows()?;
        let frontmost = Self::frontmost_pid();
        for entry in windows.iter() {
            let entry: &WindowInfo = &entry;
            let Some(owner) = Self::text(entry, "kCGWindowOwnerName") else {
                continue;
            };
            if !tracker.matches_process(&owner) {
                continue;
            }
            // layer 0 is a normal window (the menu bar, the Dock... are above)
            if Self::number(entry, "kCGWindowLayer") != Some(0.0) {
                continue;
            }
            let Some(bounds) = entry
                .objectForKey(&NSString::from_str("kCGWindowBounds"))
                .and_then(|value| value.downcast::<WindowInfo>().ok())
            else {
                continue;
            };
            let (Some(x), Some(y), Some(width), Some(height)) = (
                Self::number(&bounds, "X"),
                Self::number(&bounds, "Y"),
                Self::number(&bounds, "Width"),
                Self::number(&bounds, "Height"),
            ) else {
                continue;
            };
            let rect = ScreenRect::new(
                x.round() as i32,
                y.round() as i32,
                width.max(0.0).round() as u32,
                height.max(0.0).round() as u32,
            );
            if rect.is_empty() {
                continue;
            }
            let pid = Self::number(entry, "kCGWindowOwnerPID").map(|pid| pid as i32);
            return Some(GameWindow {
                rect,
                focused: pid.is_some() && pid == frontmost,
            });
        }
        None
    }
}
