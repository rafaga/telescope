//! Telescope's client of the public EvE-Scout API
//! (`https://api.eve-scout.com/ui/`).
//!
//! EvE-Scout keeps the wormhole connections of Thera and Turnur (the
//! scanned signatures), tracks the Pochven storms and Tom's Shuttle, and
//! calculates routes that use those wormholes. The maps draw that next to
//! what they already show; none of it is intel, and nothing here feeds the
//! rules engine.
//!
//! * [`client`]: one method for each public endpoint of the API.
//! * [`guard`]: what keeps the client from asking more than the API needs: it
//!   does not ask again for an answer that is still valid, it leaves an
//!   endpoint alone after a bad answer and it asks for one thing at a time.
//! * [`types`]: what it sends and takes, as typed values.
//!
//! When to ask, and what to do with the answers, is for the caller: the
//! client does nothing on its own.

pub mod client;
pub mod guard;
pub mod types;

#[cfg(test)]
mod testing;

pub use client::{ApiError, Cached, Client, ClientConfig, Error};
pub use guard::GuardConfig;
pub use types::{
    DailyGoal, DailyGoals, Donation, DonationStatistics, Hub, Observation, ObservationCategory,
    ObservationType, Period, PublicSystem, Route, RoutePreference, RouteRequest, ScanStatistic,
    ShipSize, Signature, SignatureType, Space, SystemClass, WormholeType, WormholeTypeFilter,
};
