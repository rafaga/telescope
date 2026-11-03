//! The killmails of a system over the last day, asked of zKillboard's query
//! API.
//!
//! The R2Z2 feed only gives what happens from now on, and the buffer of the
//! node tooltips (see [`super::kill_context`]) keeps one hour of it. The day
//! line of a tooltip, "24 h: 6 kills", comes from the other API
//! (`/api/solarSystemID/<id>/pastSeconds/86400/`).
//!
//! # One queue
//!
//! Every request goes through the API lane of the `zkill` crate
//! ([`zkill::queue::LaneQueue`]), which is the only thing that spaces them:
//!
//! * A tooltip asks for its system as an interactive request, delayed by
//!   [`Prefetch::hover_delay`] so that a pointer only passing over a node
//!   asks for nothing. The request is leased: it is dropped unless the
//!   tooltip keeps asking ([`DayCache::look`] says when to submit it again).
//! * The plan ([`DayPlan`]) around the characters is submitted as background
//!   requests, the nearest first, which the lane holds to its own, slower
//!   pace so that the rest of the rate stays free for the tooltips.
//!
//! [`DayCache`] keeps what was learned and decides what is worth asking; it
//! never waits or sleeps.
//!
//! # Kept up by the feed
//!
//! The API lists the killmails themselves, so each one is remembered with its
//! time ([`DayLog`]) and the feed adds the ones it announces. While the feed
//! has been watching without a gap since before the data of an answer, the
//! count is simply the killmails of the log inside the last day, and the API
//! is asked again only after [`Prefetch::live_max_age`]. A gap in the feed
//! (it failed, or was switched off) ends that: the count is kept as it was
//! and the API is asked again.
//!
//! # What the API answers
//!
//! The API answers with the killmails themselves (200 at most per page,
//! newest first, without the last five minutes), with the `zkb` part, so
//! ESI is not asked for anything. The count is "24 h: N kills", a full page
//! is shown as "200+". An answer is kept until zKillboard says it may change
//! (its `Expires` header: a CDN caches the answers for an hour), and the
//! killmails of the last hour go to the tooltips' buffer when the feed is not
//! giving them.

use super::resolve::JumpGraph;
use super::zkill_guard::{GuardConfig, RequestGuard};
use super::zkillboard::{FeedError, REQUEST_TIMEOUT, USER_AGENT};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::header::HeaderMap;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zkill::lane::{Job, Priority};
use zkill::queue::LaneQueue;
use zkill::types::{EsiKillmail, Killmail, Zkb};

/// Base of zKillboard's query API.
const API_URL: &str = "https://zkillboard.com/api";

/// How far back the day line looks.
const DAY_SECONDS: u64 = 24 * 60 * 60;

/// How long an answer is kept when zKillboard does not say, and the most it
/// is kept whatever it says (zKillboard caches its answers that long).
pub(crate) const TTL: Duration = Duration::from_secs(60 * 60);

/// How long to leave a system alone after a failed request.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// Killmails per page of the API: a full page may have more behind it.
const PAGE_SIZE: usize = 200;

/// How the counts of the last day are filled in the background, without
/// waiting for a tooltip to ask: first the systems around the characters,
/// the nearest first, then the rest of the region they are in, and last the
/// regions next to it. The other systems are asked about only when a tooltip
/// shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prefetch {
    /// How far from a character, in jumps, a system is "around" it.
    pub max_jumps: u8,
    /// How old the count of a system of the characters' region, beyond that
    /// radius, may get before it is asked again. (The ones around the
    /// characters are asked again as soon as zKillboard says they may have
    /// changed.)
    pub region_ttl: Duration,
    /// How old the count of a system of a region next to the characters'
    /// may get before it is asked again. These are filled in last, with the
    /// requests the rest of the plan leaves free.
    pub neighbor_ttl: Duration,
    /// How long a tooltip has to stay on a system before the request for
    /// it is made: a pointer only crossing a node asks for nothing.
    pub hover_delay: Duration,
    /// How long the request of a tooltip is kept without the tooltip asking
    /// for it again (it asks on every frame it is shown).
    pub tooltip_lease: Duration,
    /// The least time an answer is kept, whatever zKillboard says.
    pub min_fresh: Duration,
    /// The longest a count is kept up by the feed alone, without asking the
    /// API again.
    pub live_max_age: Duration,
    /// How much of the end of the data the API leaves out (it lists up to
    /// five minutes ago, and its CDN adds some).
    pub api_lag: Duration,
    /// How many requests in a row may fail before the lane is held.
    pub backoff_after: u32,
    /// How long the lane is held once that many failed, one step for each
    /// failure after that; the last one stays for as long as the failures
    /// go on.
    pub backoff: [Duration; 4],
    /// The longest the plan goes without being made again. It is also made
    /// when a character changes system and when the first count of the plan
    /// comes due.
    pub replan_every: Duration,
    /// The least time before the plan is made again because a count came
    /// due: the counts come due one after another, and this makes them share
    /// one plan.
    pub replan_floor: Duration,
    /// How soon to try again when the plan can't be made yet (the universe
    /// is not loaded).
    pub retry_every: Duration,
    /// The counts are kept on disk every this many answers (and when the
    /// queue runs dry), not after each one.
    pub save_every: u32,
}

impl Default for Prefetch {
    fn default() -> Self {
        Self {
            max_jumps: 5,
            region_ttl: Duration::from_secs(3 * 60 * 60),
            neighbor_ttl: Duration::from_secs(6 * 60 * 60),
            hover_delay: Duration::from_millis(300),
            tooltip_lease: Duration::from_secs(3),
            min_fresh: Duration::from_secs(60),
            live_max_age: Duration::from_secs(6 * 60 * 60),
            api_lag: Duration::from_secs(6 * 60),
            backoff_after: 5,
            backoff: [
                Duration::from_secs(60),
                Duration::from_secs(5 * 60),
                Duration::from_secs(10 * 60),
                Duration::from_secs(15 * 60),
            ],
            replan_every: Duration::from_secs(10 * 60),
            replan_floor: Duration::from_secs(15),
            retry_every: Duration::from_secs(60),
            save_every: 25,
        }
    }
}

/// The systems to fill in, in the order to do it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DayPlan {
    /// Within [`Prefetch::max_jumps`] of a character, the nearest first.
    pub near: Vec<usize>,
    /// The other systems of the characters' region, the nearest first.
    pub region: Vec<usize>,
    /// The systems of the regions next to the characters' (a stargate leads
    /// to them from it), the nearest first. The last ones to be asked.
    pub neighbors: Vec<usize>,
}

impl Prefetch {
    /// The order to ask for the systems of `jumps`: those at most
    /// `max_jumps` stargates from any of `origins` (the nearest first, from
    /// whichever origin is nearest), then the others of `region` (the
    /// systems of the regions the origins are in), also the nearest first,
    /// and last those of `neighbors` (the regions next to them), likewise.
    #[tracing::instrument(skip_all)]
    pub(crate) fn plan(
        &self,
        jumps: &JumpGraph,
        origins: &[u32],
        region: &HashSet<u32>,
        neighbors: &HashSet<u32>,
    ) -> DayPlan {
        let mut seen: HashSet<u32> = HashSet::new();
        let mut queue: VecDeque<(u32, u8)> = VecDeque::new();
        for &origin in origins {
            if seen.insert(origin) {
                queue.push_back((origin, 0));
            }
        }
        let mut near = Vec::new();
        let mut around = Vec::new();
        let mut beside = Vec::new();
        while let Some((system, distance)) = queue.pop_front() {
            if distance <= self.max_jumps {
                near.push(system as usize);
            } else if region.contains(&system) {
                around.push(system as usize);
            } else if neighbors.contains(&system) {
                beside.push(system as usize);
            }
            for &next in jumps.get(&system).into_iter().flatten() {
                if seen.insert(next) {
                    queue.push_back((next, distance.saturating_add(1)));
                }
            }
        }
        // The systems of the region no stargate leads to from the origins.
        let mut apart: Vec<usize> = region
            .iter()
            .filter(|system| !seen.contains(system))
            .map(|&system| system as usize)
            .collect();
        apart.sort_unstable();
        around.extend(apart);
        let mut unlinked: Vec<usize> = neighbors
            .iter()
            .filter(|system| !seen.contains(system))
            .map(|&system| system as usize)
            .collect();
        unlinked.sort_unstable();
        beside.extend(unlinked);
        DayPlan {
            near,
            region: around,
            neighbors: beside,
        }
    }

    /// The systems of the regions next to those of `region`: a region is
    /// next to them when a stargate leads to it from any system of `region`.
    /// `regions` tells the region of every system.
    #[tracing::instrument(skip_all)]
    pub(crate) fn neighbor_systems(
        jumps: &JumpGraph,
        regions: &HashMap<u32, u32>,
        region: &HashSet<u32>,
    ) -> HashSet<u32> {
        let home: HashSet<u32> = region
            .iter()
            .filter_map(|system| regions.get(system).copied())
            .collect();
        let next: HashSet<u32> = region
            .iter()
            .flat_map(|system| jumps.get(system).into_iter().flatten())
            .filter_map(|system| regions.get(system).copied())
            .filter(|id| !home.contains(id))
            .collect();
        regions
            .iter()
            .filter(|(_, id)| next.contains(id))
            .map(|(&system, _)| system)
            .collect()
    }

    /// The request of a tooltip for `system_id`: interactive, delayed by
    /// [`Self::hover_delay`], and dropped unless it is made again within
    /// [`Self::tooltip_lease`].
    pub(crate) fn tooltip(&self, system_id: usize) -> Job<usize> {
        Job::new(system_id as u64, system_id, Priority::Interactive)
            .after(self.hover_delay)
            .leased(self.tooltip_lease)
    }
}

/// What the API said about a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayCount {
    /// Killmails in the last day (the first page of them).
    pub kills: usize,
    /// Whether a full page came back, so there may be more.
    pub more: bool,
}

/// What the API said about a system.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DayAnswer {
    pub count: DayCount,
    /// The killmails listed, newest first.
    pub kills: Vec<Killmail>,
    /// How long zKillboard says the answer stays as it is (its `Expires`
    /// header), if it says.
    pub fresh_for: Option<Duration>,
    /// The moment the data is from (`Last-Modified`, or now minus `Age`): a
    /// cached answer can be old.
    pub data_time: DateTime<Utc>,
}

/// What a tooltip can say about the last day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DayLine {
    /// The request is on its way.
    Pending,
    /// The count, as of the last answer.
    Known(DayCount),
    /// The last request failed; it is tried again later.
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    /// Wanted by a tooltip, or about to be asked.
    Pending,
    Ready {
        count: DayCount,
        /// When it was asked.
        at: Instant,
        /// Until when zKillboard says it stays as it is.
        expires: Instant,
        /// While the feed has seen every killmail since the data of the
        /// answer, the count comes from the log and is good until then.
        live_until: Option<Instant>,
    },
    Failed {
        at: Instant,
    },
}

impl Entry {
    /// A count asked at `at` that stays good for `fresh`.
    fn ready(count: DayCount, at: Instant, fresh: Duration) -> Self {
        Entry::Ready {
            count,
            at,
            expires: at + fresh,
            live_until: None,
        }
    }

    /// Until when the count of a ready entry is good.
    fn good_until(&self) -> Option<Instant> {
        match self {
            Entry::Ready {
                expires,
                live_until,
                ..
            } => Some(live_until.map_or(*expires, |live| live.max(*expires))),
            _ => None,
        }
    }
}

/// A count kept on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SavedCount {
    pub system: usize,
    pub count: DayCount,
    /// How long ago it was asked.
    pub age: TimeDelta,
    /// How long it stays good from now (nothing when it is stale).
    pub fresh_for: TimeDelta,
}

/// The killmails of the last day seen per system, from the API's answers and
/// the feed.
#[derive(Debug, Default)]
struct DayLog {
    systems: HashMap<usize, BTreeSet<(DateTime<Utc>, u64)>>,
}

impl DayLog {
    /// Remembers killmail `id`, which happened at `time`, in `system`.
    fn observe(&mut self, system: usize, id: u64, time: DateTime<Utc>, now: DateTime<Utc>) {
        let cut = now - TimeDelta::days(1);
        if time < cut {
            return;
        }
        let set = self.systems.entry(system).or_default();
        set.insert((time, id));
        while set.first().is_some_and(|(oldest, _)| *oldest < cut) {
            set.pop_first();
        }
    }

    /// How many killmails of `system` happened in the last day as of `now`.
    fn count(&self, system: usize, now: DateTime<Utc>) -> usize {
        let cut = now - TimeDelta::days(1);
        self.systems
            .get(&system)
            .map_or(0, |set| set.iter().filter(|(time, _)| *time >= cut).count())
    }
}

/// What a step of the fetcher did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Fetched {
    /// A system was asked: the killmails the API listed for it.
    Answered { kills: Vec<Killmail> },
    /// A system was asked and the request failed.
    Failed,
    /// Nothing was asked: the answer is still good, or the source was
    /// switched off while the request waited.
    Skipped,
    /// The API refused the request: stay away for a long while.
    Throttled,
}

/// The reads the tooltips need from the API.
pub(crate) trait DayApi {
    /// The killmails of `system_id` in the last day.
    async fn kills(&self, system_id: usize) -> Result<DayAnswer, FeedError>;
}

/// What a waiting request is for.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueKind {
    /// The count of a system whose tooltip is waiting.
    Tooltip,
    /// The count of a system around the characters.
    Near,
    /// The count of another system of the characters' region.
    Region,
    /// The count of a system of a region next to the characters'.
    Neighbors,
}

#[cfg(debug_assertions)]
impl QueueKind {
    /// What a request of `priority` is for.
    pub(crate) fn of(priority: Priority) -> Self {
        match priority {
            Priority::Interactive => QueueKind::Tooltip,
            Priority::High => QueueKind::Near,
            Priority::Normal => QueueKind::Region,
            Priority::Low => QueueKind::Neighbors,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            QueueKind::Tooltip => "Count (tooltip)",
            QueueKind::Near => "Count (near)",
            QueueKind::Region => "Count (region)",
            QueueKind::Neighbors => "Count (neighbor regions)",
        }
    }
}

/// A request waiting for its turn.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedRequest {
    pub system_id: usize,
    pub kind: QueueKind,
    /// Estimated time until it is made.
    pub eta: Duration,
}

/// The requests waiting, in the order they will be made.
#[cfg(debug_assertions)]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DayQueue {
    /// The first ones.
    pub requests: Vec<QueuedRequest>,
    /// How many wait in all.
    pub total: usize,
}

/// The counts known; the queue of the requests is the lane's.
#[derive(Debug, Default)]
pub(crate) struct DayCache {
    entries: HashMap<usize, Entry>,
    /// The systems being asked right now.
    in_flight: HashSet<usize>,
    /// The killmails of the last day seen.
    log: DayLog,
    /// Since when the feed has been watching without a gap, if it is.
    feed_since: Option<Instant>,
    /// The last plan taken, to know when its counts come due.
    plan: DayPlan,
    /// How many requests in a row failed.
    failures: u32,
}

impl DayCache {
    /// What to show for `system_id` as of `now`. The second value tells
    /// whether the tooltip's request must be submitted (again) to the lane
    /// (see [`Prefetch::tooltip`]): there is no fresh answer and none is on
    /// its way. Submitting it again is harmless, it merges with the one
    /// waiting and renews its lease; it is what keeps it from lapsing.
    pub(crate) fn look(&mut self, system_id: usize, now: Instant) -> (DayLine, bool) {
        let (line, ask) = match self.entries.get(&system_id).copied() {
            // The old count shows while a new one is asked.
            Some(Entry::Ready {
                count,
                expires,
                live_until,
                ..
            }) => {
                let good_until = live_until.map_or(expires, |live| live.max(expires));
                let shown = self.shown(system_id, count, live_until.is_some());
                (DayLine::Known(shown), now >= good_until)
            }
            Some(Entry::Pending) => (DayLine::Pending, true),
            Some(Entry::Failed { at }) if now.saturating_duration_since(at) < RETRY_AFTER => {
                (DayLine::Unavailable, false)
            }
            Some(Entry::Failed { .. }) | None => {
                self.entries.insert(system_id, Entry::Pending);
                (DayLine::Pending, true)
            }
        };
        (line, ask && !self.in_flight.contains(&system_id))
    }

    /// Whether the count of `system_id` should be asked for now: nothing is
    /// known, or what is known may have changed by now (zKillboard's
    /// `Expires`, and `bound` after it was asked), or the last request failed
    /// long enough ago. Not when a request is already on its way.
    fn wants(&self, system_id: usize, bound: Duration, now: Instant) -> bool {
        if self.in_flight.contains(&system_id) {
            return false;
        }
        match self.entries.get(&system_id) {
            None | Some(Entry::Pending) => true,
            Some(entry @ Entry::Ready { at, .. }) => entry
                .good_until()
                .is_none_or(|until| now >= until.max(*at + bound)),
            Some(Entry::Failed { at }) => now.saturating_duration_since(*at) >= RETRY_AFTER,
        }
    }

    /// Whether the count of `system_id` is good as of `now`.
    fn is_fresh(&self, system_id: usize, now: Instant) -> bool {
        self.entries
            .get(&system_id)
            .and_then(Entry::good_until)
            .is_some_and(|until| now < until)
    }

    /// What to show for a count: when the feed keeps it up, what the log has
    /// seen in the last day.
    fn shown(&self, system_id: usize, count: DayCount, live: bool) -> DayCount {
        if live {
            DayCount {
                kills: self.log.count(system_id, Utc::now()),
                more: false,
            }
        } else {
            count
        }
    }

    /// The feed saw a killmail: it counts for the last day of its system.
    pub(crate) fn observe(&mut self, system_id: usize, id: u64, time: DateTime<Utc>) {
        self.log.observe(system_id, id, time, Utc::now());
    }

    /// The feed is watching, with no gap, from `now` on (if it was not
    /// already).
    pub(crate) fn feed_up(&mut self, now: Instant) {
        self.feed_since.get_or_insert(now);
    }

    /// The feed has a gap (it failed, or was switched off): the counts it was
    /// keeping up stay as they are and are asked again.
    pub(crate) fn feed_down(&mut self) {
        self.feed_since = None;
        let Self { entries, log, .. } = self;
        let now = Utc::now();
        for (system, entry) in entries.iter_mut() {
            if let Entry::Ready {
                count, live_until, ..
            } = entry
                && live_until.take().is_some()
            {
                *count = DayCount {
                    kills: log.count(*system, now),
                    more: false,
                };
            }
        }
    }

    /// Whether the feed has seen every killmail since the data of an answer
    /// was made, apart from the end the API leaves out.
    fn covers(
        &self,
        data_time: DateTime<Utc>,
        prefetch: &Prefetch,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) -> bool {
        let Some(since) = self.feed_since else {
            return false;
        };
        let age = (now_utc - data_time).to_std().unwrap_or_default();
        now.checked_sub(age)
            .is_some_and(|data| since + prefetch.api_lag <= data)
    }

    /// Takes `plan` as the systems to fill in while no tooltip waits,
    /// replacing the previous plan, and gives the requests for it: the ones
    /// around the characters when their count may have changed (zKillboard's
    /// `Expires`), the others of the region when it is older than
    /// [`Prefetch::region_ttl`], and the ones of the regions next to it when
    /// it is older than [`Prefetch::neighbor_ttl`] (or unknown). They are
    /// background requests, the nearest first; the caller hands them to
    /// [`LaneQueue::replace_background`].
    #[tracing::instrument(skip_all)]
    pub(crate) fn schedule(
        &mut self,
        plan: &DayPlan,
        prefetch: &Prefetch,
        now: Instant,
    ) -> Vec<Job<usize>> {
        self.plan = plan.clone();
        let mut jobs = Vec::new();
        for (systems, bound, priority) in [
            (&plan.near, Duration::ZERO, Priority::High),
            (&plan.region, prefetch.region_ttl, Priority::Normal),
            (&plan.neighbors, prefetch.neighbor_ttl, Priority::Low),
        ] {
            for &system in systems {
                if self.wants(system, bound, now) {
                    jobs.push(Job::new(system as u64, system, priority));
                }
            }
        }
        jobs
    }

    /// How long until the first count of the last plan taken comes due to be
    /// asked again (the ones around the characters when zKillboard says they
    /// may have changed, the others as long as [`Prefetch`] says; a failed one
    /// comes due after [`RETRY_AFTER`]). The ones already due are left out:
    /// they are in the queue. `None` when no count is waiting for that.
    pub(crate) fn next_due(&self, prefetch: &Prefetch, now: Instant) -> Option<Duration> {
        [
            (&self.plan.near, Duration::ZERO),
            (&self.plan.region, prefetch.region_ttl),
            (&self.plan.neighbors, prefetch.neighbor_ttl),
        ]
        .into_iter()
        .flat_map(|(systems, bound)| systems.iter().map(move |system| (system, bound)))
        .filter_map(|(system, bound)| {
            let due = match self.entries.get(system)? {
                entry @ Entry::Ready { at, .. } => entry.good_until()?.max(*at + bound),
                Entry::Failed { at } => *at + RETRY_AFTER,
                Entry::Pending => return None,
            };
            due.checked_duration_since(now)
        })
        .filter(|left| !left.is_zero())
        .min()
    }

    /// How long the lane is held after the failures in a row (see
    /// [`Prefetch::backoff_after`]).
    fn backoff(&self, prefetch: &Prefetch) -> Duration {
        let Some(extra) = self.failures.checked_sub(prefetch.backoff_after) else {
            return Duration::ZERO;
        };
        if self.failures == 0 {
            return Duration::ZERO;
        }
        let last = prefetch.backoff.len() - 1;
        let step = usize::try_from(extra).map_or(last, |extra| extra.min(last));
        prefetch.backoff[step]
    }

    /// How many requests in a row failed.
    pub(crate) fn failures(&self) -> u32 {
        self.failures
    }

    /// Stores what the API answered about `system_id`.
    fn record(
        &mut self,
        system_id: usize,
        result: &Result<DayAnswer, FeedError>,
        prefetch: &Prefetch,
        now: Instant,
    ) {
        self.in_flight.remove(&system_id);
        match result {
            Ok(answer) => {
                self.failures = 0;
                let fresh = answer
                    .fresh_for
                    .map_or(TTL, |fresh| fresh.max(prefetch.min_fresh).min(TTL));
                let now_utc = Utc::now();
                for killmail in &answer.kills {
                    if let Some(time) = killmail.time() {
                        self.log
                            .observe(system_id, killmail.killmail_id, time, now_utc);
                    }
                }
                // The log is the whole answer only when the page is not full
                // and every killmail could be read.
                let live = !answer.count.more
                    && answer.kills.len() == answer.count.kills
                    && self.covers(answer.data_time, prefetch, now, now_utc);
                let mut entry = Entry::ready(answer.count, now, fresh);
                if live && let Entry::Ready { live_until, .. } = &mut entry {
                    *live_until = Some(now + prefetch.live_max_age);
                }
                self.entries.insert(system_id, entry);
            }
            Err(_) => {
                self.failures = self.failures.saturating_add(1);
                self.entries.insert(system_id, Entry::Failed { at: now });
            }
        }
    }

    /// Gives up the request for `system_id` that was about to be made.
    fn dropped(&mut self, system_id: usize) {
        if matches!(self.entries.get(&system_id), Some(Entry::Pending)) {
            self.entries.remove(&system_id);
        }
    }

    /// The counts that are known, with how long ago each was asked, to be
    /// kept on disk.
    pub(crate) fn snapshot(&self, now: Instant) -> Vec<SavedCount> {
        self.entries
            .iter()
            .filter_map(|(system, entry)| match entry {
                Entry::Ready {
                    count, at, expires, ..
                } => {
                    let age = TimeDelta::from_std(now.saturating_duration_since(*at)).ok()?;
                    let fresh_for =
                        TimeDelta::from_std(expires.saturating_duration_since(now)).ok()?;
                    Some(SavedCount {
                        system: *system,
                        count: *count,
                        age,
                        fresh_for,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Takes back counts kept on disk (see [`Self::snapshot`]) for the
    /// systems nothing is known about yet. They are not kept up by the feed
    /// (the log is not kept): they are good for as long as they were.
    pub(crate) fn restore(&mut self, counts: impl IntoIterator<Item = SavedCount>, now: Instant) {
        for saved in counts {
            if let Ok(age) = saved.age.to_std()
                && let Some(at) = now.checked_sub(age)
            {
                let fresh = saved.fresh_for.to_std().unwrap_or_default();
                self.entries.entry(saved.system).or_insert(Entry::Ready {
                    count: saved.count,
                    at,
                    expires: now + fresh,
                    live_until: None,
                });
            }
        }
    }

    /// Forgets everything.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.plan = DayPlan::default();
        self.log = DayLog::default();
        self.feed_since = None;
    }

    /// Asks the API about the next system of `lane` and stores the answer.
    /// The lock is not held while the request is made, and the lane decides
    /// when it goes out. After a run of failures the lane is held (see
    /// [`Prefetch::backoff_after`]).
    ///
    /// `allowed` tells whether the source is still on once the request's
    /// turn has come.
    pub(crate) async fn fetch_next<A: DayApi>(
        cache: &Mutex<Self>,
        lane: &LaneQueue<usize>,
        api: &A,
        prefetch: &Prefetch,
        allowed: impl Fn() -> bool,
    ) -> Fetched {
        let system_id = lane.next().await.payload;
        {
            let mut cache = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !allowed() {
                cache.dropped(system_id);
                return Fetched::Skipped;
            }
            // Asked for twice (a tooltip and the plan, or a tooltip while
            // the answer was on its way): the second one is not needed.
            if cache.is_fresh(system_id, Instant::now()) {
                return Fetched::Skipped;
            }
            cache.in_flight.insert(system_id);
        }
        let result = api.kills(system_id).await;
        let backoff = {
            let mut cache = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.record(system_id, &result, prefetch, Instant::now());
            cache.backoff(prefetch)
        };
        if !backoff.is_zero() {
            lane.hold_for(backoff);
        }
        match result {
            Ok(answer) => Fetched::Answered {
                kills: answer.kills,
            },
            Err(FeedError::Throttled(_) | FeedError::Paused) => Fetched::Throttled,
            Err(_) => Fetched::Failed,
        }
    }
}

/// zKillboard's query API over HTTPS.
pub(crate) struct ZKillApi {
    client: Client,
    base: String,
    /// Keeps the requests away from zKillboard while it asked for it.
    guard: Arc<RequestGuard>,
}

impl ZKillApi {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base: String::from(API_URL),
            guard: Arc::new(RequestGuard::new(GuardConfig::default())),
        })
    }

    /// The same client, asking only when `guard` lets it (the one every
    /// request to the API shares).
    pub(crate) fn with_guard(mut self, guard: Arc<RequestGuard>) -> Self {
        self.guard = guard;
        self
    }

    /// The URL of the killmails of `system_id` in the last day. The query is
    /// in the path, and the trailing slash is required.
    fn day_url(&self, system_id: usize) -> String {
        format!(
            "{}/solarSystemID/{system_id}/pastSeconds/{DAY_SECONDS}/",
            self.base
        )
    }

    /// How long the answer stays as it is, as of `now`, according to its
    /// `Expires` header; `None` when it has none. An `Expires` in the past is
    /// no time at all.
    fn fresh_for(headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
        let expires = headers.get(reqwest::header::EXPIRES)?.to_str().ok()?;
        let expires = DateTime::parse_from_rfc2822(expires).ok()?;
        Some(
            (expires.with_timezone(&Utc) - now)
                .to_std()
                .unwrap_or_default(),
        )
    }

    /// The moment the data of the answer is from: `Last-Modified`, or the
    /// time of the answer minus its `Age` (a CDN serves it for a while), or
    /// `now` when it says neither.
    fn data_time(headers: &HeaderMap, now: DateTime<Utc>) -> DateTime<Utc> {
        let modified = headers
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
            .map(|time| time.with_timezone(&Utc));
        if let Some(modified) = modified {
            return modified.min(now);
        }
        headers
            .get(reqwest::header::AGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok())
            .and_then(TimeDelta::try_seconds)
            .map_or(now, |age| now - age)
    }

    /// The count and the killmails of the API's answer. A killmail that can't
    /// be read is left out of the list but still counts.
    #[tracing::instrument(skip_all, fields(items = items.len()))]
    fn parse(items: Vec<serde_json::Value>) -> (DayCount, Vec<Killmail>) {
        /// A killmail as the API lists it: ESI's fields and zKillboard's.
        #[derive(Deserialize)]
        struct Item {
            killmail_id: u64,
            #[serde(default)]
            zkb: Zkb,
            #[serde(flatten)]
            esi: EsiKillmail,
        }
        let count = DayCount {
            kills: items.len(),
            more: items.len() >= PAGE_SIZE,
        };
        let kills = items
            .into_iter()
            .filter_map(|value| {
                let raw = value.to_string();
                let item: Item = serde_json::from_value(value).ok()?;
                Some(Killmail {
                    killmail_id: item.killmail_id,
                    sequence_id: 0,
                    esi: item.esi,
                    zkb: item.zkb,
                    raw: Some(raw),
                })
            })
            .collect();
        (count, kills)
    }
}

impl DayApi for ZKillApi {
    async fn kills(&self, system_id: usize) -> Result<DayAnswer, FeedError> {
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
            .get(self.day_url(system_id))
            .send()
            .await
            .map_err(|error| {
                self.guard.failed(Utc::now());
                FeedError::Other(error.to_string())
            })?;
        match response.status() {
            status if status.is_success() => self.guard.succeeded(Utc::now()),
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
                tracing::warn!(
                    %status,
                    system_id,
                    retry_after = ?response.headers().get(reqwest::header::RETRY_AFTER),
                    sent = self.guard.sent(),
                    "zKillboard refused a day-count request"
                );
                self.guard
                    .throttled_for(Utc::now(), RequestGuard::retry_after(response.headers()));
                return Err(FeedError::Throttled(status));
            }
            status => {
                self.guard.failed(Utc::now());
                return Err(FeedError::Other(format!("the API answered {status}")));
            }
        }
        let now = Utc::now();
        let fresh_for = Self::fresh_for(response.headers(), now);
        let data_time = Self::data_time(response.headers(), now);
        let items: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|error| FeedError::Other(format!("unreadable answer: {error}")))?;
        let (count, kills) = Self::parse(items);
        Ok(DayAnswer {
            count,
            kills,
            fresh_for,
            data_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zkill::lane::LaneConfig;
    use zkill::types::Victim;

    /// An API in memory: the same answer for every system.
    struct FakeApi {
        answer: Result<DayAnswer, FeedError>,
        asked: AtomicUsize,
        /// The systems asked about, in order.
        order: Mutex<Vec<usize>>,
    }

    impl FakeApi {
        fn new(answer: Result<DayCount, FeedError>) -> Self {
            Self::fresh(answer, None)
        }

        /// An API that says its answers stay as they are for `fresh_for`.
        fn fresh(answer: Result<DayCount, FeedError>, fresh_for: Option<Duration>) -> Self {
            Self {
                answer: answer.map(|count| DayAnswer {
                    count,
                    kills: Vec::new(),
                    fresh_for,
                    data_time: Utc::now(),
                }),
                asked: AtomicUsize::new(0),
                order: Mutex::new(Vec::new()),
            }
        }
    }

    impl DayApi for FakeApi {
        async fn kills(&self, system_id: usize) -> Result<DayAnswer, FeedError> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push(system_id);
            self.answer.clone()
        }
    }

    const COUNT: DayCount = DayCount {
        kills: 6,
        more: false,
    };

    /// Things the tests set up the same way.
    struct Setup;

    impl Setup {
        /// A lane with no pace at all, to look at the order alone.
        fn lane() -> LaneQueue<usize> {
            LaneQueue::new(LaneConfig {
                min_interval: Duration::ZERO,
                background_interval: Duration::ZERO,
                reserve_every: 1000,
                low_every: 0,
            })
        }

        /// Tooltips are asked at once and a run of failures holds the lane
        /// for a moment only.
        fn prefetch() -> Prefetch {
            Prefetch {
                hover_delay: Duration::ZERO,
                backoff: [Duration::from_millis(1); 4],
                ..Prefetch::default()
            }
        }

        /// Asks until the lane is empty, giving what each step did.
        async fn drain(
            cache: &Mutex<DayCache>,
            lane: &LaneQueue<usize>,
            api: &FakeApi,
        ) -> Vec<Fetched> {
            let prefetch = Self::prefetch();
            let mut steps = Vec::new();
            while !lane.is_empty() {
                steps.push(DayCache::fetch_next(cache, lane, api, &prefetch, || true).await);
            }
            steps
        }
    }

    /// A straight line of stargates 1 - 2 - ... - `length`, plus the
    /// systems in `apart`, which have none.
    fn line(length: u32, apart: &[u32]) -> JumpGraph {
        let mut graph = JumpGraph::new();
        for system in 1..=length {
            let mut links = Vec::new();
            if system > 1 {
                links.push(system - 1);
            }
            if system < length {
                links.push(system + 1);
            }
            graph.insert(system, links);
        }
        for system in apart {
            graph.insert(*system, Vec::new());
        }
        graph
    }

    #[test]
    fn the_plan_goes_from_the_nearest_to_the_farthest_and_then_the_region() {
        let prefetch = Prefetch {
            max_jumps: 2,
            ..Prefetch::default()
        };
        // The region has no stargate to 20; 21 is another region's.
        let region: HashSet<u32> = [3, 4, 5, 6, 7, 8, 20].into();
        let plan = prefetch.plan(&line(8, &[20, 21]), &[3], &region, &HashSet::new());
        assert_eq!(plan.near, [3, 2, 4, 1, 5]);
        assert_eq!(plan.region, [6, 7, 8, 20]);
        assert!(plan.neighbors.is_empty());
    }

    #[test]
    fn the_regions_next_door_come_last_and_the_nearest_first() {
        let prefetch = Prefetch {
            max_jumps: 1,
            ..Prefetch::default()
        };
        // Systems 1-4 are one region, 5-8 and 20 the next one; 9-10 are a
        // third one that only the second one is connected to.
        let regions: HashMap<u32, u32> = (1..=4)
            .map(|system| (system, 100))
            .chain((5..=8).map(|system| (system, 200)))
            .chain([(20, 200), (9, 300), (10, 300)])
            .collect();
        let graph = line(10, &[20]);
        let region: HashSet<u32> = (1..=4).collect();
        let neighbors = Prefetch::neighbor_systems(&graph, &regions, &region);
        assert_eq!(neighbors, HashSet::from([5, 6, 7, 8, 20]));
        let plan = prefetch.plan(&graph, &[1], &region, &neighbors);
        assert_eq!(plan.near, [1, 2]);
        assert_eq!(plan.region, [3, 4]);
        // 20 has no stargate: it goes after the ones that can be reached.
        assert_eq!(plan.neighbors, [5, 6, 7, 8, 20]);
    }

    #[test]
    fn with_several_characters_each_system_counts_from_its_nearest() {
        let prefetch = Prefetch {
            max_jumps: 1,
            ..Prefetch::default()
        };
        let region: HashSet<u32> = (1..=8).collect();
        let plan = prefetch.plan(&line(8, &[]), &[1, 8, 1], &region, &HashSet::new());
        assert_eq!(plan.near, [1, 8, 2, 7]);
        assert_eq!(plan.region, [3, 6, 4, 5]);
    }

    #[test]
    fn without_characters_nothing_is_planned() {
        let plan = Prefetch::default().plan(&line(3, &[]), &[], &HashSet::new(), &HashSet::new());
        assert!(plan.near.is_empty());
        assert!(plan.region.is_empty());
        assert!(plan.neighbors.is_empty());
    }

    #[test]
    fn the_plan_becomes_background_requests_and_skips_what_is_fresh() {
        let mut cache = DayCache::default();
        let now = Instant::now();
        // System 2 was asked a moment ago.
        cache
            .entries
            .insert(2, Entry::ready(COUNT, now, Duration::from_secs(600)));
        let plan = DayPlan {
            near: vec![1, 2],
            region: vec![3],
            neighbors: vec![4],
        };
        let jobs = cache.schedule(&plan, &Prefetch::default(), now);
        let jobs: Vec<(u64, Priority)> = jobs.iter().map(|job| (job.key, job.priority)).collect();
        assert_eq!(
            jobs,
            [
                (1, Priority::High),
                (3, Priority::Normal),
                (4, Priority::Low)
            ]
        );
    }

    #[test]
    fn a_system_being_asked_is_not_planned_again() {
        let mut cache = DayCache::default();
        cache.in_flight.insert(1);
        let plan = DayPlan {
            near: vec![1, 2],
            ..DayPlan::default()
        };
        let jobs = cache.schedule(&plan, &Prefetch::default(), Instant::now());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].key, 2);
    }

    #[test]
    fn the_region_is_asked_again_less_often_than_the_surroundings() {
        let mut cache = DayCache::default();
        let prefetch = Prefetch::default();
        let start = Instant::now();
        for system in [1, 2] {
            cache
                .entries
                .insert(system, Entry::ready(COUNT, start, TTL));
        }
        let plan = DayPlan {
            near: vec![1],
            region: vec![2],
            ..DayPlan::default()
        };
        // Two hours later the near one is old; the other one is not.
        let later = start + TTL * 2;
        let jobs = cache.schedule(&plan, &prefetch, later);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].key, 1);
        // Four hours later both are.
        let jobs = cache.schedule(&plan, &prefetch, start + prefetch.region_ttl + TTL);
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn the_neighbor_regions_are_asked_again_even_less_often() {
        let mut cache = DayCache::default();
        let prefetch = Prefetch::default();
        let start = Instant::now();
        cache.entries.insert(3, Entry::ready(COUNT, start, TTL));
        let plan = DayPlan {
            neighbors: vec![3],
            ..DayPlan::default()
        };
        // Past the time of the region, still not the one of its neighbors.
        let early = cache.schedule(&plan, &prefetch, start + prefetch.region_ttl + TTL);
        assert!(early.is_empty());
        let late = cache.schedule(&plan, &prefetch, start + prefetch.neighbor_ttl);
        assert_eq!(late.len(), 1);
    }

    #[test]
    fn the_plan_comes_due_when_its_first_count_expires() {
        let mut cache = DayCache::default();
        let prefetch = Prefetch::default();
        let start = Instant::now();
        for system in [1, 2] {
            cache
                .entries
                .insert(system, Entry::ready(COUNT, start, TTL));
        }
        let plan = DayPlan {
            near: vec![1],
            region: vec![2],
            ..DayPlan::default()
        };
        assert_eq!(DayCache::default().next_due(&prefetch, start), None);
        cache.schedule(&plan, &prefetch, start);
        // The ones around the characters last until zKillboard says so.
        assert_eq!(cache.next_due(&prefetch, start), Some(TTL));
        assert_eq!(cache.next_due(&prefetch, start + TTL / 2), Some(TTL / 2));
        // That one is due by then (and in the queue): the region's is next.
        let second = Duration::from_secs(1);
        assert_eq!(
            cache.next_due(&prefetch, start + TTL + second),
            Some(prefetch.region_ttl - TTL - second)
        );
        assert_eq!(
            cache.next_due(&prefetch, start + prefetch.region_ttl + second),
            None
        );
    }

    #[test]
    fn a_tooltip_waits_a_moment_and_lapses_unless_it_asks_again() {
        let prefetch = Prefetch::default();
        let job = prefetch.tooltip(7);
        assert_eq!((job.key, job.payload), (7, 7));
        assert_eq!(job.priority, Priority::Interactive);
        assert_eq!(job.delay, prefetch.hover_delay);
        assert_eq!(job.lease, Some(prefetch.tooltip_lease));
        assert!(!prefetch.hover_delay.is_zero());
    }

    #[test]
    fn a_new_system_asks_until_its_request_is_on_the_way() {
        let mut cache = DayCache::default();
        let now = Instant::now();
        // Looked at again and again while the request waits (or lapsed).
        assert_eq!(cache.look(1, now), (DayLine::Pending, true));
        assert_eq!(cache.look(1, now), (DayLine::Pending, true));
        cache.in_flight.insert(1);
        assert_eq!(cache.look(1, now), (DayLine::Pending, false));
    }

    #[tokio::test]
    async fn the_tooltip_goes_before_the_plan_and_the_plan_by_priority() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Ok(COUNT));
        let plan = DayPlan {
            near: vec![1],
            region: vec![2],
            neighbors: vec![3, 4],
        };
        let jobs = cache
            .lock()
            .unwrap()
            .schedule(&plan, &Prefetch::default(), Instant::now());
        lane.replace_background(jobs);
        // A tooltip asks meanwhile.
        let ask = cache.lock().unwrap().look(9, Instant::now()).1;
        assert!(ask);
        lane.submit(Setup::prefetch().tooltip(9));
        let steps = Setup::drain(&cache, &lane, &api).await;
        assert_eq!(*api.order.lock().unwrap(), [9, 1, 2, 3, 4]);
        assert!(
            steps
                .iter()
                .all(|step| matches!(step, Fetched::Answered { .. }))
        );
        // Nothing else was asked: the rest of the universe waits for a tooltip.
        assert!(!cache.lock().unwrap().entries.contains_key(&5));
    }

    #[tokio::test]
    async fn a_tooltip_for_a_system_in_the_plan_is_asked_once() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Ok(COUNT));
        let plan = DayPlan {
            near: vec![1, 2],
            ..DayPlan::default()
        };
        let jobs = cache
            .lock()
            .unwrap()
            .schedule(&plan, &Prefetch::default(), Instant::now());
        lane.replace_background(jobs);
        lane.submit(Setup::prefetch().tooltip(2));
        Setup::drain(&cache, &lane, &api).await;
        assert_eq!(*api.order.lock().unwrap(), [2, 1]);
    }

    #[tokio::test]
    async fn a_new_plan_replaces_the_old_one_and_keeps_the_tooltips() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Ok(COUNT));
        lane.replace_background([Job::new(1, 1, Priority::High)]);
        lane.submit(Setup::prefetch().tooltip(9));
        lane.replace_background([Job::new(2, 2, Priority::High)]);
        Setup::drain(&cache, &lane, &api).await;
        assert_eq!(*api.order.lock().unwrap(), [9, 2]);
    }

    #[tokio::test]
    async fn failures_in_a_row_hold_the_lane_and_a_success_ends_it() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let prefetch = Setup::prefetch();
        lane.replace_background(
            (1..=10).map(|system| Job::new(system, system as usize, Priority::High)),
        );
        let down = FakeApi::new(Err(FeedError::Other(String::from("down"))));
        let mut holds = Vec::new();
        for _ in 0..9 {
            let step = DayCache::fetch_next(&cache, &lane, &down, &prefetch, || true).await;
            assert_eq!(step, Fetched::Failed);
            holds.push(cache.lock().unwrap().backoff(&prefetch));
        }
        let none = Duration::ZERO;
        let hold = Duration::from_millis(1);
        assert_eq!(
            holds,
            [none, none, none, none, hold, hold, hold, hold, hold]
        );
        // With the real pauses the steps grow, one for each failure.
        let slow = Prefetch::default();
        let steps: Vec<Duration> = (4..=9)
            .map(|failures| {
                DayCache {
                    failures,
                    ..DayCache::default()
                }
                .backoff(&slow)
            })
            .collect();
        let minute = Duration::from_secs(60);
        assert_eq!(
            steps,
            [
                none,
                minute,
                minute * 5,
                minute * 10,
                minute * 15,
                minute * 15
            ]
        );
        let up = FakeApi::new(Ok(COUNT));
        let step = DayCache::fetch_next(&cache, &lane, &up, &prefetch, || true).await;
        assert!(matches!(step, Fetched::Answered { .. }));
        assert_eq!(cache.lock().unwrap().failures(), 0);
    }

    #[tokio::test]
    async fn the_answer_is_kept_until_it_expires_and_then_asked_again() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let ten_minutes = Duration::from_secs(600);
        let api = FakeApi::fresh(Ok(COUNT), Some(ten_minutes));
        cache.lock().unwrap().look(1, Instant::now());
        lane.submit(Setup::prefetch().tooltip(1));
        Setup::drain(&cache, &lane, &api).await;
        let start = Instant::now();
        let (line, ask) = cache.lock().unwrap().look(1, start);
        assert_eq!((line, ask), (DayLine::Known(COUNT), false));
        // Past what zKillboard said the old count shows while a new one is
        // asked.
        let later = start + ten_minutes + Duration::from_secs(1);
        let (line, ask) = cache.lock().unwrap().look(1, later);
        assert_eq!((line, ask), (DayLine::Known(COUNT), true));
    }

    #[test]
    fn without_an_expires_the_answer_lasts_an_hour_and_never_less_than_the_floor() {
        let prefetch = Prefetch::default();
        let now = Instant::now();
        let mut cache = DayCache::default();
        let answer = |fresh_for| {
            Ok(DayAnswer {
                count: COUNT,
                kills: Vec::new(),
                fresh_for,
                data_time: Utc::now(),
            })
        };
        cache.record(1, &answer(None), &prefetch, now);
        cache.record(2, &answer(Some(Duration::ZERO)), &prefetch, now);
        cache.record(3, &answer(Some(TTL * 5)), &prefetch, now);
        assert!(cache.is_fresh(1, now + TTL - Duration::from_secs(1)));
        assert!(!cache.is_fresh(1, now + TTL));
        assert!(cache.is_fresh(2, now + prefetch.min_fresh - Duration::from_secs(1)));
        assert!(!cache.is_fresh(2, now + prefetch.min_fresh));
        assert!(!cache.is_fresh(3, now + TTL));
    }

    #[tokio::test]
    async fn a_fresh_answer_is_not_asked_again() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Ok(COUNT));
        cache
            .lock()
            .unwrap()
            .entries
            .insert(1, Entry::ready(COUNT, Instant::now(), TTL));
        lane.submit(Setup::prefetch().tooltip(1));
        let steps = Setup::drain(&cache, &lane, &api).await;
        assert_eq!(steps, [Fetched::Skipped]);
        assert_eq!(api.asked.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nothing_is_asked_once_the_source_is_off() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Ok(COUNT));
        cache.lock().unwrap().look(1, Instant::now());
        lane.submit(Setup::prefetch().tooltip(1));
        let step = DayCache::fetch_next(&cache, &lane, &api, &Setup::prefetch(), || false).await;
        assert_eq!(step, Fetched::Skipped);
        assert_eq!(api.asked.load(Ordering::SeqCst), 0);
        // The next tooltip starts from scratch.
        let (line, ask) = cache.lock().unwrap().look(1, Instant::now());
        assert_eq!((line, ask), (DayLine::Pending, true));
    }

    #[tokio::test]
    async fn a_failure_is_not_retried_at_once() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Err(FeedError::Other(String::from("down"))));
        cache.lock().unwrap().look(1, Instant::now());
        lane.submit(Setup::prefetch().tooltip(1));
        Setup::drain(&cache, &lane, &api).await;
        let (line, ask) = cache.lock().unwrap().look(1, Instant::now());
        assert_eq!((line, ask), (DayLine::Unavailable, false));
        let later = Instant::now() + RETRY_AFTER + Duration::from_secs(1);
        let (line, ask) = cache.lock().unwrap().look(1, later);
        assert_eq!((line, ask), (DayLine::Pending, true));
    }

    #[tokio::test]
    async fn a_refusal_makes_the_fetcher_stay_away() {
        let cache = Mutex::new(DayCache::default());
        let lane = Setup::lane();
        let api = FakeApi::new(Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS)));
        cache.lock().unwrap().look(1, Instant::now());
        lane.submit(Setup::prefetch().tooltip(1));
        let steps = Setup::drain(&cache, &lane, &api).await;
        assert_eq!(steps, [Fetched::Throttled]);
    }

    #[test]
    fn the_query_is_in_the_path_with_a_trailing_slash() {
        let api = ZKillApi::new().unwrap();
        assert_eq!(
            api.day_url(30002813),
            "https://zkillboard.com/api/solarSystemID/30002813/pastSeconds/86400/"
        );
    }

    #[test]
    fn expires_says_how_long_the_answer_stays_as_it_is() {
        let now = DateTime::parse_from_rfc2822("Mon, 02 Nov 2026 18:00:00 GMT")
            .unwrap()
            .with_timezone(&Utc);
        let mut headers = HeaderMap::new();
        assert_eq!(ZKillApi::fresh_for(&headers, now), None);
        headers.insert(
            reqwest::header::EXPIRES,
            "Mon, 02 Nov 2026 18:41:00 GMT".parse().unwrap(),
        );
        assert_eq!(
            ZKillApi::fresh_for(&headers, now),
            Some(Duration::from_secs(41 * 60))
        );
        headers.insert(
            reqwest::header::EXPIRES,
            "Mon, 02 Nov 2026 17:00:00 GMT".parse().unwrap(),
        );
        assert_eq!(ZKillApi::fresh_for(&headers, now), Some(Duration::ZERO));
        headers.insert(reqwest::header::EXPIRES, "-1".parse().unwrap());
        assert_eq!(ZKillApi::fresh_for(&headers, now), None);
    }

    #[test]
    fn the_api_lists_whole_killmails() {
        let items: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
              {"killmail_id": 11, "killmail_time": "2026-11-02T17:55:00Z",
               "solar_system_id": 30000142, "victim": {"ship_type_id": 587},
               "attackers": [{"character_id": 5, "ship_type_id": 17738}],
               "zkb": {"hash": "abc", "totalValue": 1500000.5, "solo": true}},
              {"killmail_id": 12, "nonsense": true}
            ]"#,
        )
        .unwrap();
        let (count, kills) = ZKillApi::parse(items);
        // The one that can't be read still counts, but is not listed.
        assert_eq!(
            count,
            DayCount {
                kills: 2,
                more: false
            }
        );
        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].killmail_id, 11);
        assert_eq!(kills[0].zkb.hash, "abc");
        assert!(
            kills[0]
                .raw
                .as_deref()
                .is_some_and(|raw| raw.contains("abc"))
        );
    }

    #[test]
    fn a_full_page_may_have_more() {
        let items = vec![serde_json::json!({"killmail_id": 0}); PAGE_SIZE];
        assert!(ZKillApi::parse(items).0.more);
    }

    #[test]
    fn the_counts_are_kept_and_taken_back_with_their_age_and_freshness() {
        let now = Instant::now();
        let mut cache = DayCache::default();
        let saved = SavedCount {
            system: 1,
            count: COUNT,
            age: TimeDelta::seconds(600),
            fresh_for: TimeDelta::seconds(1200),
        };
        cache.restore([saved], now);
        assert_eq!(
            cache.look(1, now).0,
            DayLine::Known(COUNT),
            "a count asked 10 minutes ago is still good"
        );
        assert_eq!(cache.snapshot(now), vec![saved]);
        // It stays good for as long as zKillboard said, not for another hour.
        let (_, ask) = cache.look(1, now + Duration::from_secs(1199));
        assert!(!ask);
        let (_, ask) = cache.look(1, now + Duration::from_secs(1201));
        assert!(ask);
        // What is already known is not replaced by what was kept.
        cache.restore(
            [SavedCount {
                count: DayCount {
                    kills: 99,
                    more: true,
                },
                age: TimeDelta::seconds(1),
                ..saved
            }],
            now,
        );
        assert_eq!(cache.look(1, now).0, DayLine::Known(COUNT));
        // A count that was already stale is asked again, showing the old one.
        let mut stale = DayCache::default();
        stale.restore(
            [SavedCount {
                system: 2,
                fresh_for: TimeDelta::zero(),
                ..saved
            }],
            now,
        );
        let (line, ask) = stale.look(2, now);
        assert_eq!(line, DayLine::Known(COUNT));
        assert!(ask, "it is asked again, showing the old count meanwhile");
    }

    impl Setup {
        /// A killmail of system 1 that happened `minutes_ago`.
        fn kill(id: u64, minutes_ago: i64) -> Killmail {
            let time = Utc::now() - TimeDelta::minutes(minutes_ago);
            Killmail {
                killmail_id: id,
                sequence_id: 0,
                esi: EsiKillmail {
                    killmail_time: time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    solar_system_id: 1,
                    victim: Victim { ship_type_id: None },
                    attackers: Vec::new(),
                },
                zkb: Zkb::default(),
                raw: None,
            }
        }

        /// An answer made of `kills`, from `data_minutes_ago`.
        fn answer(kills: Vec<Killmail>, data_minutes_ago: i64) -> Result<DayAnswer, FeedError> {
            Ok(DayAnswer {
                count: DayCount {
                    kills: kills.len(),
                    more: false,
                },
                kills,
                fresh_for: Some(Duration::from_secs(600)),
                data_time: Utc::now() - TimeDelta::minutes(data_minutes_ago),
            })
        }
    }

    #[test]
    fn while_the_feed_watches_the_count_follows_it_and_is_not_asked_again() {
        let prefetch = Prefetch::default();
        let now = Instant::now();
        let mut cache = DayCache::default();
        // The feed has been watching for half an hour.
        cache.feed_up(now.checked_sub(Duration::from_secs(1800)).unwrap_or(now));
        cache.record(
            1,
            &Setup::answer(vec![Setup::kill(1, 120), Setup::kill(2, 60)], 20),
            &prefetch,
            now,
        );
        assert_eq!(
            cache.look(1, now).0,
            DayLine::Known(DayCount {
                kills: 2,
                more: false
            })
        );
        // The feed announces another one; the API is not asked after the
        // hour zKillboard said, nor after several.
        let seen = Utc::now();
        cache.observe(1, 3, seen);
        cache.observe(1, 3, seen);
        let later = now + Duration::from_secs(3 * 60 * 60);
        let (line, ask) = cache.look(1, later);
        assert_eq!(
            line,
            DayLine::Known(DayCount {
                kills: 3,
                more: false
            })
        );
        assert!(!ask);
        assert!(cache.is_fresh(1, later));
        // After the longest the feed alone keeps it, the API is asked again.
        assert!(
            cache
                .look(1, now + prefetch.live_max_age + Duration::from_secs(1))
                .1
        );
    }

    #[test]
    fn a_gap_in_the_feed_ends_it_and_keeps_the_count() {
        let prefetch = Prefetch::default();
        let now = Instant::now();
        let mut cache = DayCache::default();
        cache.feed_up(now.checked_sub(Duration::from_secs(1800)).unwrap_or(now));
        cache.record(
            1,
            &Setup::answer(vec![Setup::kill(1, 120)], 20),
            &prefetch,
            now,
        );
        cache.observe(1, 2, Utc::now());
        cache.feed_down();
        // The two it saw stay on the line, and it is asked again once the
        // time zKillboard said has passed.
        let (line, ask) = cache.look(1, now);
        assert_eq!(
            line,
            DayLine::Known(DayCount {
                kills: 2,
                more: false
            })
        );
        assert!(!ask);
        let (_, ask) = cache.look(1, now + Duration::from_secs(601));
        assert!(ask);
    }

    #[test]
    fn an_answer_older_than_the_feed_is_not_kept_up_by_it() {
        let prefetch = Prefetch::default();
        let now = Instant::now();
        // The feed started 10 minutes ago, but the data is an hour old.
        let mut cache = DayCache::default();
        cache.feed_up(now.checked_sub(Duration::from_secs(600)).unwrap_or(now));
        cache.record(
            1,
            &Setup::answer(vec![Setup::kill(1, 90)], 60),
            &prefetch,
            now,
        );
        let (_, ask) = cache.look(1, now + Duration::from_secs(3 * 60 * 60));
        assert!(ask, "it expires as zKillboard said");
        // Nor when the feed was not watching at all, or the page was full.
        let mut cache = DayCache::default();
        cache.record(
            2,
            &Setup::answer(vec![Setup::kill(1, 90)], 20),
            &prefetch,
            now,
        );
        assert!(cache.look(2, now + Duration::from_secs(3 * 60 * 60)).1);
        let mut full = DayCache::default();
        full.feed_up(now.checked_sub(Duration::from_secs(1800)).unwrap_or(now));
        let mut page = Setup::answer(vec![Setup::kill(1, 90)], 20).unwrap();
        page.count.more = true;
        full.record(3, &Ok(page), &prefetch, now);
        assert!(full.look(3, now + Duration::from_secs(3 * 60 * 60)).1);
    }

    #[test]
    fn the_data_time_is_last_modified_or_now_minus_the_age() {
        let now = DateTime::parse_from_rfc2822("Mon, 02 Nov 2026 18:00:00 GMT")
            .unwrap()
            .with_timezone(&Utc);
        let mut headers = HeaderMap::new();
        assert_eq!(ZKillApi::data_time(&headers, now), now);
        headers.insert(reqwest::header::AGE, "120".parse().unwrap());
        assert_eq!(
            ZKillApi::data_time(&headers, now),
            now - TimeDelta::minutes(2)
        );
        headers.insert(
            reqwest::header::LAST_MODIFIED,
            "Mon, 02 Nov 2026 17:30:00 GMT".parse().unwrap(),
        );
        assert_eq!(
            ZKillApi::data_time(&headers, now),
            now - TimeDelta::minutes(30)
        );
    }
}
