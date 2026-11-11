//! What happened to the requests of one endpoint in the last minutes.
//!
//! A [`LaneStats`] counts, second by second, the requests let through and how
//! they ended (answered, refused with a 403 or 429, failed). The guard of each
//! endpoint feeds it, so every request is counted once whoever made it. The
//! Debug window, the log and Tracy read the same numbers: a total since the
//! start says little, a rate says whether the pace holds.

use chrono::{DateTime, TimeDelta, Utc};
use std::collections::VecDeque;
use std::sync::Mutex;

/// How the counts are kept. The defaults are what Telescope uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsConfig {
    /// The time each count covers.
    pub bucket: TimeDelta,
    /// How far back the counts are kept.
    pub keep: TimeDelta,
    /// The time a rate is measured over (see [`LaneStats::recent`]).
    pub rate_window: TimeDelta,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            bucket: TimeDelta::seconds(1),
            keep: TimeDelta::minutes(10),
            rate_window: TimeDelta::seconds(5),
        }
    }
}

/// The requests of one stretch of time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Requests let through.
    pub sent: u32,
    /// Requests answered well.
    pub ok: u32,
    /// Requests zKillboard refused (403 or 429).
    pub refused: u32,
    /// Requests that failed some other way.
    pub failed: u32,
}

impl Tally {
    /// The requests that did not end well.
    pub fn errors(&self) -> u32 {
        self.refused + self.failed
    }

    fn add(&mut self, other: Tally) {
        self.sent += other.sent;
        self.ok += other.ok;
        self.refused += other.refused;
        self.failed += other.failed;
    }
}

/// The requests of one bucket of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    /// Where the bucket begins.
    pub start: DateTime<Utc>,
    /// What happened in it.
    pub tally: Tally,
}

/// The recent history of the requests of one endpoint.
pub struct LaneStats {
    config: StatsConfig,
    slices: Mutex<VecDeque<Slice>>,
}

impl LaneStats {
    pub fn new(config: StatsConfig) -> Self {
        Self {
            config,
            slices: Mutex::new(VecDeque::new()),
        }
    }

    /// A request was let through.
    pub fn sent(&self, now: DateTime<Utc>) {
        self.count(
            now,
            Tally {
                sent: 1,
                ..Tally::default()
            },
        );
    }

    /// A request was answered well.
    pub fn answered(&self, now: DateTime<Utc>) {
        self.count(
            now,
            Tally {
                ok: 1,
                ..Tally::default()
            },
        );
    }

    /// A request was refused (403 or 429).
    pub fn refused(&self, now: DateTime<Utc>) {
        self.count(
            now,
            Tally {
                refused: 1,
                ..Tally::default()
            },
        );
    }

    /// A request failed some other way.
    pub fn failed(&self, now: DateTime<Utc>) {
        self.count(
            now,
            Tally {
                failed: 1,
                ..Tally::default()
            },
        );
    }

    /// What happened in the last [`StatsConfig::rate_window`].
    pub fn recent(&self, now: DateTime<Utc>) -> Tally {
        let from = now - self.config.rate_window;
        let mut total = Tally::default();
        for slice in self.lock().iter().filter(|slice| slice.start > from) {
            total.add(slice.tally);
        }
        total
    }

    /// Requests let through per second over the last
    /// [`StatsConfig::rate_window`].
    pub fn rate(&self, now: DateTime<Utc>) -> f64 {
        let seconds = self.config.rate_window.num_milliseconds().max(1) as f64 / 1000.0;
        f64::from(self.recent(now).sent) / seconds
    }

    /// Everything that is kept.
    pub fn total(&self) -> Tally {
        let mut total = Tally::default();
        for slice in self.lock().iter() {
            total.add(slice.tally);
        }
        total
    }

    /// The last `count` buckets, the one of `now` last, with the empty ones
    /// filled in (what a graph draws).
    pub fn series(&self, now: DateTime<Utc>, count: usize) -> Vec<Slice> {
        let slices = self.lock();
        let latest = self.start_of(now);
        (0..count)
            .rev()
            .map(|back| {
                let start = latest - self.config.bucket * i32::try_from(back).unwrap_or(i32::MAX);
                let tally = slices
                    .iter()
                    .rev()
                    .find(|slice| slice.start == start)
                    .map_or_else(Tally::default, |slice| slice.tally);
                Slice { start, tally }
            })
            .collect()
    }

    /// The beginning of the bucket `time` falls in.
    fn start_of(&self, time: DateTime<Utc>) -> DateTime<Utc> {
        let width = self.config.bucket.num_milliseconds().max(1);
        let millis = time.timestamp_millis();
        DateTime::from_timestamp_millis(millis - millis.rem_euclid(width)).unwrap_or(time)
    }

    fn count(&self, now: DateTime<Utc>, tally: Tally) {
        let start = self.start_of(now);
        let mut slices = self.lock();
        let oldest = start - self.config.keep;
        while slices.front().is_some_and(|slice| slice.start < oldest) {
            slices.pop_front();
        }
        if let Some(slice) = slices.iter_mut().rev().find(|slice| slice.start == start) {
            slice.tally.add(tally);
        } else if slices.back().is_none_or(|slice| slice.start < start) {
            slices.push_back(Slice { start, tally });
        }
        // A moment in the past with no bucket left is not worth keeping.
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Slice>> {
        self.slices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for LaneStats {
    fn default() -> Self {
        Self::new(StatsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64, millis: u32) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, millis * 1_000_000).unwrap()
    }

    #[test]
    fn requests_of_the_same_second_are_counted_together() {
        let stats = LaneStats::default();
        stats.sent(at(0, 100));
        stats.sent(at(0, 900));
        stats.sent(at(1, 0));
        let series = stats.series(at(1, 500), 2);
        assert_eq!(series[0].tally.sent, 2);
        assert_eq!(series[1].tally.sent, 1);
    }

    #[test]
    fn the_series_fills_the_seconds_with_nothing() {
        let stats = LaneStats::default();
        stats.sent(at(0, 0));
        let series = stats.series(at(3, 0), 4);
        let sent: Vec<u32> = series.iter().map(|slice| slice.tally.sent).collect();
        assert_eq!(sent, [1, 0, 0, 0]);
        assert_eq!(series[3].start, at(3, 0));
    }

    #[test]
    fn what_is_older_than_the_history_is_dropped() {
        let stats = LaneStats::default();
        stats.sent(at(0, 0));
        stats.sent(at(601, 0));
        assert_eq!(stats.total().sent, 1);
    }

    #[test]
    fn the_rate_covers_the_last_window_only() {
        let stats = LaneStats::default();
        for second in 0..10 {
            stats.sent(at(second, 0));
            stats.sent(at(second, 500));
        }
        // 5 seconds, 2 a second.
        assert!((stats.rate(at(9, 900)) - 2.0).abs() < f64::EPSILON);
        assert_eq!(stats.rate(at(60, 0)), 0.0);
    }

    #[test]
    fn refusals_and_failures_are_told_apart() {
        let stats = LaneStats::default();
        stats.sent(at(0, 0));
        stats.refused(at(0, 0));
        stats.failed(at(0, 0));
        stats.answered(at(0, 0));
        let tally = stats.recent(at(0, 0));
        assert_eq!(
            tally,
            Tally {
                sent: 1,
                ok: 1,
                refused: 1,
                failed: 1
            }
        );
        assert_eq!(tally.errors(), 2);
    }

    #[test]
    fn a_late_event_does_not_make_the_history_go_back() {
        let stats = LaneStats::default();
        stats.sent(at(5, 0));
        stats.sent(at(2, 0));
        assert_eq!(stats.total().sent, 1);
    }
}
