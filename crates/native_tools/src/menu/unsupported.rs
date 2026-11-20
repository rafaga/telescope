//! Windows and Linux have no system-wide menu bar to install into: the menu
//! is drawn inside the window by the GUI toolkit.

use super::MenuBar;

/// The native menu bar. Exists on macOS only; here it cannot be installed.
pub struct NativeMenu;

impl NativeMenu {
    /// Whether this system has a native menu bar to install into.
    pub const SUPPORTED: bool = false;

    /// Always fails: draw the [`MenuBar`] inside the window instead.
    pub fn install(_bar: &MenuBar, _wakeup: impl Fn() + 'static) -> Result<Self, String> {
        Err(String::from(
            "the native menu bar exists on macOS only; draw the menu in the window",
        ))
    }

    /// Nothing to update.
    pub fn sync(&self, _bar: &MenuBar) {}

    /// Never has an event.
    pub fn poll(&self) -> Option<String> {
        None
    }

    /// Never has events.
    pub fn drain(&self) -> Vec<String> {
        Vec::new()
    }
}
