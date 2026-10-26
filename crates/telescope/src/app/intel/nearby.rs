//! Events near the linked characters, for the overlay widget.
//!
//! The dispatch thread records here every reported system that is within the
//! warning radius of a linked character (the same radius the alarm uses); the
//! overlay reads them each frame. It is a small, bounded list: the newest
//! first, each entry gone once the alert duration has passed.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Most events kept; older ones are dropped.
const CAPACITY: usize = 8;

/// An event near a linked character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NearbyEvent {
    /// The reported solar system.
    pub system_id: u32,
    /// Stargate jumps from the closest linked character.
    pub jumps: u8,
    /// What was reported (the line's text).
    pub text: String,
    pub received: Instant,
}

/// The recent events near the characters, shared between the dispatch thread
/// (writer) and the UI thread (reader).
#[derive(Default)]
pub(crate) struct NearbyEvents {
    events: Mutex<VecDeque<NearbyEvent>>,
}

impl NearbyEvents {
    /// Records an event, newest first. The same report of the same system
    /// replaces the earlier entry instead of repeating it.
    pub(crate) fn push(&self, event: NearbyEvent) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        events.retain(|old| !(old.system_id == event.system_id && old.text == event.text));
        events.push_front(event);
        events.truncate(CAPACITY);
    }

    // Read only by the overlay widget (cargo feature `overlay`).
    #[allow(dead_code)]
    /// The events received less than `max_age` before `now`, newest first.
    pub(crate) fn recent(&self, now: Instant, max_age: Duration) -> Vec<NearbyEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|event| now.saturating_duration_since(event.received) < max_age)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(system_id: u32, text: &str, received: Instant) -> NearbyEvent {
        NearbyEvent {
            system_id,
            jumps: 1,
            text: text.to_string(),
            received,
        }
    }

    #[test]
    fn the_newest_event_comes_first() {
        let events = NearbyEvents::default();
        let now = Instant::now();
        events.push(event(1, "a", now));
        events.push(event(2, "b", now));
        let ids: Vec<u32> = events
            .recent(now, Duration::from_secs(60))
            .iter()
            .map(|event| event.system_id)
            .collect();
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn a_repeated_report_replaces_its_entry() {
        let events = NearbyEvents::default();
        let now = Instant::now();
        events.push(event(1, "same", now));
        events.push(event(2, "other", now));
        events.push(event(1, "same", now));
        let ids: Vec<u32> = events
            .recent(now, Duration::from_secs(60))
            .iter()
            .map(|event| event.system_id)
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn old_events_are_not_returned() {
        let events = NearbyEvents::default();
        let start = Instant::now();
        events.push(event(1, "old", start));
        let later = start + Duration::from_secs(90);
        events.push(event(2, "new", later));
        let kept = events.recent(later, Duration::from_secs(60));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].system_id, 2);
    }

    #[test]
    fn only_the_newest_events_are_kept() {
        let events = NearbyEvents::default();
        let now = Instant::now();
        for id in 0..(CAPACITY as u32 + 3) {
            events.push(event(id, "x", now));
        }
        let kept = events.recent(now, Duration::from_secs(60));
        assert_eq!(kept.len(), CAPACITY);
        assert_eq!(kept[0].system_id, CAPACITY as u32 + 2);
    }
}
