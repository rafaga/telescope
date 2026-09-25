//! Intel alerts listed in the map node tooltips.
//!
//! When an intel line raises a map alert, [`AlertSummary::from_line`]
//! condenses it into the few words the tooltip shows after the icon and the
//! age of the report: the ships reported, how many pilots, or -- when the
//! line has neither -- its leftover text, which is usually the pilot names.
//! What each rule contributes is set by its `category` in `patterns.toml`
//! (see [`IntelCategory`]). A `clear` report is listed with a check mark and
//! raises no visual alert.
//!
//! Every map pane keeps an [`AlertLog`]. Each entry lives as long as its own
//! visual alert would (received + the alert duration from Settings), no
//! matter what newer alerts on the same system do: a newer alert restarts
//! the node's animation, so the node keeps pulsing while any of its entries
//! is still listed.

use crate::patterns::{COUNT_GROUP, IntelCategory, PatternMatch, sanitize_display};
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

/// Icon of an alert line in the tooltip (painted red).
pub const ALERT_ICON: &str = "🔥";
/// Icon of a `clear` report in the tooltip (painted green).
pub const CLEAR_ICON: &str = "✔";
/// Most alert lines a tooltip lists; the rest are summed up as "+N more".
pub const MAX_TOOLTIP_ALERTS: usize = 5;
/// Longest leftover text shown, in characters (ellipsis included).
const MAX_LEFTOVER_CHARS: usize = 40;
/// Most entries kept per system. A channel flooding one system can't grow
/// the log without bound before the entries expire.
const MAX_ALERTS_PER_SYSTEM: usize = 20;

/// What the tooltip shows about one intel line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlertSummary {
    /// The word of a `clear` report, as typed (`clr`, `clear`, ...).
    pub clear: Option<String>,
    /// Ship names in order of appearance, with how many times each was
    /// named (compared ignoring ASCII case).
    pub ships: Vec<(String, usize)>,
    /// Number of pilots reported (first `count` match of the line).
    pub count: Option<u32>,
    /// The line without the reported system and the categorized matches,
    /// whitespace collapsed and cut to [`MAX_LEFTOVER_CHARS`].
    pub leftover: String,
}

impl AlertSummary {
    /// Condenses `text` (the payload of an intel line) using `matches`, the
    /// pattern matches of that same line. `system_spans` are the byte ranges
    /// of the candidates that resolved to a real system: only those are
    /// dropped from the leftover text, so a pilot name that merely fit the
    /// system pattern stays.
    pub fn from_line(
        text: &str,
        matches: &[&PatternMatch],
        system_spans: Vec<Range<usize>>,
    ) -> Self {
        let mut summary = Self::default();
        let mut spans = system_spans;
        for intel_match in matches {
            let Some(category) = intel_match.category else {
                continue;
            };
            spans.push(intel_match.span.clone());
            match category {
                IntelCategory::Ship => summary.add_ship(&intel_match.matched),
                IntelCategory::Count => {
                    if summary.count.is_none() {
                        summary.count = intel_match
                            .named
                            .get(COUNT_GROUP)
                            .and_then(|count| count.parse().ok());
                    }
                }
                IntelCategory::Clear => {
                    if summary.clear.is_none() {
                        summary.clear = Some(intel_match.matched.clone());
                    }
                }
                // `Query` lines never get here: `parse_intel_data` drops them.
                IntelCategory::Keyword | IntelCategory::Query => {}
            }
        }
        summary.leftover = leftover(text, spans);
        summary
    }

    fn add_ship(&mut self, name: &str) {
        match self
            .ships
            .iter_mut()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
        {
            Some((_, times)) => *times += 1,
            None => self.ships.push((name.to_owned(), 1)),
        }
    }

    /// Whether the line reports the system clear (no visual alert, no
    /// sound).
    pub fn is_clear(&self) -> bool {
        self.clear.is_some()
    }

    /// The text shown after the icon and the age: the clear word; else the
    /// ships and the pilot count (`pilots` words it, so it can be
    /// localized); else the leftover text. May be empty.
    pub fn detail(&self, pilots: impl Fn(u32) -> String) -> String {
        if let Some(word) = &self.clear {
            return word.clone();
        }
        let mut parts = Vec::new();
        if !self.ships.is_empty() {
            let ships: Vec<String> = self
                .ships
                .iter()
                .map(|(name, times)| match times {
                    1 => name.clone(),
                    _ => format!("{name} ×{times}"),
                })
                .collect();
            parts.push(ships.join(", "));
        }
        if let Some(count) = self.count {
            parts.push(pilots(count));
        }
        if parts.is_empty() {
            self.leftover.clone()
        } else {
            parts.join(" · ")
        }
    }
}

/// `text` without the byte ranges in `spans`, whitespace collapsed,
/// punctuation trimmed from both ends and cut to [`MAX_LEFTOVER_CHARS`].
fn leftover(text: &str, mut spans: Vec<Range<usize>>) -> String {
    spans.sort_by_key(|span| span.start);
    let mut kept = String::with_capacity(text.len());
    let mut position = 0;
    for span in spans {
        if span.start > position {
            kept.push_str(&text[position..span.start]);
        }
        kept.push(' ');
        position = position.max(span.end);
    }
    if position < text.len() {
        kept.push_str(&text[position..]);
    }
    let words: Vec<&str> = kept.split_whitespace().collect();
    let joined = words.join(" ");
    let trimmed = joined.trim_matches(|c: char| !c.is_alphanumeric());
    let clean = sanitize_display(trimmed);
    if clean.chars().count() <= MAX_LEFTOVER_CHARS {
        return clean;
    }
    let mut cut: String = clean.chars().take(MAX_LEFTOVER_CHARS - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Whether a line (`matches` are all of its matches) asks about a system
/// instead of reporting it (`category = "query"`, e.g. `H-5GUI status?`).
/// Such a line raises no map alert at all: no visual alert, no sound, no
/// tooltip entry.
pub fn is_query(matches: &[&PatternMatch]) -> bool {
    matches
        .iter()
        .any(|intel_match| intel_match.category == Some(IntelCategory::Query))
}

/// How long ago something happened, condensed: `5s`, `4m`, `2h`.
pub fn format_age(age: Duration) -> String {
    let secs = age.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// An intel report on a solar system, as sent to the maps.
#[derive(Debug, Clone)]
pub struct IntelAlert {
    pub system_id: usize,
    /// When Telescope read the line.
    pub received: Instant,
    /// How long its visual alert lasts (Settings -> Intelligence).
    pub duration: Duration,
    /// The line's text normalized (lowercase, single spaces): the same
    /// report read twice (from two channels, or repeated) replaces the
    /// earlier entry instead of adding a second one.
    pub key: String,
    pub summary: AlertSummary,
}

impl IntelAlert {
    pub fn new(
        system_id: usize,
        received: Instant,
        duration: Duration,
        text: &str,
        summary: AlertSummary,
    ) -> Self {
        let key = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        Self {
            system_id,
            received,
            duration,
            key,
            summary,
        }
    }

    /// Whether the node pulses (and the alarm may sound): every report but a
    /// `clear` one.
    pub fn raises_visual(&self) -> bool {
        !self.summary.is_clear()
    }

    fn is_active(&self, now: Instant) -> bool {
        now < self.received + self.duration
    }
}

/// The alerts of a map, by solar system, for its node tooltips.
#[derive(Debug, Default)]
pub struct AlertLog {
    /// Oldest first.
    by_system: HashMap<usize, Vec<IntelAlert>>,
}

impl AlertLog {
    /// Adds `alert`, replacing a still active entry of the same report, and
    /// drops every expired entry (as of `alert.received`).
    pub fn push(&mut self, alert: IntelAlert) {
        let now = alert.received;
        self.by_system.retain(|_, alerts| {
            alerts.retain(|entry| entry.is_active(now));
            !alerts.is_empty()
        });
        let alerts = self.by_system.entry(alert.system_id).or_default();
        alerts.retain(|entry| entry.key != alert.key);
        alerts.push(alert);
        if alerts.len() > MAX_ALERTS_PER_SYSTEM {
            let excess = alerts.len() - MAX_ALERTS_PER_SYSTEM;
            alerts.drain(..excess);
        }
    }

    /// The entries of `system_id` still active at `now`, newest first.
    /// Expired ones are dropped.
    pub fn active(&mut self, system_id: usize, now: Instant) -> Vec<&IntelAlert> {
        let Some(alerts) = self.by_system.get_mut(&system_id) else {
            return Vec::new();
        };
        alerts.retain(|entry| entry.is_active(now));
        if alerts.is_empty() {
            self.by_system.remove(&system_id);
            return Vec::new();
        }
        self.by_system
            .get(&system_id)
            .map(|alerts| alerts.iter().rev().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patterns::{ActionConfig, template_engine};

    const CHANNEL: &str = "wc.Vale+Tribute";
    /// The systems these tests treat as real (resolving needs the SDE).
    const KNOWN_SYSTEMS: [&str; 2] = ["H-5GUI", "1DQ1-A"];

    /// The summary of `text` with the rules of the shipped `patterns.toml`,
    /// counting only [`KNOWN_SYSTEMS`] as resolved.
    fn summarize(text: &str) -> AlertSummary {
        let engine = template_engine();
        let matches = engine.evaluate(CHANNEL, &format!("[ 2023.04.03 18:02:00 ] Pilot > {text}"));
        let refs: Vec<&PatternMatch> = matches.iter().collect();
        let systems: Vec<Range<usize>> = refs
            .iter()
            .filter(|m| matches!(m.action, ActionConfig::MapAlert { .. }))
            .filter(|m| KNOWN_SYSTEMS.contains(&m.matched.as_str()))
            .map(|m| m.span.clone())
            .collect();
        assert!(!systems.is_empty(), "{text} names no known system");
        AlertSummary::from_line(text, &refs, systems)
    }

    fn detail(text: &str) -> String {
        summarize(text).detail(|count| format!("{count} pilots"))
    }

    #[test]
    fn a_sighting_shows_the_pilot_names() {
        assert_eq!(detail("H-5GUI*  Floris Saucus  nv"), "Floris Saucus");
        assert!(!summarize("H-5GUI*  Floris Saucus  nv").is_clear());
    }

    #[test]
    fn a_pilot_named_before_the_system_stays_in_the_text() {
        assert_eq!(detail("Floris Saucus H-5GUI nv"), "Floris Saucus");
        assert_eq!(detail("Floris Saucus 1DQ1-A H-5GUI"), "Floris Saucus");
    }

    #[test]
    fn ships_are_grouped_and_hide_the_leftover_text() {
        assert_eq!(
            detail("1DQ1-A Some Pilot Drake drake Caracal"),
            "Drake ×2, Caracal"
        );
    }

    #[test]
    fn the_pilot_count_goes_after_the_ships() {
        assert_eq!(detail("1DQ1-A +3 Sabre"), "Sabre · 3 pilots");
        assert_eq!(detail("1DQ1-A 4 neuts"), "4 pilots");
    }

    #[test]
    fn a_clear_report_shows_its_word_and_raises_no_visual() {
        let summary = summarize("H-5GUI clr");
        assert!(summary.is_clear());
        assert_eq!(summary.detail(|_| String::new()), "clr");
        let alert = IntelAlert::new(
            1,
            Instant::now(),
            Duration::from_secs(60),
            "H-5GUI clr",
            summary,
        );
        assert!(!alert.raises_visual());
    }

    #[test]
    fn a_status_question_is_a_query() {
        let engine = template_engine();
        let query = |text: &str| {
            let matches =
                engine.evaluate(CHANNEL, &format!("[ 2023.04.03 18:02:00 ] Pilot > {text}"));
            is_query(&matches.iter().collect::<Vec<_>>())
        };
        assert!(query("H-5GUI status?"));
        assert!(query("H-5GUI status ？"));
        assert!(!query("H-5GUI  Floris Saucus  nv"));
        assert!(!query("H-5GUI clr"));
    }

    #[test]
    fn a_bare_system_has_no_detail() {
        assert_eq!(detail("H-5GUI"), "");
    }

    #[test]
    fn the_leftover_text_is_cut_to_40_characters() {
        let text = detail("H-5GUI Aaron Bartholomew Cornelius Dmitri Evangeline");
        assert!(text.chars().count() <= MAX_LEFTOVER_CHARS, "{text}");
        assert!(
            text.starts_with("Aaron Bartholomew") && text.ends_with('…'),
            "{text}"
        );
    }

    #[test]
    fn ages_are_condensed() {
        assert_eq!(format_age(Duration::from_millis(5_900)), "5s");
        assert_eq!(format_age(Duration::from_secs(59)), "59s");
        assert_eq!(format_age(Duration::from_secs(60)), "1m");
        assert_eq!(format_age(Duration::from_secs(4 * 60 + 30)), "4m");
        assert_eq!(format_age(Duration::from_secs(7200)), "2h");
    }

    fn alert(system_id: usize, received: Instant, secs: u64, text: &str) -> IntelAlert {
        IntelAlert::new(
            system_id,
            received,
            Duration::from_secs(secs),
            text,
            AlertSummary::default(),
        )
    }

    #[test]
    fn each_entry_expires_on_its_own() {
        let start = Instant::now();
        let mut log = AlertLog::default();
        log.push(alert(7, start, 60, "first"));
        log.push(alert(7, start + Duration::from_secs(30), 60, "second"));

        let listed = |log: &mut AlertLog, at: u64| -> Vec<String> {
            log.active(7, start + Duration::from_secs(at))
                .into_iter()
                .map(|entry| entry.key.clone())
                .collect()
        };
        assert_eq!(listed(&mut log, 40), ["second", "first"]);
        assert_eq!(listed(&mut log, 60), ["second"]);
        assert!(listed(&mut log, 90).is_empty());
        assert!(log.by_system.is_empty());
    }

    #[test]
    fn the_same_report_replaces_its_entry() {
        let start = Instant::now();
        let mut log = AlertLog::default();
        log.push(alert(7, start, 60, "H-5GUI  Floris nv"));
        log.push(alert(
            7,
            start + Duration::from_secs(10),
            60,
            "h-5gui floris NV",
        ));
        let active = log.active(7, start + Duration::from_secs(65));
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].received, start + Duration::from_secs(10));
    }

    #[test]
    fn other_systems_and_old_entries_are_dropped() {
        let start = Instant::now();
        let mut log = AlertLog::default();
        log.push(alert(1, start, 10, "old"));
        log.push(alert(2, start + Duration::from_secs(20), 60, "new"));
        assert!(!log.by_system.contains_key(&1));
        for index in 0..MAX_ALERTS_PER_SYSTEM + 5 {
            log.push(alert(
                2,
                start + Duration::from_secs(20),
                60,
                &index.to_string(),
            ));
        }
        assert_eq!(
            log.active(2, start + Duration::from_secs(21)).len(),
            MAX_ALERTS_PER_SYSTEM
        );
    }
}
