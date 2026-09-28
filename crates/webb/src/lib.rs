//! `webb`: Telescope's EVE Online back end.
//!
//! * [`auth_service`]: the local HTTP server that receives the EVE SSO OAuth
//!   callback.
//! * [`esi`]: the ESI client and the local player database.
//! * [`objects`]: the domain types (characters, corporations, alliances, tokens).
//! * [`patterns`]: the intel pattern-matching engine, rule/dictionary loading
//!   and evaluation against raw chat-log text.
//! * [`rules`]: the three-class intel rule model (input -> detection ->
//!   output) and the detection engine that supersedes [`patterns`]' actions.
//! * [`graph`]: the node-graph intel model (a DAG of Input/Detection/Output
//!   nodes) that supersedes [`rules`].
//! * [`map_alerts`]: condenses a line's matches into what a map's node tooltip
//!   shows.

pub mod auth_service;
pub mod esi;
pub mod graph;
pub mod map_alerts;
pub mod objects;
pub mod patterns;
pub mod rules;
