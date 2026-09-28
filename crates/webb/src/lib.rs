//! `webb`: Telescope's EVE Online back end.
//!
//! * [`auth_service`]: the local HTTP server that receives the EVE SSO OAuth
//!   callback.
//! * [`esi`]: the ESI client and the local player database.
//! * [`objects`]: the domain types (characters, corporations, alliances, tokens).
//! * [`graph`]: the intel rules as a node graph (a DAG of Input / Detection /
//!   Output / logic nodes) and the executor that runs it over each chat-log
//!   line.
//! * [`rules`]: the typed building blocks of the graph (detection and output
//!   types, the built-in word lists, the chat-log line parser).
//! * [`intel`]: types and limits the intel modules share (the parsed line,
//!   the detection categories, the validation errors).
//! * [`map_alerts`]: condenses a line's messages into what a map's node
//!   tooltip shows.

pub mod auth_service;
pub mod esi;
pub mod graph;
pub mod intel;
pub mod map_alerts;
pub mod objects;
pub mod rules;
