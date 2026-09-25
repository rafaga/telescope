//! Secondary egui windows opened from the main `TelescopeApp` UI.

mod about;
// Testing/inspection tools with no business being reachable in a
// release build -- see the cfg on TelescopeApp's `debug`/search_*
// fields and the "menu.debug" button in app.rs for the rest of this.
#[cfg(debug_assertions)]
pub(crate) mod debug;
mod settings;
