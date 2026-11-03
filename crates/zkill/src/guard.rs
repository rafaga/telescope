//! Keeping away from zKillboard after it said "enough", across restarts.
//!
//! zKillboard blocks a client that goes over its limits (403 or 429) for an
//! hour. The pause the source takes lived only in memory, so quitting and
//! starting Telescope again made it ask at once, go over the limit again and
//! earn another hour. A [`RequestGuard`] remembers, in the encrypted player
//! database (not in a file next to the program, where a pause is easy to
//! delete):
//!
//! * until when zKillboard asked us to keep away, and
//! * when the last request was made, so that a restart right after the
//!   previous run does not add its first burst to the end of the last one.
//!
//! Every request to zKillboard (the R2Z2 feed and the query API) goes through
//! it: [`RequestGuard::wait`] says whether it may be made now, and an answer
//! of 403 or 429 is told to [`RequestGuard::throttled_for`], with the wait
//! zKillboard asked for in `Retry-After` when it said one. The limit is per
//! address (15 requests a second for R2Z2), not per source, so
//! [`RequestGuard::pace`] also keeps every request at least
//! [`GuardConfig::min_interval`] away from the previous one, whoever made it.
//! A guard without a store works the same, only forgetting at the end of the
//! run.

use chrono::{DateTime, TimeDelta, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where the guard keeps what it remembers (one short text).
pub trait GuardStore: Send + Sync {
    /// What was saved, if anything.
    fn load(&self) -> Option<String>;
    /// Replaces what was saved. Failing is not an error: the guard then
    /// remembers only until the end of the run.
    fn save(&self, value: &str);
}

/// How the guard behaves. The defaults are what Telescope uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardConfig {
    /// How long to keep away after zKillboard refused a request (403 or 429).
    pub block: TimeDelta,
    /// How long after the last request of the previous run the first one of
    /// this run waits.
    pub restart_gap: TimeDelta,
    /// The time of the last request is saved at most this often (the pause
    /// after a refusal is saved at once).
    pub save_every: TimeDelta,
    /// The least time between two requests of any source. 125 ms is 8 a
    /// second, well under the 15 a second that earns a ban.
    pub min_interval: TimeDelta,
    /// Added to the wait zKillboard asks for in `Retry-After`.
    pub retry_margin: TimeDelta,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            block: TimeDelta::hours(1),
            restart_gap: TimeDelta::seconds(30),
            save_every: TimeDelta::seconds(5),
            min_interval: TimeDelta::milliseconds(125),
            retry_margin: TimeDelta::seconds(30),
        }
    }
}

/// What the guard knows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Memory {
    /// zKillboard asked us to keep away until then.
    blocked_until: Option<DateTime<Utc>>,
    /// The last request of this run or the previous one.
    last_request: Option<DateTime<Utc>>,
    /// The first request of this run waits until then.
    ready_at: Option<DateTime<Utc>>,
    /// When the memory was last saved.
    saved_at: Option<DateTime<Utc>>,
    /// The earliest moment the next request may go out (see
    /// [`RequestGuard::pace`]). Only this run's: it is not saved.
    next_slot: Option<DateTime<Utc>>,
    /// When a request was last answered well. Only this run's.
    last_ok: Option<DateTime<Utc>>,
    /// The end of the last refusal that was reported (see
    /// [`RequestGuard::unreported_refusal`]). Only this run's.
    reported: Option<DateTime<Utc>>,
}

/// Decides whether a request to zKillboard may be made, and remembers when
/// it was refused.
pub struct RequestGuard {
    config: GuardConfig,
    store: Mutex<Option<Box<dyn GuardStore>>>,
    memory: Mutex<Memory>,
    /// Requests let through in this run (for the log of a refusal).
    sent: AtomicU64,
}

/// What a saved text holds: until when zKillboard asked us to keep away, and
/// the last request.
type Remembered = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

impl RequestGuard {
    /// What is saved is written as `1:<blocked until>:<last request>`, in
    /// seconds since 1970 (0 for "never").
    const FORMAT: &'static str = "1";

    /// A guard that remembers nothing between runs until a store is
    /// attached.
    pub fn new(config: GuardConfig) -> Self {
        Self {
            config,
            store: Mutex::new(None),
            memory: Mutex::new(Memory::default()),
            sent: AtomicU64::new(0),
        }
    }

    /// How many requests this run has let through so far.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// A request was answered well (a file that does not exist counts: the
    /// server answered).
    pub fn succeeded(&self, now: DateTime<Utc>) {
        self.lock_memory().last_ok = Some(now);
    }

    /// How long ago a request was last answered well in this run.
    pub fn last_success(&self, now: DateTime<Utc>) -> Option<Duration> {
        let last = self.lock_memory().last_ok?;
        (now - last).to_std().ok()
    }

    /// The end of the refusal in force, once: `None` when there is none or it
    /// was already returned. It lets a refusal be reported a single time.
    pub fn unreported_refusal(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut memory = self.lock_memory();
        let until = memory.blocked_until.filter(|until| *until > now)?;
        if memory.reported == Some(until) {
            return None;
        }
        memory.reported = Some(until);
        Some(until)
    }

    /// How long to keep away after a refusal.
    pub fn block(&self) -> Duration {
        self.config.block.to_std().unwrap_or_default()
    }

    /// Starts using `store`: what it has saved counts from now on, and what
    /// happens next is saved in it.
    pub fn attach(&self, store: Box<dyn GuardStore>, now: DateTime<Utc>) {
        let saved = store.load().and_then(|text| Self::parse(&text));
        {
            let mut memory = self.lock_memory();
            if let Some((blocked_until, last_request)) = saved {
                memory.blocked_until = memory.blocked_until.max(blocked_until);
                memory.last_request = memory.last_request.max(last_request);
                memory.ready_at = last_request.map(|last| last + self.config.restart_gap);
            }
            memory.saved_at = Some(now);
        }
        *self
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(store);
    }

    /// How long to wait before a request may be made, or `None` if it may
    /// be made now.
    pub fn wait(&self, now: DateTime<Utc>) -> Option<Duration> {
        let memory = self.lock_memory();
        let until = memory.blocked_until.max(memory.ready_at)?;
        (until - now).to_std().ok().filter(|wait| !wait.is_zero())
    }

    /// Whether the wait comes from a refusal of zKillboard (403 or 429) that
    /// is still remembered, and not only from the pause after a restart.
    pub fn refused(&self, now: DateTime<Utc>) -> bool {
        self.lock_memory()
            .blocked_until
            .is_some_and(|until| until > now)
    }

    /// Takes the next place in line for a request and says how long to sleep
    /// before sending it, so that two requests are never closer than
    /// [`GuardConfig::min_interval`], whichever source makes them.
    pub fn pace(&self, now: DateTime<Utc>) -> Duration {
        let mut memory = self.lock_memory();
        let slot = memory.next_slot.map_or(now, |next| next.max(now));
        memory.next_slot = Some(slot + self.config.min_interval);
        (slot - now).to_std().unwrap_or_default()
    }

    /// The wait zKillboard asked for in `Retry-After`, when it is a number of
    /// seconds (a date is not read).
    pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
        headers
            .get(RETRY_AFTER)?
            .to_str()
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
    }

    /// Until when zKillboard asked us to keep away, if it still does.
    pub fn blocked_until(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.lock_memory()
            .blocked_until
            .filter(|until| *until > now)
    }

    /// A request is about to be made.
    pub fn requested(&self, now: DateTime<Utc>) {
        self.sent.fetch_add(1, Ordering::Relaxed);
        let mut memory = self.lock_memory();
        memory.last_request = Some(now);
        let due = memory
            .saved_at
            .is_none_or(|saved| now - saved >= self.config.save_every);
        if due {
            self.save(&mut memory, now);
        }
    }

    /// zKillboard answered 403 or 429: keep away for as long as it asked
    /// (`retry_after`, plus the margin; never more than four blocks) or, when
    /// it did not say, for the whole block.
    pub fn throttled_for(&self, now: DateTime<Utc>, retry_after: Option<Duration>) {
        let asked = retry_after
            .and_then(|wait| TimeDelta::from_std(wait).ok())
            .map(|wait| (wait + self.config.retry_margin).min(self.config.block * 4));
        let until = now + asked.unwrap_or(self.config.block);
        let mut memory = self.lock_memory();
        memory.last_request = Some(now);
        memory.blocked_until = memory.blocked_until.max(Some(until));
        self.save(&mut memory, now);
    }

    fn lock_memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, memory: &mut Memory, now: DateTime<Utc>) {
        memory.saved_at = Some(now);
        let seconds = |time: Option<DateTime<Utc>>| time.map_or(0, |time| time.timestamp());
        let text = format!(
            "{}:{}:{}",
            Self::FORMAT,
            seconds(memory.blocked_until),
            seconds(memory.last_request)
        );
        if let Some(store) = self
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            store.save(&text);
        }
    }

    /// The times in a saved text, `None` if it is not one of ours.
    fn parse(text: &str) -> Option<Remembered> {
        let mut parts = text.split(':');
        if parts.next()? != Self::FORMAT {
            return None;
        }
        let mut time = || -> Option<Option<DateTime<Utc>>> {
            let seconds: i64 = parts.next()?.parse().ok()?;
            Some(
                (seconds > 0)
                    .then(|| DateTime::from_timestamp(seconds, 0))
                    .flatten(),
            )
        };
        let blocked_until = time()?;
        let last_request = time()?;
        Some((blocked_until, last_request))
    }
}

/// The two places Telescope asks zKillboard for things. Each has its own
/// limits and its own bans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// The R2Z2 feed of killmails.
    R2z2,
    /// The query API (the killmails of a system).
    Api,
}

impl Endpoint {
    /// The other one.
    pub fn other(self) -> Self {
        match self {
            Self::R2z2 => Self::Api,
            Self::Api => Self::R2z2,
        }
    }

    /// Its name, for the log.
    pub fn label(self) -> &'static str {
        match self {
            Self::R2z2 => "R2Z2",
            Self::Api => "API",
        }
    }
}

/// One [`RequestGuard`] for each [`Endpoint`]: a refusal of one does not stop
/// the other, unless `cross_ban` says the ban is by address and not by
/// endpoint.
pub struct Guards {
    r2z2: Arc<RequestGuard>,
    api: Arc<RequestGuard>,
    cross_ban: bool,
}

impl Guards {
    pub fn new(r2z2: GuardConfig, api: GuardConfig, cross_ban: bool) -> Self {
        Self {
            r2z2: Arc::new(RequestGuard::new(r2z2)),
            api: Arc::new(RequestGuard::new(api)),
            cross_ban,
        }
    }

    /// The guard of `endpoint`.
    pub fn get(&self, endpoint: Endpoint) -> &Arc<RequestGuard> {
        match endpoint {
            Endpoint::R2z2 => &self.r2z2,
            Endpoint::Api => &self.api,
        }
    }

    /// Whether a refusal of one endpoint also stops the other.
    pub fn cross_ban(&self) -> bool {
        self.cross_ban
    }

    /// `endpoint` was found refused. The first time for each refusal it is
    /// logged with how the other endpoint is doing, which is what tells
    /// whether bans are by endpoint or by address; with `cross_ban` the other
    /// is kept away for as long as this one.
    pub fn refusal_seen(&self, endpoint: Endpoint, now: DateTime<Utc>) {
        let guard = self.get(endpoint);
        let Some(until) = guard.unreported_refusal(now) else {
            return;
        };
        let other = self.get(endpoint.other());
        tracing::warn!(
            endpoint = endpoint.label(),
            %until,
            other_refused = other.refused(now),
            other_last_ok_secs = ?other.last_success(now).map(|age| age.as_secs()),
            cross_ban = self.cross_ban,
            "zKillboard refused an endpoint"
        );
        if self.cross_ban
            && let Some(wait) = guard.wait(now)
            && other.wait(now).is_none_or(|left| left < wait)
        {
            other.throttled_for(now, Some(wait));
        }
    }
}

impl Default for Guards {
    /// A ban by endpoint, as it seems to be (see [`Guards::refusal_seen`]).
    fn default() -> Self {
        Self::new(GuardConfig::default(), GuardConfig::default(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A store in memory the test can look into.
    #[derive(Clone, Default)]
    struct Memo(Arc<Mutex<Option<String>>>);

    impl GuardStore for Memo {
        fn load(&self) -> Option<String> {
            self.0.lock().unwrap().clone()
        }

        fn save(&self, value: &str) {
            *self.0.lock().unwrap() = Some(value.to_string());
        }
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn a_new_guard_lets_everything_through() {
        let guard = RequestGuard::new(GuardConfig::default());
        assert_eq!(guard.wait(at(0)), None);
        assert_eq!(guard.blocked_until(at(0)), None);
    }

    #[test]
    fn a_refusal_keeps_the_source_away_for_the_block() {
        let guard = RequestGuard::new(GuardConfig::default());
        guard.throttled_for(at(0), None);
        assert_eq!(guard.wait(at(60)), Some(Duration::from_secs(3540)));
        assert_eq!(guard.blocked_until(at(60)), Some(at(3600)));
        assert_eq!(guard.wait(at(3600)), None);
        assert_eq!(guard.blocked_until(at(3601)), None);
    }

    #[test]
    fn the_block_survives_a_restart() {
        let store = Memo::default();
        let first = RequestGuard::new(GuardConfig::default());
        first.attach(Box::new(store.clone()), at(0));
        first.throttled_for(at(10), None);
        // The program is closed and opened again twenty minutes later.
        let second = RequestGuard::new(GuardConfig::default());
        second.attach(Box::new(store), at(1210));
        assert_eq!(second.wait(at(1210)), Some(Duration::from_secs(2400)));
        assert_eq!(second.wait(at(3610)), None);
    }

    #[test]
    fn the_first_request_after_a_restart_waits_a_little() {
        let store = Memo::default();
        let first = RequestGuard::new(GuardConfig::default());
        first.attach(Box::new(store.clone()), at(0));
        first.requested(at(100));
        // Saved at most every few seconds: this one was.
        let second = RequestGuard::new(GuardConfig::default());
        second.attach(Box::new(store), at(105));
        assert_eq!(second.wait(at(105)), Some(Duration::from_secs(25)));
        assert_eq!(second.wait(at(130)), None);
    }

    #[test]
    fn the_time_of_the_last_request_is_not_saved_after_each_one() {
        let store = Memo::default();
        let guard = RequestGuard::new(GuardConfig::default());
        guard.attach(Box::new(store.clone()), at(0));
        guard.requested(at(10));
        let saved = store.load();
        assert!(saved.is_some());
        guard.requested(at(11));
        guard.requested(at(12));
        assert_eq!(store.load(), saved);
        guard.requested(at(16));
        assert_ne!(store.load(), saved);
    }

    #[test]
    fn requests_are_never_closer_than_the_minimum_interval() {
        let guard = RequestGuard::new(GuardConfig::default());
        assert_eq!(guard.pace(at(0)), Duration::ZERO);
        assert_eq!(guard.pace(at(0)), Duration::from_millis(125));
        assert_eq!(guard.pace(at(0)), Duration::from_millis(250));
        assert_eq!(guard.pace(at(1)), Duration::ZERO);
    }

    #[test]
    fn the_block_lasts_as_long_as_zkillboard_asked() {
        let asked = RequestGuard::new(GuardConfig::default());
        asked.throttled_for(at(0), Some(Duration::from_secs(3306)));
        assert_eq!(asked.blocked_until(at(1)), Some(at(3336)));
        let silent = RequestGuard::new(GuardConfig::default());
        silent.throttled_for(at(0), None);
        assert_eq!(silent.blocked_until(at(1)), Some(at(3600)));
        let huge = RequestGuard::new(GuardConfig::default());
        huge.throttled_for(at(0), Some(Duration::from_secs(1_000_000)));
        assert_eq!(huge.blocked_until(at(1)), Some(at(4 * 3600)));
    }

    #[test]
    fn retry_after_is_read_when_it_is_a_number_of_seconds() {
        let mut headers = HeaderMap::new();
        assert_eq!(RequestGuard::retry_after(&headers), None);
        headers.insert(RETRY_AFTER, "3040".parse().unwrap());
        assert_eq!(
            RequestGuard::retry_after(&headers),
            Some(Duration::from_secs(3040))
        );
        headers.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(RequestGuard::retry_after(&headers), None);
    }

    #[test]
    fn only_a_remembered_refusal_counts_as_refused() {
        let store = Memo::default();
        let first = RequestGuard::new(GuardConfig::default());
        first.attach(Box::new(store.clone()), at(0));
        first.requested(at(100));
        // A restart right after: it waits, but nobody refused anything.
        let second = RequestGuard::new(GuardConfig::default());
        second.attach(Box::new(store), at(105));
        assert!(second.wait(at(105)).is_some());
        assert!(!second.refused(at(105)));
        second.throttled_for(at(106), None);
        assert!(second.refused(at(107)));
    }

    #[test]
    fn something_that_is_not_ours_is_ignored() {
        for text in ["", "banned", "2:1:1", "1:x:1", "1:5"] {
            let store = Memo(Arc::new(Mutex::new(Some(text.to_string()))));
            let guard = RequestGuard::new(GuardConfig::default());
            guard.attach(Box::new(store), at(0));
            assert_eq!(guard.wait(at(0)), None, "{text:?}");
        }
    }

    #[test]
    fn a_refusal_of_one_endpoint_leaves_the_other_alone() {
        let guards = Guards::default();
        guards
            .get(Endpoint::R2z2)
            .throttled_for(at(0), Some(Duration::from_secs(600)));
        guards.refusal_seen(Endpoint::R2z2, at(1));
        assert!(guards.get(Endpoint::R2z2).wait(at(1)).is_some());
        assert_eq!(guards.get(Endpoint::Api).wait(at(1)), None);
    }

    #[test]
    fn with_cross_ban_the_other_endpoint_is_kept_away_too() {
        let guards = Guards::new(GuardConfig::default(), GuardConfig::default(), true);
        guards
            .get(Endpoint::Api)
            .throttled_for(at(0), Some(Duration::from_secs(600)));
        guards.refusal_seen(Endpoint::Api, at(1));
        assert!(guards.get(Endpoint::R2z2).wait(at(1)).is_some());
        // Seeing the same refusal again does not stretch the other's wait.
        let before = guards.get(Endpoint::R2z2).wait(at(2));
        guards.refusal_seen(Endpoint::Api, at(2));
        assert_eq!(guards.get(Endpoint::R2z2).wait(at(2)), before);
    }

    #[test]
    fn a_refusal_is_reported_once() {
        let guard = RequestGuard::new(GuardConfig::default());
        assert_eq!(guard.unreported_refusal(at(0)), None);
        guard.throttled_for(at(0), None);
        assert_eq!(guard.unreported_refusal(at(1)), Some(at(3600)));
        assert_eq!(guard.unreported_refusal(at(2)), None);
        // A new, longer refusal is reported again.
        guard.throttled_for(at(100), Some(Duration::from_secs(7200)));
        assert!(guard.unreported_refusal(at(101)).is_some());
    }

    #[test]
    fn the_last_good_answer_is_remembered() {
        let guard = RequestGuard::new(GuardConfig::default());
        assert_eq!(guard.last_success(at(10)), None);
        guard.succeeded(at(4));
        assert_eq!(guard.last_success(at(10)), Some(Duration::from_secs(6)));
    }
}
