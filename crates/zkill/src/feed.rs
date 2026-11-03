//! The R2Z2 feed of zKillboard: the client, and the walk through the files it
//! publishes.
//!
//! zKillboard publishes each killmail as a numbered file
//! (`<sequence>.json`) and the number of the latest one in `sequence.json`.
//! [`R2z2Client`] reads those files, through the lane and the guard of the
//! R2Z2 endpoint. [`FeedCursor`] walks them in order: what was published
//! before the walk started is history, what comes after is announced once.
//! [`Backfill`] reads the files before the start, a chunk per round.

use crate::guard::{GuardConfig, RequestGuard};
use crate::lane::{Job, LaneConfig, Priority};
use crate::queue::LaneQueue;
use crate::types::{FeedError, Killmail};
use chrono::Utc;
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How the feed is read. The defaults are what zKillboard's documentation
/// asks for.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// Where the R2Z2 files are served from.
    pub url: String,
    /// The `User-Agent` of every request. zKillboard's CDN blocks a blank
    /// one; this one says who is asking.
    pub user_agent: String,
    /// How long a whole request may take before it is given up as failed.
    pub request_timeout: Duration,
    /// Wait after reaching the end of the feed: killmails average one every
    /// 5.5 seconds, so asking sooner buys nothing, and 6 seconds is what the
    /// feed's documentation asks for after a 404.
    pub poll_wait: Duration,
    /// Longest wait between two attempts while the feed can't be reached.
    pub max_backoff: Duration,
    /// How long the feed may stay unreachable before the walk starts over at
    /// the end of the feed: what was published meanwhile is too old to be
    /// intel.
    pub resync_after: Duration,
    /// How many files of the feed hold about an hour of killmails (one every
    /// five and a half seconds on average): the ones before the walk starts
    /// are read to fill the buffer the node tooltips set the reports against.
    pub backfill_files: u64,
    /// Files of that hour read per round, so the live walk is not kept
    /// waiting.
    pub backfill_chunk: u64,
    /// How many killmail ids are remembered to announce a killmail only once
    /// (zKillboard may publish a reprocessed killmail again under a new
    /// sequence number).
    pub recent_ids: usize,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            url: String::from("https://r2z2.zkillboard.com/ephemeral"),
            user_agent: String::from(concat!(
                "telescope/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/rafaga/telescope)"
            )),
            request_timeout: Duration::from_secs(15),
            poll_wait: Duration::from_secs(6),
            max_backoff: Duration::from_secs(60),
            resync_after: Duration::from_secs(2 * 60),
            backfill_files: 700,
            backfill_chunk: 20,
            recent_ids: 4096,
        }
    }
}

/// How far a [`Backfill`] got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Files read.
    pub done: u64,
    /// Files to read.
    pub total: u64,
}

/// The two reads the walk needs from the feed.
pub trait Feed {
    /// The latest sequence number published (`sequence.json`).
    fn pointer(&self) -> impl Future<Output = Result<u64, FeedError>> + Send;
    /// The killmail with this sequence number; `None` when there isn't one
    /// (yet).
    fn entry(
        &self,
        sequence: u64,
    ) -> impl Future<Output = Result<Option<Killmail>, FeedError>> + Send;
}

/// Where the requests to the feed wait for their turn: the R2Z2 lane, which
/// is the only thing that spaces them.
#[derive(Clone)]
pub struct FeedSlots {
    lane: Arc<LaneQueue<()>>,
    /// How much the requests made through these slots matter.
    priority: Priority,
    /// Tells the requests apart (the lane merges the ones with the same key).
    next_key: Arc<AtomicU64>,
}

impl FeedSlots {
    /// Slots in `lane`, for the requests somebody is waiting for.
    pub fn new(lane: Arc<LaneQueue<()>>) -> Self {
        Self {
            lane,
            priority: Priority::Interactive,
            next_key: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The same lane for requests of `priority`.
    pub fn with_priority(&self, priority: Priority) -> Self {
        Self {
            priority,
            ..self.clone()
        }
    }

    /// Waits for the turn of a request.
    pub async fn wait(&self) {
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
pub struct R2z2Client {
    client: Client,
    base: String,
    /// Keeps the requests away from zKillboard while it asked for it.
    guard: Arc<RequestGuard>,
    /// Where each request waits for its turn.
    slots: FeedSlots,
}

impl R2z2Client {
    pub fn new(config: &FeedConfig) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(config.user_agent.as_str())
            .timeout(config.request_timeout)
            .build()?;
        Ok(Self {
            client,
            base: config.url.clone(),
            guard: Arc::new(RequestGuard::new(GuardConfig::default())),
            slots: FeedSlots::default(),
        })
    }

    /// The same client, taking its turns in `slots`.
    pub fn with_slots(mut self, slots: FeedSlots) -> Self {
        self.slots = slots;
        self
    }

    /// The same client for requests of `priority`: the live walk is
    /// interactive, the hour read back when the feed starts is not.
    pub fn with_priority(&self, priority: Priority) -> Self {
        Self {
            client: self.client.clone(),
            base: self.base.clone(),
            guard: Arc::clone(&self.guard),
            slots: self.slots.with_priority(priority),
        }
    }

    /// The same client, asking only when `guard` lets it (the one every
    /// request to zKillboard shares).
    pub fn with_guard(mut self, guard: Arc<RequestGuard>) -> Self {
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
            .map_err(|error| {
                self.guard.failed(Utc::now());
                FeedError::Other(error.to_string())
            })?;
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
            status => {
                self.guard.failed(Utc::now());
                Err(FeedError::Other(format!("the feed answered {status}")))
            }
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
struct RecentIds {
    order: VecDeque<u64>,
    known: HashSet<u64>,
    /// How many ids are remembered.
    capacity: usize,
}

impl RecentIds {
    fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            known: HashSet::new(),
            capacity,
        }
    }

    /// Remembers `id`; `false` if it was already remembered.
    fn insert(&mut self, id: u64) -> bool {
        if !self.known.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.known.remove(&oldest);
        }
        true
    }
}

/// How a round of [`FeedCursor::advance`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Everything published so far was read; ask again after a while.
    Waiting,
    /// The feed could not be read; the same position is tried again later.
    Failed(FeedError),
    /// Whoever listens to the killmails is gone: stop.
    Closed,
}

/// Where the walk through the feed is.
pub struct FeedCursor {
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
    pub fn new(pace: Duration, config: &FeedConfig) -> Self {
        Self {
            next: None,
            caught_up: false,
            recent: RecentIds::new(config.recent_ids),
            pace,
            latest: None,
            holes: 0,
            origin: None,
            history: Vec::new(),
        }
    }

    /// The pointer the walk started at, once known.
    pub fn origin(&self) -> Option<u64> {
        self.origin
    }

    /// The killmails read on the way to the end of the feed since the last
    /// call: not announced, but the context of the tooltips wants them.
    pub fn take_history(&mut self) -> Vec<Killmail> {
        std::mem::take(&mut self.history)
    }

    /// The next sequence number to read, once known.
    pub fn next(&self) -> Option<u64> {
        self.next
    }

    /// The latest number `sequence.json` has given.
    pub fn latest(&self) -> Option<u64> {
        self.latest
    }

    /// Whether the end of the feed was reached since the walk (re)started.
    pub fn is_caught_up(&self) -> bool {
        self.caught_up
    }

    /// Holes in the series skipped so far.
    pub fn holes(&self) -> u64 {
        self.holes
    }

    /// Starts over at the end of the feed the next time: what was published
    /// meanwhile is not announced. The ids already announced stay known.
    pub fn resync(&mut self) {
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
    pub async fn advance<F: Feed>(
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
pub struct Backfill {
    /// The next sequence number to read.
    next: u64,
    /// Where it stops: the pointer the walk started at.
    end: u64,
    /// Where it started, to tell how far it got.
    first: u64,
    /// Pause between two files.
    pace: Duration,
    /// Files read per round.
    chunk: u64,
}

impl Backfill {
    /// The files before `origin`, or `None` when there are none. With
    /// `resume`, the sequence number a previous run had got to, only the
    /// files published since then are read (the store has the rest), as long
    /// as that is less than the hour the backfill covers anyway.
    pub fn new(origin: u64, resume: Option<u64>, config: &FeedConfig) -> Option<Self> {
        let hour = origin.saturating_sub(config.backfill_files).max(1);
        let first = resume.map_or(hour, |resume| resume.clamp(hour, origin));
        (first < origin).then_some(Self {
            next: first,
            end: origin,
            first,
            pace: Duration::ZERO,
            chunk: config.backfill_chunk,
        })
    }

    /// Reads the next chunk and hands each killmail to `sink`. `true` when
    /// every file was read. A failed read leaves the position where it was,
    /// to be tried again in the next round.
    pub async fn step<F: Feed>(&mut self, feed: &F, sink: &mut impl FnMut(&Killmail)) -> bool {
        for _ in 0..self.chunk {
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
    pub fn progress(&self) -> Progress {
        Progress {
            done: self.next - self.first,
            total: self.end - self.first,
        }
    }
}

/// Waits between two attempts while the feed can't be reached: 6 seconds,
/// doubling up to a minute.
pub struct Backoff {
    delay: Duration,
    /// Where the wait starts, and returns to.
    start: Duration,
    /// The longest wait.
    max: Duration,
}

impl Backoff {
    pub fn new(config: &FeedConfig) -> Self {
        Self {
            delay: config.poll_wait,
            start: config.poll_wait,
            max: config.max_backoff,
        }
    }

    /// The wait before the next attempt, longer than the last one.
    pub fn next_wait(&mut self) -> Duration {
        let delay = self.delay;
        self.delay = (self.delay * 2).min(self.max);
        delay
    }

    pub fn reset(&mut self) {
        self.delay = self.start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EsiKillmail, Victim, Zkb};
    use chrono::TimeDelta;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::Instant;

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

    /// Builders for the tests.
    struct Fixture;

    impl Fixture {
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
    fn recent_ids_forget_the_oldest_first() {
        let capacity = FeedConfig::default().recent_ids;
        let mut recent = RecentIds::new(capacity);
        assert!(recent.insert(1));
        assert!(!recent.insert(1));
        for id in 2..=capacity as u64 + 1 {
            assert!(recent.insert(id));
        }
        // 1 fell out of the window, the newest ones are still known.
        assert!(recent.insert(1));
        assert!(!recent.insert(capacity as u64 + 1));
    }

    #[test]
    fn the_backoff_doubles_up_to_a_minute_and_resets() {
        let mut backoff = Backoff::new(&FeedConfig::default());
        let waits: Vec<u64> = (0..6).map(|_| backoff.next_wait().as_secs()).collect();
        assert_eq!(waits, [6, 12, 24, 48, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next_wait().as_secs(), 6);
    }

    #[tokio::test]
    async fn a_new_walk_starts_at_the_end_and_announces_only_what_comes_after() {
        let feed = FakeFeed::with(100, 130);
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());

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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
        Fixture::announced(&mut cursor, &feed).await;
        feed.publish(Fixture::kill(101, 101));
        let step = cursor.advance(&feed, &mut |_| false).await;
        assert_eq!(step, Step::Closed);
    }

    #[tokio::test]
    async fn the_killmails_read_on_the_way_to_the_end_are_kept_as_history() {
        let feed = FakeFeed::with(100, 103);
        let mut cursor = FeedCursor::new(Duration::ZERO, &FeedConfig::default());
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
        let files = Backfill::new(1000, None, &FeedConfig::default()).unwrap();
        assert_eq!(
            (files.first, files.end),
            (1000 - FeedConfig::default().backfill_files, 1000)
        );
        // A feed that has not got an hour of files yet.
        let files = Backfill::new(5, None, &FeedConfig::default()).unwrap();
        assert_eq!((files.first, files.end), (1, 5));
        assert!(Backfill::new(1, None, &FeedConfig::default()).is_none());
        // A previous run got to 900: the files before it are in the store.
        let files = Backfill::new(1000, Some(900), &FeedConfig::default()).unwrap();
        assert_eq!((files.first, files.end), (900, 1000));
        // Too long ago to help, or nothing published since.
        let files = Backfill::new(1000, Some(10), &FeedConfig::default()).unwrap();
        assert_eq!(files.first, 1000 - FeedConfig::default().backfill_files);
        assert!(Backfill::new(1000, Some(1000), &FeedConfig::default()).is_none());
        assert!(Backfill::new(1000, Some(5000), &FeedConfig::default()).is_none());
    }

    #[tokio::test]
    async fn the_backfill_reads_in_chunks_and_retries_after_a_failure() {
        let feed = FakeFeed::with(1, 100);
        let mut files = Backfill {
            next: 1,
            end: 101,
            first: 1,
            pace: Duration::ZERO,
            chunk: FeedConfig::default().backfill_chunk,
        };
        let mut read = Vec::new();
        assert!(
            !files
                .step(&feed, &mut |killmail| read.push(killmail.sequence_id))
                .await
        );
        assert_eq!(read.len() as u64, FeedConfig::default().backfill_chunk);
        assert_eq!(
            files.progress(),
            Progress {
                done: FeedConfig::default().backfill_chunk,
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
        assert_eq!(files.progress().done, FeedConfig::default().backfill_chunk);

        feed.fail_with(None);
        while !files
            .step(&feed, &mut |killmail| read.push(killmail.sequence_id))
            .await
        {}
        assert_eq!(read, (1..=100).collect::<Vec<u64>>());
        assert_eq!(
            files.progress(),
            Progress {
                done: 100,
                total: 100
            }
        );
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
                    .user_agent(FeedConfig::default().user_agent)
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
