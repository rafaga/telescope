//! What zKillboard sends: the killmails of the R2Z2 feed and of the query
//! API, and the ways a request to it can fail. Nothing here knows what
//! Telescope does with them.

use crate::guard::RequestGuard;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::Deserialize;

/// One file of the feed: a killmail with zKillboard's metadata. Only the
/// fields Telescope uses are read; the rest (and any field added later) is
/// ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Killmail {
    /// The killmail's id in ESI.
    pub killmail_id: u64,
    /// Position in the feed: strictly increasing, never reused.
    pub sequence_id: u64,
    /// The killmail as ESI has it.
    pub esi: EsiKillmail,
    /// zKillboard's own data about it.
    #[serde(default)]
    pub zkb: Zkb,
    /// The file as it was read (the feed's, or ESI's for a killmail asked
    /// of ESI), so that the cache can keep it whole. Not part of the file.
    #[serde(skip)]
    pub raw: Option<String>,
}

/// The ESI part of a [`Killmail`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EsiKillmail {
    /// When the ship died, RFC 3339 in UTC (`2026-10-14T05:01:05Z`).
    pub killmail_time: String,
    /// The solar system it happened in.
    pub solar_system_id: u32,
    /// The one that died.
    pub victim: Victim,
    /// Everyone that shot at it.
    #[serde(default)]
    pub attackers: Vec<Attacker>,
}

/// What died.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Victim {
    /// Type id of the ship or structure.
    #[serde(default)]
    pub ship_type_id: Option<u32>,
}

/// One of the attackers of a killmail.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Attacker {
    /// The pilot; absent for an NPC.
    #[serde(default)]
    pub character_id: Option<u64>,
    /// Type id of the ship it flew.
    #[serde(default)]
    pub ship_type_id: Option<u32>,
}

/// zKillboard's data about a killmail.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Zkb {
    /// The killmail's hash, which with its id names it in ESI.
    #[serde(default)]
    pub hash: String,
    /// The station or structure it happened at.
    #[serde(default, rename = "locationID")]
    pub location_id: Option<u64>,
    /// Estimated value of everything involved, in ISK.
    #[serde(default, rename = "totalValue")]
    pub total_value: f64,
    /// The attackers shot a ship of their own side.
    #[serde(default)]
    pub awox: bool,
    /// zKillboard's labels of the killmail (`pvp`, `loc:lowsec`, `ganked`…).
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Killmail {
    /// When the ship died, if the time can be parsed.
    pub fn time(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.esi.killmail_time)
            .ok()
            .map(|time| time.with_timezone(&Utc))
    }

    /// `value` as a short amount: `950`, `12K`, `12.3M`, `1.5B`.
    pub fn isk(value: f64) -> String {
        match value {
            value if value >= 1e9 => format!("{:.1}B", value / 1e9),
            value if value >= 1e6 => format!("{:.1}M", value / 1e6),
            value if value >= 1e3 => format!("{:.0}K", value / 1e3),
            value => format!("{value:.0}"),
        }
    }
}

/// Why the feed could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    /// The feed answered 403 or 429: throttled, or blocked for asking too
    /// much. Asking again soon makes it worse.
    Throttled(StatusCode),
    /// Nothing was sent: the guard is waiting (the pause after a restart).
    /// zKillboard refused nothing.
    Paused,
    /// Anything else (no network, a server error, an unreadable file).
    Other(String),
}

impl FeedError {
    /// Why the guard does not let a request go out now, or `None` when it
    /// does: a refusal it still remembers, or the pause after a restart.
    pub fn kept_away(guard: &RequestGuard) -> Option<Self> {
        let now = Utc::now();
        guard.wait(now)?;
        Some(if guard.refused(now) {
            Self::Throttled(StatusCode::TOO_MANY_REQUESTS)
        } else {
            Self::Paused
        })
    }
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Throttled(status) => write!(f, "the feed refused the request ({status})"),
            Self::Paused => f.write_str("waiting a moment before asking the feed again"),
            Self::Other(reason) => f.write_str(reason),
        }
    }
}
