//! What the killmails the zKillboard source has read say about a system,
//! for the node tooltips.
//!
//! The killmails of the last hour are facts: [`KillBuffer`] keeps them by
//! system, and [`KillBuffer::context`] says whether something is being shot
//! in that system right now. (A killmail can't be matched to a chat report:
//! the reports are not set against them.)
//!
//! The buffer only holds the last [`WINDOW`]: the feed is read live, and the
//! hour before the source started is read backwards from the feed's files
//! (see `Backfill` in the zKillboard source). Anything older is not here.

use super::kill_day::DayLine;
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::HashMap;

/// How far back the killmails are kept.
pub(crate) const WINDOW: TimeDelta = TimeDelta::seconds(60 * 60);

/// How recent the killmails of the shortest count are.
pub(crate) const RECENT: TimeDelta = TimeDelta::seconds(15 * 60);

/// The most victim ships listed.
const MAX_VICTIMS: usize = 3;

/// What zKillboard says about how a killmail came about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KillTags {
    /// The attackers shot a ship of their own side (zKillboard's `awox`).
    pub awox: bool,
    /// The kill is a gank (zKillboard's `ganked` label).
    pub ganked: bool,
}

/// One killmail, reduced to what the tooltips tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillRecord {
    pub killmail_id: u64,
    /// When the ship died.
    pub time: DateTime<Utc>,
    pub system_id: usize,
    /// The ship that died, when its name is known.
    pub victim: Option<String>,
    /// The ships of the attackers that were pilots (NPCs left out).
    pub attacker_ships: Vec<String>,
    /// How many of the attackers were pilots.
    pub pilots: u32,
    pub tags: KillTags,
    /// The killmail's hash, which with its id names it in ESI. Empty when
    /// unknown.
    pub hash: String,
    /// The station or structure it happened at, when zKillboard says.
    pub location_id: Option<u64>,
}

/// How far the buffer got reading the hour before the source started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Fill {
    pub done: u64,
    pub total: u64,
}

impl Fill {
    /// The part read, 0 to 100.
    pub(crate) fn percent(self) -> u32 {
        if self.total == 0 {
            return 100;
        }
        u32::try_from((self.done * 100 / self.total).min(100)).unwrap_or(100)
    }
}

/// What the killmails say about a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Ships have died in the system in the last [`WINDOW`].
    Active,
    /// No killmail in the last [`WINDOW`]. Not that it is safe: nobody has
    /// died.
    Quiet,
}

/// Where the killmails of a [`KillContext`] come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// The live feed.
    Feed,
    /// The query API and ESI: slower, and without the last five minutes.
    Api,
}

/// The killmails of a system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillContext {
    pub verdict: Verdict,
    /// Killmails in the last [`RECENT`].
    pub recent: usize,
    /// Killmails in the last [`WINDOW`].
    pub hour: usize,
    /// How long ago the last one was.
    pub last_age: Option<TimeDelta>,
    /// The ships of the attackers of the last killmail, with how many flew
    /// each.
    pub ships: Vec<(String, usize)>,
    /// How many pilots that killmail had.
    pub pilots: u32,
    /// The ships that died, newest first, without repeats.
    pub victims: Vec<String>,
    /// What zKillboard says about the killmail shown.
    pub tags: KillTags,
    /// While the hour before the source started is still being read, so the
    /// counts may be short.
    pub updating: bool,
    /// The killmails of the last day, from the query API. Set by the source
    /// handle, not by the buffer.
    pub day: Option<DayLine>,
    pub source: Source,
    /// Whether killmails are still being read, so none yet is not "none".
    pub loading: bool,
}

/// The killmails of the last [`WINDOW`], by solar system.
#[derive(Debug, Default)]
pub(crate) struct KillBuffer {
    /// Oldest first.
    by_system: HashMap<usize, Vec<KillRecord>>,
    fill: Option<Fill>,
}

impl KillBuffer {
    /// Adds `record`, unless it is older than the window or already there
    /// (the feed republishes the killmails it reprocesses). `true` when it
    /// was added.
    pub(crate) fn insert(&mut self, record: KillRecord, now: DateTime<Utc>) -> bool {
        let Some(oldest) = now.checked_sub_signed(WINDOW) else {
            return false;
        };
        if record.time < oldest {
            return false;
        }
        let records = self.by_system.entry(record.system_id).or_default();
        if records
            .iter()
            .any(|known| known.killmail_id == record.killmail_id)
        {
            return false;
        }
        let at = records.partition_point(|known| known.time <= record.time);
        records.insert(at, record);
        true
    }

    /// Drops what is older than the window as of `now`.
    pub(crate) fn prune(&mut self, now: DateTime<Utc>) {
        let Some(oldest) = now.checked_sub_signed(WINDOW) else {
            return;
        };
        self.by_system.retain(|_, records| {
            records.retain(|record| record.time >= oldest);
            !records.is_empty()
        });
    }

    /// Forgets everything: the source stopped, and what it knew goes stale.
    pub(crate) fn clear(&mut self) {
        self.by_system.clear();
        self.fill = None;
    }

    /// Tells how far the reading of the hour before the start got, or that
    /// it is done (`None`).
    pub(crate) fn set_fill(&mut self, fill: Option<Fill>) {
        self.fill = fill;
    }

    /// How far the reading of the hour before the start got (`None` once it
    /// is done).
    pub(crate) fn fill(&self) -> Option<Fill> {
        self.fill
    }

    /// How many killmails are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_system.values().map(Vec::len).sum()
    }

    /// How many killmails each system has in the window as of `now`, for the
    /// heat of the maps. Systems with none are left out.
    pub(crate) fn counts(&self, now: DateTime<Utc>) -> Vec<(usize, usize)> {
        let oldest = now.checked_sub_signed(WINDOW);
        self.by_system
            .iter()
            .filter_map(|(system, records)| {
                let kept = records
                    .iter()
                    .filter(|record| oldest.is_none_or(|oldest| record.time >= oldest))
                    .count();
                (kept > 0).then_some((*system, kept))
            })
            .collect()
    }

    /// How many killmails each system has that happened after `since` and
    /// are still in the window as of `now`: what the buffer adds to the
    /// answer of ESI computed at `since`.
    pub(crate) fn counts_since(
        &self,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Vec<(usize, usize)> {
        let oldest = now.checked_sub_signed(WINDOW);
        self.by_system
            .iter()
            .filter_map(|(system, records)| {
                let kept = records
                    .iter()
                    .filter(|record| {
                        record.time > since && oldest.is_none_or(|oldest| record.time >= oldest)
                    })
                    .count();
                (kept > 0).then_some((*system, kept))
            })
            .collect()
    }

    /// The killmails of `system_id` as of `now`.
    pub(crate) fn context(&self, system_id: usize, now: DateTime<Utc>) -> KillContext {
        let oldest = now.checked_sub_signed(WINDOW);
        let recent_from = now.checked_sub_signed(RECENT);
        let records: Vec<&KillRecord> = self
            .by_system
            .get(&system_id)
            .map(|records| {
                records
                    .iter()
                    .filter(|record| oldest.is_none_or(|oldest| record.time >= oldest))
                    .collect()
            })
            .unwrap_or_default();
        let is_recent = |record: &KillRecord| recent_from.is_none_or(|from| record.time >= from);
        let recent = records.iter().filter(|record| is_recent(record)).count();
        let verdict = if records.is_empty() {
            Verdict::Quiet
        } else {
            Verdict::Active
        };
        let shown = records.last();
        let mut ships: Vec<(String, usize)> = Vec::new();
        if let Some(record) = shown {
            for ship in &record.attacker_ships {
                match ships.iter_mut().find(|(name, _)| name == ship) {
                    Some((_, times)) => *times += 1,
                    None => ships.push((ship.clone(), 1)),
                }
            }
        }
        let mut victims: Vec<String> = Vec::new();
        for record in records.iter().rev() {
            if let Some(victim) = &record.victim
                && !victims.contains(victim)
                && victims.len() < MAX_VICTIMS
            {
                victims.push(victim.clone());
            }
        }
        KillContext {
            verdict,
            recent,
            hour: records.len(),
            last_age: records
                .last()
                .map(|record| (now - record.time).max(TimeDelta::zero())),
            ships,
            pilots: shown.map_or(0, |record| record.pilots),
            victims,
            tags: shown.map_or_else(KillTags::default, |record| record.tags),
            updating: self.fill.is_some(),
            day: None,
            source: Source::Feed,
            loading: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: usize = 30002813;

    fn at(now: DateTime<Utc>, minutes_ago: u64) -> DateTime<Utc> {
        now - TimeDelta::minutes(i64::try_from(minutes_ago).unwrap())
    }

    fn kill(id: u64, time: DateTime<Utc>, ships: &[&str], victim: &str) -> KillRecord {
        KillRecord {
            killmail_id: id,
            time,
            system_id: SYSTEM,
            victim: Some(victim.to_string()),
            attacker_ships: ships.iter().map(|ship| ship.to_string()).collect(),
            pilots: u32::try_from(ships.len()).unwrap(),
            tags: KillTags::default(),
            hash: String::new(),
            location_id: None,
        }
    }

    #[test]
    fn counts_give_the_killmails_of_the_hour_per_system() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        assert!(buffer.counts(now).is_empty());
        buffer.insert(kill(1, at(now, 5), &["Sabre"], "Catalyst"), now);
        buffer.insert(kill(2, at(now, 40), &["Sabre"], "Catalyst"), now);
        let mut other = kill(3, at(now, 10), &["Sabre"], "Catalyst");
        other.system_id = SYSTEM + 1;
        buffer.insert(other, now);
        let mut counts = buffer.counts(now);
        counts.sort_unstable();
        assert_eq!(counts, [(SYSTEM, 2), (SYSTEM + 1, 1)]);
        // Seen from later, the oldest one has left the hour.
        let later = now + TimeDelta::minutes(30);
        let mut counts = buffer.counts(later);
        counts.sort_unstable();
        assert_eq!(counts, [(SYSTEM, 1), (SYSTEM + 1, 1)]);
    }

    #[test]
    fn counts_since_leave_out_what_an_hourly_answer_already_has() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(kill(1, at(now, 50), &["Sabre"], "Catalyst"), now);
        buffer.insert(kill(2, at(now, 20), &["Sabre"], "Catalyst"), now);
        buffer.insert(kill(3, at(now, 5), &["Sabre"], "Catalyst"), now);
        // An answer computed 10 minutes ago has the first two.
        let since = now - TimeDelta::minutes(10);
        assert_eq!(buffer.counts_since(since, now), [(SYSTEM, 1)]);
        // An answer older than the buffer's hour adds the whole hour.
        let since = now - TimeDelta::hours(3);
        assert_eq!(buffer.counts_since(since, now), [(SYSTEM, 3)]);
    }

    #[test]
    fn a_system_without_killmails_is_quiet() {
        let now = Utc::now();
        let context = KillBuffer::default().context(SYSTEM, now);
        assert_eq!(context.verdict, Verdict::Quiet);
        assert_eq!((context.recent, context.hour), (0, 0));
        assert_eq!(context.last_age, None);
        assert!(context.ships.is_empty());
    }

    #[test]
    fn an_old_killmail_does_not_corroborate_but_still_counts_in_the_hour() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(kill(1, at(now, 40), &["Sabre"], "Catalyst"), now);
        let context = buffer.context(SYSTEM, now);
        assert_eq!(context.verdict, Verdict::Active);
        assert_eq!((context.recent, context.hour), (0, 1));
    }

    #[test]
    fn what_is_older_than_the_window_is_not_kept() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        assert!(!buffer.insert(kill(1, at(now, 61), &["Sabre"], "Catalyst"), now));
        assert!(buffer.insert(kill(2, at(now, 59), &["Sabre"], "Catalyst"), now));
        assert_eq!(buffer.len(), 1);
        // An hour later it has expired.
        buffer.prune(now + TimeDelta::seconds(2 * 60));
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn a_killmail_is_kept_once_and_in_order() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        assert!(buffer.insert(kill(2, at(now, 5), &["Sabre"], "B"), now));
        assert!(!buffer.insert(kill(2, at(now, 5), &["Sabre"], "B"), now));
        // Read afterwards but older (the hour before the start).
        assert!(buffer.insert(kill(1, at(now, 20), &["Rifter"], "A"), now));
        let context = buffer.context(SYSTEM, now);
        assert_eq!(context.hour, 2);
        // Newest first.
        assert_eq!(context.victims, ["B", "A"]);
        assert_eq!(context.last_age, Some(TimeDelta::seconds(5 * 60)));
    }

    #[test]
    fn the_victims_are_listed_without_repeats_and_no_more_than_three() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        for (id, victim) in ["A", "B", "A", "C", "D"].into_iter().enumerate() {
            let id = u64::try_from(id).unwrap() + 1;
            buffer.insert(kill(id, at(now, 30 - id), &["Sabre"], victim), now);
        }
        let context = buffer.context(SYSTEM, now);
        assert_eq!(context.victims, ["D", "C", "A"]);
    }

    #[test]
    fn the_tags_of_the_killmail_shown_are_passed_on() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        let mut tagged = kill(1, at(now, 5), &["Sabre"], "A");
        tagged.tags = KillTags {
            awox: true,
            ganked: false,
        };
        buffer.insert(tagged.clone(), now);
        let context = buffer.context(SYSTEM, now);
        assert_eq!(context.tags, tagged.tags);
        let plain = KillBuffer::default().context(SYSTEM, now);
        assert_eq!(plain.tags, KillTags::default());
    }

    #[test]
    fn the_fill_is_kept_and_cleared_with_the_buffer() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.set_fill(Some(Fill {
            done: 62,
            total: 100,
        }));
        assert_eq!(buffer.fill().map(Fill::percent), Some(62));
        assert!(buffer.context(SYSTEM, now).updating);
        buffer.clear();
        assert_eq!(buffer.fill(), None);
        assert!(!buffer.context(SYSTEM, now).updating);
        assert_eq!(Fill::default().percent(), 100);
    }
}
