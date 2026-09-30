//! Types and limits shared by the intel rule model ([`crate::rules`],
//! [`crate::graph`]) and the tooltip summaries ([`crate::map_alerts`]): the
//! parsed chat-log line, the detection categories, the validation errors, the
//! size limits and the small text helpers they all use.
//!
//! This is what remained of the old `patterns.toml` engine once the node
//! graph replaced its loader and evaluator.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Maximum length of a node id.
pub(crate) const MAX_ID_LEN: usize = 64;
/// Maximum length of a regex pattern source.
pub(crate) const MAX_PATTERN_LEN: usize = 1024;
/// Maximum number of channels in a single input node.
pub(crate) const MAX_CHANNELS: usize = 32;
/// Maximum length of a channel name.
pub(crate) const MAX_CHANNEL_LEN: usize = 64;
/// Compiled regex size limit (bytes) to prevent memory exhaustion.
pub(crate) const REGEX_SIZE_LIMIT: usize = 10 * (1 << 20);
/// Maximum length (bytes) of a chat-log line the rules look at; longer lines
/// are cut (see [`crate::rules::parse_line`]).
pub(crate) const MAX_LINE_LEN: usize = 2048;
/// Maximum number of matches a detection reports per line.
pub(crate) const MAX_MATCHES_PER_CHUNK: usize = 100;
/// Maximum number of candidate systems a system detection reports per line:
/// a pilot name written before the system also fits the system pattern, and
/// only resolving each candidate tells them apart.
pub const MAX_SYSTEM_CANDIDATES: usize = 8;
/// Maximum length of captured text displayed in notifications.
const MAX_DISPLAY_LEN: usize = 200;
/// Maximum number of words in a single word list. An
/// [`aho_corasick::AhoCorasick`] automaton is linear in total pattern size,
/// so this exists only as a sanity ceiling against a corrupted or hostile
/// rules file, not a real usage constraint.
pub(crate) const MAX_DICTIONARY_WORDS: usize = 4096;
/// Maximum length (bytes) of a single word of a word list.
pub(crate) const MAX_DICTIONARY_WORD_LEN: usize = 128;

/// Regex that parses a standard EVE Online chat log line:
/// `[ 2021.09.08 22:56:47 ] Character Name > message`
pub(crate) const LINE_PATTERN: &str =
    r"^\[\s(?P<ts>\d{4}\.\d{2}\.\d{2}\s\d{2}:\d{2}:\d{2})\s\]\s(?P<author>.+?)\s>\s(?P<text>.+)$";

/// Timestamp format used by EVE Online chat logs (UTC).
pub(crate) const LINE_TIMESTAMP_FORMAT: &str = "%Y.%m.%d %H:%M:%S";

/// What a detection's match tells the map tooltip about its line. Built-in
/// detection types imply their own; a `custom` detection chooses one.
///
/// ```toml
/// category = "ship"
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntelCategory {
    /// The matched text is a ship name.
    Ship,
    /// The rule's `count` capture group is the number of pilots reported.
    Count,
    /// The line reports the system clear: no visual alert, no sound.
    Clear,
    /// Shorthand with nothing to show; only dropped from the leftover text.
    Keyword,
    /// The line asks about the system instead of reporting it: it raises no
    /// map alert at all (no visual alert, no sound, no tooltip entry).
    Query,
}

/// Name of the capture group holding the number of pilots, read by
/// [`IntelCategory::Count`] detections.
pub const COUNT_GROUP: &str = "count";

/// A parsed EVE Online chat log line.
///
/// Produced by [`crate::rules::parse_line`] from raw lines such as
/// `[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear`.
#[derive(Debug, Clone, PartialEq)]
pub struct IntelLine {
    /// Moment the message was written, in UTC (the log's own timestamp).
    pub timestamp: DateTime<Utc>,
    /// Name of the character that posted the message.
    pub author: String,
    /// Message payload; the rules are evaluated against this text.
    pub text: String,
}

impl Display for IntelLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[ {} ] {} > {}",
            self.timestamp.format(LINE_TIMESTAMP_FORMAT),
            sanitize_display(&self.author),
            sanitize_display(&self.text)
        )
    }
}

/// Errors found while validating or compiling the intel rules (the nodes and
/// edges of a [`crate::graph::RuleGraph`]). Each one describes a single node
/// or edge; the rest of the graph keeps loading.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternError {
    /// A rule id is empty, too long or contains characters outside
    /// `[A-Za-z0-9_-]`.
    InvalidId(String),
    /// Two rules share the same id.
    DuplicateId(String),
    /// A rule pattern is empty or exceeds 1024 characters.
    PatternTooLong(String),
    /// A rule pattern does not compile or exceeds the regex size limits.
    InvalidPattern {
        /// Id of the offending rule.
        id: String,
        /// Compiler diagnostic.
        reason: String,
    },
    /// A rule declares more channels than allowed (max 32).
    TooManyChannels(String),
    /// A channel name is empty, too long or contains characters outside
    /// `[A-Za-z0-9_-]`.
    InvalidChannel(String),
    /// A `map_alert` action references a capture group that is invalid or
    /// missing in the rule pattern.
    InvalidSystemGroup {
        /// Id of the offending rule.
        id: String,
        /// Name of the offending capture group.
        group: String,
    },
    /// A dictionary defines no words, or more than allowed (max 4096).
    InvalidDictionarySize(String),
    /// A dictionary word is empty or exceeds 128 bytes.
    InvalidDictionaryWord {
        /// Id of the offending dictionary.
        id: String,
        /// The offending word (truncated for display if very long).
        word: String,
    },
    /// A `category = "count"` rule has no `count` capture group (always the
    /// case for a dictionary).
    MissingCountGroup(String),
    /// The automaton for a dictionary could not be built.
    DictionaryBuildFailed {
        /// Id of the offending dictionary.
        id: String,
        /// Builder diagnostic.
        reason: String,
    },
    /// A detection rule declares a tag that is empty, too long or contains
    /// characters outside `[A-Za-z0-9_.-]`.
    InvalidTag {
        /// Id of the offending rule.
        id: String,
        /// The offending tag.
        tag: String,
    },
    /// An output rule condition is invalid: a bad tag, or a quantifier larger
    /// than the tag list it applies to.
    InvalidCondition {
        /// Id of the offending output rule.
        id: String,
        /// Description of the problem.
        reason: String,
    },
    /// An input rule is invalid (empty path or similar).
    InvalidInput {
        /// Id of the offending input rule.
        id: String,
        /// Description of the problem.
        reason: String,
    },
    /// A detection references a source that is not a valid input id.
    InvalidSource {
        /// Id of the offending detection rule.
        id: String,
        /// The offending source id.
        source: String,
    },
    /// An output is not wired to any detection.
    OutputWithoutDetection(String),
    /// A detection (other than a `drop` one) is not wired to any output.
    DetectionWithoutOutput(String),
    /// A rule references a dictionary that does not exist in
    /// `dictionaries.toml`.
    UnknownDictionary {
        /// Id of the offending rule.
        id: String,
        /// The missing dictionary name.
        name: String,
    },
    /// Two output rules share the same kind (there is at most one per kind).
    DuplicateOutputKind(String),
    /// A node is missing the input cable it requires.
    MissingInput(String),
    /// An edge uses a pin the node does not expose.
    InvalidPin {
        /// Id of the node.
        node: String,
        /// The offending pin name.
        pin: String,
    },
    /// The node graph contains a cycle; it must be a DAG.
    GraphCycle(Vec<String>),
    /// An aggregator has more incoming cables than it has input pins.
    TooManyInputs(String),
}

impl Display for PatternError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => {
                write!(
                    f,
                    "invalid rule id '{id}' (use [A-Za-z0-9_-], max {MAX_ID_LEN} chars)"
                )
            }
            Self::DuplicateId(id) => write!(f, "duplicated rule id '{id}'"),
            Self::PatternTooLong(id) => {
                write!(
                    f,
                    "pattern of rule '{id}' is empty or exceeds {MAX_PATTERN_LEN} chars"
                )
            }
            Self::InvalidPattern { id, reason } => {
                write!(f, "pattern of rule '{id}' does not compile: {reason}")
            }
            Self::TooManyChannels(id) => {
                write!(
                    f,
                    "rule '{id}' defines too many channels (max {MAX_CHANNELS})"
                )
            }
            Self::InvalidChannel(name) => write!(
                f,
                "invalid channel name '{name}' (use [A-Za-z0-9_.+ -], max {MAX_CHANNEL_LEN} chars)"
            ),
            Self::InvalidSystemGroup { id, group } => write!(
                f,
                "rule '{id}' references the capture group '{group}' which is invalid or missing in its pattern"
            ),
            Self::InvalidDictionarySize(id) => write!(
                f,
                "dictionary '{id}' has no words or exceeds {MAX_DICTIONARY_WORDS} words"
            ),
            Self::InvalidDictionaryWord { id, word } => write!(
                f,
                "dictionary '{id}' has an empty word or a word exceeding {MAX_DICTIONARY_WORD_LEN} bytes ('{}...')",
                truncate_str(word, 32)
            ),
            Self::DictionaryBuildFailed { id, reason } => {
                write!(f, "dictionary '{id}' could not be built: {reason}")
            }
            Self::MissingCountGroup(id) => write!(
                f,
                "rule '{id}' has category \"count\" but no '{COUNT_GROUP}' capture group (dictionaries cannot use it)"
            ),
            Self::InvalidTag { id, tag } => write!(
                f,
                "rule '{id}' declares an invalid tag '{tag}' (use [A-Za-z0-9_.-], max 64 chars)"
            ),
            Self::InvalidCondition { id, reason } => {
                write!(f, "output rule '{id}' has an invalid condition: {reason}")
            }
            Self::InvalidInput { id, reason } => {
                write!(f, "input rule '{id}' is invalid: {reason}")
            }
            Self::InvalidSource { id, source } => write!(
                f,
                "detection rule '{id}' references an invalid source '{source}'"
            ),
            Self::OutputWithoutDetection(id) => {
                write!(f, "output rule '{id}' is not wired to any detection")
            }
            Self::DetectionWithoutOutput(id) => {
                write!(f, "detection rule '{id}' is not wired to any output")
            }
            Self::UnknownDictionary { id, name } => {
                write!(f, "rule '{id}' references an unknown dictionary '{name}'")
            }
            Self::DuplicateOutputKind(kind) => {
                write!(f, "more than one output rule of kind '{kind}'")
            }
            Self::MissingInput(id) => {
                write!(f, "node '{id}' has no input cable")
            }
            Self::InvalidPin { node, pin } => {
                write!(f, "node '{node}' does not expose the pin '{pin}'")
            }
            Self::GraphCycle(nodes) => {
                write!(
                    f,
                    "the node graph has a cycle through: {}",
                    nodes.join(" -> ")
                )
            }
            Self::TooManyInputs(id) => {
                write!(f, "aggregator '{id}' has more inputs than pins")
            }
        }
    }
}

impl Error for PatternError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Returns `true` when the byte span `[start, end)` of `text` is bounded on
/// both sides by a non-word character (or the start/end of the string),
/// emulating regex's `\b` for word-list matches: [`aho_corasick::AhoCorasick`]
/// finds raw substrings, not whole-word matches, so e.g. a dictionary word
/// `"Rifter"` must not match inside `"Rifterhampton"`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(crate) fn has_word_boundaries(text: &str, start: usize, end: usize) -> bool {
    let before_ok = match text[..start].chars().next_back() {
        Some(c) => !is_word_char(c),
        None => true,
    };
    let after_ok = match text[end..].chars().next() {
        Some(c) => !is_word_char(c),
        None => true,
    };
    before_ok && after_ok
}

/// Strips control characters (newlines, ANSI escapes, etc.) and truncates the
/// input to 200 characters so captured text can be safely displayed in the UI
/// without forging log lines or breaking the layout.
///
/// # Examples
///
/// ```
/// use webb::intel::sanitize_display;
///
/// assert_eq!(sanitize_display("hello\x1b[31m world\n"), "hello[31m world");
/// ```
pub fn sanitize_display(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DISPLAY_LEN)
        .collect()
}

/// Truncates a string to at most `max` bytes without splitting a UTF-8
/// character boundary.
pub(crate) fn truncate_str(input: &str, max: usize) -> &str {
    if input.len() <= max {
        return input;
    }
    let mut end = max;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

pub(crate) fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(crate) fn is_valid_channel(name: &str) -> bool {
    // Real EVE channel names (as extracted verbatim from the log file name
    // prefix by `load_intel_file`) commonly contain '.' and '+', e.g. an
    // alliance channel literally named "wc.Vale+Tribute". A plain
    // `[A-Za-z0-9_-]` filter, as used for rule/channel *ids*, rejected such
    // names outright, making `channels` filtering unusable for them. This
    // stays ASCII-only and excludes characters with structural meaning
    // elsewhere (quotes, backslashes, path separators, control characters)
    // since `channels` is only ever used for exact string comparison
    // (`HashSet<String>::contains`), never as a path or in a query.
    !name.is_empty()
        && name.len() <= MAX_CHANNEL_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | ' '))
}

pub(crate) fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_loses_control_characters_and_is_capped() {
        assert_eq!(sanitize_display("a\nb\x1b[31mc"), "ab[31mc");
        assert_eq!(sanitize_display(&"x".repeat(500)).len(), MAX_DISPLAY_LEN);
    }

    #[test]
    fn word_boundaries_reject_matches_inside_a_word() {
        let text = "Rifterhampton Rifter";
        assert!(!has_word_boundaries(text, 0, 6));
        assert!(has_word_boundaries(text, 14, 20));
        // Non-ASCII letters count as word characters.
        assert!(!has_word_boundaries("éRifter", 2, 8));
    }

    #[test]
    fn truncation_never_splits_a_character() {
        assert_eq!(truncate_str("añb", 2), "a");
        assert_eq!(truncate_str("abc", 10), "abc");
    }

    #[test]
    fn real_channel_names_are_valid() {
        assert!(is_valid_channel("wc.Vale+Tribute"));
        assert!(is_valid_channel("My Intel_Channel-2"));
        assert!(!is_valid_channel(""));
        assert!(!is_valid_channel("bad/name"));
        assert!(!is_valid_channel(&"x".repeat(MAX_CHANNEL_LEN + 1)));
    }

    #[test]
    fn ids_and_group_names_are_checked() {
        assert!(is_valid_id("system_reported-2"));
        assert!(!is_valid_id("has space"));
        assert!(is_valid_group_name("count"));
        assert!(!is_valid_group_name("co-unt"));
    }

    #[test]
    fn intel_line_display_reproduces_the_log_format() {
        let line =
            crate::rules::parse_line("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear").unwrap();
        assert_eq!(
            line.to_string(),
            "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear"
        );
    }

    #[test]
    fn overlong_lines_are_cut_before_parsing() {
        let raw = format!("[ 2021.09.08 22:56:47 ] P > {}", "x".repeat(10_000));
        let line = crate::rules::parse_line(&raw).unwrap();
        assert!(line.text.len() < MAX_LINE_LEN);
    }
}
