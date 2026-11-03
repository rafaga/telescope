//! Everything Telescope asks of zKillboard.
//!
//! * [`guard`]: keeping away from zKillboard after it said "enough", across
//!   restarts, and never asking faster than it allows.
//! * [`types`]: the killmails it sends and the ways a request can fail.
//! * [`stats`]: how many requests each endpoint let through and how they
//!   ended, second by second.
//! * [`lane`] and [`queue`]: the order and the pace of the requests of one
//!   endpoint, with room kept for the urgent ones.
//!
//! The requests themselves will go through one scheduler of this crate, so
//! that nothing else in Telescope talks to zKillboard.

pub mod guard;
pub mod lane;
pub mod queue;
pub mod stats;
pub mod types;
