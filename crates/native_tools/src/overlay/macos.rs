//! macOS side of the game window lookup.
//!
//! Where the window is comes from CoreGraphics' window list (position and
//! size, plus the owner's name and process id), and whether the game is the
//! frontmost application from `NSWorkspace`. Neither needs a permission: the
//! window *titles* would need Screen Recording, so the game is recognised by
//! the name of its process instead.

use super::{GameWindow, GameWindowTracker, ScreenRect, WindowFacts, game_is_covered};
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

    /// The rectangle of `entry` in screen coordinates.
    fn rect(entry: &WindowInfo) -> Option<ScreenRect> {
        let bounds = entry
            .objectForKey(&NSString::from_str("kCGWindowBounds"))
            .and_then(|value| value.downcast::<WindowInfo>().ok())?;
        Some(ScreenRect::new(
            Self::number(&bounds, "X")?.round() as i32,
            Self::number(&bounds, "Y")?.round() as i32,
            Self::number(&bounds, "Width")?.max(0.0).round() as u32,
            Self::number(&bounds, "Height")?.max(0.0).round() as u32,
        ))
    }

    /// What the list says about `entry`, to tell whether it covers the game.
    fn facts(entry: &WindowInfo) -> Option<WindowFacts> {
        Some(WindowFacts {
            pid: Self::number(entry, "kCGWindowOwnerPID")? as i32,
            layer: Self::number(entry, "kCGWindowLayer")? as i64,
            rect: Self::rect(entry)?,
            alpha: Self::number(entry, "kCGWindowAlpha").unwrap_or(1.0),
        })
    }

    /// The game's window, if it is on screen. It counts as in front when no
    /// other window above it in the list covers most of it (Telescope's own do
    /// not count), or when the system says the game is the frontmost
    /// application. "Frontmost application" alone is wrong with several
    /// screens, where the game can be in plain view on one while the focus is
    /// on another, and the client's windows can also belong to a process other
    /// than the application NSWorkspace names.
    pub(super) fn locate(tracker: &GameWindowTracker) -> Option<GameWindow> {
        let windows = Self::windows()?;
        let frontmost = Self::frontmost_pid();
        let own_pid = i32::try_from(std::process::id()).unwrap_or(-1);
        // the windows of other applications seen so far, front to back
        let mut above: Vec<WindowFacts> = Vec::new();
        for entry in windows.iter() {
            let Some(entry) = entry.downcast_ref::<WindowInfo>() else {
                continue;
            };
            let Some(owner) = Self::text(entry, "kCGWindowOwnerName") else {
                continue;
            };
            if !tracker.matches_process(&owner) {
                above.extend(Self::facts(entry));
                continue;
            }
            // layer 0 is a normal window (the menu bar, the Dock... are above)
            if Self::number(entry, "kCGWindowLayer") != Some(0.0) {
                continue;
            }
            let Some(rect) = Self::rect(entry) else {
                continue;
            };
            if rect.is_empty() {
                continue;
            }
            let pid = Self::number(entry, "kCGWindowOwnerPID").map(|pid| pid as i32);
            return Some(GameWindow {
                rect,
                focused: !game_is_covered(&rect, &above, own_pid)
                    || (pid.is_some() && pid == frontmost),
            });
        }
        None
    }
}
