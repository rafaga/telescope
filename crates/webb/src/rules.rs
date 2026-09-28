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

use crate::patterns::{
    ActionConfig, IntelCategory, LINE_PATTERN, LINE_TIMESTAMP_FORMAT, MAX_DICTIONARY_WORD_LEN,
    MAX_DICTIONARY_WORDS, MAX_PATTERN_LEN, PatternError, PatternRuleConfig, is_valid_group_name,
};
pub use crate::patterns::IntelLine;
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
        toml::from_str(DEFAULT_DICTIONARIES_TOML)
            .expect("the embedded dictionaries.toml must parse")
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
    /// Reports a solar system; it is highlighted on the maps. Uses the
    /// built-in solar-system shape.
    SystemReport,
    /// Reports the system clear.
    ClearReport {
        /// Words that mean "clear".
        keywords: Vec<String>,
    },
    /// Reports ship names, using the built-in dictionaries by name.
    ShipNames {
        /// Names of the dictionaries (in `dictionaries.toml`) to match.
        dictionaries: Vec<String>,
    },
    /// Reports Chinese ship names; uses the built-in shape regex.
    ShipNamesZh,
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

/// Built-in solar-system shape used by [`DetectionRuleKind::SystemReport`]
/// (covers every name in the SDE). Its capture group is `system`.
pub(crate) const DEFAULT_SYSTEM_PATTERN: &str = r"\b(?P<system>J\d{6}|[A-Z0-9]{1,5}-[A-Z0-9]{1,4}|[A-Z]{2}\d{3}|[A-Z][a-z]{1,13}(?:[ -][A-Z][a-z]{1,13}){0,2})\b";

/// Built-in patterns used by [`DetectionRuleKind::PilotCount`]; each has a
/// `count` capture group.
const DEFAULT_COUNT_PATTERNS: [&str; 2] = [
    r"(?:^|\s)[+xX](?P<count>\d{1,3})\b",
    r"\b(?P<count>\d{1,3})\s?(?:x|neuts?|neutrals?|reds?|hostiles?|pilots?)\b",
];

/// Built-in shape used by [`DetectionRuleKind::ShipNamesZh`]: a run of Han
/// characters ending in the `级` ("-class") suffix, the Chinese client's ship
/// name marker.
pub(crate) const DEFAULT_SHIP_ZH_PATTERN: &str = r"[一-鿿]{2,14}级";

/// The matcher a detection type compiles to.
pub enum DetectionMatcher {
    /// One or more regular expressions.
    Regex(Vec<String>),
    /// A flat list of whole-word literals.
    Dictionary(Vec<String>),
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
    /// A short name for the type, for logs and the GUI.
    fn type_name(&self) -> &'static str;
    /// Validates the type-specific parameters.
    fn validate(&self, id: &str, dictionaries: &Dictionaries) -> Result<(), PatternError>;
}

impl DetectionType for DetectionRuleKind {
    fn matcher(&self, dictionaries: &Dictionaries) -> DetectionMatcher {
        match self {
            Self::SystemReport => {
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
            Self::ShipNames { dictionaries: names } => {
                let mut words = Vec::new();
                for name in names {
                    if let Some(dictionary) = dictionaries.words(name) {
                        words.extend(dictionary.iter().cloned());
                    }
                }
                DetectionMatcher::Dictionary(words)
            }
            Self::ShipNamesZh => {
                DetectionMatcher::Regex(vec![String::from(DEFAULT_SHIP_ZH_PATTERN)])
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
            Self::ShipNames { .. } | Self::ShipNamesZh => Some(IntelCategory::Ship),
            Self::PilotCount => Some(IntelCategory::Count),
            Self::Keyword { .. } => Some(IntelCategory::Keyword),
            Self::Query { .. } => Some(IntelCategory::Query),
            Self::Custom { category, .. } => *category,
            _ => None,
        }
    }

    fn system_group(&self) -> Option<String> {
        match self {
            Self::SystemReport => Some(String::from("system")),
            Self::Custom { system_group, .. } => system_group.clone(),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Self::SystemReport => "system_report",
            Self::ClearReport { .. } => "clear_report",
            Self::ShipNames { .. } => "ship_names",
            Self::ShipNamesZh => "ship_names_zh",
            Self::PilotCount => "pilot_count",
            Self::Keyword { .. } => "keyword",
            Self::Query { .. } => "query",
            Self::Custom { .. } => "custom",
        }
    }

    fn validate(&self, id: &str, dictionaries: &Dictionaries) -> Result<(), PatternError> {
        match self {
            Self::SystemReport | Self::PilotCount | Self::ShipNamesZh => {}
            Self::ClearReport { keywords }
            | Self::Keyword { keywords }
            | Self::Query { keywords } => validate_words(id, keywords)?,
            Self::ShipNames {
                dictionaries: names,
            } => {
                if names.is_empty() {
                    return Err(PatternError::InvalidDictionarySize(id.to_string()));
                }
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
                ..
            } => {
                match pattern {
                    Some(pattern) => validate_pattern(id, pattern)?,
                    None => validate_words(id, words)?,
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

/// Maps a legacy `patterns.toml` pattern rule to the closest specific type.
pub(crate) fn kind_from_legacy(rule: &PatternRuleConfig) -> DetectionRuleKind {
    match &rule.action {
        ActionConfig::Ignore => DetectionRuleKind::Custom {
            pattern: Some(rule.pattern.clone()),
            words: Vec::new(),
            category: None,
            system_group: None,
        },
        ActionConfig::MapAlert { system_group } => {
            if rule.pattern == DEFAULT_SYSTEM_PATTERN {
                DetectionRuleKind::SystemReport
            } else {
                DetectionRuleKind::Custom {
                    pattern: Some(rule.pattern.clone()),
                    words: Vec::new(),
                    category: None,
                    system_group: Some(system_group.clone()),
                }
            }
        }
        ActionConfig::Notify => match rule.category {
            Some(IntelCategory::Count) => DetectionRuleKind::PilotCount,
            Some(IntelCategory::Ship) if rule.pattern == DEFAULT_SHIP_ZH_PATTERN => {
                DetectionRuleKind::ShipNamesZh
            }
            Some(category) => DetectionRuleKind::Custom {
                pattern: Some(rule.pattern.clone()),
                words: Vec::new(),
                category: Some(category),
                system_group: None,
            },
            None => DetectionRuleKind::Custom {
                pattern: Some(rule.pattern.clone()),
                words: Vec::new(),
                category: None,
                system_group: None,
            },
        },
    }
}

/// Parses a raw log line into an [`IntelLine`]. Returns `None` when the line
/// does not follow the EVE chat log format
/// (`[ yyyy.MM.dd hh:mm:ss ] Author > message`).
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
    let caps = re.captures(raw)?;
    let naive_ts =
        NaiveDateTime::parse_from_str(caps.name("ts")?.as_str(), LINE_TIMESTAMP_FORMAT).ok()?;
    Some(IntelLine {
        timestamp: naive_ts.and_utc(),
        author: caps.name("author")?.as_str().to_string(),
        text: caps.name("text")?.as_str().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patterns::{ActionConfig, PatternRuleConfig};

    #[test]
    fn detection_rule_round_trips_through_toml() {
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let text = toml::to_string(&kind).unwrap();
        let back: DetectionRuleKind = toml::from_str(&text).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn a_system_report_pattern_maps_to_the_system_type() {
        let rule = PatternRuleConfig {
            id: String::from("system_reported"),
            pattern: String::from(DEFAULT_SYSTEM_PATTERN),
            case_insensitive: false,
            enabled: true,
            channels: Vec::new(),
            category: None,
            action: ActionConfig::MapAlert {
                system_group: String::from("system"),
            },
        };
        assert_eq!(kind_from_legacy(&rule), DetectionRuleKind::SystemReport);
    }
}
