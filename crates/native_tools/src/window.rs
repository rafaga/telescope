//! The application's own windows, as the operating system names them.
//!
//! The UI toolkit gives no native handle for the secondary windows it opens
//! (the Settings window, the overlay), so such a window is found by its
//! (unique) title. This is the one place that does it, for the file dialogs
//! ([`OwnWindow::id_by_title`]) and for the overlay (`overlay`), which then
//! does its own work on the window it was given. On X11 the lookup lives in
//! the session that also makes the overlay's requests (`overlay/x11.rs`),
//! which has the connection it needs.

#[cfg(target_os = "macos")]
use crate::overlay::OverlayError;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSWindow};
#[cfg(target_os = "macos")]
use objc2_foundation::MainThreadMarker;
#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        },
    },
    core::BOOL,
};

/// Lookups among the windows of this process.
pub struct OwnWindow;

impl OwnWindow {
    /// The native id (the `HWND` on Windows) of the visible window of this
    /// process whose title is `title`, in the form `Dialog::set_owner` takes.
    /// Only this process's windows count: another program may have a window
    /// with the same title. `None` when there is no such window, and always
    /// `None` on the systems where the id is of no use.
    #[cfg(target_os = "windows")]
    pub fn id_by_title(title: &str) -> Option<isize> {
        Self::hwnd_by_title(title).map(|hwnd| hwnd.0 as isize)
    }

    /// The window of this process titled `title` (see [`Self::id_by_title`]),
    /// for the code of this crate that works on it.
    #[cfg(target_os = "windows")]
    #[allow(unsafe_code)]
    pub(crate) fn hwnd_by_title(title: &str) -> Option<HWND> {
        let mut search = TitleSearch {
            title,
            process: std::process::id(),
            found: None,
        };
        // SAFETY: `search` outlives the call and `visit` is the only reader
        // of the pointer; `EnumWindows` is synchronous.
        unsafe {
            let _ = EnumWindows(
                Some(TitleSearch::visit),
                LPARAM(&mut search as *mut TitleSearch as isize),
            );
        }
        search.found
    }

    /// See the Windows version: nothing needs the id here.
    #[cfg(not(target_os = "windows"))]
    pub fn id_by_title(_title: &str) -> Option<isize> {
        None
    }

    /// Runs `action` on the window of this process titled `title`.
    /// [`OverlayError::InvalidHandle`] when there is none (the window may not
    /// exist yet on the first frames). Must run on the main thread.
    #[cfg(target_os = "macos")]
    pub(crate) fn with_ns_window<T>(
        title: &str,
        action: impl FnOnce(&NSWindow) -> T,
    ) -> Result<T, OverlayError> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| OverlayError::Os(String::from("not on the main thread")))?;
        let application = NSApplication::sharedApplication(mtm);
        for window in application.windows().iter() {
            if window.title().to_string() == title {
                return Ok(action(&window));
            }
        }
        Err(OverlayError::InvalidHandle)
    }
}

/// State of one `EnumWindows` pass. `visit` is an associated function (not a
/// free one) because the callback has to be an `extern "system"` pointer, which
/// a closure cannot be.
#[cfg(target_os = "windows")]
struct TitleSearch<'a> {
    title: &'a str,
    process: u32,
    found: Option<HWND>,
}

#[cfg(target_os = "windows")]
impl TitleSearch<'_> {
    #[allow(unsafe_code)]
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut TitleSearch` passed by `id_by_title`.
        let search = unsafe { &mut *(lparam.0 as *mut TitleSearch) };
        if search.inspect(hwnd) {
            // keep enumerating
            BOOL(1)
        } else {
            BOOL(0)
        }
    }

    /// Looks at one window. Returns `false` once the window is found.
    #[allow(unsafe_code)]
    fn inspect(&mut self, hwnd: HWND) -> bool {
        // SAFETY: plain Win32 queries on a handle `EnumWindows` just gave.
        unsafe {
            let mut process = 0_u32;
            GetWindowThreadProcessId(hwnd, Some(&mut process));
            if process != self.process || !IsWindowVisible(hwnd).as_bool() {
                return true;
            }
            let mut buffer = [0u16; 256];
            let length = GetWindowTextW(hwnd, &mut buffer);
            if length <= 0 {
                return true;
            }
            let text = String::from_utf16_lossy(&buffer[..length as usize]);
            if text != self.title {
                return true;
            }
            self.found = Some(hwnd);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_that_does_not_exist_has_no_id() {
        assert_eq!(
            OwnWindow::id_by_title("telescope-test: there is no window with this title"),
            None
        );
    }
}
