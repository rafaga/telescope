//! System permissions the overlay may need.
//!
//! Only macOS asks for them. On the other systems every check answers
//! "granted" and a request does nothing, so callers need no `cfg`.

#[cfg(target_os = "macos")]
use objc2_application_services::{
    AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt,
};
#[cfg(target_os = "macos")]
use objc2_core_foundation::{CFBoolean, CFDictionary};

/// The macOS *Accessibility* permission (System Settings -> Privacy &
/// Security -> Accessibility).
///
/// Showing the overlay does not need it. It is needed to register global
/// keyboard shortcuts (the "edit widgets" toggle) and to read the position of
/// other applications' windows through the accessibility API.
///
/// macOS shows its prompt only the first time an application asks; after that
/// the user has to turn the switch on by hand, and the application may have to
/// be restarted. The grant belongs to the application's code signature: an
/// ad-hoc signed build can lose it when it is rebuilt, and a binary started
/// from a terminal (`cargo run`) is covered by the terminal's entry instead.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccessibilityPermission;

impl AccessibilityPermission {
    pub fn new() -> Self {
        Self
    }

    /// Whether the permission is granted now. Never shows anything.
    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    pub fn is_granted(&self) -> bool {
        // SAFETY: a plain query with no arguments.
        unsafe { AXIsProcessTrusted() }
    }

    /// Whether the permission is granted now. Never shows anything.
    #[cfg(not(target_os = "macos"))]
    pub fn is_granted(&self) -> bool {
        true
    }

    /// Asks for the permission: if it is not granted, macOS shows its dialog
    /// (the first time) that leads to System Settings. Returns whether it is
    /// granted right now, which is almost always `false` the first time: the
    /// user answers after this returns, so check [`Self::is_granted`] later.
    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    pub fn request(&self) -> bool {
        // SAFETY: reads the framework's constant string, valid for the whole
        // process.
        let key = unsafe { kAXTrustedCheckOptionPrompt };
        let options = CFDictionary::from_slices(&[key], &[CFBoolean::new(true)]);
        let options: &CFDictionary = (*options).as_ref();
        // SAFETY: the options map a `CFString` key to a `CFBoolean`, as the
        // call requires, and they outlive the call.
        unsafe { AXIsProcessTrustedWithOptions(Some(options)) }
    }

    /// Asks for the permission (nothing to ask here).
    #[cfg(not(target_os = "macos"))]
    pub fn request(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "macos"))]
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_systems_always_count_as_granted() {
        let permission = AccessibilityPermission::new();
        assert!(permission.is_granted());
        assert!(permission.request());
    }
}
