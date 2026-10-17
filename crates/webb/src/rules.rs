//! Typed building blocks of the intel node graph.
//!
//! [`crate::graph`] holds the graph model and its executor; this module keeps
//! what they share:
//!
//! * [`InputKind`]: the kind of source an Input node reads.
//! * [`DetectionRuleKind`] with the [`DetectionType`] behaviour: how a
//!   Detection node matches and what it means.
//! * [`OutputKind`] with the [`OutputType`] behaviour: which queue an Output
//!   node feeds.
//! * [`Dictionaries`]: the built-in `dictionaries.toml` word lists.
//! * [`IntelLine`] and [`parse_line`]: the parsed chat-log line.

pub use crate::intel::IntelLine;
use crate::intel::{
    IntelCategory, LINE_PATTERN, LINE_TIMESTAMP_FORMAT, MAX_DICTIONARY_WORD_LEN,
    MAX_DICTIONARY_WORDS, MAX_LINE_LEN, MAX_PATTERN_LEN, PatternError, UNDATED_LINE_PATTERN,
    is_valid_group_name, truncate_str,
};
use chrono::NaiveDateTime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Kind of an input node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// An EVE Online chat-log file or directory of them (UTF-16LE).
    #[default]
    ChatLog,
    /// The live killmail feed of zKillboard (its R2Z2 stream). It has no path
    /// or channels: every killmail enters the graph as one line.
    #[serde(rename = "zkillboard")]
    ZKillboard,
}

impl InputKind {
    /// The channel name the lines of a source that has no chat channel
    /// carry, or `None` for a kind whose lines name their own channel.
    /// Shown in logs and in the debug tools.
    pub fn fixed_channel(self) -> Option<&'static str> {
        match self {
            Self::ChatLog => None,
            Self::ZKillboard => Some("zkillboard"),
        }
    }
}

/// Behaviour shared by every input type. Implement it for a new type to add
/// it to the model: there is more than one kind of source, not only chat-log
/// files.
pub trait InputType {
    /// A short name for the type, for logs and the GUI.
    fn type_name(&self) -> &'static str;
}

impl InputType for InputKind {
    fn type_name(&self) -> &'static str {
        match self {
            Self::ChatLog => "chat_log",
            Self::ZKillboard => "zkillboard",
        }
    }
}

/// The built-in word lists (default dictionaries), loaded from the embedded
/// `dictionaries.toml`. A [`DetectionRuleKind::ShipNames`] rule references
/// them by name; they are not editable from the graph editor.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Dictionaries(pub HashMap<String, DictionaryEntry>);

/// One dictionary of the built-in `dictionaries.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DictionaryEntry {
    /// The words matched whole-word.
    pub words: Vec<String>,
}

/// The embedded built-in dictionaries.
const DEFAULT_DICTIONARIES_TOML: &str = include_str!("../../../dictionaries.toml");

impl Dictionaries {
    /// The built-in dictionaries (parsed from the embedded `dictionaries.toml`).
    pub fn defaults() -> Self {
        Self::shared().clone()
    }

    /// The built-in dictionaries, parsed once and shared.
    pub fn shared() -> &'static Self {
        static SHARED: OnceLock<Dictionaries> = OnceLock::new();
        SHARED.get_or_init(|| {
            toml::from_str(DEFAULT_DICTIONARIES_TOML)
                .expect("the embedded dictionaries.toml must parse")
        })
    }

    /// The dictionary names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.0.keys().cloned().collect();
        names.sort();
        names
    }

    /// The words of the named dictionary, if it exists.
    pub fn words(&self, name: &str) -> Option<&[String]> {
        self.0.get(name).map(|entry| entry.words.as_slice())
    }

    /// Whether there is no dictionary at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The semantic type of a detection node. The type decides what the node
/// means (its category, whether it names a solar system) and which parameters
/// it needs.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DetectionRuleKind {
    /// Reports a solar system; it is highlighted on the maps. Systems are
    /// read whatever their case (`H-5GUI`, `h-5gui`, `JITA`, `old man star`):
    /// the code-like names (`J123456`, `1DQ1-A`, `AD001`) by the built-in
    /// pattern, the names made of words (`Jita`, `Old Man Star`) by trying the
    /// words of the line against the universe. The node's case-insensitive
    /// switch does not apply to it.
    SystemReport {
        /// Words that are never taken for a system, although the universe has
        /// a system of that name (`Van`, `Jan`, `Pain`: pilot names and
        /// everyday words). The case does not matter. Such a word still
        /// counts when it is marked as a report (`Van*`, `VAN`) or when it is
        /// the only candidate of a line that says `nv`, `clr` or a count.
        /// Code-like names (`H-5GUI`, `J123456`) are never ignored.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ignore_words: Vec<String>,
    },
    /// Reports the system clear.
    ClearReport {
        /// Words that mean "clear".
        keywords: Vec<String>,
    },
    /// Reports ship names: the words of the dictionaries it names, and the
    /// Chinese class names (the built-in shape regex), which need no
    /// dictionary. Graphs saved with the former `ship_names_zh` type load as
    /// this one with no dictionaries.
    #[serde(alias = "ship_names_zh")]
    ShipNames {
        /// Names of the dictionaries (in `dictionaries.toml`) to match.
        #[serde(default)]
        dictionaries: Vec<String>,
    },
    /// Reports how many pilots a line is about. Uses the built-in count
    /// patterns.
    PilotCount,
    /// A plain keyword report.
    Keyword {
        /// Words to match.
        keywords: Vec<String>,
    },
    /// A question about a system (no map alert).
    Query {
        /// Words that mark a question.
        keywords: Vec<String>,
    },
    /// A free-form regex or dictionary with an explicit category.
    Custom {
        /// Regex matched against the line payload.
        #[serde(default)]
        pattern: Option<String>,
        /// Words matched whole-word (used when `pattern` is `None`).
        #[serde(default)]
        words: Vec<String>,
        /// What the match contributes to the tooltip summary.
        #[serde(default)]
        category: Option<IntelCategory>,
        /// Capture group holding a reported solar system, if any.
        #[serde(default)]
        system_group: Option<String>,
    },
}

/// Built-in shape of the code-like solar-system names used by
/// [`DetectionRuleKind::SystemReport`], in any case: wormholes (`J123456`),
/// letters and digits around a dash (`1DQ1-A`) and the `AA000` shape. Its
/// capture group is `system`. The names made of words are not in the pattern:
/// once the case is ignored they cannot be told from other words by their
/// shape, so the executor looks them up instead (`graph::named_systems`).
pub(crate) const DEFAULT_SYSTEM_PATTERN: &str =
    r"(?i)\b(?P<system>J\d{6}|[A-Z0-9]{1,5}-[A-Z0-9]{1,4}|[A-Z]{2}\d{3})\b";

/// Built-in patterns used by [`DetectionRuleKind::PilotCount`]; each has a
/// `count` capture group. The counts are written `+5`, `x5`, `5 neuts` and
/// `5+` (the way most intel channels write them: the number, a plus, then
/// anything: `15+`, `30+kkmora`). A number needs a blank before it and no
/// digit after the plus, so `1+1` and the `5+` glued to a word (`ESS12+`) are
/// not counts.
const DEFAULT_COUNT_PATTERNS: [&str; 3] = [
    r"(?:^|\s)[+xX](?P<count>\d{1,3})\b",
    r"\b(?P<count>\d{1,3})\s?(?:x|neuts?|neutrals?|reds?|hostiles?|pilots?)\b",
    r"(?:^|\s)(?P<count>\d{1,3})\+(?:\D|$)",
];

/// Built-in shape used by [`DetectionRuleKind::ShipNames`]: a run of Han
/// characters ending in the `级` ("-class") suffix, the Chinese client's ship
/// name marker.
pub(crate) const DEFAULT_SHIP_ZH_PATTERN: &str = r"[一-鿿]{2,14}级";

/// The matcher a detection type compiles to.
pub enum DetectionMatcher {
    /// One or more regular expressions.
    Regex(Vec<String>),
    /// A flat list of whole-word literals.
    Dictionary(Vec<String>),
    /// Both at once: a match of any of the regexes or of any of the words.
    Mixed {
        /// The regular expressions.
        patterns: Vec<String>,
        /// The whole-word literals.
        words: Vec<String>,
    },
}

/// A fact of an [`IntelEvent`](crate::event::IntelEvent) a detection can read
/// instead of searching the text for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    /// The solar systems the event is about.
    Systems,
    /// The ships it involves.
    Ships,
    /// How many pilots it is about.
    Pilots,
}

/// Behaviour shared by every detection type. Implement it for a new type to
/// add it to the model: the graph, the database and the GUI all go through
/// this trait.
pub trait DetectionType {
    /// How the node matches text.
    fn matcher(&self, dictionaries: &Dictionaries) -> DetectionMatcher;
    /// What the match contributes to the tooltip summary.
    fn category(&self) -> Option<IntelCategory>;
    /// The capture group holding a reported solar system, if any.
    fn system_group(&self) -> Option<String> {
        None
    }
    /// The fact this type reads from an event when its source gives it,
    /// instead of matching the text. `None` for a type that only reads text.
    fn fact(&self) -> Option<FactKind> {
        None
    }
    /// A short name for the type, for logs and the GUI.
    fn type_name(&self) -> &'static str;
    /// Validates the type-specific parameters.
    fn validate(&self, id: &str, dictionaries: &Dictionaries) -> Result<(), PatternError>;
}

impl DetectionType for DetectionRuleKind {
    fn matcher(&self, dictionaries: &Dictionaries) -> DetectionMatcher {
        match self {
            Self::SystemReport { .. } => {
                DetectionMatcher::Regex(vec![String::from(DEFAULT_SYSTEM_PATTERN)])
            }
            Self::PilotCount => DetectionMatcher::Regex(
                DEFAULT_COUNT_PATTERNS
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect(),
            ),
            Self::ClearReport { keywords }
            | Self::Keyword { keywords }
            | Self::Query { keywords } => DetectionMatcher::Dictionary(keywords.clone()),
            Self::ShipNames {
                dictionaries: names,
            } => {
                let mut words = Vec::new();
                for name in names {
                    if let Some(dictionary) = dictionaries.words(name) {
                        words.extend(dictionary.iter().cloned());
                    }
                }
                DetectionMatcher::Mixed {
                    patterns: vec![String::from(DEFAULT_SHIP_ZH_PATTERN)],
                    words,
                }
            }
            Self::Custom { pattern, words, .. } => match pattern {
                Some(pattern) => DetectionMatcher::Regex(vec![pattern.clone()]),
                None => DetectionMatcher::Dictionary(words.clone()),
            },
        }
    }

    fn category(&self) -> Option<IntelCategory> {
        match self {
            Self::ClearReport { .. } => Some(IntelCategory::Clear),
            Self::ShipNames { .. } => Some(IntelCategory::Ship),
            Self::PilotCount => Some(IntelCategory::Count),
            Self::Keyword { .. } => Some(IntelCategory::Keyword),
            Self::Query { .. } => Some(IntelCategory::Query),
            Self::Custom { category, .. } => *category,
            _ => None,
        }
    }

    fn system_group(&self) -> Option<String> {
        match self {
            Self::SystemReport { .. } => Some(String::from("system")),
            Self::Custom { system_group, .. } => system_group.clone(),
            _ => None,
        }
    }

    fn fact(&self) -> Option<FactKind> {
        match self {
            Self::SystemReport { .. } => Some(FactKind::Systems),
            Self::ShipNames { .. } => Some(FactKind::Ships),
            Self::PilotCount => Some(FactKind::Pilots),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::SystemReport { .. } => "system_report",
            Self::ClearReport { .. } => "clear_report",
            Self::ShipNames { .. } => "ship_names",
            Self::PilotCount => "pilot_count",
            Self::Keyword { .. } => "keyword",
            Self::Query { .. } => "query",
            Self::Custom { .. } => "custom",
        }
    }

    fn validate(&self, id: &str, dictionaries: &Dictionaries) -> Result<(), PatternError> {
        match self {
            Self::PilotCount => {}
            Self::SystemReport { ignore_words } => validate_ignore_words(id, ignore_words)?,
            Self::ClearReport { keywords }
            | Self::Keyword { keywords }
            | Self::Query { keywords } => validate_words(id, keywords)?,
            Self::ShipNames {
                dictionaries: names,
            } => {
                // No dictionary is fine: the Chinese class names need none.
                for name in names {
                    if dictionaries.words(name).is_none() {
                        return Err(PatternError::UnknownDictionary {
                            id: id.to_string(),
                            name: name.clone(),
                        });
                    }
                }
            }
            Self::Custom {
                pattern,
                words,
                system_group,
                category,
            } => {
                match pattern {
                    Some(pattern) => validate_pattern(id, pattern)?,
                    None => validate_words(id, words)?,
                }
                // A word list has no capture groups: it can't carry a count
                // or name the group of a reported system. (A pattern's groups
                // are checked when it is compiled.)
                if pattern.is_none() {
                    if *category == Some(IntelCategory::Count) {
                        return Err(PatternError::MissingCountGroup(id.to_string()));
                    }
                    if let Some(group) = system_group {
                        return Err(PatternError::InvalidSystemGroup {
                            id: id.to_string(),
                            group: group.clone(),
                        });
                    }
                }
                if let Some(group) = system_group
                    && !is_valid_group_name(group)
                {
                    return Err(PatternError::InvalidSystemGroup {
                        id: id.to_string(),
                        group: group.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn validate_pattern(id: &str, pattern: &str) -> Result<(), PatternError> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_LEN {
        return Err(PatternError::PatternTooLong(id.to_string()));
    }
    Ok(())
}

fn validate_words(id: &str, words: &[String]) -> Result<(), PatternError> {
    if words.is_empty() || words.len() > MAX_DICTIONARY_WORDS {
        return Err(PatternError::InvalidDictionarySize(id.to_string()));
    }
    for word in words {
        if word.is_empty() || word.len() > MAX_DICTIONARY_WORD_LEN {
            return Err(PatternError::InvalidDictionaryWord {
                id: id.to_string(),
                word: word.clone(),
            });
        }
    }
    Ok(())
}

/// An ignore list may be empty; its words are checked like any dictionary's.
fn validate_ignore_words(id: &str, words: &[String]) -> Result<(), PatternError> {
    if words.is_empty() {
        return Ok(());
    }
    validate_words(id, words)
}

/// Which queue an Output node feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// Highlight the reported system on the maps.
    Visual,
    /// Play the alarm.
    Sound,
    /// Append a line to the status log.
    Log,
    /// Veto the visual and sound alerts.
    Suppress,
    /// List the line in the map node tooltips.
    Tooltip,
}

/// Behaviour shared by every output type. Implement it for a new type to add
/// it to the model: there is more than one kind of output.
pub trait OutputType {
    /// A short name for the type, for logs and the GUI.
    fn type_name(&self) -> &'static str;
}

impl OutputType for OutputKind {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Visual => "visual",
            Self::Sound => "sound",
            Self::Log => "log",
            Self::Suppress => "suppress",
            Self::Tooltip => "tooltip",
        }
    }
}

/// Parses a raw log line into an [`IntelLine`]. Returns `None` when the line
/// does not follow the EVE chat log format
/// (`[ yyyy.MM.dd hh:mm:ss ] Author > message`). Lines longer than 2 KiB are
/// cut first, so a flood of text can't make every detection scan it whole.
///
/// # Examples
///
/// ```
/// use webb::rules::parse_line;
///
/// let line = parse_line("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear").unwrap();
/// assert_eq!(line.author, "Some Pilot");
/// assert_eq!(line.text, "1DQ1-A clear");
/// assert!(parse_line("not a log line").is_none());
/// ```
pub fn parse_line(raw: &str) -> Option<IntelLine> {
    static LINE_RE: OnceLock<Regex> = OnceLock::new();
    let re = LINE_RE
        .get_or_init(|| Regex::new(LINE_PATTERN).expect("hardcoded line regex must compile"));
    let caps = re.captures(truncate_str(raw, MAX_LINE_LEN))?;
    let naive_ts =
        NaiveDateTime::parse_from_str(caps.name("ts")?.as_str(), LINE_TIMESTAMP_FORMAT).ok()?;
    Some(IntelLine {
        timestamp: naive_ts.and_utc(),
        author: caps.name("author")?.as_str().to_string(),
        text: caps.name("text")?.as_str().to_string(),
    })
}

impl IntelLine {
    /// Like [`parse_line`], but a line without a readable date is not lost:
    /// EVE's default settings can leave the timestamp out of the logs. It
    /// takes `now` as the moment of such a line, and says so with
    /// [`TimeOrigin::Assumed`]; a line with a date is [`TimeOrigin::Reported`].
    /// `None` when the line has no `author > text` either.
    pub fn parse_or_assume(
        raw: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<(Self, crate::event::TimeOrigin)> {
        use crate::event::TimeOrigin;
        if let Some(line) = parse_line(raw) {
            return Some((line, TimeOrigin::Reported));
        }
        static UNDATED_RE: OnceLock<Regex> = OnceLock::new();
        let re = UNDATED_RE.get_or_init(|| {
            Regex::new(UNDATED_LINE_PATTERN).expect("hardcoded line regex must compile")
        });
        let caps = re.captures(truncate_str(raw, MAX_LINE_LEN))?;
        let line = Self {
            timestamp: now,
            author: caps.name("author")?.as_str().to_string(),
            text: caps.name("text")?.as_str().to_string(),
        };
        Some((line, TimeOrigin::Assumed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_rule_round_trips_through_toml() {
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let text = toml::to_string(&kind).unwrap();
        let back: DetectionRuleKind = toml::from_str(&text).unwrap();
        assert_eq!(back, kind);
    }

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| word.to_string()).collect()
    }

    fn custom(pattern: Option<&str>, list: &[&str]) -> DetectionRuleKind {
        DetectionRuleKind::Custom {
            pattern: pattern.map(str::to_string),
            words: words(list),
            category: None,
            system_group: None,
        }
    }

    // parse_line

    #[test]
    fn parse_line_reads_timestamp_author_and_text() {
        let line = parse_line("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear").unwrap();
        assert_eq!(line.author, "Some Pilot");
        assert_eq!(line.text, "1DQ1-A clear");
        assert_eq!(
            line.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2021-09-08 22:56:47"
        );
    }

    #[test]
    fn parse_line_keeps_the_first_separator_as_author_end() {
        let line = parse_line("[ 2021.09.08 22:56:47 ] A > B > C").unwrap();
        assert_eq!(line.author, "A");
        assert_eq!(line.text, "B > C");
    }

    #[test]
    fn parse_line_rejects_lines_out_of_format() {
        for raw in [
            "",
            "not a log line",
            "[ 2021.09.08 22:56:47 ] no separator",
            "[ 2021.09.08 22:56:47 ] Pilot > ",
            "[ 2021.09.08 22:56 ] Pilot > text",
            "  [ 2021.09.08 22:56:47 ] Pilot > text",
        ] {
            assert!(parse_line(raw).is_none(), "{raw:?} should not parse");
        }
    }

    #[test]
    fn parse_line_rejects_an_impossible_date() {
        assert!(parse_line("[ 2021.13.40 25:61:61 ] Pilot > text").is_none());
    }

    #[test]
    fn parse_line_cuts_long_lines() {
        let raw = format!("[ 2021.09.08 22:56:47 ] Pilot > {}", "a".repeat(5000));
        let line = parse_line(&raw).unwrap();
        assert!(line.text.len() < 2048);
        assert!(line.text.starts_with("aaa"));
    }

    #[test]
    fn a_dated_line_keeps_its_date_and_is_reported() {
        let now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let (line, origin) =
            IntelLine::parse_or_assume("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear", now)
                .unwrap();
        assert_eq!(origin, crate::event::TimeOrigin::Reported);
        assert_eq!(
            line.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2021-09-08 22:56:47"
        );
    }

    #[test]
    fn a_line_without_a_date_is_read_at_the_moment_given_and_assumed() {
        let now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        for raw in [
            "Some Pilot > 1DQ1-A clear",
            "[ 22:56:47 ] Some Pilot > 1DQ1-A clear",
            "[ 2021.13.40 25:61:61 ] Some Pilot > 1DQ1-A clear",
        ] {
            let (line, origin) = IntelLine::parse_or_assume(raw, now).expect(raw);
            assert_eq!(origin, crate::event::TimeOrigin::Assumed, "{raw}");
            assert_eq!(line.timestamp, now, "{raw}");
            assert_eq!(line.author, "Some Pilot", "{raw}");
            assert_eq!(line.text, "1DQ1-A clear", "{raw}");
        }
    }

    #[test]
    fn a_line_without_author_and_text_is_not_a_line_either_way() {
        let now = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        for raw in ["", "not a log line", "[ 2021.09.08 22:56:47 ] no separator"] {
            assert!(IntelLine::parse_or_assume(raw, now).is_none(), "{raw:?}");
        }
    }

    // Dictionaries

    #[test]
    fn built_in_dictionaries_load_and_have_words() {
        let dictionaries = Dictionaries::defaults();
        assert!(!dictionaries.is_empty());
        let names = dictionaries.names();
        assert!(names.contains(&String::from("ship_report_en")));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for name in &names {
            let list = dictionaries.words(name).unwrap();
            assert!(!list.is_empty(), "{name} has no words");
            assert!(list.len() <= MAX_DICTIONARY_WORDS, "{name} is too big");
            assert!(list.iter().all(|word| !word.is_empty()));
        }
    }

    #[test]
    fn unknown_dictionary_has_no_words() {
        assert!(Dictionaries::defaults().words("nope").is_none());
        assert!(Dictionaries::default().is_empty());
    }

    #[test]
    fn shared_dictionaries_equal_the_defaults() {
        assert_eq!(&Dictionaries::defaults(), Dictionaries::shared());
    }

    // type_name, category, system_group

    #[test]
    fn type_names_match_the_serialized_tag() {
        let kinds = [
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            DetectionRuleKind::ClearReport {
                keywords: words(&["clr"]),
            },
            DetectionRuleKind::ShipNames {
                dictionaries: words(&["ship_report_en"]),
            },
            DetectionRuleKind::PilotCount,
            DetectionRuleKind::Keyword {
                keywords: words(&["k"]),
            },
            DetectionRuleKind::Query {
                keywords: words(&["q"]),
            },
            custom(None, &["w"]),
        ];
        for kind in kinds {
            let text = toml::to_string(&kind).unwrap();
            assert!(
                text.contains(&format!("type = \"{}\"", kind.type_name())),
                "{text}"
            );
        }
    }

    #[test]
    fn input_and_output_type_names() {
        assert_eq!(InputKind::default().type_name(), "chat_log");
        assert_eq!(InputKind::ZKillboard.type_name(), "zkillboard");
        let outputs = [
            (OutputKind::Visual, "visual"),
            (OutputKind::Sound, "sound"),
            (OutputKind::Log, "log"),
            (OutputKind::Suppress, "suppress"),
            (OutputKind::Tooltip, "tooltip"),
        ];
        for (kind, name) in outputs {
            assert_eq!(kind.type_name(), name);
        }
    }

    #[test]
    fn input_kind_is_stored_under_its_type_name() {
        #[derive(Debug, PartialEq, Deserialize, Serialize)]
        struct Holder {
            kind: InputKind,
        }
        for kind in [InputKind::ChatLog, InputKind::ZKillboard] {
            let text = toml::to_string(&Holder { kind }).unwrap();
            assert_eq!(text.trim(), format!("kind = \"{}\"", kind.type_name()));
            let back: Holder = toml::from_str(&text).unwrap();
            assert_eq!(back, Holder { kind });
        }
    }

    #[test]
    fn only_a_source_without_chat_channels_has_a_fixed_channel() {
        assert_eq!(InputKind::ChatLog.fixed_channel(), None);
        assert_eq!(InputKind::ZKillboard.fixed_channel(), Some("zkillboard"));
    }

    #[test]
    fn built_in_types_imply_their_category() {
        let keywords = words(&["x"]);
        assert_eq!(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new()
            }
            .category(),
            None
        );
        assert_eq!(
            DetectionRuleKind::ClearReport {
                keywords: keywords.clone()
            }
            .category(),
            Some(IntelCategory::Clear)
        );
        assert_eq!(
            DetectionRuleKind::ShipNames {
                dictionaries: Vec::new()
            }
            .category(),
            Some(IntelCategory::Ship)
        );
        assert_eq!(
            DetectionRuleKind::PilotCount.category(),
            Some(IntelCategory::Count)
        );
        assert_eq!(
            DetectionRuleKind::Keyword {
                keywords: keywords.clone()
            }
            .category(),
            Some(IntelCategory::Keyword)
        );
        assert_eq!(
            DetectionRuleKind::Query { keywords }.category(),
            Some(IntelCategory::Query)
        );
    }

    #[test]
    fn custom_category_and_system_group_come_from_its_fields() {
        let kind = DetectionRuleKind::Custom {
            pattern: Some(String::from("(?P<sys>x)")),
            words: Vec::new(),
            category: Some(IntelCategory::Ship),
            system_group: Some(String::from("sys")),
        };
        assert_eq!(kind.category(), Some(IntelCategory::Ship));
        assert_eq!(kind.system_group().as_deref(), Some("sys"));
        assert_eq!(custom(None, &["w"]).system_group(), None);
        assert_eq!(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new()
            }
            .system_group()
            .as_deref(),
            Some("system")
        );
        assert_eq!(DetectionRuleKind::PilotCount.system_group(), None);
    }

    // matcher

    fn regexes(matcher: DetectionMatcher) -> Vec<String> {
        match matcher {
            DetectionMatcher::Regex(patterns) => patterns,
            _ => panic!("expected a regex matcher"),
        }
    }

    fn dictionary(matcher: DetectionMatcher) -> Vec<String> {
        match matcher {
            DetectionMatcher::Dictionary(list) => list,
            _ => panic!("expected a dictionary matcher"),
        }
    }

    #[test]
    fn built_in_patterns_compile() {
        let dictionaries = Dictionaries::defaults();
        for kind in [
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            DetectionRuleKind::PilotCount,
        ] {
            for pattern in regexes(kind.matcher(&dictionaries)) {
                Regex::new(&pattern).unwrap();
            }
        }
    }

    #[test]
    fn system_pattern_captures_common_system_names() {
        let re = Regex::new(DEFAULT_SYSTEM_PATTERN).unwrap();
        for (text, expected) in [
            ("J123456 red", "J123456"),
            ("1DQ1-A clear", "1DQ1-A"),
            ("AD001 gate", "AD001"),
            // Whatever the case.
            ("j123456 red", "j123456"),
            ("h-5gui clear", "h-5gui"),
            ("1dq1-a", "1dq1-a"),
            ("ad001", "ad001"),
        ] {
            let caps = re.captures(text).unwrap();
            assert_eq!(&caps["system"], expected, "{text}");
        }
        // The names made of words are not this pattern's business: the
        // executor looks them up (`graph::named_systems`).
        for text in ["go to Jita now", "Old Man Star", "Floris Saucus"] {
            assert!(re.captures(text).is_none(), "{text}");
        }
    }

    #[test]
    fn pilot_count_patterns_capture_the_number() {
        let dictionaries = Dictionaries::defaults();
        let patterns: Vec<Regex> = regexes(DetectionRuleKind::PilotCount.matcher(&dictionaries))
            .iter()
            .map(|pattern| Regex::new(pattern).unwrap())
            .collect();
        let count = |text: &str| -> Option<String> {
            patterns
                .iter()
                .find_map(|re| re.captures(text))
                .map(|caps| caps["count"].to_string())
        };
        assert_eq!(count("Jita +5").as_deref(), Some("5"));
        assert_eq!(count("Jita x12").as_deref(), Some("12"));
        assert_eq!(count("Jita 3 neuts").as_deref(), Some("3"));
        assert_eq!(count("Jita 7x").as_deref(), Some("7"));
        assert_eq!(count("Jita clear"), None);
    }

    #[test]
    fn a_number_followed_by_a_plus_is_a_pilot_count() {
        let dictionaries = Dictionaries::defaults();
        let patterns: Vec<Regex> = regexes(DetectionRuleKind::PilotCount.matcher(&dictionaries))
            .iter()
            .map(|pattern| Regex::new(pattern).unwrap())
            .collect();
        let count = |text: &str| -> Option<String> {
            patterns
                .iter()
                .find_map(|re| re.captures(text))
                .map(|caps| caps["count"].to_string())
        };
        for (text, expected) in [
            ("8+", "8"),
            ("H-5GUI* 15+ Captain Ginjuic", "15"),
            ("2CG-5V* 7+ Leafless Zan", "7"),
            ("Cryptic Sharvas  0-R5TS* 29+ nv", "29"),
            ("U54-1L*  Almjaime 4+", "4"),
            ("P3EN-E* 40+https://dscan.info/v/36e0e2f324d9", "40"),
            ("0MV-4W* 200+kiki", "200"),
            ("V0DF-2*  Devil Seven 8+kiki", "8"),
            ("M-OEE8* 6+\u{9b54}\u{50cf}", "6"),
        ] {
            assert_eq!(count(text).as_deref(), Some(expected), "{text:?}");
        }
        // Not counts: a sum, a number glued to a word, a number too long.
        for text in ["1+1", "ESS12+", "v15+", "4HYV30+", "2000+ m3", "Jita clear"] {
            assert_eq!(count(text), None, "{text:?}");
        }
    }

    #[test]
    fn ship_zh_pattern_matches_han_class_names() {
        let re = Regex::new(DEFAULT_SHIP_ZH_PATTERN).unwrap();
        assert!(re.is_match("\u{53d1}\u{73b0}\u{6cf0}\u{5766}\u{7ea7}"));
        assert!(!re.is_match("Titan class"));
    }

    #[test]
    fn ship_names_match_dictionaries_and_the_chinese_shape() {
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: words(&["ship_report_en"]),
        };
        match kind.matcher(&Dictionaries::defaults()) {
            DetectionMatcher::Mixed { patterns, words } => {
                assert_eq!(patterns, vec![String::from(DEFAULT_SHIP_ZH_PATTERN)]);
                assert!(!words.is_empty());
            }
            _ => panic!("expected a mixed matcher"),
        }
    }

    #[test]
    fn the_former_chinese_type_loads_as_ship_names() {
        let kind: DetectionRuleKind = toml::from_str("type = \"ship_names_zh\"").unwrap();
        assert_eq!(
            kind,
            DetectionRuleKind::ShipNames {
                dictionaries: Vec::new()
            }
        );
        let text = toml::to_string(&kind).unwrap();
        assert!(text.contains("ship_names"), "{text}");
        assert!(!text.contains("_zh"), "{text}");
    }

    #[test]
    fn keyword_types_use_their_words_as_a_dictionary() {
        let dictionaries = Dictionaries::defaults();
        let keywords = words(&["clr", "clear"]);
        for kind in [
            DetectionRuleKind::ClearReport {
                keywords: keywords.clone(),
            },
            DetectionRuleKind::Keyword {
                keywords: keywords.clone(),
            },
            DetectionRuleKind::Query {
                keywords: keywords.clone(),
            },
        ] {
            assert_eq!(dictionary(kind.matcher(&dictionaries)), keywords);
        }
    }

    #[test]
    fn ship_names_matcher_concatenates_dictionaries_and_skips_unknown() {
        let dictionaries = Dictionaries::defaults();
        let en = dictionaries.words("ship_report_en").unwrap().len();
        let es = dictionaries.words("ship_report_es").unwrap().len();
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: words(&["ship_report_en", "missing", "ship_report_es"]),
        };
        match kind.matcher(&dictionaries) {
            DetectionMatcher::Mixed { words, .. } => assert_eq!(words.len(), en + es),
            _ => panic!("expected a mixed matcher"),
        }
    }

    #[test]
    fn custom_matcher_prefers_the_pattern_over_words() {
        let dictionaries = Dictionaries::defaults();
        assert_eq!(
            regexes(custom(Some("ab+"), &["w"]).matcher(&dictionaries)),
            words(&["ab+"])
        );
        assert_eq!(
            dictionary(custom(None, &["w"]).matcher(&dictionaries)),
            words(&["w"])
        );
    }

    // validate

    fn validate(kind: &DetectionRuleKind) -> Result<(), PatternError> {
        kind.validate("rule", &Dictionaries::defaults())
    }

    #[test]
    fn built_in_types_without_parameters_are_always_valid() {
        for kind in [
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            DetectionRuleKind::PilotCount,
        ] {
            assert_eq!(validate(&kind), Ok(()));
        }
    }

    #[test]
    fn keyword_lists_are_checked() {
        let empty = DetectionRuleKind::Keyword {
            keywords: Vec::new(),
        };
        assert_eq!(
            validate(&empty),
            Err(PatternError::InvalidDictionarySize(String::from("rule")))
        );
        let blank_word = DetectionRuleKind::Query {
            keywords: words(&["ok", ""]),
        };
        assert_eq!(
            validate(&blank_word),
            Err(PatternError::InvalidDictionaryWord {
                id: String::from("rule"),
                word: String::new()
            })
        );
        let long_word = "w".repeat(MAX_DICTIONARY_WORD_LEN + 1);
        assert!(matches!(
            validate(&DetectionRuleKind::ClearReport {
                keywords: vec![long_word]
            }),
            Err(PatternError::InvalidDictionaryWord { .. })
        ));
        let at_limit = "w".repeat(MAX_DICTIONARY_WORD_LEN);
        assert_eq!(
            validate(&DetectionRuleKind::Keyword {
                keywords: vec![at_limit]
            }),
            Ok(())
        );
    }

    #[test]
    fn too_many_words_are_rejected() {
        let many: Vec<String> = (0..=MAX_DICTIONARY_WORDS).map(|n| n.to_string()).collect();
        assert_eq!(
            validate(&DetectionRuleKind::Keyword { keywords: many }),
            Err(PatternError::InvalidDictionarySize(String::from("rule")))
        );
    }

    #[test]
    fn ship_names_need_known_dictionaries() {
        // The Chinese class names need no dictionary.
        let none = DetectionRuleKind::ShipNames {
            dictionaries: Vec::new(),
        };
        assert_eq!(validate(&none), Ok(()));
        let unknown = DetectionRuleKind::ShipNames {
            dictionaries: words(&["ship_report_en", "nope"]),
        };
        assert_eq!(
            validate(&unknown),
            Err(PatternError::UnknownDictionary {
                id: String::from("rule"),
                name: String::from("nope")
            })
        );
        let known = DetectionRuleKind::ShipNames {
            dictionaries: words(&["ship_report_en"]),
        };
        assert_eq!(validate(&known), Ok(()));
    }

    #[test]
    fn custom_pattern_length_is_checked() {
        assert_eq!(validate(&custom(Some("a+"), &[])), Ok(()));
        assert_eq!(
            validate(&custom(Some(""), &[])),
            Err(PatternError::PatternTooLong(String::from("rule")))
        );
        let long = "a".repeat(MAX_PATTERN_LEN + 1);
        assert_eq!(
            validate(&custom(Some(&long), &[])),
            Err(PatternError::PatternTooLong(String::from("rule")))
        );
    }

    #[test]
    fn custom_word_list_cannot_carry_count_or_system_group() {
        let counting = DetectionRuleKind::Custom {
            pattern: None,
            words: words(&["w"]),
            category: Some(IntelCategory::Count),
            system_group: None,
        };
        assert_eq!(
            validate(&counting),
            Err(PatternError::MissingCountGroup(String::from("rule")))
        );
        let grouped = DetectionRuleKind::Custom {
            pattern: None,
            words: words(&["w"]),
            category: None,
            system_group: Some(String::from("sys")),
        };
        assert!(matches!(
            validate(&grouped),
            Err(PatternError::InvalidSystemGroup { .. })
        ));
    }

    #[test]
    fn custom_pattern_system_group_must_be_a_valid_name() {
        let with_group = |group: &str| DetectionRuleKind::Custom {
            pattern: Some(String::from("(?P<sys>x)")),
            words: Vec::new(),
            category: None,
            system_group: Some(group.to_string()),
        };
        assert_eq!(validate(&with_group("sys")), Ok(()));
        assert!(matches!(
            validate(&with_group("bad name")),
            Err(PatternError::InvalidSystemGroup { .. })
        ));
        assert!(matches!(
            validate(&with_group("")),
            Err(PatternError::InvalidSystemGroup { .. })
        ));
    }

    #[test]
    fn custom_pattern_with_count_category_passes_validation() {
        // The `count` group of a pattern is checked when it is compiled.
        let kind = DetectionRuleKind::Custom {
            pattern: Some(String::from(r"(?P<count>\d+)")),
            words: Vec::new(),
            category: Some(IntelCategory::Count),
            system_group: None,
        };
        assert_eq!(validate(&kind), Ok(()));
    }

    #[test]
    fn custom_fields_default_when_missing_in_toml() {
        let kind: DetectionRuleKind = toml::from_str("type = \"custom\"").unwrap();
        assert_eq!(kind, custom(None, &[]));
    }

    #[test]
    fn unknown_rule_type_fails_to_deserialize() {
        assert!(toml::from_str::<DetectionRuleKind>("type = \"nope\"").is_err());
    }
}
