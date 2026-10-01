//! Intel alerts listed in the map node tooltips.
//!
//! When an intel line raises a map alert, [`AlertSummary::from_messages`]
//! condenses it into the few words the tooltip shows after the icon and the
//! age of the report: the ships reported, how many pilots, or -- when the
//! line has neither -- its leftover text, which is usually the pilot names.
//! What each detection contributes is set by its category (see
//! [`IntelCategory`]). A `clear` report is listed with a check mark.
//!
//! Every map pane keeps an [`AlertLog`]. Each entry lives as long as its own
//! visual alert would (received + the alert duration from Settings), no
//! matter what newer alerts on the same system do: a newer alert restarts
//! the node's animation, so the node keeps pulsing while any of its entries
//! is still listed.

use crate::graph::{Data, Mensaje};
use crate::intel::{IntelCategory, sanitize_display};
use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

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
    /// whitespace collapsed.
    pub leftover: String,
}

impl AlertSummary {
    /// Condenses the messages of one input line into what the tooltip shows.
    pub fn from_messages(messages: &[&Mensaje]) -> Self {
        let mut summary = Self::default();
        for message in messages {
            match &message.data {
                Data::Ships(ships) => {
                    for (name, times) in ships {
                        for _ in 0..*times {
                            summary.add_ship(name);
                        }
                    }
                }
                Data::Count(count) => {
                    if summary.count.is_none() {
                        summary.count = Some(*count);
                    }
                }
                Data::Words(words) => {
                    if message.category == Some(IntelCategory::Clear) && summary.clear.is_none() {
                        summary.clear = words.first().cloned();
                    }
                }
                Data::Systems(_) | Data::Text(_) => {}
            }
        }
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

    /// The text shown after the icon and the age, split into sections so the
    /// caller can add an emoji and a color per kind. `pilots` words the count,
    /// so it can be localized.
    pub fn parts(&self, pilots: impl Fn(u32) -> String) -> Vec<AlertPart> {
        let mut parts = Vec::new();
        if let Some(word) = &self.clear {
            parts.push(AlertPart::Clear(word.clone()));
        }
        if !self.ships.is_empty() {
            let ships: Vec<String> = self
                .ships
                .iter()
                .map(|(name, times)| match times {
                    1 => name.clone(),
                    _ => format!("{name} ×{times}"),
                })
                .collect();
            parts.push(AlertPart::Ships(ships.join(", ")));
        }
        if let Some(count) = self.count {
            parts.push(AlertPart::Count(pilots(count)));
        }
        if parts.is_empty() && !self.leftover.is_empty() {
            parts.push(AlertPart::Text(self.leftover.clone()));
        }
        parts
    }
}

/// One section of a tooltip line, so the renderer can prefix it with its own
/// emoji and color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertPart {
    /// The `clear` word.
    Clear(String),
    /// Ship names, grouped and joined.
    Ships(String),
    /// The pilot count, already worded.
    Count(String),
    /// The leftover text.
    Text(String),
}

/// What is left of `line` once every span of `messages` is removed (the
/// reported system, the ships, the count, keywords such as `nv`...): usually
/// the pilot names. Whitespace is collapsed, punctuation trimmed from both
/// ends, and the result cut to 40 characters (ellipsis included).
///
/// Messages that did not read the line itself carry no spans and remove
/// nothing.
pub fn leftover(line: &str, messages: &[&Mensaje]) -> String {
    let mut spans: Vec<Range<usize>> = messages
        .iter()
        .flat_map(|message| message.spans.iter().cloned())
        .filter(|span| {
            span.end <= line.len()
                && line.is_char_boundary(span.start)
                && line.is_char_boundary(span.end)
        })
        .collect();
    spans.sort_by_key(|span| span.start);
    let mut kept = String::with_capacity(line.len());
    let mut position = 0;
    for span in spans {
        if span.start > position {
            kept.push_str(&line[position..span.start]);
        }
        kept.push(' ');
        position = position.max(span.end);
    }
    if position < line.len() {
        kept.push_str(&line[position..]);
    }
    let joined = kept.split_whitespace().collect::<Vec<_>>().join(" ");
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
    /// How long its visual alert lasts (Settings -> Alerts).
    pub duration: Duration,
    /// The line's text normalized (lowercase, single spaces): the same
    /// report read twice (from two channels, or repeated) replaces the
    /// earlier entry instead of adding a second one.
    pub key: String,
    pub summary: AlertSummary,
    /// Whether the tooltip line prefixes each section with its emoji.
    pub emojis: bool,
}

impl IntelAlert {
    pub fn new(
        system_id: usize,
        received: Instant,
        duration: Duration,
        text: &str,
        summary: AlertSummary,
        emojis: bool,
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
            emojis,
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

    /// A message standing for the bytes `start..end` of the line.
    fn message(start: usize, end: usize) -> Mensaje {
        Mensaje {
            tag: String::from("keyword"),
            text: String::new(),
            data: Data::Words(Vec::new()),
            spans: std::iter::once(start..end).collect(),
            category: None,
        }
    }

    #[test]
    fn the_leftover_is_the_line_without_the_spans() {
        let line = "H-5GUI*  Floris Saucus  nv";
        let system = message(0, 6);
        let nv = message(24, 26);
        assert_eq!(leftover(line, &[&system, &nv]), "Floris Saucus");
        // Without spans nothing is removed.
        assert_eq!(leftover(line, &[]), line.replace("  ", " "));
    }

    #[test]
    fn the_leftover_is_cut_to_40_characters() {
        let line = "H-5GUI Aaron Bartholomew Cornelius Dmitri Evangeline";
        let text = leftover(line, &[&message(0, 6)]);
        assert!(text.chars().count() <= MAX_LEFTOVER_CHARS, "{text}");
        assert!(
            text.starts_with("Aaron Bartholomew") && text.ends_with('…'),
            "{text}"
        );
    }

    #[test]
    fn spans_outside_the_line_are_ignored() {
        assert_eq!(leftover("abc", &[&message(1, 99)]), "abc");
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
            true,
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

    fn with_data(data: Data, category: Option<IntelCategory>) -> Mensaje {
        Mensaje {
            tag: String::from("test"),
            text: String::new(),
            data,
            spans: Vec::new(),
            category,
        }
    }

    fn ships(list: &[(&str, u32)]) -> Mensaje {
        with_data(
            Data::Ships(
                list.iter()
                    .map(|(name, times)| (name.to_string(), *times))
                    .collect(),
            ),
            Some(IntelCategory::Ship),
        )
    }

    fn pilots(count: u32) -> String {
        format!("{count} pilots")
    }

    #[test]
    fn a_line_with_nothing_categorized_has_an_empty_summary() {
        let system = with_data(Data::Systems(vec![30000142]), None);
        let text = with_data(Data::Text(String::from("hi")), None);
        let summary = AlertSummary::from_messages(&[&system, &text]);
        assert_eq!(summary, AlertSummary::default());
        assert!(!summary.is_clear());
        assert!(summary.parts(pilots).is_empty());
    }

    #[test]
    fn ships_are_grouped_ignoring_case_in_order_of_appearance() {
        let first = ships(&[("Rifter", 2), ("Loki", 1)]);
        let second = ships(&[("rifter", 1)]);
        let summary = AlertSummary::from_messages(&[&first, &second]);
        assert_eq!(
            summary.ships,
            vec![(String::from("Rifter"), 3), (String::from("Loki"), 1)]
        );
    }

    #[test]
    fn the_first_count_wins() {
        let a = with_data(Data::Count(4), Some(IntelCategory::Count));
        let b = with_data(Data::Count(9), Some(IntelCategory::Count));
        assert_eq!(AlertSummary::from_messages(&[&a, &b]).count, Some(4));
    }

    #[test]
    fn only_a_clear_category_marks_the_line_clear_and_the_first_word_is_kept() {
        let clear = with_data(
            Data::Words(vec![String::from("clr"), String::from("clear")]),
            Some(IntelCategory::Clear),
        );
        let later = with_data(
            Data::Words(vec![String::from("clear")]),
            Some(IntelCategory::Clear),
        );
        let summary = AlertSummary::from_messages(&[&clear, &later]);
        assert_eq!(summary.clear.as_deref(), Some("clr"));
        assert!(summary.is_clear());

        // Other words (keywords, queries) do not make a line clear.
        let keyword = with_data(
            Data::Words(vec![String::from("nv")]),
            Some(IntelCategory::Keyword),
        );
        let query = with_data(
            Data::Words(vec![String::from("status")]),
            Some(IntelCategory::Query),
        );
        let summary = AlertSummary::from_messages(&[&keyword, &query]);
        assert!(!summary.is_clear());
    }

    #[test]
    fn the_parts_come_in_order_and_word_the_count_and_repeats() {
        let summary = AlertSummary {
            clear: Some(String::from("clr")),
            ships: vec![(String::from("Rifter"), 3), (String::from("Loki"), 1)],
            count: Some(5),
            leftover: String::from("ignored while there are other parts"),
        };
        assert_eq!(
            summary.parts(pilots),
            vec![
                AlertPart::Clear(String::from("clr")),
                AlertPart::Ships(String::from("Rifter ×3, Loki")),
                AlertPart::Count(String::from("5 pilots")),
            ]
        );
    }

    #[test]
    fn the_leftover_text_is_shown_only_when_nothing_else_is() {
        let only_text = AlertSummary {
            leftover: String::from("Floris Saucus"),
            ..AlertSummary::default()
        };
        assert_eq!(
            only_text.parts(pilots),
            vec![AlertPart::Text(String::from("Floris Saucus"))]
        );
    }
}
