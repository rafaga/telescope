//! What the intel channels said a moment ago.
//!
//! The engine reads every line by itself, but players do not write that way:
//! someone reports `H-5GUI*  Floris Saucus` and the lines that follow say
//! `nv`, `+4` or `clr` without naming the system again. A [`ChannelMemory`]
//! keeps the last system each channel reported so that those follow-ups can
//! be attributed to it for a while (see `Executor::run_remembering`).

use crate::event::SystemFact;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;

/// The system a channel reported last, and when.
#[derive(Debug, Clone, PartialEq)]
struct Remembered {
    system: SystemFact,
    at: DateTime<Utc>,
}

/// The last system reported in each channel.
#[derive(Debug, Clone)]
pub struct ChannelMemory {
    window: Duration,
    last: HashMap<String, Remembered>,
}

impl ChannelMemory {
    /// How long a reported system stays attributable to the lines that
    /// follow it, in seconds.
    pub const DEFAULT_WINDOW_SECS: i64 = 90;

    /// A memory whose reports are good for `window`.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            last: HashMap::new(),
        }
    }

    /// A memory with the default window ([`Self::DEFAULT_WINDOW_SECS`]).
    pub fn with_default_window() -> Self {
        Self::new(Duration::seconds(Self::DEFAULT_WINDOW_SECS))
    }

    /// How long a report is remembered.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Notes that `channel` reported `system` at `at`, replacing what it
    /// reported before.
    pub fn remember(&mut self, channel: &str, system: SystemFact, at: DateTime<Utc>) {
        self.last
            .insert(channel.to_string(), Remembered { system, at });
    }

    /// The system `channel` reported last, if it did so no earlier than the
    /// window before `at` (and not after it: a line cannot follow a report
    /// that has not been made yet).
    pub fn recall(&self, channel: &str, at: DateTime<Utc>) -> Option<&SystemFact> {
        let seen = self.last.get(channel)?;
        let elapsed = at - seen.at;
        (elapsed >= Duration::zero() && elapsed <= self.window).then_some(&seen.system)
    }

    /// Forgets everything (the rules changed, or the user asked for it).
    pub fn clear(&mut self) {
        self.last.clear();
    }
}

impl Default for ChannelMemory {
    fn default() -> Self {
        Self::with_default_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(id: usize, name: &str) -> SystemFact {
        SystemFact {
            id,
            name: name.to_string(),
            span: None,
        }
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn a_report_is_recalled_inside_the_window_only() {
        let mut memory = ChannelMemory::new(Duration::seconds(60));
        memory.remember("intel", system(1, "H-5GUI"), at(0));
        assert_eq!(memory.recall("intel", at(0)).map(|s| s.id), Some(1));
        assert_eq!(memory.recall("intel", at(60)).map(|s| s.id), Some(1));
        assert!(memory.recall("intel", at(61)).is_none());
    }

    #[test]
    fn channels_do_not_share_what_they_reported() {
        let mut memory = ChannelMemory::default();
        memory.remember("vale", system(1, "H-5GUI"), at(0));
        assert!(memory.recall("imperium", at(1)).is_none());
    }

    #[test]
    fn a_later_report_replaces_the_earlier_one() {
        let mut memory = ChannelMemory::default();
        memory.remember("intel", system(1, "H-5GUI"), at(0));
        memory.remember("intel", system(2, "Jita"), at(10));
        assert_eq!(memory.recall("intel", at(11)).map(|s| s.id), Some(2));
    }

    #[test]
    fn a_line_cannot_follow_a_report_that_comes_after_it() {
        let mut memory = ChannelMemory::default();
        memory.remember("intel", system(1, "H-5GUI"), at(100));
        assert!(memory.recall("intel", at(50)).is_none());
    }

    #[test]
    fn clearing_forgets_every_channel() {
        let mut memory = ChannelMemory::default();
        memory.remember("intel", system(1, "H-5GUI"), at(0));
        memory.clear();
        assert!(memory.recall("intel", at(1)).is_none());
    }
}
