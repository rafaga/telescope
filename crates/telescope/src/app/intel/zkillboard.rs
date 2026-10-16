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
//! [ 2026.10.14 05:01:05 ] zKillboard > Jita: Rifter destroyed by 3 pilots (Sabre, Sabre, Merlin), 12.3M ISK
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
//! * Up to 15 requests per second are allowed; the walk makes about 10.
//! * A blank `User-Agent` is blocked, and a throttled or blocked client gets
//!   a 403/429: it backs off for a long while ([`THROTTLE_PAUSE`]).
//!
//! What was already in the feed when the source starts is history, as with
//! the chat logs: the walk starts at the end of the feed and the killmails it
//! passes on the way are not announced (see [`FeedCursor`]).
//!
//! The source runs on its own thread, like the reader and the detection
//! stage, and only while the graph has an enabled zKillboard Input node
//! ([`ZKillboardHandle::follow`]): without one it makes no request at all.

use super::detection::ResolverHandle;
use super::input::InputEvent;
use crate::app::messages::{Message, Type, try_send_app_message};
use chrono::{DateTime, Utc};
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use webb::event::{EventSource, Facts, IntelEvent, ShipCount, SystemFact, TimeOrigin};
use webb::graph::{NodeKind, RuleGraph, SystemResolver};
use webb::rules::{InputKind, IntelLine};

/// Where the R2Z2 files are served from.
const FEED_URL: &str = "https://r2z2.zkillboard.com/ephemeral";

/// The `User-Agent` of every request. zKillboard's CDN blocks a blank one;
/// this one says who is asking.
const USER_AGENT: &str = concat!(
    "telescope/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/rafaga/telescope)"
);

/// How long a whole request may take before it is given up as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Pause between two killmail files read back to back (about 10 requests per
/// second, under the 15 the feed allows).
const PACE: Duration = Duration::from_millis(100);

/// Wait after reaching the end of the feed: killmails average one every 5.5
/// seconds, so asking sooner buys nothing, and 6 seconds is what the feed's
/// documentation asks for after a 404.
const POLL_WAIT: Duration = Duration::from_secs(6);

/// Pause after the feed answered 403 or 429 (throttled, or blocked for
/// asking too fast): the ban for that lasts an hour, so keep well away.
const THROTTLE_PAUSE: Duration = Duration::from_secs(10 * 60);

/// Longest wait between two attempts while the feed can't be reached.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// How long the feed may stay unreachable before the walk starts over at the
/// end of the feed: what was published meanwhile is too old to be intel.
const RESYNC_AFTER: Duration = Duration::from_secs(2 * 60);

/// How many killmail ids are remembered to announce a killmail only once
/// (zKillboard may publish a reprocessed killmail again under a new
/// sequence number).
const RECENT_IDS: usize = 4096;

/// The most attackers whose ship is named in a line.
const MAX_LISTED_ATTACKERS: usize = 8;

/// Author of the lines made from killmails.
const AUTHOR: &str = "zKillboard";

/// One file of the feed: a killmail with zKillboard's metadata. Only the
/// fields Telescope uses are read; the rest (and any field added later) is
/// ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Killmail {
    /// The killmail's id in ESI.
    pub killmail_id: u64,
    /// Position in the feed: strictly increasing, never reused.
    pub sequence_id: u64,
    /// The killmail as ESI has it.
    pub esi: EsiKillmail,
    /// zKillboard's own data about it.
    #[serde(default)]
    pub zkb: Zkb,
}

/// The ESI part of a [`Killmail`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct EsiKillmail {
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
pub(crate) struct Victim {
    /// Type id of the ship or structure.
    #[serde(default)]
    pub ship_type_id: Option<u32>,
}

/// One of the attackers of a killmail.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct Attacker {
    /// The pilot; absent for an NPC.
    #[serde(default)]
    pub character_id: Option<u64>,
    /// Type id of the ship it flew.
    #[serde(default)]
    pub ship_type_id: Option<u32>,
}

/// zKillboard's data about a killmail.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct Zkb {
    /// Estimated value of everything involved, in ISK.
    #[serde(default, rename = "totalValue")]
    pub total_value: f64,
}

impl Killmail {
    /// The line the detections read for this killmail, or `None` when its
    /// system has no name (the SDE isn't built yet or doesn't know it): a
    /// kill that can't be placed has nothing to alert about.
    ///
    /// `resolver` keeps a ship from being reported as a system: a ship named
    /// like one (there are a few) is left out of the text, since the *System
    /// report* detection would take it for the system.
    pub(crate) fn to_line(
        &self,
        names: &dyn NameSource,
        resolver: &dyn SystemResolver,
    ) -> Option<IntelLine> {
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
        let ships: Vec<String> = pilots
            .iter()
            .filter_map(|attacker| ship(attacker.ship_type_id))
            .take(MAX_LISTED_ATTACKERS)
            .collect();
        let victim = ship(self.esi.victim.ship_type_id).unwrap_or_else(|| String::from("ship"));

        let mut text = format!("{system}: {victim} destroyed by {by}");
        if !ships.is_empty() {
            text.push_str(&format!(" ({})", ships.join(", ")));
        }
        text.push_str(&format!(", {} ISK", Self::isk(self.zkb.total_value)));
        Some(IntelLine {
            timestamp: self.time().unwrap_or_else(Utc::now),
            author: String::from(AUTHOR),
            text,
        })
    }

    /// When the ship died, if the feed's time can be parsed.
    fn time(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.esi.killmail_time)
            .ok()
            .map(|time| time.with_timezone(&Utc))
    }

    /// `value` as a short amount: `950`, `12K`, `12.3M`, `1.5B`.
    fn isk(value: f64) -> String {
        match value {
            value if value >= 1e9 => format!("{:.1}B", value / 1e9),
            value if value >= 1e6 => format!("{:.1}M", value / 1e6),
            value if value >= 1e3 => format!("{:.0}K", value / 1e3),
            value => format!("{value:.0}"),
        }
    }
}

/// Turns the killmails of the feed into events: the format of the feed is
/// read here and nowhere else.
///
/// The event's text is the sentence [`Killmail::to_line`] writes, for the log,
/// the tooltips and the patterns of the user. Its facts come from the killmail
/// itself, not from that sentence: the system with its id, every ship that
/// took part with how many times, and the number of pilots.
pub(crate) struct ZKillboardDecoder<'a> {
    names: &'a dyn NameSource,
    resolver: &'a dyn SystemResolver,
}

impl<'a> ZKillboardDecoder<'a> {
    /// A decoder that names ids with `names`; `resolver` keeps a ship named
    /// like a system out of the text (see [`Killmail::to_line`]).
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

impl NameSource for SdeNames {
    fn system_name(&self, id: u32) -> Option<String> {
        self.lookup(id, Self::SYSTEM_SQL, |cache| &mut cache.systems)
    }

    fn type_name(&self, id: u32) -> Option<String> {
        self.lookup(id, Self::TYPE_SQL, |cache| &mut cache.types)
    }
}

/// Why the feed could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FeedError {
    /// The feed answered 403 or 429: throttled, or blocked for asking too
    /// much. Asking again soon makes it worse.
    Throttled(StatusCode),
    /// Anything else (no network, a server error, an unreadable file).
    Other(String),
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Throttled(status) => write!(f, "the feed refused the request ({status})"),
            Self::Other(reason) => f.write_str(reason),
        }
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

/// The R2Z2 feed over HTTPS.
pub(crate) struct R2z2Client {
    client: Client,
    base: String,
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
        })
    }

    /// `path` of the feed, or `None` for a 404.
    async fn get(&self, path: &str) -> Result<Option<Response>, FeedError> {
        let response = self
            .client
            .get(format!("{}/{path}", self.base))
            .send()
            .await
            .map_err(|error| FeedError::Other(error.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(response)),
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
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
        response
            .json()
            .await
            .map(Some)
            .map_err(|error| FeedError::Other(format!("unreadable killmail {sequence}: {error}")))
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
}

impl FeedCursor {
    pub(crate) fn new(pace: Duration) -> Self {
        Self {
            next: None,
            caught_up: false,
            recent: RecentIds::default(),
            pace,
        }
    }

    /// Starts over at the end of the feed the next time: what was published
    /// meanwhile is not announced. The ids already announced stay known.
    pub(crate) fn resync(&mut self) {
        self.next = None;
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
                    if self.recent.insert(killmail.killmail_id) && self.caught_up && !emit(killmail)
                    {
                        return Step::Closed;
                    }
                    if !self.pace.is_zero() {
                        tokio::time::sleep(self.pace).await;
                    }
                }
                Ok(None) => match feed.pointer().await {
                    Ok(pointer) if pointer > next => {
                        tracing::debug!(sequence = next, "zKillboard sequence hole skipped");
                        next += 1;
                        self.next = Some(next);
                    }
                    Ok(_) => {
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

/// What the UI and the source thread share.
struct Shared {
    /// Whether the graph has an enabled zKillboard Input node.
    enabled: AtomicBool,
    /// Wakes the source when it changes.
    wake: Notify,
    /// The SDE database the names come from.
    sde: Arc<RwLock<PathBuf>>,
}

/// The UI's handle on the zKillboard source: tells it whether anything wants
/// its killmails, and where the SDE is. Cheap to clone.
#[derive(Clone)]
pub(crate) struct ZKillboardHandle(Arc<Shared>);

impl ZKillboardHandle {
    /// A handle for a source that is off, reading names from the SDE at `sde`.
    pub(crate) fn new(sde: &Path) -> Self {
        Self(Arc::new(Shared {
            enabled: AtomicBool::new(false),
            wake: Notify::new(),
            sde: Arc::new(RwLock::new(sde.to_path_buf())),
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
    /// [`Killmail::to_line`]); `app_msg` is where trouble with the feed is
    /// reported to.
    pub(crate) fn spawn(
        &self,
        input: mpsc::Sender<InputEvent>,
        resolver: ResolverHandle,
        app_msg: Arc<mpsc::Sender<Message>>,
    ) {
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
            Ok(client) => client,
            Err(error) => {
                self.report(Type::Error, &format!("could not start: {error}"));
                return;
            }
        };
        let mut cursor = FeedCursor::new(PACE);
        let mut backoff = Backoff::new();
        let mut failing_since: Option<Instant> = None;
        let mut reported = false;
        loop {
            if !self.shared.enabled.load(Ordering::Acquire) {
                cursor.resync();
                backoff.reset();
                failing_since = None;
                reported = false;
                self.shared.wake.notified().await;
                continue;
            }
            let delay = match cursor.advance(&client, &mut |kill| self.emit(kill)).await {
                Step::Waiting => {
                    backoff.reset();
                    failing_since = None;
                    reported = false;
                    POLL_WAIT
                }
                Step::Closed => return,
                Step::Failed(error) => {
                    if failing_since.get_or_insert_with(Instant::now).elapsed() > RESYNC_AFTER {
                        cursor.resync();
                    }
                    let delay = match error {
                        FeedError::Throttled(_) => THROTTLE_PAUSE,
                        FeedError::Other(_) => backoff.next(),
                    };
                    tracing::warn!(%error, ?delay, "zKillboard feed not read");
                    // Once per outage: the status log is not for a retry loop.
                    if !reported {
                        reported = true;
                        self.report(
                            Type::Warning,
                            &format!("{error}; trying again in {}s", delay.as_secs()),
                        );
                    }
                    delay
                }
            };
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = self.shared.wake.notified() => {}
            }
        }
    }

    /// Sends one killmail to the detection stage. `false` when it has no
    /// receiver any more.
    fn emit(&self, killmail: Killmail) -> bool {
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
            Ok(()) => true,
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
    use webb::graph::Executor;
    use webb::rules::parse_line;

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
            "Jita: Rifter destroyed by 3 pilots (Sabre, Sabre, Merlin), 12.3M ISK"
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
            "Jita: Rifter destroyed by 3 pilots (Sabre, Sabre, Merlin), 12.3M ISK"
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
        assert_eq!(text.matches("Merlin").count(), MAX_LISTED_ATTACKERS);
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

        let mut graph = RuleGraph::default_graph();
        for node in &mut graph.nodes {
            if let NodeKind::Input(input) = &mut node.kind {
                input.kind = InputKind::ZKillboard;
            }
        }
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

        // The default rules, taking killmails.
        let mut graph = RuleGraph::default_graph();
        for node in &mut graph.nodes {
            if let NodeKind::Input(input) = &mut node.kind {
                input.kind = InputKind::ZKillboard;
            }
        }
        let (executor, errors) = Executor::new(graph);
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
        // The shipped graph reads chat logs only: no request is ever made
        // unless the user adds the node.
        assert!(!ZKillboardHandle::wanted_by(&RuleGraph::default_graph()));
    }

    #[test]
    fn the_handle_follows_the_graph() {
        let handle = ZKillboardHandle::new(Path::new("sde.db"));
        assert!(!handle.is_enabled());
        let mut graph = RuleGraph::default_graph();
        handle.follow(&graph);
        assert!(!handle.is_enabled());
        for node in &mut graph.nodes {
            if let NodeKind::Input(input) = &mut node.kind {
                input.kind = InputKind::ZKillboard;
            }
        }
        handle.follow(&graph);
        assert!(handle.is_enabled());
        handle.set_enabled(false);
        assert!(!handle.is_enabled());
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
        };
        assert!(matches!(client.pointer().await, Err(FeedError::Other(_))));
    }
}
