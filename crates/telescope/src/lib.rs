//! # Telescope
//!
//! Application to gather intel in EVE Online and present alerts to the
//! player. It monitors the game's chat log files, evaluates their text
//! against configurable regex pattern rules (see [`patterns`]) and presents
//! the results on interactive maps of the universe.

#[macro_use]
extern crate rust_i18n;

// Interface texts, embedded from `locales/*.toml`; see `i18n`.
rust_i18n::i18n!("locales", fallback = "en");

mod app;
pub mod app_dirs;
mod i18n;
pub mod log_bridge;
pub use app::TelescopeApp;
pub use sputnik::patterns;
