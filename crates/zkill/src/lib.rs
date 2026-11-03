//! Everything Telescope asks of zKillboard.
//!
//! * [`feed`]: the R2Z2 feed: its client, the walk through the files it
//!   publishes (what is announced and what is history) and the read-back of
//!   the last hour.
//! * [`guard`]: keeping away from zKillboard after it said "enough", across
//!   restarts, and never asking faster than it allows.
//! * [`types`]: the killmails it sends and the ways a request can fail.
//! * [`stats`]: how many requests each endpoint let through and how they
//!   ended, second by second.
//! * [`lane`] and [`queue`]: the order and the pace of the requests of one
//!   endpoint, with room kept for the urgent ones.
//!
//! The R2Z2 client lives here and takes its turns in the lane of its endpoint.
//! The client of the query API is still in Telescope (`intel/kill_day.rs`);
//! it uses the lanes and the guards of this crate too.

pub mod feed;
pub mod guard;
pub mod lane;
pub mod queue;
pub mod stats;
pub mod types;
