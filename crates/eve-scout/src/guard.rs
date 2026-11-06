//! What keeps the client from asking more than the API needs.
//!
//! The API is free, says how long each answer stays valid (`Cache-Control:
//! max-age`) and gave a 502 or a 503 more often than not on some endpoints.
//! The [`Guard`] makes the client behave:
//!
//! * **It does not ask again for what is still valid.** The answer is held
//!   until its `max-age` runs out, and asking for the same thing in the
//!   meantime gets that copy, with no request.
//! * **It keeps away after a bad answer.** After a `429` or a `5xx` (or a
//!   request that could not be made) the endpoint is left alone for a while:
//!   what `Retry-After` says, or a wait that doubles with every failure in a
//!   row, up to a limit. While it lasts nothing is asked of that endpoint.
//! * **It asks for one thing at a time.** Two askers of the same thing at
//!   the same time share one request: the second waits and takes the answer
//!   the first got.
//!
//! The guard has no clock: the caller passes the time in, so everything here
//! can be tested to the millisecond. It forgets everything when the program
//! ends: the API does not document limits or blocks, so there is nothing to
//! remember across runs.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How the guard behaves. The defaults are what Telescope uses.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// The wait after the first bad answer of an endpoint, when the API does
    /// not say how long (it doubles with every one after it).
    pub backoff_base: Duration,
    /// The longest wait the guard picks by itself. What `Retry-After` says is
    /// followed as it is.
    pub backoff_max: Duration,
    /// How many answers are held at most. When there are more, the oldest
    /// go: the requests can be about any system, so the set has no end.
    pub max_entries: usize,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            backoff_base: Duration::from_secs(15),
            backoff_max: Duration::from_secs(10 * 60),
            max_entries: 256,
        }
    }
}

/// An answer that is still valid.
#[derive(Debug, Clone)]
pub(crate) struct Fresh {
    pub(crate) body: Arc<Vec<u8>>,
    /// How long it stays valid from now.
    pub(crate) remaining: Duration,
    pub(crate) last_hub_interaction: Option<DateTime<Utc>>,
}

struct Held {
    body: Arc<Vec<u8>>,
    fetched: Instant,
    max_age: Duration,
    last_hub_interaction: Option<DateTime<Utc>>,
}

struct Pause {
    until: Instant,
    failures: u32,
}

#[derive(Default)]
struct State {
    held: HashMap<String, Held>,
    pauses: HashMap<String, Pause>,
}

type Lane = Arc<tokio::sync::Mutex<()>>;

pub(crate) struct Guard {
    config: GuardConfig,
    state: Mutex<State>,
    lanes: Mutex<HashMap<String, Lane>>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Guard {
    pub(crate) fn new(config: GuardConfig) -> Self {
        Self {
            config,
            state: Mutex::new(State::default()),
            lanes: Mutex::new(HashMap::new()),
        }
    }

    /// The held answer to `key`, if it is still valid at `now`.
    pub(crate) fn fresh(&self, key: &str, now: Instant) -> Option<Fresh> {
        let state = lock(&self.state);
        let held = state.held.get(key)?;
        let age = now.saturating_duration_since(held.fetched);
        let remaining = held
            .max_age
            .checked_sub(age)
            .filter(|left| !left.is_zero())?;
        Some(Fresh {
            body: Arc::clone(&held.body),
            remaining,
            last_hub_interaction: held.last_hub_interaction,
        })
    }

    /// Holds the answer to `key` for `max_age`. An answer that is valid for
    /// no time is not held.
    pub(crate) fn store(
        &self,
        key: String,
        body: Vec<u8>,
        max_age: Duration,
        last_hub_interaction: Option<DateTime<Utc>>,
        now: Instant,
    ) {
        if max_age.is_zero() {
            return;
        }
        let mut state = lock(&self.state);
        state
            .held
            .retain(|_, held| now.saturating_duration_since(held.fetched) < held.max_age);
        while state.held.len() >= self.config.max_entries.max(1) && !state.held.contains_key(&key) {
            let oldest = state
                .held
                .iter()
                .min_by_key(|(_, held)| held.fetched)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => state.held.remove(&oldest),
                None => break,
            };
        }
        state.held.insert(
            key,
            Held {
                body: Arc::new(body),
                fetched: now,
                max_age,
                last_hub_interaction,
            },
        );
    }

    /// How long the endpoint `path` is to be left alone, if it is.
    pub(crate) fn pause(&self, path: &str, now: Instant) -> Option<Duration> {
        let state = lock(&self.state);
        let pause = state.pauses.get(path)?;
        pause
            .until
            .checked_duration_since(now)
            .filter(|left| !left.is_zero())
    }

    /// The endpoint `path` gave a bad answer: `retry_after` is what it asked
    /// to wait, if it said.
    pub(crate) fn failed(&self, path: &str, retry_after: Option<Duration>, now: Instant) {
        let mut state = lock(&self.state);
        let pause = state.pauses.entry(path.to_string()).or_insert(Pause {
            until: now,
            failures: 0,
        });
        pause.failures = pause.failures.saturating_add(1);
        let doublings = pause.failures.saturating_sub(1).min(16);
        let wait = retry_after.unwrap_or_else(|| {
            self.config
                .backoff_base
                .saturating_mul(1 << doublings)
                .min(self.config.backoff_max.max(self.config.backoff_base))
        });
        pause.until = now + wait;
    }

    /// The endpoint `path` answered well: it is not held back anymore.
    pub(crate) fn succeeded(&self, path: &str) {
        lock(&self.state).pauses.remove(path);
    }

    /// The turn of `key`: whoever holds it is the one asking for it.
    pub(crate) fn lane(&self, key: &str) -> Lane {
        let mut lanes = lock(&self.lanes);
        Arc::clone(lanes.entry(key.to_string()).or_default())
    }

    /// Forgets the turn of `key` when nobody else is waiting for it.
    pub(crate) fn release_lane(&self, key: &str, lane: Lane) {
        let mut lanes = lock(&self.lanes);
        // The map holds one reference and the caller the other.
        if Arc::strong_count(&lane) <= 2 {
            lanes.remove(key);
        }
    }

    /// Forgets the answers that are held. The pauses stay.
    pub(crate) fn forget(&self) {
        lock(&self.state).held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        Guard::new(GuardConfig::default())
    }

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn an_answer_is_held_until_its_max_age_runs_out() {
        let guard = guard();
        let start = Instant::now();
        guard.store("a".into(), b"[]".to_vec(), secs(300), None, start);

        let fresh = guard.fresh("a", start + secs(100)).unwrap();
        assert_eq!(fresh.body.as_slice(), b"[]");
        assert_eq!(fresh.remaining, secs(200));
        // Not a second more than it says.
        assert!(guard.fresh("a", start + secs(300)).is_none());
        assert!(guard.fresh("a", start + secs(301)).is_none());
        // And what was never asked is not there.
        assert!(guard.fresh("b", start).is_none());
    }

    #[test]
    fn an_answer_valid_for_no_time_is_not_held() {
        let guard = guard();
        let start = Instant::now();
        guard.store("a".into(), b"[]".to_vec(), Duration::ZERO, None, start);
        assert!(guard.fresh("a", start).is_none());
    }

    #[test]
    fn the_hub_interaction_is_held_with_the_answer() {
        let guard = guard();
        let start = Instant::now();
        let at = Utc::now();
        guard.store("a".into(), b"[]".to_vec(), secs(60), Some(at), start);
        assert_eq!(
            guard.fresh("a", start).unwrap().last_hub_interaction,
            Some(at)
        );
    }

    #[test]
    fn when_there_are_too_many_answers_the_oldest_go() {
        let guard = Guard::new(GuardConfig {
            max_entries: 2,
            ..GuardConfig::default()
        });
        let start = Instant::now();
        guard.store("old".into(), vec![1], secs(600), None, start);
        guard.store("mid".into(), vec![2], secs(600), None, start + secs(1));
        guard.store("new".into(), vec![3], secs(600), None, start + secs(2));

        let now = start + secs(3);
        assert!(guard.fresh("old", now).is_none());
        assert!(guard.fresh("mid", now).is_some());
        assert!(guard.fresh("new", now).is_some());
        // Holding again what is already held takes nobody out.
        guard.store("new".into(), vec![4], secs(600), None, start + secs(4));
        assert!(guard.fresh("mid", start + secs(5)).is_some());
    }

    #[test]
    fn the_answers_that_ran_out_are_dropped_when_another_is_held() {
        let guard = Guard::new(GuardConfig {
            max_entries: 2,
            ..GuardConfig::default()
        });
        let start = Instant::now();
        guard.store("short".into(), vec![1], secs(10), None, start);
        guard.store("long".into(), vec![2], secs(600), None, start);
        // `short` has run out: it goes instead of `long`.
        guard.store("other".into(), vec![3], secs(600), None, start + secs(20));
        assert!(guard.fresh("long", start + secs(21)).is_some());
        assert!(guard.fresh("other", start + secs(21)).is_some());
    }

    #[test]
    fn a_bad_answer_pauses_the_endpoint_and_the_wait_doubles_up_to_the_limit() {
        let guard = guard();
        let start = Instant::now();
        assert!(guard.pause("/routes", start).is_none());

        let mut now = start;
        for (failure, expected) in [
            (1, 15),
            (2, 30),
            (3, 60),
            (4, 120),
            (5, 240),
            (6, 480),
            (7, 600),
            (8, 600),
        ] {
            guard.failed("/routes", None, now);
            assert_eq!(
                guard.pause("/routes", now),
                Some(secs(expected)),
                "failure {failure}"
            );
            // The pause ends, and the next failure is a longer one.
            now += secs(expected);
            assert!(guard.pause("/routes", now).is_none());
        }
    }

    #[test]
    fn what_retry_after_says_is_followed_as_it_is() {
        let guard = guard();
        let start = Instant::now();
        // More than the limit the guard picks by itself.
        guard.failed("/routes", Some(secs(3600)), start);
        assert_eq!(guard.pause("/routes", start), Some(secs(3600)));
        // And less than the base.
        guard.failed("/signatures", Some(secs(2)), start);
        assert_eq!(guard.pause("/signatures", start), Some(secs(2)));
    }

    #[test]
    fn a_pause_is_of_one_endpoint_and_a_good_answer_ends_it() {
        let guard = guard();
        let start = Instant::now();
        guard.failed("/routes", None, start);
        assert!(guard.pause("/routes", start).is_some());
        assert!(guard.pause("/signatures", start).is_none());

        guard.succeeded("/routes");
        assert!(guard.pause("/routes", start).is_none());
        // The count starts over.
        guard.failed("/routes", None, start);
        assert_eq!(guard.pause("/routes", start), Some(secs(15)));
    }

    #[test]
    fn forgetting_drops_the_answers_but_not_the_pauses() {
        let guard = guard();
        let start = Instant::now();
        guard.store("a".into(), vec![1], secs(60), None, start);
        guard.failed("/routes", None, start);

        guard.forget();

        assert!(guard.fresh("a", start).is_none());
        assert!(guard.pause("/routes", start).is_some());
    }

    #[test]
    fn a_turn_is_forgotten_when_nobody_else_waits_for_it() {
        let guard = guard();
        let first = guard.lane("a");
        let second = guard.lane("a");
        assert!(Arc::ptr_eq(&first, &second));

        // Two hold it: it stays while one of them lets go.
        guard.release_lane("a", first);
        assert_eq!(lock(&guard.lanes).len(), 1);
        guard.release_lane("a", second);
        assert!(lock(&guard.lanes).is_empty());
    }
}
