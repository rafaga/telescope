//! `sputnik`: the pattern-matching engine that turns EVE Online intel chat
//! lines into notifications and map alerts.
//!
//! Split out of `telescope` so this half -- the part that only reads text
//! and produces a structured alert -- has no dependency on `TelescopeApp`,
//! `eframe`/`egui`, `sde`, `notify` or Telescope's own message bus. It only
//! depends on `regex`/`aho-corasick` (matching), `serde`/`toml` (loading
//! `patterns.toml`), `chrono` (timestamps) and `tracing` (span macros only --
//! no `tracing-tracy`: its spans reach whatever subscriber `telescope`'s own
//! binary installs for free, via Cargo's feature unification on this same
//! `tracing` dependency, same as `webb`/`native_tools`). This also keeps it
//! buildable for `wasm32-unknown-unknown` for free, same as before the split.
//!
//! * [`patterns`]: [`patterns::PatternEngine`], rule/dictionary loading and
//!   evaluation against raw chat-log text.
//! * [`map_alerts`][]: [`map_alerts::AlertSummary`]/[`map_alerts::IntelAlert`]/
//!   [`map_alerts::AlertLog`], which condense a line's matches into what a
//!   map's node tooltip shows.
//!
//! Reading the file, resolving a reported system name against the SDE,
//! deciding whether the alarm should sound and dispatching the result to
//! the UI stays in `telescope` (`app/intel.rs`) -- that part is genuinely
//! specific to `TelescopeApp` (`Settings`, the ESI-tracked characters, the
//! audio player, the message bus) and doesn't belong in a crate this small
//! and dependency-free.

pub mod map_alerts;
pub mod patterns;
