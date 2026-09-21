//! `webb`: Telescope's EVE Online back end.
//!
//! * [`auth_service`]: the local HTTP server that receives the EVE SSO OAuth
//!   callback.
//! * [`esi`]: the ESI client and the local player database.
//! * [`objects`]: the domain types (characters, corporations, alliances, tokens).

pub mod auth_service;
pub mod esi;
pub mod objects;
