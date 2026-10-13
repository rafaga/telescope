//! macOS side of the game window lookup.
//!
//! Where the window is comes from CoreGraphics' window list (position and
//! size, plus the owner's name and process id), and whether the game is the
//! frontmost application from `NSWorkspace`. Neither needs a permission: the
//! window *titles* would need Screen Recording, so the game is recognised by
//! the name of its process instead.

use super::{GameWindow, GameWindowTracker, ScreenRect};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{CFRetained, CFType};
use objc2_core_graphics::{CGShieldingWindowLevel, CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// One entry of the window list (an untyped dictionary, read by key).
type WindowInfo = NSDictionary;

/// Reads the on-screen windows. The keys are the values of the CoreGraphics
/// `kCGWindow*` constants.
pub(super) struct MacWindowList;

impl MacWindowList {
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
    pub(super) fn overlay_level() -> isize {
        isize::try_from(CGShieldingWindowLevel())
            .unwrap_or(isize::MAX)
            .saturating_add(1)
    }

    /// The on-screen windows, front to back.
    fn windows() -> Option<Retained<NSArray>> {
        let list = CGWindowListCopyWindowInfo(
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
            0,
        )?;
        // A `CFArray` is toll-free bridged to `NSArray`: view it as an
        // Objective-C object and let the runtime check its class.
        let list: CFRetained<CFType> = list.into();
        let object: &AnyObject = (*list).as_ref();
        object.downcast_ref::<NSArray>().map(|array| array.retain())
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
            let Some(entry) = entry.downcast_ref::<WindowInfo>() else {
                continue;
            };
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
