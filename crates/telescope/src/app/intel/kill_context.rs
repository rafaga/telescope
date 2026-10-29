//! What the killmails the zKillboard source has read say about a system,
//! for the node tooltips.
//!
//! A chat report ("Tama +3 Sabre Rifter") is a human's word. The killmails
//! of the last hour are facts: [`KillBuffer`] keeps them by system, and
//! [`KillBuffer::context`] sets a report against them: is something being
//! shot in that system right now, and do the ships of the killers match the
//! ones reported?
//!
//! The buffer only holds the last [`WINDOW`]: the feed is read live, and the
//! hour before the source started is read backwards from the feed's files
//! (see `Backfill` in the zKillboard source). Anything older is not here.

use super::kill_day::DayLine;
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::HashMap;
use webb::map_alerts::AlertSummary;

/// How far back the killmails are kept.
pub(crate) const WINDOW: TimeDelta = TimeDelta::seconds(60 * 60);

/// Killmails this recent can confirm a report.
pub(crate) const RECENT: TimeDelta = TimeDelta::seconds(15 * 60);

/// The most victim ships listed.
const MAX_VICTIMS: usize = 3;

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

/// What a report names, to set against the killmails.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Ship names, lowercase.
    pub ships: Vec<String>,
    /// How many pilots it names.
    pub pilots: Option<u32>,
}

impl Report {
    /// What the summary of an intel alert names.
    pub(crate) fn from_summary(summary: &AlertSummary) -> Self {
        Self {
            ships: summary
                .ships
                .iter()
                .map(|(name, _)| name.to_lowercase())
                .collect(),
            pilots: summary.count,
        }
    }

    /// Whether `record` agrees with the report: one of the ships the
    /// attackers flew is named, or, when the report names no ship, the
    /// number of pilots is the same.
    fn matches(&self, record: &KillRecord) -> bool {
        if self.ships.is_empty() {
            return self.pilots.is_some_and(|pilots| pilots == record.pilots);
        }
        record
            .attacker_ships
            .iter()
            .any(|ship| self.ships.contains(&ship.to_lowercase()))
    }
}

/// What the killmails say about a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// A killmail of the last [`RECENT`] agrees with the report.
    Corroborated,
    /// Ships have died in the system in the last [`WINDOW`], none that
    /// agrees with the report.
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

/// Which neighbours a tooltip lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NeighbourConfig {
    /// How far from a system the neighbours are looked at, in jumps.
    pub jumps: u8,
    /// The most neighbours listed.
    pub max: usize,
}

impl Default for NeighbourConfig {
    fn default() -> Self {
        Self { jumps: 2, max: 3 }
    }
}

/// A system close to the one the context is about, with its killmails of the
/// last [`WINDOW`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Neighbour {
    pub system_id: usize,
    /// Its name, set by the source handle (empty when unknown).
    pub name: String,
    /// Stargate jumps from the system.
    pub jumps: u8,
    pub kills: usize,
    /// How long ago the last one was.
    pub last_age: TimeDelta,
}

/// The killmails of a system, set against a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillContext {
    pub verdict: Verdict,
    /// Killmails in the last [`RECENT`].
    pub recent: usize,
    /// Killmails in the last [`WINDOW`].
    pub hour: usize,
    /// How long ago the last one was.
    pub last_age: Option<TimeDelta>,
    /// The ships of the attackers of the killmail that agrees with the
    /// report, or else of the last one, with how many flew each.
    pub ships: Vec<(String, usize)>,
    /// How many pilots that killmail had.
    pub pilots: u32,
    /// The ships that died, newest first, without repeats.
    pub victims: Vec<String>,
    /// While the hour before the source started is still being read.
    pub fill: Option<Fill>,
    /// The killmails of the last day, from the query API. Set by the source
    /// handle, not by the buffer.
    pub day: Option<DayLine>,
    /// Systems within [`NeighbourConfig::jumps`] jumps with killmails in the last
    /// [`WINDOW`], nearest first. Set by the source handle, not by the buffer.
    pub neighbours: Vec<Neighbour>,
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

    /// How many killmails are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_system.values().map(Vec::len).sum()
    }

    /// Which of the systems `near` (id, jumps from the system of interest)
    /// have killmails as of `now`: nearest first, then the busiest, at most
    /// [`NeighbourConfig::max`]. The names are left empty.
    pub(crate) fn neighbours(
        &self,
        near: &[(u32, u8)],
        max: usize,
        now: DateTime<Utc>,
    ) -> Vec<Neighbour> {
        let oldest = now.checked_sub_signed(WINDOW);
        let mut found: Vec<Neighbour> = near
            .iter()
            .filter_map(|&(id, jumps)| {
                let system_id = usize::try_from(id).ok()?;
                let records: Vec<&KillRecord> = self
                    .by_system
                    .get(&system_id)?
                    .iter()
                    .filter(|record| oldest.is_none_or(|oldest| record.time >= oldest))
                    .collect();
                let last = records.last()?;
                Some(Neighbour {
                    system_id,
                    name: String::new(),
                    jumps,
                    kills: records.len(),
                    last_age: (now - last.time).max(TimeDelta::zero()),
                })
            })
            .collect();
        found.sort_by(|a, b| a.jumps.cmp(&b.jumps).then(b.kills.cmp(&a.kills)));
        found.truncate(max);
        found
    }

    /// The killmails of `system_id` as of `now`, set against `report`.
    pub(crate) fn context(
        &self,
        system_id: usize,
        report: &Report,
        now: DateTime<Utc>,
    ) -> KillContext {
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
        let confirming = records
            .iter()
            .rev()
            .find(|record| is_recent(record) && report.matches(record));
        let verdict = match (confirming, records.is_empty()) {
            (Some(_), _) => Verdict::Corroborated,
            (None, false) => Verdict::Active,
            (None, true) => Verdict::Quiet,
        };
        let shown = confirming.or_else(|| records.last());
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
            fill: self.fill,
            day: None,
            neighbours: Vec::new(),
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
        }
    }

    fn report(ships: &[&str], pilots: Option<u32>) -> Report {
        Report {
            ships: ships.iter().map(|ship| ship.to_string()).collect(),
            pilots,
        }
    }

    #[test]
    fn a_system_without_killmails_is_quiet() {
        let now = Utc::now();
        let context = KillBuffer::default().context(SYSTEM, &report(&["sabre"], None), now);
        assert_eq!(context.verdict, Verdict::Quiet);
        assert_eq!((context.recent, context.hour), (0, 0));
        assert_eq!(context.last_age, None);
        assert!(context.ships.is_empty());
    }

    #[test]
    fn a_recent_killmail_with_the_reported_ship_corroborates_the_report() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(
            kill(1, at(now, 3), &["Sabre", "Rifter", "Rifter"], "Catalyst"),
            now,
        );
        let context = buffer.context(SYSTEM, &report(&["sabre"], Some(3)), now);
        assert_eq!(context.verdict, Verdict::Corroborated);
        assert_eq!((context.recent, context.hour), (1, 1));
        assert_eq!(
            context.ships,
            [(String::from("Sabre"), 1), (String::from("Rifter"), 2)]
        );
        assert_eq!(context.pilots, 3);
        assert_eq!(context.victims, ["Catalyst"]);
        assert_eq!(context.last_age, Some(TimeDelta::seconds(180)));
    }

    #[test]
    fn other_ships_make_the_system_active_but_do_not_corroborate() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(kill(1, at(now, 3), &["Merlin", "Merlin"], "Venture"), now);
        let context = buffer.context(SYSTEM, &report(&["sabre"], Some(2)), now);
        assert_eq!(context.verdict, Verdict::Active);
        // The last killmail is shown instead.
        assert_eq!(context.ships, [(String::from("Merlin"), 2)]);
    }

    #[test]
    fn without_ships_the_number_of_pilots_decides() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(
            kill(1, at(now, 2), &["Merlin", "Merlin", "Merlin"], "Venture"),
            now,
        );
        assert_eq!(
            buffer.context(SYSTEM, &report(&[], Some(3)), now).verdict,
            Verdict::Corroborated
        );
        assert_eq!(
            buffer.context(SYSTEM, &report(&[], Some(2)), now).verdict,
            Verdict::Active
        );
        // A report that names nothing can't be corroborated.
        assert_eq!(
            buffer.context(SYSTEM, &report(&[], None), now).verdict,
            Verdict::Active
        );
    }

    #[test]
    fn an_old_killmail_does_not_corroborate_but_still_counts_in_the_hour() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.insert(kill(1, at(now, 40), &["Sabre"], "Catalyst"), now);
        let context = buffer.context(SYSTEM, &report(&["sabre"], None), now);
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
        let context = buffer.context(SYSTEM, &report(&[], None), now);
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
        let context = buffer.context(SYSTEM, &report(&[], None), now);
        assert_eq!(context.victims, ["D", "C", "A"]);
    }

    #[test]
    fn the_fill_is_passed_on_and_cleared_with_the_buffer() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        buffer.set_fill(Some(Fill {
            done: 62,
            total: 100,
        }));
        let context = buffer.context(SYSTEM, &report(&[], None), now);
        assert_eq!(context.fill.map(Fill::percent), Some(62));
        buffer.clear();
        assert_eq!(buffer.context(SYSTEM, &report(&[], None), now).fill, None);
        assert_eq!(Fill::default().percent(), 100);
    }

    #[test]
    fn neighbours_are_the_near_systems_with_kills() {
        let now = Utc::now();
        let mut buffer = KillBuffer::default();
        let mut other = |id: u64, system: usize, minutes: u64| {
            let mut record = kill(id, at(now, minutes), &["Rifter"], "Merlin");
            record.system_id = system;
            assert!(buffer.insert(record, now));
        };
        other(1, 100, 30);
        other(2, 100, 10);
        other(3, 200, 5);
        other(4, 300, 1);
        let found = buffer.neighbours(&[(100, 2), (200, 1), (300, 2), (400, 1)], 3, now);
        let ids: Vec<usize> = found.iter().map(|n| n.system_id).collect();
        assert_eq!(ids, vec![200, 100, 300]);
        assert_eq!(found[1].kills, 2);
        assert_eq!(found[1].last_age, TimeDelta::seconds(10 * 60));
        assert!(buffer.neighbours(&[], 3, now).is_empty());
        // Only as many as asked for, the nearest first.
        let one = buffer.neighbours(&[(100, 2), (200, 1), (300, 2)], 1, now);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].system_id, 200);
    }
}
