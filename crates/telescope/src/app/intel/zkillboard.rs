//! zKillboard input: the live killmail feed, as intel lines.
//!
//! An Input node of kind [`InputKind::ZKillboard`] reads zKillboard's R2Z2
//! feed (`https://r2z2.zkillboard.com/ephemeral/`), the successor of RedisQ.
//! The feed is a numbered series of files, one killmail each, and every file
//! already carries the whole ESI killmail, so no other request is needed to
//! know the system, the victim's ship and the attackers. Only the *names* are
//! missing (the feed has ids): they come from the local SDE, see
//! [`SdeNames`].
//!
//! Each killmail becomes one [`IntelLine`] such as
//!
//! ```text
//! [ 2026.10.14 05:01:05 ] zKillboard > Jita: Rifter destroyed by 3 pilots (Sabre ×2, Merlin), 12.3M ISK
//! ```
//!
//! sent to the detection stage as an [`InputEvent`] of kind
//! [`InputKind::ZKillboard`] (only the Input nodes of that kind receive it).
//! Being plain text, the detections work on it as on a chat line: *System
//! report* finds the system, *Ship names* the ships involved and *Pilot
//! count* the attackers.
//!
//! The feed is unfiltered (every kill of the universe, roughly one every five
//! seconds): what to do with it is up to the rules wired after the node.
//!
//! # Polling
//!
//! The protocol is zKillboard's own (see its API documentation):
//!
//! * `sequence.json` holds the latest sequence number (refreshed every 51
//!   killmails); `<n>.json` is the killmail with sequence `n`.
//! * Read `n`, then `n + 1`, and so on; a 404 means "nothing there yet": wait
//!   at least 6 seconds and ask again.
//! * Up to 15 requests per second are allowed; every request waits its turn in
//!   the R2Z2 lane of the `zkill` crate, which keeps to 8 (the live walk goes
//!   before the hour read back at the start). The query API has a lane and a
//!   guard of its own ([`kill_day`]).
//! * A blank `User-Agent` is blocked, and a throttled or blocked client gets
//!   a 403/429: it backs off for a long while ([`RequestGuard`]), and
//!   remembers it across restarts.
//!
//! What was already in the feed when the source starts is history, as with
//! the chat logs: the walk starts at the end of the feed and the killmails it
//! passes on the way are not announced (see [`FeedCursor`]).
//!
//! `sequence.json` is served fresh (no `cache-control`, not cached by the
//! CDN) and the last file of the feed is only a few numbers past it, so that
//! walk is a handful of requests. Checked against the live feed on
//! 2026-10-28: the pointer's killmail was 41 seconds old and the last file
//! was the pointer plus 7. The "1 day" cache the documentation lists belongs
//! to the numbered files, not to the pointer.
//!
//! The source runs on its own thread, like the reader and the detection
//! stage, and only while the graph has an enabled zKillboard Input node
//! ([`ZKillboardHandle::follow`]): without one it makes no request at all.

use super::cache_store::{CacheStore, CacheStoreConfig};
use super::detection::ResolverHandle;
use super::input::InputEvent;
use super::kill_context::{Fill, KillBuffer, KillContext, KillRecord, KillTags, Source, WINDOW};
use super::kill_day::{self, DayCache, DayLine, Fetched, Prefetch, ZKillApi};
use super::resolve::JumpGraph;
use super::zkill_guard::{GuardConfig, GuardStore, RequestGuard};
use crate::app::messages::{Message, Type, try_send_app_message};
use crate::app::settings::ZKillboardOptions;
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use webb::esi::HourKills;
use webb::event::{EventSource, Facts, IntelEvent, ShipCount, SystemFact, TimeOrigin};
use webb::graph::{NodeKind, RuleGraph, SystemResolver};
use webb::rules::{InputKind, IntelLine};
use zkill::guard::{Endpoint, Guards};
use zkill::lane::{Job, LaneConfig, Priority};
use zkill::queue::LaneQueue;

/// Where the R2Z2 files are served from.
pub(crate) const FEED_URL: &str = "https://r2z2.zkillboard.com/ephemeral";

/// The `User-Agent` of every request. zKillboard's CDN blocks a blank one;
/// this one says who is asking.
pub(super) const USER_AGENT: &str = concat!(
    "telescope/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/rafaga/telescope)"
);

/// How long a whole request may take before it is given up as failed.
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Wait after reaching the end of the feed: killmails average one every 5.5
/// seconds, so asking sooner buys nothing, and 6 seconds is what the feed's
/// documentation asks for after a 404.
pub(crate) const POLL_WAIT: Duration = Duration::from_secs(6);

/// Longest wait between two attempts while the feed can't be reached.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How long the feed may stay unreachable before the walk starts over at the
/// end of the feed: what was published meanwhile is too old to be intel.
const RESYNC_AFTER: Duration = Duration::from_secs(2 * 60);

/// How many files of the feed hold about an hour of killmails (one every five
/// and a half seconds on average): the ones before the walk starts are read
/// to fill the buffer the node tooltips set the reports against.
const BACKFILL_FILES: u64 = 700;

/// Files of that hour read per round, so the live walk is not kept waiting.
const BACKFILL_CHUNK: u64 = 20;

/// How many killmail ids are remembered to announce a killmail only once
/// (zKillboard may publish a reprocessed killmail again under a new
/// sequence number).
const RECENT_IDS: usize = 4096;

/// The most attackers whose ship is named in a line.
const MAX_LISTED_ATTACKERS: usize = 8;

/// Author of the lines made from killmails.
const AUTHOR: &str = "zKillboard";

pub(crate) use zkill::types::{Attacker, FeedError, Killmail};

/// What Telescope makes of a [`Killmail`]: lines for the detections, records
/// for the tooltips and the filters of the settings. The killmail itself
/// lives in the `zkill` crate, which knows nothing of these.
pub(crate) trait KillmailExt {
    /// The line the detections read for this killmail, or `None` when its
    /// system has no name (the SDE isn't built yet or doesn't know it): a
    /// kill that can't be placed has nothing to alert about.
    ///
    /// `resolver` keeps a ship from being reported as a system: a ship named
    /// like one (there are a few) is left out of the text, since the *System
    /// report* detection would take it for the system.
    fn to_line(&self, names: &dyn NameSource, resolver: &dyn SystemResolver) -> Option<IntelLine>;

    /// The killmail reduced to what the node tooltips tell, with the ships
    /// named by `names`.
    fn to_record(&self, names: &dyn NameSource) -> KillRecord;

    /// Whether the killmail passes the filters of the settings: it is worth
    /// at least `min_isk` and, with `ignore_npc_only`, some pilot took part.
    /// The feed has no filters of its own, so this is done here.
    fn is_worth(&self, min_isk: u64, ignore_npc_only: bool) -> bool;
}

impl KillmailExt for Killmail {
    /// The line the detections read for this killmail, or `None` when its
    /// system has no name (the SDE isn't built yet or doesn't know it): a
    /// kill that can't be placed has nothing to alert about.
    ///
    /// `resolver` keeps a ship from being reported as a system: a ship named
    /// like one (there are a few) is left out of the text, since the *System
    /// report* detection would take it for the system.
    fn to_line(&self, names: &dyn NameSource, resolver: &dyn SystemResolver) -> Option<IntelLine> {
        let system = names.system_name(self.esi.solar_system_id)?;
        let ship = |type_id: Option<u32>| -> Option<String> {
            let name = names.type_name(type_id?)?;
            resolver.resolve(&name).is_none().then_some(name)
        };

        let pilots: Vec<&Attacker> = self
            .esi
            .attackers
            .iter()
            .filter(|attacker| attacker.character_id.is_some())
            .collect();
        let by = match pilots.len() {
            0 => String::from("NPCs"),
            1 => String::from("1 pilot"),
            count => format!("{count} pilots"),
        };
        // One entry per ship type, with how many pilots flew it.
        let mut counted: Vec<ShipCount> = Vec::new();
        for name in pilots
            .iter()
            .filter_map(|attacker| ship(attacker.ship_type_id))
            .take(MAX_LISTED_ATTACKERS)
        {
            ShipCount::tally(&mut counted, ShipCount::new(name, 1));
        }
        let ships: Vec<String> = counted
            .iter()
            .map(|ship| match ship.times {
                1 => ship.name.clone(),
                times => format!("{} ×{times}", ship.name),
            })
            .collect();
        let victim = ship(self.esi.victim.ship_type_id).unwrap_or_else(|| String::from("ship"));

        let mut text = format!("{system}: {victim} destroyed by {by}");
        if !ships.is_empty() {
            text.push_str(&format!(" ({})", ships.join(", ")));
        }
        text.push_str(&format!(", {} ISK", Killmail::isk(self.zkb.total_value)));
        Some(IntelLine {
            timestamp: self.time().unwrap_or_else(Utc::now),
            author: String::from(AUTHOR),
            text,
        })
    }

    /// The killmail reduced to what the node tooltips tell, with the ships
    /// named by `names`.
    fn to_record(&self, names: &dyn NameSource) -> KillRecord {
        let ship = |type_id: Option<u32>| type_id.and_then(|id| names.type_name(id));
        let pilots: Vec<&Attacker> = self
            .esi
            .attackers
            .iter()
            .filter(|attacker| attacker.character_id.is_some())
            .collect();
        KillRecord {
            killmail_id: self.killmail_id,
            time: self.time().unwrap_or_else(Utc::now),
            system_id: self.esi.solar_system_id as usize,
            victim: ship(self.esi.victim.ship_type_id),
            attacker_ships: pilots
                .iter()
                .filter_map(|attacker| ship(attacker.ship_type_id))
                .collect(),
            pilots: u32::try_from(pilots.len()).unwrap_or(u32::MAX),
            tags: KillTags {
                awox: self.zkb.awox || self.zkb.labels.iter().any(|label| label == "awox"),
                ganked: self.zkb.labels.iter().any(|label| label == "ganked"),
            },
            hash: self.zkb.hash.clone(),
            location_id: self.zkb.location_id,
        }
    }

    /// Whether the killmail passes the filters of the settings: it is worth
    /// at least `min_isk` and, with `ignore_npc_only`, some pilot took part.
    /// The feed has no filters of its own, so this is done here.
    fn is_worth(&self, min_isk: u64, ignore_npc_only: bool) -> bool {
        if self.zkb.total_value < min_isk as f64 {
            return false;
        }
        !ignore_npc_only
            || self
                .esi
                .attackers
                .iter()
                .any(|attacker| attacker.character_id.is_some())
    }
}

/// Turns the killmails of the feed into events: the format of the feed is
/// read here and nowhere else.
///
/// The event's text is the sentence [`KillmailExt::to_line`] writes, for the log,
/// the tooltips and the patterns of the user. Its facts come from the killmail
/// itself, not from that sentence: the system with its id, every ship that
/// took part with how many times, and the number of pilots.
pub(crate) struct ZKillboardDecoder<'a> {
    names: &'a dyn NameSource,
    resolver: &'a dyn SystemResolver,
}

impl<'a> ZKillboardDecoder<'a> {
    /// A decoder that names ids with `names`; `resolver` keeps a ship named
    /// like a system out of the text (see [`KillmailExt::to_line`]).
    pub(crate) fn new(names: &'a dyn NameSource, resolver: &'a dyn SystemResolver) -> Self {
        Self { names, resolver }
    }

    /// What `killmail` says by itself. `system` is the name of its system,
    /// which its text starts with.
    fn facts(&self, killmail: &Killmail, system: String) -> Facts {
        let pilots: Vec<&Attacker> = killmail
            .esi
            .attackers
            .iter()
            .filter(|attacker| attacker.character_id.is_some())
            .collect();
        // The victim's ship first, then the ones the pilots flew (an NPC's
        // ship is not part of a fight between pilots).
        let mut ships: Vec<ShipCount> = Vec::new();
        let flown = std::iter::once(killmail.esi.victim.ship_type_id)
            .chain(pilots.iter().map(|attacker| attacker.ship_type_id));
        for type_id in flown.flatten() {
            let Some(name) = self.names.type_name(type_id) else {
                continue;
            };
            ShipCount::tally(&mut ships, ShipCount::new(name, 1).with_type_id(type_id));
        }
        Facts {
            systems: Some(vec![SystemFact {
                id: killmail.esi.solar_system_id as usize,
                // The text begins with the name of the system.
                span: Some(0..system.len()),
                name: system,
            }]),
            ships: Some(ships),
            pilots: Some(u32::try_from(pilots.len()).unwrap_or(u32::MAX)),
        }
    }
}

impl EventSource for ZKillboardDecoder<'_> {
    type Raw = Killmail;

    fn kind(&self) -> InputKind {
        InputKind::ZKillboard
    }

    fn decode(&self, killmail: &Killmail) -> Option<IntelEvent> {
        let line = killmail.to_line(self.names, self.resolver)?;
        let system = self.names.system_name(killmail.esi.solar_system_id)?;
        let channel = self.kind().fixed_channel().unwrap_or_default();
        // A killmail whose time can't be read is still a killmail: it is taken
        // to have happened when it was read.
        let origin = if killmail.time().is_some() {
            TimeOrigin::Reported
        } else {
            TimeOrigin::Assumed
        };
        Some(
            IntelEvent::text(line, channel, self.kind())
                .with_facts(self.facts(killmail, system))
                .with_time_origin(origin),
        )
    }
}

/// Names for the ids the feed carries.
pub(crate) trait NameSource {
    /// The name of a solar system.
    fn system_name(&self, id: u32) -> Option<String>;
    /// The name of an item type (a ship or a structure).
    fn type_name(&self, id: u32) -> Option<String>;
}

/// [`NameSource`] backed by the SDE database (`mapSolarSystems` and
/// `invTypes`), read lazily and kept in memory.
///
/// The database is opened only for a name that isn't known yet and closed
/// right after: the SDE updater replaces the file while Telescope runs, and
/// an open handle would get in its way (on Windows, not even replace it).
/// The path is shared with the UI, which keeps it current.
pub(crate) struct SdeNames {
    path: Arc<RwLock<PathBuf>>,
    cache: Mutex<NameCache>,
}

/// What [`SdeNames`] already looked up, and for which database.
#[derive(Default)]
struct NameCache {
    path: PathBuf,
    systems: HashMap<u32, String>,
    types: HashMap<u32, String>,
}

impl SdeNames {
    const SYSTEM_SQL: &'static str =
        "SELECT solarSystemName FROM mapSolarSystems WHERE solarSystemId = ?1";
    const TYPE_SQL: &'static str = "SELECT typeName FROM invTypes WHERE typeId = ?1";

    fn new(path: Arc<RwLock<PathBuf>>) -> Self {
        Self {
            path,
            cache: Mutex::new(NameCache::default()),
        }
    }

    /// Runs `sql` (one `?1` parameter, one text column) on the database at
    /// `path`, opened read-only. `None` if it can't be opened, has no such
    /// table or has no such row.
    fn query(path: &Path, sql: &str, id: u32) -> Option<String> {
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        connection.query_row(sql, [id], |row| row.get(0)).ok()
    }

    /// The cached name for `id`, looked up with `sql` when it isn't there.
    /// `table` picks the cache table of the [`NameCache`].
    fn lookup(
        &self,
        id: u32,
        sql: &str,
        table: impl Fn(&mut NameCache) -> &mut HashMap<u32, String>,
    ) -> Option<String> {
        let path = self
            .path
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.path != path {
            // Another SDE: nothing known about it yet.
            *cache = NameCache {
                path: path.clone(),
                ..NameCache::default()
            };
        }
        if let Some(name) = table(&mut cache).get(&id) {
            return Some(name.clone());
        }
        let name = Self::query(&path, sql, id)?;
        table(&mut cache).insert(id, name.clone());
        Some(name)
    }
}

/// The regions of the SDE.
struct SdeRegions;

impl SdeRegions {
    /// The systems that share a region with any of `systems`, `systems`
    /// included. Empty if the database can't be read.
    fn systems_with(path: &Path, systems: &[u32]) -> HashSet<u32> {
        if systems.is_empty() {
            return HashSet::new();
        }
        let marks = vec!["?"; systems.len()].join(",");
        let sql = format!(
            "SELECT s.solarSystemId FROM mapSolarSystems s \
             JOIN mapConstellations c USING (constellationId) \
             WHERE c.regionId IN (SELECT c2.regionId FROM mapSolarSystems s2 \
             JOIN mapConstellations c2 USING (constellationId) \
             WHERE s2.solarSystemId IN ({marks}))"
        );
        let Ok(connection) = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return HashSet::new();
        };
        let Ok(mut statement) = connection.prepare(&sql) else {
            return HashSet::new();
        };
        let Ok(rows) = statement.query_map(rusqlite::params_from_iter(systems), |row| {
            row.get::<_, u32>(0)
        }) else {
            return HashSet::new();
        };
        rows.flatten().collect()
    }

    /// The region of every system. Empty if the database can't be read.
    fn layout(path: &Path) -> HashMap<u32, u32> {
        let Ok(connection) = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return HashMap::new();
        };
        let Ok(mut statement) = connection.prepare(
            "SELECT s.solarSystemId, c.regionId FROM mapSolarSystems s \
             JOIN mapConstellations c USING (constellationId)",
        ) else {
            return HashMap::new();
        };
        let Ok(rows) =
            statement.query_map([], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)))
        else {
            return HashMap::new();
        };
        rows.flatten().collect()
    }
}

impl NameSource for SdeNames {
    fn system_name(&self, id: u32) -> Option<String> {
        self.lookup(id, Self::SYSTEM_SQL, |cache| &mut cache.systems)
    }

    fn type_name(&self, id: u32) -> Option<String> {
        self.lookup(id, Self::TYPE_SQL, |cache| &mut cache.types)
    }
}

/// The two reads the walk needs from the feed.
pub(crate) trait Feed {
    /// The latest sequence number published (`sequence.json`).
    async fn pointer(&self) -> Result<u64, FeedError>;
    /// The killmail with this sequence number; `None` when there isn't one
    /// (yet).
    async fn entry(&self, sequence: u64) -> Result<Option<Killmail>, FeedError>;
}

/// Where the requests to the feed wait for their turn: the R2Z2 lane, which
/// is the only thing that spaces them.
#[derive(Clone)]
pub(crate) struct FeedSlots {
    lane: Arc<LaneQueue<()>>,
    /// How much the requests made through these slots matter.
    priority: Priority,
    /// Tells the requests apart (the lane merges the ones with the same key).
    next_key: Arc<AtomicU64>,
}

impl FeedSlots {
    /// Slots in `lane`, for the requests somebody is waiting for.
    pub(crate) fn new(lane: Arc<LaneQueue<()>>) -> Self {
        Self {
            lane,
            priority: Priority::Interactive,
            next_key: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The same lane for requests of `priority`.
    pub(crate) fn with_priority(&self, priority: Priority) -> Self {
        Self {
            priority,
            ..self.clone()
        }
    }

    /// Waits for the turn of a request.
    async fn wait(&self) {
        let key = self.next_key.fetch_add(1, Ordering::Relaxed);
        self.lane.submit(Job::new(key, (), self.priority));
        self.lane.next().await;
    }
}

impl Default for FeedSlots {
    /// A lane with no pace at all.
    fn default() -> Self {
        Self::new(Arc::new(LaneQueue::new(LaneConfig {
            min_interval: Duration::ZERO,
            background_interval: Duration::ZERO,
            reserve_every: 1,
            low_every: 0,
        })))
    }
}

/// The R2Z2 feed over HTTPS.
pub(crate) struct R2z2Client {
    client: Client,
    base: String,
    /// Keeps the requests away from zKillboard while it asked for it.
    guard: Arc<RequestGuard>,
    /// Where each request waits for its turn.
    slots: FeedSlots,
}

impl R2z2Client {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base: String::from(FEED_URL),
            guard: Arc::new(RequestGuard::new(GuardConfig::default())),
            slots: FeedSlots::default(),
        })
    }

    /// The same client, taking its turns in `slots`.
    pub(crate) fn with_slots(mut self, slots: FeedSlots) -> Self {
        self.slots = slots;
        self
    }

    /// The same client for requests of `priority`: the live walk is
    /// interactive, the hour read back when the feed starts is not.
    pub(crate) fn with_priority(&self, priority: Priority) -> Self {
        Self {
            client: self.client.clone(),
            base: self.base.clone(),
            guard: Arc::clone(&self.guard),
            slots: self.slots.with_priority(priority),
        }
    }

    /// The same client, asking only when `guard` lets it (the one every
    /// request to zKillboard shares).
    pub(crate) fn with_guard(mut self, guard: Arc<RequestGuard>) -> Self {
        self.guard = guard;
        self
    }

    /// `path` of the feed, or `None` for a 404.
    async fn get(&self, path: &str) -> Result<Option<Response>, FeedError> {
        // The lane sets the pace between requests, and what the others (the
        // hour read back) leave free is for the live walk.
        self.slots.wait().await;
        // Never closer to the previous request, whoever made it, than the
        // guard says: the limit is per address, not per source.
        tokio::time::sleep(self.guard.pace(Utc::now())).await;
        // While zKillboard asked to be left alone, nothing is sent.
        if let Some(error) = FeedError::kept_away(&self.guard) {
            return Err(error);
        }
        self.guard.requested(Utc::now());
        let response = self
            .client
            .get(format!("{}/{path}", self.base))
            .send()
            .await
            .map_err(|error| FeedError::Other(error.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND => {
                self.guard.succeeded(Utc::now());
                Ok(None)
            }
            status if status.is_success() => {
                self.guard.succeeded(Utc::now());
                Ok(Some(response))
            }
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
                tracing::warn!(
                    %status,
                    base = %self.base,
                    path,
                    retry_after = ?response.headers().get(reqwest::header::RETRY_AFTER),
                    sent = self.guard.sent(),
                    "zKillboard refused a feed request"
                );
                self.guard
                    .throttled_for(Utc::now(), RequestGuard::retry_after(response.headers()));
                Err(FeedError::Throttled(status))
            }
            status => Err(FeedError::Other(format!("the feed answered {status}"))),
        }
    }
}

impl Feed for R2z2Client {
    async fn pointer(&self) -> Result<u64, FeedError> {
        #[derive(Deserialize)]
        struct Pointer {
            sequence: u64,
        }
        let response = self
            .get("sequence.json")
            .await?
            .ok_or_else(|| FeedError::Other(String::from("sequence.json is missing")))?;
        let pointer: Pointer = response
            .json()
            .await
            .map_err(|error| FeedError::Other(format!("unreadable sequence.json: {error}")))?;
        Ok(pointer.sequence)
    }

    async fn entry(&self, sequence: u64) -> Result<Option<Killmail>, FeedError> {
        let Some(response) = self.get(&format!("{sequence}.json")).await? else {
            return Ok(None);
        };
        let unreadable = |error: &dyn std::fmt::Display| {
            FeedError::Other(format!("unreadable killmail {sequence}: {error}"))
        };
        let text = response.text().await.map_err(|error| unreadable(&error))?;
        let mut killmail: Killmail =
            serde_json::from_str(&text).map_err(|error| unreadable(&error))?;
        killmail.raw = Some(text);
        Ok(Some(killmail))
    }
}

/// The killmail ids announced lately, oldest forgotten first.
#[derive(Default)]
struct RecentIds {
    order: VecDeque<u64>,
    known: HashSet<u64>,
}

impl RecentIds {
    /// Remembers `id`; `false` if it was already remembered.
    fn insert(&mut self, id: u64) -> bool {
        if !self.known.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > RECENT_IDS
            && let Some(oldest) = self.order.pop_front()
        {
            self.known.remove(&oldest);
        }
        true
    }
}

/// How a round of [`FeedCursor::advance`] ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Everything published so far was read; ask again after a while.
    Waiting,
    /// The feed could not be read; the same position is tried again later.
    Failed(FeedError),
    /// Whoever listens to the killmails is gone: stop.
    Closed,
}

/// Where the walk through the feed is.
pub(crate) struct FeedCursor {
    /// The next sequence number to read; `None` until the first read of
    /// `sequence.json` (and again after [`Self::resync`]).
    next: Option<u64>,
    /// Whether the end of the feed was reached since the walk (re)started.
    /// Until then the killmails read are history: remembered, not announced.
    caught_up: bool,
    recent: RecentIds,
    /// Pause between two files read back to back.
    pace: Duration,
    /// The latest number `sequence.json` has given. It is refreshed only
    /// every 51 killmails, so it can be behind the real end of the feed.
    latest: Option<u64>,
    /// Holes in the series skipped so far.
    holes: u64,
    /// Where the walk started: the pointer it began at.
    origin: Option<u64>,
    /// The killmails read on the way to the end of the feed, which are
    /// history and are not announced. See [`Self::take_history`].
    history: Vec<Killmail>,
}

impl FeedCursor {
    pub(crate) fn new(pace: Duration) -> Self {
        Self {
            next: None,
            caught_up: false,
            recent: RecentIds::default(),
            pace,
            latest: None,
            holes: 0,
            origin: None,
            history: Vec::new(),
        }
    }

    /// The pointer the walk started at, once known.
    pub(crate) fn origin(&self) -> Option<u64> {
        self.origin
    }

    /// The killmails read on the way to the end of the feed since the last
    /// call: not announced, but the context of the tooltips wants them.
    pub(crate) fn take_history(&mut self) -> Vec<Killmail> {
        std::mem::take(&mut self.history)
    }

    /// The next sequence number to read, once known.
    pub(crate) fn next(&self) -> Option<u64> {
        self.next
    }

    /// The latest number `sequence.json` has given.
    pub(crate) fn latest(&self) -> Option<u64> {
        self.latest
    }

    /// Whether the end of the feed was reached since the walk (re)started.
    pub(crate) fn is_caught_up(&self) -> bool {
        self.caught_up
    }

    /// Holes in the series skipped so far.
    pub(crate) fn holes(&self) -> u64 {
        self.holes
    }

    /// Starts over at the end of the feed the next time: what was published
    /// meanwhile is not announced. The ids already announced stay known.
    pub(crate) fn resync(&mut self) {
        self.next = None;
        self.origin = None;
        self.caught_up = false;
    }

    /// Reads every killmail available now, one after the other, and hands
    /// the new ones to `emit` (which returns `false` when nobody listens
    /// any more).
    ///
    /// A missing file ends the round, unless `sequence.json` says the feed
    /// is already past it: then it is a hole in the series and the walk goes
    /// on with the next number. A killmail that was announced before (the
    /// feed republishes the ones it reprocesses) is skipped.
    pub(crate) async fn advance<F: Feed>(
        &mut self,
        feed: &F,
        emit: &mut impl FnMut(Killmail) -> bool,
    ) -> Step {
        let mut next = match self.next {
            Some(next) => next,
            None => match feed.pointer().await {
                Ok(pointer) => {
                    self.latest = Some(pointer);
                    self.origin = Some(pointer);
                    self.next = Some(pointer);
                    self.caught_up = false;
                    pointer
                }
                Err(error) => return Step::Failed(error),
            },
        };
        loop {
            match feed.entry(next).await {
                Ok(Some(killmail)) => {
                    next += 1;
                    self.next = Some(next);
                    if self.recent.insert(killmail.killmail_id) {
                        if self.caught_up {
                            if !emit(killmail) {
                                return Step::Closed;
                            }
                        } else {
                            self.history.push(killmail);
                        }
                    }
                    if !self.pace.is_zero() {
                        tokio::time::sleep(self.pace).await;
                    }
                }
                Ok(None) => match feed.pointer().await {
                    Ok(pointer) if pointer > next => {
                        tracing::debug!(sequence = next, "zKillboard sequence hole skipped");
                        self.latest = Some(pointer);
                        self.holes += 1;
                        next += 1;
                        self.next = Some(next);
                    }
                    Ok(pointer) => {
                        self.latest = Some(pointer);
                        self.caught_up = true;
                        return Step::Waiting;
                    }
                    Err(error) => return Step::Failed(error),
                },
                Err(error) => return Step::Failed(error),
            }
        }
    }
}

/// The files of the feed published before the walk started, read to fill the
/// buffer of the node tooltips with the last hour. They are read oldest
/// first, a chunk per round of the live walk.
struct Backfill {
    /// The next sequence number to read.
    next: u64,
    /// Where it stops: the pointer the walk started at.
    end: u64,
    /// Where it started, to tell how far it got.
    first: u64,
    /// Pause between two files.
    pace: Duration,
}

impl Backfill {
    /// The files before `origin`, or `None` when there are none. With
    /// `resume`, the sequence number a previous run had got to, only the
    /// files published since then are read (the store has the rest), as long
    /// as that is less than the hour the backfill covers anyway.
    fn new(origin: u64, resume: Option<u64>) -> Option<Self> {
        let hour = origin.saturating_sub(BACKFILL_FILES).max(1);
        let first = resume.map_or(hour, |resume| resume.clamp(hour, origin));
        (first < origin).then_some(Self {
            next: first,
            end: origin,
            first,
            pace: Duration::ZERO,
        })
    }

    /// Reads the next chunk and hands each killmail to `sink`. `true` when
    /// every file was read. A failed read leaves the position where it was,
    /// to be tried again in the next round.
    async fn step<F: Feed>(&mut self, feed: &F, sink: &mut impl FnMut(&Killmail)) -> bool {
        for _ in 0..BACKFILL_CHUNK {
            if self.next >= self.end {
                break;
            }
            match feed.entry(self.next).await {
                Ok(Some(killmail)) => sink(&killmail),
                Ok(None) => {}
                Err(_) => return false,
            }
            self.next += 1;
            if !self.pace.is_zero() {
                tokio::time::sleep(self.pace).await;
            }
        }
        self.next >= self.end
    }

    /// How much of the files was read.
    fn progress(&self) -> Fill {
        Fill {
            done: self.next - self.first,
            total: self.end - self.first,
        }
    }
}

/// Waits between two attempts while the feed can't be reached: 6 seconds,
/// doubling up to a minute.
struct Backoff {
    delay: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self { delay: POLL_WAIT }
    }

    /// The wait before the next attempt, longer than the last one.
    fn next(&mut self) -> Duration {
        let delay = self.delay;
        self.delay = (self.delay * 2).min(MAX_BACKOFF);
        delay
    }

    fn reset(&mut self) {
        self.delay = POLL_WAIT;
    }
}

/// What the source is doing, as the Settings page tells it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FeedState {
    /// Not reading: no rule asks for the feed, or it is paused.
    #[default]
    Off,
    /// Switched on, the first read is still to come.
    Starting,
    /// Reading the feed.
    Live,
    /// The last read failed; it is tried again.
    Failing,
}

/// How the walk through the feed is going. Only the debug window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) struct FeedDiagnostics {
    /// The next sequence number to read.
    pub next_sequence: Option<u64>,
    /// The latest number `sequence.json` has given (refreshed every 51
    /// killmails, so it can be behind the real end of the feed).
    pub latest_sequence: Option<u64>,
    /// Whether the end of the feed was reached since the walk (re)started.
    pub caught_up: bool,
    /// Holes in the series skipped.
    pub holes_skipped: u64,
    /// Times the walk was sent back to the end of the feed after a failure.
    pub resyncs: u64,
    /// Rounds of reading the feed.
    pub polls: u64,
    /// The wait before the next round.
    pub next_delay: Option<Duration>,
}

/// What the source has done since it was last switched on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeedStatus {
    pub diagnostics: FeedDiagnostics,
    pub state: FeedState,
    /// Why the last read failed, while [`FeedState::Failing`].
    pub last_error: Option<String>,
    /// When the last killmail was passed to the rules.
    pub last_killmail: Option<DateTime<Utc>>,
    /// Killmails passed to the rules.
    pub announced: u64,
    /// Killmails left out by the filters of the settings.
    pub filtered: u64,
}

/// The kills of the last hour ESI reported, one request for the whole
/// universe: the first reading of the heat, until the killmails of the feed
/// and the query API make it up (see [`ZKillboardHandle::hour_kills`]).
struct EsiHour {
    /// When ESI computed it: the hour it counts ends then.
    modified: DateTime<Utc>,
    /// Ships and capsules of players destroyed in each system (NPCs left out).
    kills: HashMap<usize, usize>,
}

impl EsiHour {
    /// The answer of ESI, if it says when it was computed.
    fn new(hour: &HourKills) -> Option<Self> {
        let kills = hour
            .systems
            .iter()
            .filter_map(|system| {
                let kills = system.ship_kills.saturating_add(system.pod_kills);
                let (Ok(id), Ok(kills)) =
                    (usize::try_from(system.system_id), usize::try_from(kills))
                else {
                    return None;
                };
                (kills > 0).then_some((id, kills))
            })
            .collect();
        Some(Self {
            modified: hour.modified?,
            kills,
        })
    }

    /// How many of the kills ESI reported in `system` are still inside the
    /// last [`WINDOW`] as of `now`. ESI counts the hour that ended at
    /// `modified`; as time goes by the oldest of those kills leave the
    /// window, and where they were within the hour is not known, so they are
    /// taken to be spread evenly: the share left falls from all of them to
    /// none over the hour.
    fn remaining(&self, system: usize, now: DateTime<Utc>) -> usize {
        let Some(&kills) = self.kills.get(&system) else {
            return 0;
        };
        let aged = (now - self.modified).num_milliseconds() as f64
            / WINDOW.num_milliseconds().max(1) as f64;
        let left = (1.0 - aged).clamp(0.0, 1.0);
        (kills as f64 * left).round() as usize
    }
}

/// What the UI and the source thread share.
struct Shared {
    /// Whether the graph has an enabled zKillboard Input node.
    enabled: AtomicBool,
    /// Whether the settings pause the source.
    paused: AtomicBool,
    /// Killmails worth less than this many ISK are left out.
    min_isk: AtomicU64,
    /// Killmails with no pilot among the attackers are left out.
    ignore_npc_only: AtomicBool,
    /// What the source has done, for the Settings page.
    status: Mutex<FeedStatus>,
    /// The killmails of the last hour, for the node tooltips.
    buffer: Mutex<KillBuffer>,
    /// The killmails of the last day per system, asked of the query API.
    days: Mutex<DayCache>,
    /// The requests to the query API waiting for their turn: the only thing
    /// that paces them.
    api_lane: LaneQueue<usize>,
    /// The requests to the R2Z2 feed waiting for their turn.
    r2z2_lane: Arc<LaneQueue<()>>,
    /// The killmails of the last hour read from the query API while the feed is not
    /// running (see [`kill_day`]).
    api_buffer: Mutex<KillBuffer>,
    /// The newest hourly answer of ESI's `system_kills`.
    esi_hour: Mutex<Option<EsiHour>>,
    /// Whether the hour of ESI the store kept has been read.
    esi_hour_loaded: AtomicBool,
    /// How old the answer of ESI may be and still count for the heat.
    esi_max_age: TimeDelta,
    /// Whether a map shows the heat, so that the answer of ESI is worth
    /// asking for.
    heat_wanted: AtomicBool,
    /// Whether the buffer still lacks the hour before the source started:
    /// set when it starts, cleared once the feed's files are read.
    starting_short: AtomicBool,
    /// Until when (Unix seconds) the buffer lacks the killmails published
    /// while the feed could not be read; 0 when it lacks none.
    outage_until: AtomicI64,
    /// How the counts of the last day are filled in the background.
    prefetch: Prefetch,
    /// Wakes the day fetcher when a system is queued.
    day_wake: Notify,
    /// Wakes the source when it changes.
    wake: Notify,
    /// The SDE database the names come from.
    sde: Arc<RwLock<PathBuf>>,
    /// Names of the systems, for the Debug window.
    #[cfg(debug_assertions)]
    names: SdeNames,
    /// What was learned, kept on disk between runs.
    store: CacheStore,
    /// Keeps the requests to zKillboard away while it asked for it, even
    /// after a restart: one guard for each endpoint.
    guards: Guards,
}

impl Shared {
    /// Puts into `buffer` the killmails of the last hour the store kept.
    fn load_records(&self, buffer: &mut KillBuffer) {
        let now = Utc::now();
        for record in self.store.records(now) {
            buffer.insert(record, now);
        }
    }

    /// Takes back what the store kept for the tooltips that don't have the
    /// feed: the killmails read from the query API and the counts of the last day.
    #[tracing::instrument(skip_all)]
    fn restore(&self) {
        self.load_records(&mut self.api_buffer());
        let counts = self.store.days(
            Utc::now(),
            TimeDelta::from_std(kill_day::TTL).unwrap_or(TimeDelta::hours(1)),
        );
        self.days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .restore(counts, Instant::now());
    }

    /// Keeps the counts of the last day that are known.
    #[tracing::instrument(skip_all)]
    fn save_days(&self) {
        let counts = self
            .days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot(Instant::now());
        self.store.save_days(&counts, Utc::now());
    }

    /// Whether the source should be reading the feed: a rule asks for it and
    /// the settings don't pause it.
    fn active(&self) -> bool {
        self.enabled.load(Ordering::Acquire) && !self.paused.load(Ordering::Acquire)
    }

    /// Whether the feed is running but the buffer may lack killmails, so
    /// the query API makes up for it: the hour before the start is
    /// still being read, the feed can't be read now, or it could not be a
    /// while ago (what was published then is not coming back). False when the
    /// feed is not running: the API is then the only source, not a help.
    fn compensating(&self) -> bool {
        if !self.active() {
            return false;
        }
        let failing = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .state
            == FeedState::Failing;
        failing
            || self.starting_short.load(Ordering::Acquire)
            || Utc::now().timestamp() < self.outage_until.load(Ordering::Acquire)
    }

    /// The feed could not be read at `now` (throttled with a 429 or 403,
    /// unreachable, or any other error): from this moment the API makes up
    /// for it, and keeps doing so until the hour that may lack killmails is
    /// gone. A new failure extends it.
    fn feed_failed(&self, now: DateTime<Utc>) {
        // What the feed missed is not in the counts it was keeping up.
        self.days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .feed_down();
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .state = FeedState::Failing;
        self.outage_until
            .store((now + WINDOW).timestamp(), Ordering::Release);
    }

    /// Sends the state of the lanes to Tracy's plots: how many requests wait
    /// in each (the API's, split into tooltips and background) and how many
    /// each endpoint has let through. Does nothing without the profiler.
    #[cfg(feature = "profile")]
    fn plot_lanes(&self) {
        use tracing_tracy::client::{Client, plot_name};
        let Some(client) = Client::running() else {
            return;
        };
        let api = self.api_lane.lengths();
        let background: usize = api.iter().skip(1).sum();
        client.plot(plot_name!("zkill api lane: tooltips"), api[0] as f64);
        client.plot(plot_name!("zkill api lane: background"), background as f64);
        client.plot(plot_name!("zkill r2z2 lane"), self.r2z2_lane.len() as f64);
        client.plot(
            plot_name!("zkill api requests sent"),
            self.guards.get(Endpoint::Api).sent() as f64,
        );
        client.plot(
            plot_name!("zkill r2z2 requests sent"),
            self.guards.get(Endpoint::R2z2).sent() as f64,
        );
    }

    #[cfg(not(feature = "profile"))]
    fn plot_lanes(&self) {}

    /// What the tooltip can say about the last day of `system_id`. When the
    /// count is not at hand its request goes to the API lane (delayed: see
    /// [`Prefetch::tooltip`]); the tooltip asks again on every frame, which
    /// keeps the request from lapsing while it stays.
    fn look_day(&self, system_id: usize) -> DayLine {
        let (line, ask) = self
            .days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .look(system_id, Instant::now());
        if ask {
            self.api_lane.submit(self.prefetch.tooltip(system_id));
            self.day_wake.notify_one();
        }
        line
    }

    /// Keeps the killmails of the last hour that the query API listed, for
    /// the tooltips: in the buffer of the API and in the store (see
    /// [`Self::keep`]).
    #[tracing::instrument(skip_all, fields(kills = kills.len()))]
    fn keep_day_kills(&self, kills: Vec<Killmail>, names: &SdeNames) {
        let now = Utc::now();
        let oldest = now.checked_sub_signed(WINDOW);
        for killmail in kills {
            let record = killmail.to_record(names);
            if oldest.is_some_and(|oldest| record.time < oldest) {
                continue;
            }
            if self.keep(record.clone(), now) {
                self.store
                    .insert_with(&record, killmail.raw.as_deref(), now);
            }
        }
    }

    /// Keeps a killmail read from the query API for the tooltips: in the buffer of the
    /// API and, while the feed runs, in the feed's, which is the one that
    /// shows (a killmail both have is kept once). `true` when it was new to
    /// the buffer of the API.
    fn keep(&self, record: KillRecord, now: DateTime<Utc>) -> bool {
        if self.active() {
            self.buffer().insert(record.clone(), now);
        }
        let mut buffer = self.api_buffer();
        let added = buffer.insert(record, now);
        buffer.prune(now);
        added
    }

    /// How long to stay away after zKillboard refused a request of `endpoint`:
    /// what is left of the pause its guard remembers, or a whole block.
    fn pause_after_throttle(&self, endpoint: Endpoint) -> Duration {
        let now = Utc::now();
        self.guards.refusal_seen(endpoint, now);
        let guard = self.guards.get(endpoint);
        guard.wait(now).unwrap_or_else(|| guard.block())
    }

    /// Whether the tooltips may ask the query API: the settings
    /// don't pause zKillboard.
    fn fetching(&self) -> bool {
        !self.paused.load(Ordering::Acquire)
    }

    /// The killmails read from the query API for the node tooltips.
    fn api_buffer(&self) -> std::sync::MutexGuard<'_, KillBuffer> {
        self.api_buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes from the store the hour of ESI it kept (so that a restart does
    /// not ask again), once it can be read. A newer hour already known stays.
    fn load_esi_hour(&self) {
        if self.esi_hour_loaded.load(Ordering::Acquire) {
            return;
        }
        let Some(saved) = self.store.esi_hour() else {
            return;
        };
        self.esi_hour_loaded.store(true, Ordering::Release);
        let Some((modified, counts)) = saved else {
            return;
        };
        let mut kept = self
            .esi_hour
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if kept.as_ref().is_none_or(|known| known.modified < modified) {
            *kept = Some(EsiHour {
                modified,
                kills: counts.into_iter().collect(),
            });
        }
    }

    /// The killmails per system the buffers hold as of `now` (the feed's only
    /// while it runs), the most either has; those after `since` when given.
    fn live_counts(
        &self,
        since: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> HashMap<usize, usize> {
        let read = |buffer: &KillBuffer| match since {
            Some(since) => buffer.counts_since(since, now),
            None => buffer.counts(now),
        };
        let mut totals: HashMap<usize, usize> = HashMap::new();
        let mut counts = read(&self.api_buffer());
        if self.active() {
            counts.extend(read(&self.buffer()));
        }
        for (system, kills) in counts {
            let known = totals.entry(system).or_default();
            *known = (*known).max(kills);
        }
        totals
    }

    /// The killmails kept for the node tooltips.
    fn buffer(&self) -> std::sync::MutexGuard<'_, KillBuffer> {
        self.buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The UI's handle on the zKillboard source: tells it whether anything wants
/// its killmails, and where the SDE is. Cheap to clone.
#[derive(Clone)]
pub(crate) struct ZKillboardHandle(Arc<Shared>);

impl ZKillboardHandle {
    /// A handle for a source that is off, reading names from the SDE at `sde`.
    pub(crate) fn new(sde: &Path) -> Self {
        let sde = Arc::new(RwLock::new(sde.to_path_buf()));
        Self(Arc::new(Shared {
            enabled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            min_isk: AtomicU64::new(0),
            ignore_npc_only: AtomicBool::new(false),
            status: Mutex::new(FeedStatus::default()),
            buffer: Mutex::new(KillBuffer::default()),
            days: Mutex::new(DayCache::default()),
            api_lane: LaneQueue::new(LaneConfig::api()),
            r2z2_lane: Arc::new(LaneQueue::new(LaneConfig::r2z2())),
            api_buffer: Mutex::new(KillBuffer::default()),
            esi_hour: Mutex::new(None),
            esi_hour_loaded: AtomicBool::new(false),
            esi_max_age: TimeDelta::hours(2),
            heat_wanted: AtomicBool::new(false),
            starting_short: AtomicBool::new(false),
            outage_until: AtomicI64::new(0),
            prefetch: Prefetch::default(),
            day_wake: Notify::new(),
            wake: Notify::new(),
            #[cfg(debug_assertions)]
            names: SdeNames::new(Arc::clone(&sde)),
            store: CacheStore::new(Arc::clone(&sde), CacheStoreConfig::default()),
            guards: Guards::default(),
            sde,
        }))
    }

    /// Whether `graph` has an enabled zKillboard Input node, that is, whether
    /// anything wants the killmails.
    pub(crate) fn wanted_by(graph: &RuleGraph) -> bool {
        graph.nodes.iter().any(|node| {
            node.enabled
                && matches!(&node.kind, NodeKind::Input(input) if input.kind == InputKind::ZKillboard)
        })
    }

    /// Switches the source on or off to what `graph` needs.
    pub(crate) fn follow(&self, graph: &RuleGraph) {
        self.set_enabled(Self::wanted_by(graph));
    }

    /// Switches the source on or off. Off, it stops reading the feed and
    /// forgets where it was: on again, it starts over at the end of the feed.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        if self.0.enabled.swap(enabled, Ordering::AcqRel) != enabled {
            self.0.wake.notify_one();
        }
    }

    /// Applies the settings: whether the source is paused and the filters.
    pub(crate) fn set_options(&self, options: &ZKillboardOptions) {
        self.0.min_isk.store(options.min_isk, Ordering::Release);
        self.0
            .ignore_npc_only
            .store(options.ignore_npc_only, Ordering::Release);
        let paused = !options.enabled;
        if self.0.paused.swap(paused, Ordering::AcqRel) != paused {
            if paused {
                // Nothing is asked while paused, and what was asked would be
                // stale when it is switched on again.
                self.0
                    .days
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clear();
                self.0.api_lane.replace_background([]);
                self.0.api_buffer().clear();
            } else {
                self.0.restore();
            }
            self.0.wake.notify_one();
        }
    }

    /// What the source is doing and has done, for the Settings page.
    pub(crate) fn status(&self) -> FeedStatus {
        self.0
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// How far the reading of the hour before the start got, while it is
    /// going on.
    pub(crate) fn fill(&self) -> Option<Fill> {
        self.0.buffer().fill()
    }

    /// What the killmails of the last hour say about the system
    /// `system_id`, or `None` while zKillboard is paused in the settings
    /// (nothing is known then, and a stale answer would mislead).
    ///
    /// While the feed runs they come from its buffer. Without it they come
    /// from the query API, through its lane (see [`kill_day`]): the
    /// context says it is still `loading` until they are in.
    ///
    /// When the feed runs but the buffer may lack killmails (the hour before
    /// the start is still being read, or the feed could not be read for a
    /// while: see [`Shared::compensating`]) the API makes up for it:
    /// what they read goes into the feed's buffer and the day line comes
    /// from the API, as without the feed.
    pub(crate) fn context(&self, system_id: usize) -> Option<KillContext> {
        if !self.0.fetching() {
            return None;
        }
        let from_feed = self.0.active();
        let compensating = self.0.compensating();
        let now = Utc::now();
        if from_feed && !compensating {
            // The count of the day is what the background fill has found
            // around the characters; any other system is asked for the
            // first time its tooltip is shown. (The killmails themselves
            // are not: the feed gives them.)
            let line = self.0.look_day(system_id);
            let mut context = self.0.buffer().context(system_id, now);
            context.day = Some(line);
            return Some(context);
        }
        let line = self.0.look_day(system_id);
        let mut context = if from_feed {
            self.0.buffer().context(system_id, now)
        } else {
            self.0.api_buffer().context(system_id, now)
        };
        let feed_failing = self.status().state == FeedState::Failing;
        context.source = if from_feed && !feed_failing {
            Source::Feed
        } else {
            Source::Api
        };
        // The counts may be short while the API is still filling them in.
        context.updating |= compensating;
        context.loading = line == DayLine::Pending;
        context.day = Some(line);
        Some(context)
    }

    /// How the counts of the last day are filled in the background.
    pub(crate) fn prefetch_settings(&self) -> Prefetch {
        self.0.prefetch
    }

    /// Makes the day fetcher fill in the counts of the last day on its own:
    /// first the systems of `jumps` around `origins` (the characters),
    /// nearest first, then the rest of the regions the origins are in, and
    /// last the regions next to them (see [`Prefetch::neighbor_systems`]).
    /// The other systems wait for a tooltip to ask. Replaces the previous
    /// plan.
    /// Does nothing while the settings pause zKillboard.
    pub(crate) fn prefetch_days(&self, jumps: &JumpGraph, origins: &[u32]) {
        if !self.0.fetching() {
            return;
        }
        let path = self
            .0
            .sde
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let region = SdeRegions::systems_with(&path, origins);
        let neighbors = Prefetch::neighbor_systems(jumps, &SdeRegions::layout(&path), &region);
        let plan = self.0.prefetch.plan(jumps, origins, &region, &neighbors);
        let jobs = self
            .0
            .days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .schedule(&plan, &self.0.prefetch, Instant::now());
        // The plan is made again from scratch; the tooltips' requests stay.
        self.0.api_lane.replace_background(jobs);
        self.0.day_wake.notify_one();
        self.0.plot_lanes();
    }

    /// The requests waiting to be made to the query API, the first
    /// `limit` of them, with when each is estimated to go out.
    #[cfg(debug_assertions)]
    pub(crate) fn day_queue(&self, limit: usize) -> kill_day::DayQueue {
        let start = self
            .0
            .guards
            .get(Endpoint::Api)
            .wait(Utc::now())
            .unwrap_or_default();
        let lane = &self.0.api_lane;
        kill_day::DayQueue {
            requests: lane
                .forecast(limit)
                .into_iter()
                .map(|queued| kill_day::QueuedRequest {
                    system_id: queued.key as usize,
                    kind: kill_day::QueueKind::of(queued.priority),
                    eta: start + queued.eta,
                })
                .collect(),
            total: lane.len(),
        }
    }

    /// How many requests wait in the lane of `endpoint`.
    #[cfg(debug_assertions)]
    pub(crate) fn requests_waiting(&self, endpoint: Endpoint) -> usize {
        match endpoint {
            Endpoint::Api => self.0.api_lane.len(),
            Endpoint::R2z2 => self.0.r2z2_lane.len(),
        }
    }

    /// How long zKillboard asks us to keep away from `endpoint`, if it does.
    #[cfg(debug_assertions)]
    pub(crate) fn request_wait(&self, endpoint: Endpoint) -> Option<Duration> {
        self.0.guards.get(endpoint).wait(Utc::now())
    }

    /// How many requests the guard of `endpoint` has let through.
    #[cfg(debug_assertions)]
    pub(crate) fn requests_sent(&self, endpoint: Endpoint) -> u64 {
        self.0.guards.get(endpoint).sent()
    }

    /// The name of a solar system, from the SDE.
    #[cfg(debug_assertions)]
    pub(crate) fn system_name(&self, system_id: usize) -> Option<String> {
        self.0.names.system_name(u32::try_from(system_id).ok()?)
    }

    /// How long until the plan of the day counts should be made again: when
    /// the first count of the last plan comes due, between
    /// [`Prefetch::replan_floor`] and [`Prefetch::replan_every`].
    pub(crate) fn day_replan_in(&self) -> Duration {
        let prefetch = self.0.prefetch;
        self.0
            .days
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .next_due(&prefetch, Instant::now())
            .map_or(prefetch.replan_every, |due| {
                due.max(prefetch.replan_floor).min(prefetch.replan_every)
            })
    }

    /// Whether the query API is in use for the tooltips: the
    /// settings don't pause zKillboard and the feed is not running, or it
    /// runs and the API makes up for what it lacks.
    pub(crate) fn api_in_use(&self) -> bool {
        self.0.fetching() && (!self.0.active() || self.0.compensating())
    }

    /// The killmails of the last hour per system, for the heat of the maps.
    ///
    /// With the feed running and nothing to make up for, the feed's buffer is
    /// exact. Otherwise (the feed is off, still reading the hour before it
    /// started, or failing) the buffers hold only part of the hour, so the
    /// answer of ESI ([`Self::record_hour`]) is the first reading, and the
    /// killmails since it was computed are added to it. Each system shows the
    /// most of what the sources say, so that the reading only goes up as the
    /// feed and the query API fill in. Empty when the source is off.
    ///
    /// The estimate from ESI is its count aged by the time since it was
    /// computed (see [`EsiHour::remaining`]) plus the killmails since, so it
    /// does not count twice the hour that has already left the window.
    pub(crate) fn hour_kills(&self) -> Vec<(usize, usize)> {
        if !self.0.fetching() {
            return Vec::new();
        }
        let now = Utc::now();
        if self.0.active() && !self.0.compensating() {
            return self.0.buffer().counts(now);
        }
        self.0.load_esi_hour();
        let mut totals = self.0.live_counts(None, now);
        let esi = self
            .0
            .esi_hour
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(esi) = esi
            .as_ref()
            .filter(|esi| now - esi.modified < self.0.esi_max_age)
        {
            let after = self.0.live_counts(Some(esi.modified), now);
            let systems: HashSet<usize> = esi.kills.keys().chain(after.keys()).copied().collect();
            for system in systems {
                let estimate =
                    esi.remaining(system, now) + after.get(&system).copied().unwrap_or(0);
                let known = totals.entry(system).or_default();
                *known = (*known).max(estimate);
            }
        }
        totals.into_iter().collect()
    }

    /// Keeps the answer of ESI's `system_kills` when it is a newer hour than
    /// the one kept (ESI repeats its answer until the next one is ready).
    /// Returns whether it was. An answer without a date is ignored.
    pub(crate) fn record_hour(&self, hour: &HourKills) -> bool {
        let Some(fresh) = EsiHour::new(hour) else {
            return false;
        };
        self.0.load_esi_hour();
        let mut kept = self
            .0
            .esi_hour
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if kept
            .as_ref()
            .is_some_and(|old| old.modified >= fresh.modified)
        {
            return false;
        }
        let counts: Vec<(usize, usize)> = fresh.kills.iter().map(|(s, k)| (*s, *k)).collect();
        let modified = fresh.modified;
        *kept = Some(fresh);
        drop(kept);
        self.0.store.save_esi_hour(modified, &counts);
        true
    }

    /// When the answer of ESI kept was computed; the next is ready an hour
    /// later.
    pub(crate) fn hour_modified(&self) -> Option<DateTime<Utc>> {
        self.0.load_esi_hour();
        self.0
            .esi_hour
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|esi| esi.modified)
    }

    /// Whether the feed alone gives the killmails of the whole last hour:
    /// it runs and nothing is left to make up for. Then the answer of ESI
    /// has nothing to add.
    pub(crate) fn covers_the_hour(&self) -> bool {
        self.0.fetching() && self.0.active() && !self.0.compensating()
    }

    /// Says whether a map shows the heat: only then is the answer of ESI
    /// asked for.
    pub(crate) fn set_heat_wanted(&self, wanted: bool) {
        self.0.heat_wanted.store(wanted, Ordering::Release);
    }

    /// Whether a map shows the heat.
    pub(crate) fn heat_wanted(&self) -> bool {
        self.0.heat_wanted.load(Ordering::Acquire)
    }

    /// Whether a rule asks for the killmails (an enabled zKillboard input),
    /// whether or not the settings pause the source.
    pub(crate) fn is_wanted(&self) -> bool {
        self.0.enabled.load(Ordering::Acquire)
    }

    /// Whether the source is on.
    #[cfg(test)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.0.enabled.load(Ordering::Acquire)
    }

    /// Points the source at the SDE database at `path`. Does nothing when it
    /// is the one already in use.
    pub(crate) fn set_sde_path(&self, path: &Path) {
        let mut current = self
            .0
            .sde
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.as_path() != path {
            *current = path.to_path_buf();
        }
    }

    /// Spawns the source thread and returns immediately. The thread idles
    /// until [`Self::set_enabled`] switches it on, and ends when `input` has
    /// no receiver left.
    ///
    /// `resolver` is the one the detection stage uses (see
    /// [`KillmailExt::to_line`]); `app_msg` is where trouble with the feed is
    /// reported to.
    pub(crate) fn spawn(
        &self,
        input: mpsc::Sender<InputEvent>,
        resolver: ResolverHandle,
        app_msg: Arc<mpsc::Sender<Message>>,
        r2z2_store: Box<dyn GuardStore>,
        api_store: Box<dyn GuardStore>,
    ) {
        // What zKillboard asked of the previous runs counts from the start.
        let now = Utc::now();
        self.0.guards.get(Endpoint::R2z2).attach(r2z2_store, now);
        self.0.guards.get(Endpoint::Api).attach(api_store, now);
        let session = Session {
            shared: Arc::clone(&self.0),
            names: SdeNames::new(Arc::clone(&self.0.sde)),
            input,
            resolver,
            app_msg,
        };
        let spawned = std::thread::Builder::new()
            .name(String::from("telescope-intel-zkillboard"))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("zKillboard thread runtime");
                runtime.block_on(session.run());
            });
        if let Err(error) = spawned {
            tracing::error!("could not start the zKillboard thread: {error}");
        }
        let shared = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new()
            .name(String::from("telescope-intel-zkillboard-days"))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("zKillboard days runtime");
                runtime.block_on(Self::serve_days(&shared));
            });
        if let Err(error) = spawned {
            tracing::error!("could not start the zKillboard days thread: {error}");
        }
    }

    /// The day fetcher's life: asks the query API about the systems its lane
    /// hands out, in the pace the lane sets, while the source is on.
    async fn serve_days(shared: &Shared) {
        let api = match ZKillApi::new() {
            Ok(api) => api.with_guard(Arc::clone(shared.guards.get(Endpoint::Api))),
            Err(error) => {
                tracing::error!("could not start the zKillboard API client: {error}");
                return;
            }
        };
        let names = SdeNames::new(Arc::clone(&shared.sde));
        shared.restore();
        let mut unsaved = 0_u32;
        loop {
            if !shared.fetching() {
                shared.day_wake.notified().await;
                continue;
            }
            shared.plot_lanes();
            let fetched = DayCache::fetch_next(
                &shared.days,
                &shared.api_lane,
                &api,
                &shared.prefetch,
                || shared.fetching(),
            )
            .await;
            match fetched {
                Fetched::Skipped => continue,
                Fetched::Throttled => {
                    // The lane stays quiet for as long as the guard says.
                    shared
                        .api_lane
                        .hold_for(shared.pause_after_throttle(Endpoint::Api));
                    continue;
                }
                Fetched::Failed => {
                    let failures = shared
                        .days
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .failures();
                    if failures == shared.prefetch.backoff_after {
                        tracing::warn!(
                            failures,
                            "the day counts keep failing: asking zKillboard less often"
                        );
                    }
                }
                Fetched::Answered { kills } => {
                    // The killmails of the last hour are only needed when the
                    // feed is not giving them (yet).
                    if !shared.active() || shared.compensating() {
                        shared.keep_day_kills(kills, &names);
                    }
                }
            }
            // Not after each answer: the plan asks for thousands.
            unsaved += 1;
            if unsaved >= shared.prefetch.save_every || shared.api_lane.is_empty() {
                shared.save_days();
                unsaved = 0;
            }
        }
    }
}

/// Everything the source thread works with.
struct Session {
    shared: Arc<Shared>,
    names: SdeNames,
    input: mpsc::Sender<InputEvent>,
    resolver: ResolverHandle,
    app_msg: Arc<mpsc::Sender<Message>>,
}

impl Session {
    /// The source's life: idle while switched off, walking the feed while on.
    async fn run(self) {
        let client = match R2z2Client::new() {
            Ok(client) => client.with_guard(Arc::clone(self.shared.guards.get(Endpoint::R2z2))),
            Err(error) => {
                self.report(Type::Error, &format!("could not start: {error}"));
                return;
            }
        };
        let client = client.with_slots(FeedSlots::new(Arc::clone(&self.shared.r2z2_lane)));
        // The hour read back when the feed starts waits behind the live walk.
        let history_client = client.with_priority(Priority::Normal);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        let mut backoff = Backoff::new();
        let mut failing_since: Option<Instant> = None;
        let mut reported = false;
        let mut resyncs = 0_u64;
        let mut backfill: Option<Backfill> = None;
        let mut backfilled = false;
        let mut loaded = false;
        let mut resume: Option<u64> = None;
        loop {
            if !self.shared.active() {
                self.shared.starting_short.store(false, Ordering::Release);
                self.shared.outage_until.store(0, Ordering::Release);
                resyncs = 0;
                backfill = None;
                backfilled = false;
                loaded = false;
                resume = None;
                self.shared.buffer().clear();
                self.shared
                    .days
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .feed_down();
                cursor.resync();
                backoff.reset();
                failing_since = None;
                reported = false;
                self.update(|status| *status = FeedStatus::default());
                self.shared.wake.notified().await;
                continue;
            }
            self.update(|status| {
                if status.state == FeedState::Off {
                    status.state = FeedState::Starting;
                }
            });
            // Once per start: what the previous runs kept, so that the hour
            // before the start is not read from the feed again.
            if !loaded {
                loaded = true;
                self.shared.starting_short.store(true, Ordering::Release);
                self.shared.load_records(&mut self.shared.buffer());
                resume = self.shared.store.feed_next();
            }
            let delay = match cursor.advance(&client, &mut |kill| self.emit(kill)).await {
                Step::Waiting => {
                    // At the end of the feed: from here on it sees everything.
                    self.shared
                        .days
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .feed_up(Instant::now());
                    backoff.reset();
                    failing_since = None;
                    reported = false;
                    self.update(|status| {
                        status.state = FeedState::Live;
                        status.last_error = None;
                    });
                    POLL_WAIT
                }
                Step::Closed => return,
                Step::Failed(error) => {
                    if failing_since.get_or_insert_with(Instant::now).elapsed() > RESYNC_AFTER {
                        resyncs += 1;
                        cursor.resync();
                    }
                    let delay = match error {
                        FeedError::Throttled(_) | FeedError::Paused => {
                            self.shared.pause_after_throttle(Endpoint::R2z2)
                        }
                        FeedError::Other(_) => backoff.next(),
                    };
                    self.update(|status| {
                        status.state = FeedState::Failing;
                        status.last_error = Some(error.to_string());
                    });
                    // What is published from now on may never be read: the API
                    // makes up for it until the hour it left out is gone.
                    self.shared.feed_failed(Utc::now());
                    // Once per outage: the logs are not for a retry loop (the
                    // wake-ups at the start would repeat it at once).
                    if reported {
                        tracing::debug!(%error, ?delay, "zKillboard feed not read");
                    } else {
                        tracing::warn!(%error, ?delay, "zKillboard feed not read");
                        reported = true;
                        self.report(
                            Type::Warning,
                            &format!("{error}; trying again in {}s", delay.as_secs()),
                        );
                    }
                    delay
                }
            };
            for killmail in cursor.take_history() {
                self.record(&killmail);
            }
            // Once per start, when the end of the feed is reached: the hour
            // before it, a chunk per round.
            if !backfilled && cursor.is_caught_up() {
                backfilled = true;
                backfill = cursor
                    .origin()
                    .and_then(|origin| Backfill::new(origin, resume));
            }
            if let Some(files) = backfill.as_mut() {
                let finished = files
                    .step(&history_client, &mut |killmail| self.record(killmail))
                    .await;
                let progress = (!finished).then(|| files.progress());
                self.shared.buffer().set_fill(progress);
                if finished {
                    backfill = None;
                }
            }
            self.shared.buffer().prune(Utc::now());
            // Everything up to here is in the store only once the files
            // before the start are read too.
            if backfilled && backfill.is_none() {
                self.shared.starting_short.store(false, Ordering::Release);
                if let Some(next) = cursor.next() {
                    self.shared.store.set_feed_next(next);
                }
            }
            self.update(|status| {
                status.diagnostics = FeedDiagnostics {
                    next_sequence: cursor.next(),
                    latest_sequence: cursor.latest(),
                    caught_up: cursor.is_caught_up(),
                    holes_skipped: cursor.holes(),
                    resyncs,
                    polls: status.diagnostics.polls + 1,
                    next_delay: Some(delay),
                };
            });
            self.shared.plot_lanes();
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = self.shared.wake.notified() => {}
            }
        }
    }

    /// Keeps the killmail for the node tooltips, whatever the filters say.
    fn record(&self, killmail: &Killmail) {
        let record = killmail.to_record(&self.names);
        let now = Utc::now();
        // The count of the last day of its system follows the feed.
        if let Some(time) = killmail.time() {
            self.shared
                .days
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observe(
                    killmail.esi.solar_system_id as usize,
                    killmail.killmail_id,
                    time,
                );
        }
        if self.shared.buffer().insert(record.clone(), now) {
            self.shared
                .store
                .insert_with(&record, killmail.raw.as_deref(), now);
        }
    }

    /// Sends one killmail to the detection stage. `false` when it has no
    /// receiver any more.
    fn emit(&self, killmail: Killmail) -> bool {
        self.record(&killmail);
        let worth = killmail.is_worth(
            self.shared.min_isk.load(Ordering::Acquire),
            self.shared.ignore_npc_only.load(Ordering::Acquire),
        );
        if !worth {
            self.update(|status| status.filtered += 1);
            return true;
        }
        let decoder = ZKillboardDecoder::new(&self.names, self.resolver.as_ref());
        let Some(event) = decoder.decode(&killmail) else {
            tracing::trace!(
                killmail = killmail.killmail_id,
                "killmail without a system name"
            );
            return true;
        };
        if event.time_origin == TimeOrigin::Assumed {
            tracing::warn!(killmail = killmail.killmail_id, "killmail without a time");
            self.report(
                Type::Warning,
                &format!(
                    "Killmail {} has no readable time: it is taken to happen when it was read.",
                    killmail.killmail_id
                ),
            );
        }
        let event = InputEvent::from_event(format!("r2z2:{}", killmail.sequence_id), event);
        match self.input.try_send(event) {
            Ok(()) => {
                self.update(|status| {
                    status.announced += 1;
                    status.last_killmail = Some(Utc::now());
                });
                true
            }
            // Same policy as the chat-log reader: a lost line is better than
            // a stalled reader.
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    killmail = killmail.killmail_id,
                    "intel queue full, killmail dropped"
                );
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Changes what the Settings page shows of the source.
    fn update(&self, change: impl FnOnce(&mut FeedStatus)) {
        let mut status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut status);
    }

    /// Tells the UI about trouble with the feed.
    fn report(&self, kind: Type, text: &str) {
        let _ = try_send_app_message(
            &self.app_msg,
            Message::GenericNotification((
                kind,
                String::from("zKillboard"),
                String::from("r2z2"),
                text.to_string(),
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use webb::esi::SystemHourKills;
    use webb::graph::Executor;
    use webb::rules::parse_line;
    use zkill::types::{EsiKillmail, Victim, Zkb};

    /// Samples for the heat tests.
    struct Sample;

    impl Sample {
        fn esi_hour(modified: DateTime<Utc>) -> HourKills {
            let system = |system_id, ship_kills, pod_kills, npc_kills| SystemHourKills {
                system_id,
                ship_kills,
                pod_kills,
                npc_kills,
            };
            HourKills {
                modified: Some(modified),
                // Ships and capsules count, the NPCs do not.
                systems: vec![system(30000142, 3, 1, 50), system(30000144, 0, 0, 9)],
            }
        }

        fn killmail(id: u64, system_id: usize, time: DateTime<Utc>) -> KillRecord {
            KillRecord {
                killmail_id: id,
                time,
                system_id,
                victim: None,
                attacker_ships: Vec::new(),
                pilots: 1,
                tags: KillTags::default(),
                hash: String::new(),
                location_id: None,
            }
        }
    }

    #[test]
    fn the_answer_of_esi_is_the_first_reading_and_the_killmails_correct_it() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        assert!(handle.hour_kills().is_empty());
        assert_eq!(handle.hour_modified(), None);
        // Without the feed running, ESI is the only one that knows the hour.
        assert!(!handle.covers_the_hour());
        let now = Utc::now();
        let modified = now - TimeDelta::minutes(15);
        assert!(handle.record_hour(&Sample::esi_hour(modified)));
        // ESI repeats its answer until the next hour: it is kept once.
        assert!(!handle.record_hour(&Sample::esi_hour(modified)));
        assert_eq!(handle.hour_modified(), Some(modified));
        let kills = |handle: &ZKillboardHandle, system| {
            handle
                .hour_kills()
                .into_iter()
                .find(|(id, _)| *id == system)
                .map_or(0, |(_, kills)| kills)
        };
        // ESI said 4 for an hour that ended 15 minutes ago: a quarter of
        // them have left the window.
        assert_eq!(kills(&handle, 30000142), 3);
        assert_eq!(kills(&handle, 30000144), 0);
        // A killmail after the answer is added to it.
        handle.0.api_buffer().insert(
            Sample::killmail(1, 30000142, now - TimeDelta::minutes(5)),
            now,
        );
        assert_eq!(kills(&handle, 30000142), 4);
        // One before it is part of the answer already.
        handle.0.api_buffer().insert(
            Sample::killmail(2, 30000142, now - TimeDelta::minutes(30)),
            now,
        );
        assert_eq!(kills(&handle, 30000142), 4);
        // A system ESI does not list shows what the buffer has.
        handle.0.api_buffer().insert(
            Sample::killmail(3, 30000999, now - TimeDelta::minutes(1)),
            now,
        );
        assert_eq!(kills(&handle, 30000999), 1);
    }

    #[test]
    fn the_kills_of_esi_leave_the_window_as_the_hour_passes() {
        let now = Utc::now();
        let hour =
            |minutes| EsiHour::new(&Sample::esi_hour(now - TimeDelta::minutes(minutes))).unwrap();
        // Just computed: all of them. Half an hour on: half. An hour on or
        // more: none. A system ESI does not list: none.
        assert_eq!(hour(0).remaining(30000142, now), 4);
        assert_eq!(hour(30).remaining(30000142, now), 2);
        assert_eq!(hour(60).remaining(30000142, now), 0);
        assert_eq!(hour(90).remaining(30000142, now), 0);
        assert_eq!(hour(0).remaining(1, now), 0);
        // A date in the future (the clocks differ) does not add kills.
        assert_eq!(hour(-10).remaining(30000142, now), 4);
    }

    #[test]
    fn an_old_answer_of_esi_and_an_undated_one_do_not_count() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        let undated = HourKills {
            modified: None,
            systems: Sample::esi_hour(Utc::now()).systems,
        };
        assert!(!handle.record_hour(&undated));
        assert!(handle.record_hour(&Sample::esi_hour(Utc::now() - TimeDelta::hours(3))));
        assert!(handle.hour_kills().is_empty());
        // A newer one replaces the old; an older one does not replace it.
        let newer = Utc::now() - TimeDelta::minutes(5);
        assert!(handle.record_hour(&Sample::esi_hour(newer)));
        assert!(!handle.record_hour(&Sample::esi_hour(newer - TimeDelta::hours(1))));
        assert_eq!(handle.hour_modified(), Some(newer));
    }

    /// A killmail file as the feed serves it (trimmed, with the fields
    /// Telescope ignores kept in so they are known to be harmless).
    const SAMPLE: &str = r#"{
        "killmail_id": 139148857,
        "hash": "40d156ad1babd4a7760858f3a413aaf016c82341",
        "esi": {
            "attackers": [
                {"character_id": 757344657, "corporation_id": 98182376,
                 "alliance_id": 99013187, "damage_done": 300, "final_blow": true,
                 "security_status": -10, "ship_type_id": 22456, "weapon_type_id": 3520},
                {"character_id": 1, "damage_done": 158, "final_blow": false,
                 "ship_type_id": 22456},
                {"character_id": 2, "damage_done": 0, "final_blow": false,
                 "ship_type_id": 584}
            ],
            "killmail_id": 139148857,
            "killmail_time": "2026-10-14T05:01:05Z",
            "solar_system_id": 30000142,
            "victim": {"character_id": 2124108583, "corporation_id": 98828639,
                       "damage_taken": 458, "items": [], "ship_type_id": 587}
        },
        "zkb": {
            "locationID": 60003760, "hash": "abc", "fittedValue": 10000.0,
            "droppedValue": 0.0, "destroyedValue": 10000.0, "totalValue": 12345678.9,
            "points": 1, "npc": false, "solo": false, "awox": false,
            "labels": ["pvp", "loc:lowsec"], "attackerCount": 3, "href": "https://x"
        },
        "uploaded_at": 1791954065,
        "sequence_id": 100023300
    }"#;

    /// Ships and systems by id.
    struct Names;

    impl NameSource for Names {
        fn system_name(&self, id: u32) -> Option<String> {
            match id {
                30000142 => Some(String::from("Jita")),
                30002050 => Some(String::from("H-5GUI")),
                _ => None,
            }
        }

        fn type_name(&self, id: u32) -> Option<String> {
            match id {
                587 => Some(String::from("Rifter")),
                22456 => Some(String::from("Sabre")),
                584 => Some(String::from("Merlin")),
                // A ship that is also the name of a system.
                1 => Some(String::from("Jita")),
                _ => None,
            }
        }
    }

    /// Resolves the systems [`Names`] knows.
    struct Systems;

    impl SystemResolver for Systems {
        fn resolve(&self, name: &str) -> Option<usize> {
            match name.to_lowercase().as_str() {
                "jita" => Some(30000142),
                "h-5gui" => Some(30002050),
                _ => None,
            }
        }
    }

    /// Builders for the tests.
    struct Fixture;

    impl Fixture {
        /// The default rules without the zKillboard ones, with the chat
        /// input taking killmails: the chat detections reading a killmail.
        fn chat_rules_taking_killmails() -> RuleGraph {
            let mut graph = RuleGraph::default_graph();
            let is_zkillboard = |id: &str| id == "zkillboard" || id.starts_with("z_");
            graph.nodes.retain(|node| !is_zkillboard(&node.id));
            graph
                .edges
                .retain(|edge| !is_zkillboard(&edge.from) && !is_zkillboard(&edge.to));
            for node in &mut graph.nodes {
                if let NodeKind::Input(input) = &mut node.kind {
                    input.kind = InputKind::ZKillboard;
                }
            }
            graph
        }

        fn sample() -> Killmail {
            serde_json::from_str(SAMPLE).unwrap()
        }

        /// A killmail with this id and sequence, in system 30000142.
        fn kill(sequence: u64, id: u64) -> Killmail {
            Killmail {
                killmail_id: id,
                sequence_id: sequence,
                esi: EsiKillmail {
                    killmail_time: String::from("2026-10-14T05:01:05Z"),
                    solar_system_id: 30000142,
                    victim: Victim {
                        ship_type_id: Some(587),
                    },
                    attackers: Vec::new(),
                },
                zkb: Zkb::default(),
                raw: None,
            }
        }

        /// The sequences announced by one round of the cursor.
        async fn announced(cursor: &mut FeedCursor, feed: &FakeFeed) -> (Vec<u64>, Step) {
            let mut sequences = Vec::new();
            let step = cursor
                .advance(feed, &mut |kill| {
                    sequences.push(kill.sequence_id);
                    true
                })
                .await;
            (sequences, step)
        }
    }

    /// A feed in memory: the killmails by sequence, the published pointer and
    /// an error to fail with.
    struct FakeFeed {
        pointer: Mutex<u64>,
        entries: Mutex<BTreeMap<u64, Killmail>>,
        failure: Mutex<Option<FeedError>>,
    }

    impl FakeFeed {
        /// Entries `from..=to`, each with the id of its sequence, and the
        /// pointer at `from`.
        fn with(from: u64, to: u64) -> Self {
            let feed = Self {
                pointer: Mutex::new(from),
                entries: Mutex::new(BTreeMap::new()),
                failure: Mutex::new(None),
            };
            for sequence in from..=to {
                feed.publish(Fixture::kill(sequence, sequence));
            }
            feed
        }

        fn publish(&self, killmail: Killmail) {
            self.entries
                .lock()
                .unwrap()
                .insert(killmail.sequence_id, killmail);
        }

        fn fail_with(&self, error: Option<FeedError>) {
            *self.failure.lock().unwrap() = error;
        }

        fn failure(&self) -> Option<FeedError> {
            self.failure.lock().unwrap().clone()
        }
    }

    impl Feed for FakeFeed {
        async fn pointer(&self) -> Result<u64, FeedError> {
            match self.failure() {
                Some(error) => Err(error),
                None => Ok(*self.pointer.lock().unwrap()),
            }
        }

        async fn entry(&self, sequence: u64) -> Result<Option<Killmail>, FeedError> {
            match self.failure() {
                Some(error) => Err(error),
                None => Ok(self.entries.lock().unwrap().get(&sequence).cloned()),
            }
        }
    }

    #[test]
    fn a_feed_file_is_read_ignoring_what_telescope_does_not_use() {
        let killmail = Fixture::sample();
        assert_eq!(killmail.killmail_id, 139148857);
        assert_eq!(killmail.sequence_id, 100023300);
        assert_eq!(killmail.esi.solar_system_id, 30000142);
        assert_eq!(killmail.esi.victim.ship_type_id, Some(587));
        assert_eq!(killmail.esi.attackers.len(), 3);
        assert_eq!(killmail.zkb.total_value, 12345678.9);
    }

    #[test]
    fn a_file_with_only_the_required_fields_is_read() {
        let killmail: Killmail = serde_json::from_str(
            r#"{"killmail_id": 1, "sequence_id": 2,
                "esi": {"killmail_time": "2026-01-01T00:00:00Z", "solar_system_id": 3,
                        "victim": {}}}"#,
        )
        .unwrap();
        assert!(killmail.esi.attackers.is_empty());
        assert_eq!(killmail.esi.victim.ship_type_id, None);
        assert_eq!(killmail.zkb, Zkb::default());
    }

    #[test]
    fn a_killmail_becomes_a_line_of_chat_log_shape() {
        let line = Fixture::sample().to_line(&Names, &Systems).unwrap();
        assert_eq!(line.author, "zKillboard");
        assert_eq!(
            line.text,
            "Jita: Rifter destroyed by 3 pilots (Sabre ×2, Merlin), 12.3M ISK"
        );
        assert_eq!(line.timestamp.to_rfc3339(), "2026-10-14T05:01:05+00:00");
        // What the log shows is a line the chat-log parser understands too.
        assert_eq!(parse_line(&line.to_string()).unwrap().text, line.text);
    }

    #[test]
    fn a_ship_named_like_a_system_is_left_out_of_the_text() {
        // Ship type 1 is called "Jita" in the fake SDE.
        let mut killmail = Fixture::sample();
        killmail.esi.attackers = vec![
            Attacker {
                character_id: Some(1),
                ship_type_id: Some(1),
            },
            Attacker {
                character_id: Some(2),
                ship_type_id: Some(584),
            },
        ];
        let line = killmail.to_line(&Names, &Systems).unwrap();
        assert_eq!(
            line.text,
            "Jita: Rifter destroyed by 2 pilots (Merlin), 12.3M ISK"
        );
    }

    #[test]
    fn npcs_are_not_counted_as_pilots() {
        let mut killmail = Fixture::sample();
        killmail.esi.attackers = vec![Attacker {
            character_id: None,
            ship_type_id: Some(584),
        }];
        killmail.zkb.total_value = 950.0;
        assert_eq!(
            killmail.to_line(&Names, &Systems).unwrap().text,
            "Jita: Rifter destroyed by NPCs, 950 ISK"
        );
        killmail.esi.attackers.clear();
        assert_eq!(
            killmail.to_line(&Names, &Systems).unwrap().text,
            "Jita: Rifter destroyed by NPCs, 950 ISK"
        );
    }

    #[test]
    fn a_single_attacker_is_one_pilot() {
        let mut killmail = Fixture::sample();
        killmail.esi.attackers.truncate(1);
        assert!(
            killmail
                .to_line(&Names, &Systems)
                .unwrap()
                .text
                .contains("by 1 pilot (Sabre)")
        );
    }

    #[test]
    fn an_unknown_ship_does_not_stop_the_line() {
        let mut killmail = Fixture::sample();
        killmail.esi.victim.ship_type_id = Some(999_999);
        killmail.esi.attackers = vec![Attacker {
            character_id: Some(1),
            ship_type_id: None,
        }];
        assert_eq!(
            killmail.to_line(&Names, &Systems).unwrap().text,
            "Jita: ship destroyed by 1 pilot, 12.3M ISK"
        );
    }

    #[test]
    fn a_killmail_in_an_unknown_system_makes_no_line() {
        let mut killmail = Fixture::sample();
        killmail.esi.solar_system_id = 1;
        assert!(killmail.to_line(&Names, &Systems).is_none());
    }

    #[test]
    fn a_killmail_event_has_the_sentence_and_the_facts_of_the_killmail() {
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&Fixture::sample())
            .unwrap();
        assert_eq!(event.kind, InputKind::ZKillboard);
        assert_eq!(event.channel, "zkillboard");
        assert_eq!(
            event.line.text,
            "Jita: Rifter destroyed by 3 pilots (Sabre ×2, Merlin), 12.3M ISK"
        );

        let systems = event.facts.systems.unwrap();
        assert_eq!(systems.len(), 1);
        assert_eq!(
            (systems[0].id, systems[0].name.as_str()),
            (30000142, "Jita")
        );
        // The span is where the name stands in the text.
        let span = systems[0].span.clone().unwrap();
        assert_eq!(&event.line.text[span], "Jita");

        assert_eq!(
            event.facts.ships.unwrap(),
            vec![
                ShipCount::new("Rifter", 1).with_type_id(587),
                ShipCount::new("Sabre", 2).with_type_id(22456),
                ShipCount::new("Merlin", 1).with_type_id(584)
            ]
        );
        assert_eq!(event.facts.pilots, Some(3));
    }

    #[test]
    fn the_facts_know_what_the_text_leaves_out() {
        // Ship type 1 is called "Jita": the text leaves it out so that it is
        // not read as the system, but it is a ship all the same.
        let mut killmail = Fixture::sample();
        killmail.esi.attackers = vec![Attacker {
            character_id: Some(1),
            ship_type_id: Some(1),
        }];
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&killmail)
            .unwrap();
        assert_eq!(
            event.line.text,
            "Jita: Rifter destroyed by 1 pilot, 12.3M ISK"
        );
        assert_eq!(
            event.facts.ships.unwrap(),
            vec![
                ShipCount::new("Rifter", 1).with_type_id(587),
                ShipCount::new("Jita", 1).with_type_id(1)
            ]
        );

        // The text lists the first eight pilots' ships; the facts count all.
        killmail.esi.attackers = (0..30)
            .map(|index| Attacker {
                character_id: Some(index),
                ship_type_id: Some(584),
            })
            .collect();
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&killmail)
            .unwrap();
        assert_eq!(
            event.facts.ships.unwrap(),
            vec![
                ShipCount::new("Rifter", 1).with_type_id(587),
                ShipCount::new("Merlin", 30).with_type_id(584)
            ]
        );
        assert_eq!(event.facts.pilots, Some(30));
    }

    #[test]
    fn npcs_are_no_pilots_in_the_facts_either() {
        let mut killmail = Fixture::sample();
        killmail.esi.attackers = vec![Attacker {
            character_id: None,
            ship_type_id: Some(584),
        }];
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&killmail)
            .unwrap();
        assert_eq!(event.facts.pilots, Some(0));
        assert_eq!(
            event.facts.ships.unwrap(),
            vec![ShipCount::new("Rifter", 1).with_type_id(587)]
        );
    }

    #[test]
    fn a_killmail_that_cannot_be_placed_is_no_event() {
        let mut killmail = Fixture::sample();
        killmail.esi.solar_system_id = 1;
        assert!(
            ZKillboardDecoder::new(&Names, &Systems)
                .decode(&killmail)
                .is_none()
        );
    }

    #[test]
    fn only_the_first_attackers_are_listed() {
        let mut killmail = Fixture::sample();
        killmail.esi.attackers = (0..30)
            .map(|index| Attacker {
                character_id: Some(index),
                ship_type_id: Some(584),
            })
            .collect();
        let text = killmail.to_line(&Names, &Systems).unwrap().text;
        assert!(text.contains("by 30 pilots"), "{text}");
        // The ships are grouped by type: only the first attackers are listed,
        // and they are all one type here.
        assert_eq!(text.matches("Merlin").count(), 1, "{text}");
        assert!(
            text.contains(&format!("Merlin ×{MAX_LISTED_ATTACKERS}")),
            "{text}"
        );
    }

    #[test]
    fn the_event_carries_the_time_of_the_killmail() {
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&Fixture::sample())
            .unwrap();
        assert_eq!(event.time_origin, TimeOrigin::Reported);
        assert_eq!(event.at(), event.line.timestamp);
        assert_eq!(event.at().to_rfc3339(), "2026-10-14T05:01:05+00:00");
    }

    #[test]
    fn an_event_with_an_unreadable_time_is_still_decoded_and_says_so() {
        let mut killmail = Fixture::sample();
        killmail.esi.killmail_time = String::from("yesterday");
        let before = Utc::now();
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&killmail)
            .unwrap();
        assert_eq!(event.time_origin, TimeOrigin::Assumed);
        assert!(event.at() >= before);
    }

    #[test]
    fn an_unreadable_time_falls_back_to_now() {
        let mut killmail = Fixture::sample();
        killmail.esi.killmail_time = String::from("yesterday");
        let before = Utc::now();
        let line = killmail.to_line(&Names, &Systems).unwrap();
        assert!(line.timestamp >= before);
    }

    #[test]
    fn values_are_shortened() {
        for (value, text) in [
            (0.0, "0"),
            (950.4, "950"),
            (1_000.0, "1K"),
            (12_345.0, "12K"),
            (999_999.0, "1000K"),
            (1_000_000.0, "1.0M"),
            (12_345_678.0, "12.3M"),
            (1_500_000_000.0, "1.5B"),
            (2_345_000_000_000.0, "2345.0B"),
        ] {
            assert_eq!(Killmail::isk(value), text, "{value}");
        }
    }

    #[test]
    fn the_detections_read_a_killmail_like_a_report() {
        use webb::graph::Data;

        let graph = Fixture::chat_rules_taking_killmails();
        assert!(ZKillboardHandle::wanted_by(&graph));
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");

        let line = Fixture::sample().to_line(&Names, &Systems).unwrap();
        let context = webb::event::IntelEvent::text(line, "zkillboard", InputKind::ZKillboard);
        let activations = executor.run(&context, &Systems);
        let messages: Vec<_> = activations
            .iter()
            .flat_map(|activation| activation.messages.iter())
            .collect();
        // The system, once...
        assert!(
            messages
                .iter()
                .any(|message| message.data == Data::Systems(vec![30000142])),
            "{messages:?}"
        );
        // ...the attackers...
        assert!(
            messages
                .iter()
                .any(|message| message.data == Data::Count(3)),
            "{messages:?}"
        );
        // ...and the ships involved, repeats counted.
        let ships = messages.iter().find_map(|message| match &message.data {
            Data::Ships(ships) => Some(ships.clone()),
            _ => None,
        });
        let ships = ships.expect("ship names");
        assert!(ships.contains(&ShipCount::new("Sabre", 2)), "{ships:?}");
        assert!(ships.contains(&ShipCount::new("Merlin", 1)), "{ships:?}");

        // The same graph on a chat line of the same text finds nothing: the
        // input only takes killmails.
        let chat = webb::event::IntelEvent {
            kind: InputKind::ChatLog,
            ..context
        };
        assert!(executor.run(&chat, &Systems).is_empty());
    }

    #[test]
    fn the_detections_read_the_facts_of_a_killmail_event() {
        use webb::graph::Data;

        // The rules of the chat, taking killmails.
        let (executor, errors) = Executor::new(Fixture::chat_rules_taking_killmails());
        assert!(errors.is_empty(), "{errors:?}");

        // A big fight: a count the pilot patterns cannot read from the text
        // and more ships than the text lists.
        let mut killmail = Fixture::sample();
        killmail.esi.solar_system_id = 30002050;
        killmail.esi.attackers = (0..1500)
            .map(|index| Attacker {
                character_id: Some(index),
                ship_type_id: Some(584),
            })
            .collect();
        let event = ZKillboardDecoder::new(&Names, &Systems)
            .decode(&killmail)
            .unwrap();

        let data = |event: &webb::event::IntelEvent| -> Vec<Data> {
            executor
                .run(event, &Systems)
                .iter()
                .flat_map(|activation| activation.messages.iter())
                .map(|message| message.data.clone())
                .collect()
        };
        let with_facts = data(&event);
        assert!(
            with_facts.contains(&Data::Systems(vec![30002050])),
            "{with_facts:?}"
        );
        assert!(with_facts.contains(&Data::Count(1500)), "{with_facts:?}");
        let ships = with_facts.iter().find_map(|data| match data {
            Data::Ships(ships) => Some(ships.clone()),
            _ => None,
        });
        let ships = ships.expect("ship names");
        assert!(
            ships.contains(&ShipCount::new("Rifter", 1).with_type_id(587)),
            "{ships:?}"
        );
        assert!(
            ships.contains(&ShipCount::new("Merlin", 1500).with_type_id(584)),
            "{ships:?}"
        );

        // Reading only the sentence loses both.
        let text_only =
            webb::event::IntelEvent::text(event.line.clone(), "zkillboard", InputKind::ZKillboard);
        let from_text = data(&text_only);
        assert!(
            from_text.contains(&Data::Systems(vec![30002050])),
            "{from_text:?}"
        );
        assert!(!from_text.contains(&Data::Count(1500)), "{from_text:?}");
        let ships = from_text.iter().find_map(|data| match data {
            Data::Ships(ships) => Some(ships.clone()),
            _ => None,
        });
        let merlins = MAX_LISTED_ATTACKERS as u32;
        assert!(
            ships
                .expect("ship names")
                .contains(&ShipCount::new("Merlin", merlins))
        );
    }

    #[test]
    fn recent_ids_forget_the_oldest_first() {
        let mut recent = RecentIds::default();
        assert!(recent.insert(1));
        assert!(!recent.insert(1));
        for id in 2..=RECENT_IDS as u64 + 1 {
            assert!(recent.insert(id));
        }
        // 1 fell out of the window, the newest ones are still known.
        assert!(recent.insert(1));
        assert!(!recent.insert(RECENT_IDS as u64 + 1));
    }

    #[test]
    fn the_backoff_doubles_up_to_a_minute_and_resets() {
        let mut backoff = Backoff::new();
        let waits: Vec<u64> = (0..6).map(|_| backoff.next().as_secs()).collect();
        assert_eq!(waits, [6, 12, 24, 48, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next().as_secs(), 6);
    }

    #[tokio::test]
    async fn a_new_walk_starts_at_the_end_and_announces_only_what_comes_after() {
        let feed = FakeFeed::with(100, 130);
        let mut cursor = FeedCursor::new(Duration::ZERO);

        // The 31 killmails already in the feed are history.
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        assert_eq!(step, Step::Waiting);

        // Nothing new: nothing announced.
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());

        // New ones are announced once, in order.
        feed.publish(Fixture::kill(131, 131));
        feed.publish(Fixture::kill(132, 132));
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [131, 132]);
        assert_eq!(step, Step::Waiting);
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
    }

    #[tokio::test]
    async fn a_hole_in_the_series_is_skipped_when_the_feed_is_past_it() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;

        // 101 never existed; 102 and 103 did, and the pointer moved past.
        feed.publish(Fixture::kill(102, 102));
        feed.publish(Fixture::kill(103, 103));
        *feed.pointer.lock().unwrap() = 103;
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [102, 103]);
        assert_eq!(step, Step::Waiting);
    }

    #[tokio::test]
    async fn a_missing_file_the_feed_is_not_past_is_just_the_end() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;
        // The pointer is behind (it only moves every 51 killmails): 101 is
        // simply not published yet.
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        assert_eq!(step, Step::Waiting);
        feed.publish(Fixture::kill(101, 101));
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [101]);
    }

    #[tokio::test]
    async fn a_republished_killmail_is_announced_once() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;

        feed.publish(Fixture::kill(101, 500));
        feed.publish(Fixture::kill(102, 501));
        // The feed reprocessed killmail 500 and published it again.
        feed.publish(Fixture::kill(103, 500));
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [101, 102]);
    }

    #[tokio::test]
    async fn a_failure_keeps_the_position_and_the_walk_goes_on_after_it() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;
        feed.publish(Fixture::kill(101, 101));

        let throttled = FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS);
        feed.fail_with(Some(throttled.clone()));
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        assert_eq!(step, Step::Failed(throttled));

        feed.fail_with(None);
        feed.publish(Fixture::kill(102, 102));
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [101, 102]);
    }

    #[tokio::test]
    async fn a_failure_before_the_first_read_is_reported_and_retried() {
        let feed = FakeFeed::with(100, 100);
        feed.fail_with(Some(FeedError::Other(String::from("offline"))));
        let mut cursor = FeedCursor::new(Duration::ZERO);
        let (_, step) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(
            step,
            Step::Failed(FeedError::Other(String::from("offline")))
        );
        feed.fail_with(None);
        let (announced, step) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        assert_eq!(step, Step::Waiting);
    }

    #[tokio::test]
    async fn resyncing_skips_what_was_published_meanwhile() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;

        // An outage long enough for the walk to start over: what came out
        // meanwhile is too old to alert about.
        cursor.resync();
        for sequence in 101..=120 {
            feed.publish(Fixture::kill(sequence, sequence));
        }
        *feed.pointer.lock().unwrap() = 120;
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        feed.publish(Fixture::kill(121, 121));
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [121]);
    }

    #[tokio::test]
    async fn the_walk_stops_when_nobody_listens() {
        let feed = FakeFeed::with(100, 100);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        Fixture::announced(&mut cursor, &feed).await;
        feed.publish(Fixture::kill(101, 101));
        let step = cursor.advance(&feed, &mut |_| false).await;
        assert_eq!(step, Step::Closed);
    }

    #[test]
    fn the_source_is_wanted_only_by_an_enabled_zkillboard_input() {
        use webb::graph::{InputNode, Node};

        let input = |kind: InputKind, enabled: bool| Node {
            id: String::from("in"),
            enabled,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Input(InputNode {
                description: String::from("input"),
                kind,
                path: String::new(),
                channels: Vec::new(),
                exclude_motd: true,
            }),
        };
        let graph = |node: Node| RuleGraph {
            nodes: vec![node],
            edges: Vec::new(),
        };
        assert!(ZKillboardHandle::wanted_by(&graph(input(
            InputKind::ZKillboard,
            true
        ))));
        assert!(!ZKillboardHandle::wanted_by(&graph(input(
            InputKind::ZKillboard,
            false
        ))));
        assert!(!ZKillboardHandle::wanted_by(&graph(input(
            InputKind::ChatLog,
            true
        ))));
        // The shipped graph has the zKillboard input: the feed runs from the
        // start (the settings switch pauses it).
        assert!(ZKillboardHandle::wanted_by(&RuleGraph::default_graph()));
    }

    #[test]
    fn the_filters_of_the_settings_leave_killmails_out() {
        let mut killmail = Fixture::sample();
        killmail.zkb.total_value = 5_000_000.0;
        assert!(killmail.is_worth(0, false));
        assert!(killmail.is_worth(5_000_000, false));
        assert!(!killmail.is_worth(5_000_001, false));

        // Only NPCs shot: left out when asked, kept otherwise.
        for attacker in &mut killmail.esi.attackers {
            attacker.character_id = None;
        }
        assert!(killmail.is_worth(0, false));
        assert!(!killmail.is_worth(0, true));
        killmail.esi.attackers.clear();
        assert!(!killmail.is_worth(0, true));
        killmail.esi.attackers.push(Attacker {
            character_id: Some(1),
            ship_type_id: None,
        });
        assert!(killmail.is_worth(0, true));
    }

    #[test]
    fn the_options_pause_the_source_and_set_the_filters() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        handle.set_enabled(true);
        assert!(handle.0.active());
        handle.set_options(&ZKillboardOptions {
            enabled: false,
            min_isk: 7,
            ignore_npc_only: true,
        });
        // A rule still asks for the feed, but the settings pause it.
        assert!(handle.is_wanted());
        assert!(!handle.0.active());
        assert_eq!(handle.0.min_isk.load(Ordering::Acquire), 7);
        assert!(handle.0.ignore_npc_only.load(Ordering::Acquire));
        handle.set_options(&ZKillboardOptions::default());
        assert!(handle.0.active());
        assert_eq!(handle.status(), FeedStatus::default());
    }

    #[test]
    fn a_killmail_becomes_a_record_for_the_tooltips() {
        let killmail = Fixture::sample();
        let record = killmail.to_record(&Names);
        assert_eq!(record.killmail_id, killmail.killmail_id);
        assert_eq!(record.system_id, 30000142);
        assert_eq!(record.victim.as_deref(), Some("Rifter"));
        let pilots = killmail
            .esi
            .attackers
            .iter()
            .filter(|attacker| attacker.character_id.is_some())
            .count();
        assert_eq!(record.pilots as usize, pilots);
        assert!(record.attacker_ships.len() <= pilots);
        // 2026-10-14T05:01:05Z
        assert_eq!(
            record.time,
            DateTime::from_timestamp(1_791_954_065, 0).unwrap()
        );
    }

    #[test]
    fn the_context_comes_from_the_feed_or_else_the_api_and_not_at_all_when_paused() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        // No rule asks for the feed: the API gives it, still loading.
        let context = handle.context(1).unwrap();
        assert_eq!(context.source, Source::Api);
        assert!(context.loading);
        assert_eq!(context.day, Some(DayLine::Pending));
        handle.set_enabled(true);
        let context = handle.context(1).unwrap();
        assert_eq!(context.source, Source::Feed);
        assert!(!context.loading);
        // The day is asked for the first time the tooltip is shown.
        assert_eq!(context.day, Some(DayLine::Pending));
        handle.set_options(&ZKillboardOptions {
            enabled: false,
            ..ZKillboardOptions::default()
        });
        assert!(handle.context(1).is_none());
    }

    #[test]
    fn the_api_makes_up_for_what_the_feed_lacks() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        handle.set_enabled(true);
        // The feed runs and has everything: the API is not asked.
        assert!(!handle.api_in_use());
        let context = handle.context(1).unwrap();
        assert_eq!(context.day, Some(DayLine::Pending));
        assert!(!context.updating);
        assert!(!context.loading);

        // The hour before the start is still to be read.
        handle.0.starting_short.store(true, Ordering::Release);
        assert!(handle.api_in_use());
        let context = handle.context(1).unwrap();
        assert_eq!(context.source, Source::Feed);
        assert!(context.updating);
        assert_eq!(context.day, Some(DayLine::Pending));
        handle.0.starting_short.store(false, Ordering::Release);
        assert!(!handle.api_in_use());

        // The feed can't be read: the API is the source shown.
        handle.0.status.lock().unwrap().state = FeedState::Failing;
        assert!(handle.api_in_use());
        assert_eq!(handle.context(1).unwrap().source, Source::Api);
        handle.0.status.lock().unwrap().state = FeedState::Live;
        assert!(!handle.api_in_use());

        // Back, but what was published meanwhile is not coming back.
        let until = (Utc::now() + WINDOW).timestamp();
        handle.0.outage_until.store(until, Ordering::Release);
        assert!(handle.api_in_use());
        assert_eq!(handle.context(1).unwrap().source, Source::Feed);
        handle
            .0
            .outage_until
            .store(Utc::now().timestamp() - 1, Ordering::Release);
        assert!(!handle.api_in_use());

        // Paused or not wanted, there is nothing to make up for.
        handle.0.starting_short.store(true, Ordering::Release);
        handle.set_enabled(false);
        assert!(!handle.0.compensating());
    }

    #[test]
    fn a_failure_of_the_feed_starts_the_api_at_once_and_it_stays_after_it() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        handle.set_enabled(true);
        assert!(!handle.api_in_use());
        let now = Utc::now();
        handle.0.feed_failed(now);
        assert!(handle.api_in_use());
        assert_eq!(handle.context(1).unwrap().source, Source::Api);
        // The feed answers again: the API goes on until the hour is gone.
        handle.0.status.lock().unwrap().state = FeedState::Live;
        assert!(handle.api_in_use());
        assert_eq!(handle.context(1).unwrap().source, Source::Feed);
        // A later failure pushes the end further.
        handle.0.feed_failed(now + TimeDelta::minutes(10));
        assert_eq!(
            handle.0.outage_until.load(Ordering::Acquire),
            (now + TimeDelta::minutes(10) + WINDOW).timestamp()
        );
    }

    #[test]
    fn what_esi_gives_goes_to_the_buffer_the_tooltips_show() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        let now = Utc::now();
        let record = |id: u64| KillRecord {
            killmail_id: id,
            time: now - TimeDelta::minutes(5),
            system_id: 1,
            victim: None,
            attacker_ships: Vec::new(),
            pilots: 1,
            tags: KillTags::default(),
            hash: String::new(),
            location_id: None,
        };
        // The feed is not running: only the API's buffer has it.
        assert!(handle.0.keep(record(1), now));
        assert_eq!(handle.0.buffer().len(), 0);
        // The feed runs: the one that shows has it too, once.
        handle.set_enabled(true);
        assert!(handle.0.keep(record(2), now));
        assert!(!handle.0.keep(record(2), now));
        assert_eq!(handle.0.buffer().len(), 1);
    }

    #[tokio::test]
    async fn the_killmails_read_on_the_way_to_the_end_are_kept_as_history() {
        let feed = FakeFeed::with(100, 103);
        let mut cursor = FeedCursor::new(Duration::ZERO);
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert!(announced.is_empty());
        assert_eq!(cursor.origin(), Some(100));
        let history: Vec<u64> = cursor
            .take_history()
            .iter()
            .map(|killmail| killmail.sequence_id)
            .collect();
        assert_eq!(history, [100, 101, 102, 103]);
        assert!(cursor.take_history().is_empty());
        // What comes after is announced, not history.
        feed.publish(Fixture::kill(104, 104));
        let (announced, _) = Fixture::announced(&mut cursor, &feed).await;
        assert_eq!(announced, [104]);
        assert!(cursor.take_history().is_empty());
    }

    #[test]
    fn the_backfill_covers_about_an_hour_before_the_start() {
        let files = Backfill::new(1000, None).unwrap();
        assert_eq!((files.first, files.end), (1000 - BACKFILL_FILES, 1000));
        // A feed that has not got an hour of files yet.
        let files = Backfill::new(5, None).unwrap();
        assert_eq!((files.first, files.end), (1, 5));
        assert!(Backfill::new(1, None).is_none());
        // A previous run got to 900: the files before it are in the store.
        let files = Backfill::new(1000, Some(900)).unwrap();
        assert_eq!((files.first, files.end), (900, 1000));
        // Too long ago to help, or nothing published since.
        let files = Backfill::new(1000, Some(10)).unwrap();
        assert_eq!(files.first, 1000 - BACKFILL_FILES);
        assert!(Backfill::new(1000, Some(1000)).is_none());
        assert!(Backfill::new(1000, Some(5000)).is_none());
    }

    #[tokio::test]
    async fn the_backfill_reads_in_chunks_and_retries_after_a_failure() {
        let feed = FakeFeed::with(1, 100);
        let mut files = Backfill {
            next: 1,
            end: 101,
            first: 1,
            pace: Duration::ZERO,
        };
        let mut read = Vec::new();
        assert!(
            !files
                .step(&feed, &mut |killmail| read.push(killmail.sequence_id))
                .await
        );
        assert_eq!(read.len() as u64, BACKFILL_CHUNK);
        assert_eq!(
            files.progress(),
            Fill {
                done: BACKFILL_CHUNK,
                total: 100
            }
        );

        // A failure keeps the position.
        feed.fail_with(Some(FeedError::Other(String::from("down"))));
        assert!(
            !files
                .step(&feed, &mut |killmail| read.push(killmail.sequence_id))
                .await
        );
        assert_eq!(files.progress().done, BACKFILL_CHUNK);

        feed.fail_with(None);
        while !files
            .step(&feed, &mut |killmail| read.push(killmail.sequence_id))
            .await
        {}
        assert_eq!(read, (1..=100).collect::<Vec<u64>>());
        assert_eq!(files.progress().percent(), 100);
    }

    #[test]
    fn the_handle_follows_the_graph() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        assert!(!handle.is_enabled());
        let mut graph = RuleGraph::default_graph();
        handle.follow(&graph);
        assert!(handle.is_enabled());
        // Without the input (a graph of chat logs only) it stops.
        for node in &mut graph.nodes {
            if node.id == "zkillboard" {
                node.enabled = false;
            }
        }
        handle.follow(&graph);
        assert!(!handle.is_enabled());
        handle.set_enabled(true);
        assert!(handle.is_enabled());
    }

    #[test]
    fn names_come_from_the_sde_and_follow_it_when_it_moves() {
        let dir = std::env::temp_dir().join(format!("telescope-zkb-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let build = |file: &str, system: &str| {
            let path = dir.join(file);
            let _ = std::fs::remove_file(&path);
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(&format!(
                    "CREATE TABLE mapSolarSystems (solarSystemId INTEGER, solarSystemName TEXT);
                     CREATE TABLE invTypes (typeId INTEGER, typeName TEXT);
                     INSERT INTO mapSolarSystems VALUES (30000142, '{system}');
                     INSERT INTO invTypes VALUES (587, 'Rifter');"
                ))
                .unwrap();
            path
        };
        let first = build("first.db", "Jita");
        let second = build("second.db", "Perimeter");

        let path = Arc::new(RwLock::new(first.clone()));
        let names = SdeNames::new(Arc::clone(&path));
        assert_eq!(names.system_name(30000142).as_deref(), Some("Jita"));
        assert_eq!(names.type_name(587).as_deref(), Some("Rifter"));
        assert_eq!(names.system_name(1), None);
        assert_eq!(names.type_name(1), None);

        // Looked up once: still known when the file can't be read any more.
        std::fs::rename(&first, dir.join("moved.db")).unwrap();
        assert_eq!(names.system_name(30000142).as_deref(), Some("Jita"));

        // Another SDE brings its own names.
        *path.write().unwrap() = second;
        assert_eq!(names.system_name(30000142).as_deref(), Some("Perimeter"));

        // No database at all (the SDE isn't built yet): no names, no panic.
        *path.write().unwrap() = dir.join("missing.db");
        assert_eq!(names.system_name(30000142), None);
        assert_eq!(names.type_name(587), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-thread HTTP server answering canned responses, to try the real
    /// client on.
    struct StubServer {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl StubServer {
        /// Serves `routes` (path, status, body) and ends after `requests`
        /// requests.
        fn start(routes: Vec<(&'static str, u16, &'static str)>, requests: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = Arc::clone(&seen);
            std::thread::spawn(move || {
                for _ in 0..requests {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                    log.lock().unwrap().push(request);
                    let (status, body) = routes
                        .iter()
                        .find(|(route, _, _)| *route == path)
                        .map_or((404, ""), |(_, status, body)| (*status, *body));
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            });
            Self {
                base,
                requests: seen,
            }
        }

        fn client(&self) -> R2z2Client {
            R2z2Client {
                // No proxy: the server is on this machine.
                client: Client::builder()
                    .user_agent(USER_AGENT)
                    .no_proxy()
                    .build()
                    .unwrap(),
                base: self.base.clone(),
                slots: FeedSlots::default(),
                // Open: a refusal must not stop the next request of a test.
                guard: Arc::new(RequestGuard::new(GuardConfig {
                    block: TimeDelta::zero(),
                    restart_gap: TimeDelta::zero(),
                    ..GuardConfig::default()
                })),
            }
        }
    }

    #[tokio::test]
    async fn the_client_reads_the_pointer_and_the_killmails() {
        let server = StubServer::start(
            vec![
                ("/sequence.json", 200, r#"{"sequence":100023300}"#),
                ("/100023300.json", 200, SAMPLE),
            ],
            3,
        );
        let client = server.client();
        assert_eq!(client.pointer().await, Ok(100023300));
        let killmail = client.entry(100023300).await.unwrap().unwrap();
        assert_eq!(killmail.killmail_id, 139148857);
        // The hash and the location are read, and the file is kept as it came.
        assert_eq!(killmail.zkb.hash, "abc");
        assert_eq!(killmail.zkb.location_id, Some(60003760));
        assert_eq!(killmail.raw.as_deref(), Some(SAMPLE));
        // Nothing published yet at the next sequence number.
        assert_eq!(client.entry(100023301).await, Ok(None));

        let requests = server.requests.lock().unwrap();
        assert!(
            requests[0].starts_with("GET /sequence.json "),
            "{requests:?}"
        );
        // zKillboard blocks a blank User-Agent.
        for request in requests.iter() {
            assert!(
                request.to_lowercase().contains("user-agent: telescope/"),
                "{request}"
            );
        }
    }

    #[tokio::test]
    async fn the_client_tells_a_throttle_from_other_failures() {
        let server = StubServer::start(
            vec![
                ("/1.json", 403, ""),
                ("/2.json", 429, ""),
                ("/3.json", 500, ""),
                ("/4.json", 200, "not json"),
                ("/sequence.json", 200, r#"{"unexpected":1}"#),
            ],
            5,
        );
        let client = server.client();
        assert_eq!(
            client.entry(1).await,
            Err(FeedError::Throttled(StatusCode::FORBIDDEN))
        );
        assert_eq!(
            client.entry(2).await,
            Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS))
        );
        assert!(matches!(client.entry(3).await, Err(FeedError::Other(_))));
        assert!(matches!(client.entry(4).await, Err(FeedError::Other(_))));
        assert!(matches!(client.pointer().await, Err(FeedError::Other(_))));
    }

    #[tokio::test]
    async fn after_a_refusal_nothing_is_sent_until_the_block_is_over() {
        // Only one request is expected: the second must not be sent.
        let server = StubServer::start(vec![("/1.json", 429, "")], 1);
        let mut client = server.client();
        client.guard = Arc::new(RequestGuard::new(GuardConfig::default()));
        assert_eq!(
            client.entry(1).await,
            Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS))
        );
        assert_eq!(
            client.entry(2).await,
            Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS))
        );
        assert!(client.guard.wait(Utc::now()).is_some());
    }

    #[tokio::test]
    async fn an_unreachable_feed_is_a_plain_failure() {
        // Nothing listens on a port that was just released.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = R2z2Client {
            client: Client::builder().no_proxy().build().unwrap(),
            base: format!("http://127.0.0.1:{port}"),
            guard: Arc::new(RequestGuard::new(GuardConfig::default())),
            slots: FeedSlots::default(),
        };
        assert!(matches!(client.pointer().await, Err(FeedError::Other(_))));
    }

    #[tokio::test]
    async fn the_slots_of_the_feed_keep_the_pace_of_their_lane() {
        let lane = Arc::new(LaneQueue::new(LaneConfig {
            min_interval: Duration::from_millis(40),
            background_interval: Duration::from_millis(80),
            reserve_every: 1,
            low_every: 0,
        }));
        let live = FeedSlots::new(Arc::clone(&lane));
        let history = live.with_priority(Priority::Normal);
        // Three live requests: two gaps of the least time between requests.
        let started = Instant::now();
        for _ in 0..3 {
            live.wait().await;
        }
        assert!(started.elapsed() >= Duration::from_millis(80));
        // The hour read back is held to the slower pace of the background.
        let started = Instant::now();
        for _ in 0..3 {
            history.wait().await;
        }
        assert!(started.elapsed() >= Duration::from_millis(160));
        assert!(lane.is_empty());
    }
}
