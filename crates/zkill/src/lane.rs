//! The order and the pace of the requests of one endpoint.
//!
//! A [`Lane`] holds the requests waiting to be made and says, given the time,
//! which goes next or how long to wait. It makes no request and has no clock
//! of its own: the caller passes the time in, so everything here can be tested
//! to the millisecond.
//!
//! * **Spacing.** Two requests are never closer than
//!   [`LaneConfig::min_interval`].
//! * **Room for what is urgent.** Requests that nobody waits for
//!   (everything but [`Priority::Interactive`]) are also held to
//!   [`LaneConfig::background_interval`] between them, which is longer. The
//!   gaps that leaves are for the interactive ones, so the whole rate is
//!   never taken by the background work however long its queue is.
//! * **No starvation.** After [`LaneConfig::reserve_every`] interactive
//!   requests in a row, a background one goes next if it may. And every
//!   [`LaneConfig::low_every`]-th background request is taken from the lowest
//!   priority waiting, so the last ones are not left for ever behind the
//!   first.
//! * **Delay and lease.** A request can wait a moment before it is eligible
//!   (a tooltip that the mouse only crosses on its way elsewhere is never
//!   asked), and can be leased: it is dropped unless it is submitted again
//!   before the lease ends.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How much a request matters. The lower, the sooner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// Somebody is waiting for the answer. Not held to the background pace.
    Interactive,
    /// Background work that is likely to be looked at soon.
    High,
    /// Background work in the same area.
    Normal,
    /// Background work nobody will look at for a while.
    Low,
}

impl Priority {
    /// Every priority, the most urgent first.
    pub const ALL: [Priority; 4] = [
        Priority::Interactive,
        Priority::High,
        Priority::Normal,
        Priority::Low,
    ];

    /// Whether nobody is waiting for the answer.
    pub fn is_background(self) -> bool {
        self != Priority::Interactive
    }

    fn level(self) -> usize {
        self as usize
    }
}

/// How a lane paces its requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneConfig {
    /// The least time between two requests of any priority.
    pub min_interval: Duration,
    /// The least time between two background requests.
    pub background_interval: Duration,
    /// After this many interactive requests in a row a background one is
    /// let through, if its pace allows it.
    pub reserve_every: u32,
    /// Every this many background requests, one is taken from the lowest
    /// priority waiting instead of the highest. 0 never does.
    pub low_every: u32,
}

impl LaneConfig {
    /// The query API: 8 requests a second at most, 2 of them background.
    pub fn api() -> Self {
        Self {
            min_interval: Duration::from_millis(125),
            background_interval: Duration::from_millis(500),
            reserve_every: 8,
            low_every: 10,
        }
    }

    /// The R2Z2 feed: 8 requests a second at most, 4 of them background (the
    /// hour read back when the feed starts).
    pub fn r2z2() -> Self {
        Self {
            min_interval: Duration::from_millis(125),
            background_interval: Duration::from_millis(250),
            reserve_every: 8,
            low_every: 10,
        }
    }
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self::api()
    }
}

/// A request to queue.
#[derive(Debug, Clone)]
pub struct Job<T> {
    /// What the request is about: asking again for the same key merges with
    /// the request already waiting.
    pub key: u64,
    /// What the caller needs to make it.
    pub payload: T,
    pub priority: Priority,
    /// How long before it may go.
    pub delay: Duration,
    /// How long it is kept without being submitted again, if it is leased.
    pub lease: Option<Duration>,
}

impl<T> Job<T> {
    pub fn new(key: u64, payload: T, priority: Priority) -> Self {
        Self {
            key,
            payload,
            priority,
            delay: Duration::ZERO,
            lease: None,
        }
    }

    /// The request may go only after `delay`.
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The request is dropped unless it is submitted again within `lease`.
    pub fn leased(mut self, lease: Duration) -> Self {
        self.lease = Some(lease);
        self
    }
}

/// What [`Lane::submit`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submitted {
    /// A new request is waiting.
    Queued,
    /// There was one for the same key already: it keeps its place, takes the
    /// higher of the two priorities and renews its lease.
    Merged,
}

/// What the lane says to do now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next<T> {
    /// Make this request.
    Send {
        key: u64,
        payload: T,
        priority: Priority,
    },
    /// Nothing may go for this long.
    Wait(Duration),
    /// Nothing is waiting.
    Empty,
}

/// A waiting request and when it is expected to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    pub key: u64,
    pub priority: Priority,
    pub eta: Duration,
}

#[derive(Debug, Clone)]
struct Item<T> {
    key: u64,
    payload: T,
    not_before: Instant,
    lease_until: Option<Instant>,
}

/// The requests of one endpoint, in the order they will be made.
#[derive(Debug, Clone)]
pub struct Lane<T> {
    config: LaneConfig,
    queues: [VecDeque<Item<T>>; 4],
    /// Nothing goes before this.
    ready_at: Option<Instant>,
    /// No background request goes before this.
    background_ready_at: Option<Instant>,
    /// Interactive requests made since the last background one.
    interactive_run: u32,
    /// Background requests made so far.
    background_picks: u64,
}

impl<T> Lane<T> {
    pub fn new(config: LaneConfig) -> Self {
        Self {
            config,
            queues: std::array::from_fn(|_| VecDeque::new()),
            ready_at: None,
            background_ready_at: None,
            interactive_run: 0,
            background_picks: 0,
        }
    }

    pub fn config(&self) -> LaneConfig {
        self.config
    }

    /// How many requests wait.
    pub fn len(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.queues.iter().all(VecDeque::is_empty)
    }

    /// Whether a request for `key` waits.
    pub fn contains(&self, key: u64) -> bool {
        self.find(key).is_some()
    }

    /// How many requests of each priority wait.
    pub fn lengths(&self) -> [usize; 4] {
        std::array::from_fn(|level| self.queues[level].len())
    }

    /// Queues `job`, or merges it with the one waiting for the same key.
    pub fn submit(&mut self, job: Job<T>, now: Instant) -> Submitted {
        let lease_until = job.lease.map(|lease| now + lease);
        if let Some((level, position)) = self.find(job.key) {
            let wanted = job.priority.level();
            if let Some(new) = lease_until {
                let item = &mut self.queues[level][position];
                item.lease_until = Some(item.lease_until.map_or(new, |old| old.max(new)));
            }
            if wanted < level
                && let Some(item) = self.queues[level].remove(position)
            {
                self.queues[wanted].push_back(item);
            }
            return Submitted::Merged;
        }
        self.queues[job.priority.level()].push_back(Item {
            key: job.key,
            payload: job.payload,
            not_before: now + job.delay,
            lease_until,
        });
        Submitted::Queued
    }

    /// Forgets the request for `key`. `false` when there was none.
    pub fn cancel(&mut self, key: u64) -> bool {
        match self.find(key) {
            Some((level, position)) => self.queues[level].remove(position).is_some(),
            None => false,
        }
    }

    /// Forgets every background request and keeps the interactive ones (the
    /// plan is being made again). Returns how many were dropped.
    pub fn clear_background(&mut self) -> usize {
        let mut dropped = 0;
        for priority in Priority::ALL {
            if priority.is_background() {
                let queue = &mut self.queues[priority.level()];
                dropped += queue.len();
                queue.clear();
            }
        }
        dropped
    }

    /// Nothing goes before `time` (a refusal of the server, a failure that
    /// asks for a longer pause).
    pub fn hold_until(&mut self, time: Instant) {
        self.ready_at = Some(self.ready_at.map_or(time, |ready| ready.max(time)));
    }

    /// The next step as of `now`.
    pub fn next(&mut self, now: Instant) -> Next<T> {
        self.drop_lapsed(now);
        let Some(sendable) = self.sendable_in(now) else {
            return Next::Empty;
        };
        let spacing = self
            .ready_at
            .map_or(Duration::ZERO, |ready| ready.saturating_duration_since(now));
        let wait = sendable.max(spacing);
        if !wait.is_zero() {
            return Next::Wait(wait);
        }
        let Some((level, position)) = self.choose(now) else {
            // Cannot happen: something was sendable. Do not spin.
            return Next::Wait(self.config.min_interval);
        };
        let Some(item) = self.queues[level].remove(position) else {
            return Next::Wait(self.config.min_interval);
        };
        self.ready_at = Some(now + self.config.min_interval);
        if level == Priority::Interactive.level() {
            self.interactive_run = self.interactive_run.saturating_add(1);
        } else {
            self.interactive_run = 0;
            self.background_picks = self.background_picks.saturating_add(1);
            self.background_ready_at = Some(now + self.config.background_interval);
        }
        Next::Send {
            key: item.key,
            payload: item.payload,
            priority: Priority::ALL[level],
        }
    }

    /// The first `limit` requests in the order they will go, with how long
    /// each is expected to wait from `now`: [`Self::next`] played forward on a
    /// copy. Leases are taken as renewed, and what is submitted later is not
    /// known, so it is an estimate.
    pub fn forecast(&self, now: Instant, limit: usize) -> Vec<Queued>
    where
        T: Clone,
    {
        let mut lane = self.clone();
        for queue in &mut lane.queues {
            for item in queue.iter_mut() {
                item.lease_until = None;
            }
        }
        let mut forecast = Vec::new();
        let mut at = now;
        for _ in 0..limit.saturating_mul(2).saturating_add(2) {
            if forecast.len() >= limit {
                break;
            }
            match lane.next(at) {
                Next::Send { key, priority, .. } => forecast.push(Queued {
                    key,
                    priority,
                    eta: at.saturating_duration_since(now),
                }),
                Next::Wait(wait) => at += wait,
                Next::Empty => break,
            }
        }
        forecast
    }

    fn find(&self, key: u64) -> Option<(usize, usize)> {
        self.queues.iter().enumerate().find_map(|(level, queue)| {
            queue
                .iter()
                .position(|item| item.key == key)
                .map(|position| (level, position))
        })
    }

    fn drop_lapsed(&mut self, now: Instant) {
        for queue in &mut self.queues {
            queue.retain(|item| item.lease_until.is_none_or(|until| until > now));
        }
    }

    /// How long until something could go, ignoring the spacing; `None` when
    /// nothing waits.
    fn sendable_in(&self, now: Instant) -> Option<Duration> {
        let background_wait = self
            .background_ready_at
            .map_or(Duration::ZERO, |ready| ready.saturating_duration_since(now));
        self.queues
            .iter()
            .enumerate()
            .flat_map(|(level, queue)| {
                queue.iter().map(move |item| {
                    let due = item.not_before.saturating_duration_since(now);
                    if level == Priority::Interactive.level() {
                        due
                    } else {
                        due.max(background_wait)
                    }
                })
            })
            .min()
    }

    /// The request to make now: its queue and its place in it.
    fn choose(&self, now: Instant) -> Option<(usize, usize)> {
        let interactive = self.first_eligible(Priority::Interactive.level(), now);
        let background_allowed = self.background_ready_at.is_none_or(|ready| ready <= now);
        let background = background_allowed
            .then(|| self.choose_background(now))
            .flatten();
        match (interactive, background) {
            (Some(interactive), Some(background)) => {
                if self.interactive_run >= self.config.reserve_every.max(1) {
                    Some(background)
                } else {
                    Some(interactive)
                }
            }
            (Some(interactive), None) => Some(interactive),
            (None, background) => background,
        }
    }

    fn choose_background(&self, now: Instant) -> Option<(usize, usize)> {
        let every = u64::from(self.config.low_every);
        let lowest_turn = every > 0 && (self.background_picks + 1).is_multiple_of(every);
        let levels = [
            Priority::High.level(),
            Priority::Normal.level(),
            Priority::Low.level(),
        ];
        if lowest_turn {
            levels
                .iter()
                .rev()
                .find_map(|&level| self.first_eligible(level, now))
        } else {
            levels
                .iter()
                .find_map(|&level| self.first_eligible(level, now))
        }
    }

    fn first_eligible(&self, level: usize, now: Instant) -> Option<(usize, usize)> {
        self.queues[level]
            .iter()
            .position(|item| item.not_before <= now)
            .map(|position| (level, position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A starting point to measure the test's times from.
    struct Clock(Instant);

    impl Clock {
        fn new() -> Self {
            Self(Instant::now())
        }

        fn at(&self, millis: u64) -> Instant {
            self.0 + Duration::from_millis(millis)
        }

        fn ms(millis: u64) -> Duration {
            Duration::from_millis(millis)
        }

        /// No pace at all, to look at the order alone.
        fn unpaced(reserve_every: u32, low_every: u32) -> LaneConfig {
            LaneConfig {
                min_interval: Duration::ZERO,
                background_interval: Duration::ZERO,
                reserve_every,
                low_every,
            }
        }

        /// The keys of the next `count` requests, all asked for at `at`.
        fn keys(lane: &mut Lane<()>, at: Instant, count: usize) -> Vec<u64> {
            (0..count)
                .filter_map(|_| match lane.next(at) {
                    Next::Send { key, .. } => Some(key),
                    _ => None,
                })
                .collect()
        }
    }

    #[test]
    fn two_requests_are_never_closer_than_the_minimum_interval() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        lane.submit(Job::new(1, (), Priority::Interactive), clock.at(0));
        lane.submit(Job::new(2, (), Priority::Interactive), clock.at(0));
        assert!(matches!(lane.next(clock.at(0)), Next::Send { key: 1, .. }));
        assert_eq!(lane.next(clock.at(0)), Next::Wait(Clock::ms(125)));
        assert!(matches!(
            lane.next(clock.at(125)),
            Next::Send { key: 2, .. }
        ));
        assert_eq!(lane.next(clock.at(125)), Next::Empty);
    }

    #[test]
    fn background_requests_keep_to_their_own_slower_pace() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        for key in 1..=3 {
            lane.submit(Job::new(key, (), Priority::Normal), clock.at(0));
        }
        assert!(matches!(lane.next(clock.at(0)), Next::Send { key: 1, .. }));
        assert_eq!(lane.next(clock.at(0)), Next::Wait(Clock::ms(500)));
        assert!(matches!(
            lane.next(clock.at(500)),
            Next::Send { key: 2, .. }
        ));
        assert_eq!(lane.next(clock.at(500)), Next::Wait(Clock::ms(500)));
    }

    #[test]
    fn an_interactive_request_goes_in_the_gap_between_background_ones() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        lane.submit(Job::new(1, (), Priority::Normal), clock.at(0));
        lane.submit(Job::new(2, (), Priority::Normal), clock.at(0));
        assert!(matches!(lane.next(clock.at(0)), Next::Send { key: 1, .. }));
        lane.submit(Job::new(9, (), Priority::Interactive), clock.at(100));
        // Only the minimum interval is left to wait, not the background one.
        assert_eq!(lane.next(clock.at(100)), Next::Wait(Clock::ms(25)));
        assert!(matches!(
            lane.next(clock.at(125)),
            Next::Send { key: 9, .. }
        ));
        assert_eq!(lane.next(clock.at(125)), Next::Wait(Clock::ms(375)));
    }

    #[test]
    fn what_is_interactive_goes_before_the_background() {
        let clock = Clock::new();
        let mut lane = Lane::new(Clock::unpaced(100, 0));
        lane.submit(Job::new(1, (), Priority::High), clock.at(0));
        lane.submit(Job::new(2, (), Priority::Interactive), clock.at(0));
        assert_eq!(Clock::keys(&mut lane, clock.at(0), 2), vec![2, 1]);
    }

    #[test]
    fn a_background_request_goes_after_a_run_of_interactive_ones() {
        let clock = Clock::new();
        let mut lane = Lane::new(Clock::unpaced(2, 0));
        for key in 1..=4 {
            lane.submit(Job::new(key, (), Priority::Interactive), clock.at(0));
        }
        lane.submit(Job::new(10, (), Priority::Normal), clock.at(0));
        assert_eq!(Clock::keys(&mut lane, clock.at(0), 5), vec![1, 2, 10, 3, 4]);
    }

    #[test]
    fn every_so_often_the_lowest_priority_is_served() {
        let clock = Clock::new();
        let mut lane = Lane::new(Clock::unpaced(100, 3));
        for key in 1..=4 {
            lane.submit(Job::new(key, (), Priority::Normal), clock.at(0));
        }
        lane.submit(Job::new(9, (), Priority::Low), clock.at(0));
        assert_eq!(Clock::keys(&mut lane, clock.at(0), 5), vec![1, 2, 9, 3, 4]);
    }

    #[test]
    fn a_delayed_request_waits_and_can_be_cancelled() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        lane.submit(
            Job::new(1, (), Priority::Interactive).after(Clock::ms(250)),
            clock.at(0),
        );
        assert_eq!(lane.next(clock.at(0)), Next::Wait(Clock::ms(250)));
        assert!(matches!(
            lane.next(clock.at(250)),
            Next::Send { key: 1, .. }
        ));

        lane.submit(
            Job::new(2, (), Priority::Interactive).after(Clock::ms(250)),
            clock.at(1000),
        );
        assert!(lane.cancel(2));
        assert!(!lane.cancel(2));
        assert_eq!(lane.next(clock.at(1000)), Next::Empty);
    }

    #[test]
    fn a_leased_request_is_dropped_unless_it_is_renewed() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        let job = || {
            Job::new(1, (), Priority::Interactive)
                .after(Clock::ms(1000))
                .leased(Clock::ms(500))
        };
        lane.submit(job(), clock.at(0));
        // Submitted again at 400 ms: the lease now lasts until 900 ms.
        assert_eq!(lane.submit(job(), clock.at(400)), Submitted::Merged);
        assert_eq!(lane.next(clock.at(800)), Next::Wait(Clock::ms(200)));
        // Nobody renewed it: it is gone before its turn.
        assert_eq!(lane.next(clock.at(950)), Next::Empty);
        assert!(lane.is_empty());
    }

    #[test]
    fn asking_again_merges_and_takes_the_higher_priority() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        assert_eq!(
            lane.submit(Job::new(5, (), Priority::Low), clock.at(0)),
            Submitted::Queued
        );
        assert_eq!(
            lane.submit(Job::new(5, (), Priority::Normal), clock.at(0)),
            Submitted::Merged
        );
        // It is not lowered again.
        lane.submit(Job::new(5, (), Priority::Low), clock.at(0));
        assert_eq!(lane.len(), 1);
        assert_eq!(lane.lengths(), [0, 0, 1, 0]);
        assert!(matches!(
            lane.next(clock.at(0)),
            Next::Send {
                key: 5,
                priority: Priority::Normal,
                ..
            }
        ));
    }

    #[test]
    fn a_hold_keeps_everything_back() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        lane.submit(Job::new(1, (), Priority::Interactive), clock.at(0));
        lane.hold_until(clock.at(2000));
        assert_eq!(lane.next(clock.at(0)), Next::Wait(Clock::ms(2000)));
        assert!(matches!(
            lane.next(clock.at(2000)),
            Next::Send { key: 1, .. }
        ));
    }

    #[test]
    fn the_forecast_says_when_each_request_goes() {
        let clock = Clock::new();
        let mut lane = Lane::new(LaneConfig::api());
        for key in 1..=3 {
            lane.submit(Job::new(key, (), Priority::Normal), clock.at(0));
        }
        let etas: Vec<(u64, Duration)> = lane
            .forecast(clock.at(0), 10)
            .iter()
            .map(|queued| (queued.key, queued.eta))
            .collect();
        assert_eq!(
            etas,
            vec![(1, Clock::ms(0)), (2, Clock::ms(500)), (3, Clock::ms(1000))]
        );
        // Looking ahead changes nothing.
        assert_eq!(lane.len(), 3);
    }

    #[test]
    fn clearing_the_background_keeps_the_interactive_requests() {
        let now = Instant::now();
        let mut lane = Lane::new(LaneConfig::default());
        lane.submit(Job::new(1, (), Priority::Interactive), now);
        lane.submit(Job::new(2, (), Priority::High), now);
        lane.submit(Job::new(3, (), Priority::Low), now);
        assert_eq!(lane.clear_background(), 2);
        assert!(lane.contains(1));
        assert!(!lane.contains(2));
        assert_eq!(lane.len(), 1);
    }
}
