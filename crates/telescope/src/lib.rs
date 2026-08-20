//! # Telescope
//!
//! Application to gather intel in EVE Online and present alerts to the
//! player. It monitors the game's chat log files, evaluates their text
//! against configurable regex pattern rules (see [`patterns`]) and presents
//! the results on interactive maps of the universe.

mod app;
pub use app::TelescopeApp;
pub use app::patterns;

