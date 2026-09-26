//! Rule model of the intel pipeline: **input -> detection -> output**.
//!
//! Where [`crate::patterns`] describes the legacy `patterns.toml` engine (a
//! rule carries the action to run when it matches), this module splits the
//! model in three independent classes:
//!
//! * [`InputRule`]: where intel text comes from (a chat-log feed).
//! * [`DetectionRule`]: how a line is recognized; it produces a
//!   [`Detection`] carrying free-form [`Tag`]s and never an action.
//! * [`OutputRule`]: what to do with a batch of detections, subscribed by
//!   [`Condition`] over those tags.
//!
//! [`DetectionEngine`] compiles the [`DetectionRule`]s once and evaluates
//! chunks of log text into [`Detection`]s. Routing detections to the outputs
//! (joining every detection of one input line, evaluating the conditions and
//! dispatching to the visual/sound/log queues) is the application's job, not
//! this crate's.
//!
//! [`RulesConfig`] is the whole three-class configuration. It is what the
//! `telescope.db` rule tables store, and it can also be imported from / export
//! to TOML.

use crate::patterns::{
    ActionConfig, COUNT_GROUP, DictionaryActionConfig, IntelCategory, LINE_PATTERN,
    LINE_TIMESTAMP_FORMAT, MAX_CHANNELS, MAX_DICTIONARIES, MAX_DICTIONARY_WORD_LEN,
    MAX_DICTIONARY_WORDS, MAX_LINE_LEN, MAX_MATCHES_PER_CHUNK, MAX_PATTERN_LEN, MAX_RULES,
    MAX_SYSTEM_CANDIDATES, PatternConfig, PatternError, REGEX_SIZE_LIMIT, has_word_boundaries,
    is_valid_channel, is_valid_group_name, is_valid_id, sanitize_display, truncate_str,
};
pub use crate::patterns::IntelLine;
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use chrono::NaiveDateTime;
use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

/// Maximum number of tags on a single detection rule.
const MAX_TAGS: usize = 64;
/// Maximum length of a tag. A migration tag is `rule_` plus a rule id (which
/// itself can be up to 64 chars), so this is a little longer than the id.
const MAX_TAG_LEN: usize = 80;
/// Maximum number of output rules in a configuration.
const MAX_OUTPUTS: usize = 64;

fn default_true() -> bool {
    true
}

/// A free-form label attached to a [`Detection`] and matched by the
/// [`Condition`] of an [`OutputRule`].
pub type Tag = String;

fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= MAX_TAG_LEN
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Boolean expression over the tags of a detection batch.
///
/// A `[`OutputRule`]` fires when its condition evaluates to `true` against
/// the union of the tags of every detection in the batch. The quantifiers
/// (`all_true`, `any_true`, ...) are the primitives; `and`/`or`/`not`
/// combine them into arbitrary expressions.
///
/// ```toml
/// when = { op = "all_true", tags = ["ship", "hostile"] }
/// when = { op = "at_least_n_true", n = 2, tags = ["ship", "hostile", "capital"] }
/// when = { op = "or", any = [
///     { op = "all_true", tags = ["clear"] },
///     { op = "none_true", tags = ["ship"] },
/// ] }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Condition {
    /// Always fires; the output ignores the batch's tags.
    #[default]
    Always,
    /// Every listed tag is present.
    AllTrue {
        /// Tags that must all be present.
        tags: Vec<Tag>,
    },
    /// At least one listed tag is present.
    AnyTrue {
        /// Tags of which at least one must be present.
        tags: Vec<Tag>,
    },
    /// None of the listed tags is present.
    NoneTrue {
        /// Tags that must all be absent.
        tags: Vec<Tag>,
    },
    /// At least one listed tag is absent.
    AtLeastOneFalse {
        /// Tags of which at least one must be absent.
        tags: Vec<Tag>,
    },
    /// At least `n` of the listed tags are present.
    AtLeastNTrue {
        /// How many of `tags` must be present.
        n: usize,
        /// Tags to count.
        tags: Vec<Tag>,
    },
    /// Every sub-condition is true.
    And {
        /// Sub-conditions that must all hold.
        all: Vec<Condition>,
    },
    /// At least one sub-condition is true.
    Or {
        /// Sub-conditions of which at least one must hold.
        any: Vec<Condition>,
    },
    /// The sub-condition is false.
    Not {
        /// Negated sub-condition.
        not: Box<Condition>,
    },
}

impl Condition {
    /// Evaluates the condition against the tags present in a batch.
    pub fn evaluate(&self, tags: &HashSet<Tag>) -> bool {
        match self {
            Self::Always => true,
            Self::AllTrue { tags: wanted } => wanted.iter().all(|tag| tags.contains(tag)),
            Self::AnyTrue { tags: wanted } => wanted.iter().any(|tag| tags.contains(tag)),
            Self::NoneTrue { tags: wanted } => wanted.iter().all(|tag| !tags.contains(tag)),
            Self::AtLeastOneFalse { tags: wanted } => wanted.iter().any(|tag| !tags.contains(tag)),
            Self::AtLeastNTrue { n, tags: wanted } => {
                wanted.iter().filter(|tag| tags.contains(*tag)).count() >= *n
            }
            Self::And { all } => all.iter().all(|condition| condition.evaluate(tags)),
            Self::Or { any } => any.iter().any(|condition| condition.evaluate(tags)),
            Self::Not { not } => !not.evaluate(tags),
        }
    }

    fn validate(&self, id: &str) -> Result<(), PatternError> {
        fn check_tags(id: &str, tags: &[Tag]) -> Result<(), PatternError> {
            for tag in tags {
                if !is_valid_tag(tag) {
                    return Err(PatternError::InvalidTag {
                        id: id.to_string(),
                        tag: tag.clone(),
                    });
                }
            }
            Ok(())
        }
        match self {
            Self::Always => Ok(()),
            Self::AllTrue { tags }
            | Self::AnyTrue { tags }
            | Self::NoneTrue { tags }
            | Self::AtLeastOneFalse { tags } => check_tags(id, tags),
            Self::AtLeastNTrue { n, tags } => {
                check_tags(id, tags)?;
                if *n > tags.len() {
                    return Err(PatternError::InvalidCondition {
                        id: id.to_string(),
                        reason: format!(
                            "at_least_n_true asks for {n} tags but only {} are listed",
                            tags.len()
                        ),
                    });
                }
                Ok(())
            }
            Self::And { all } => {
                for condition in all {
                    condition.validate(id)?;
                }
                Ok(())
            }
            Self::Or { any } => {
                for condition in any {
                    condition.validate(id)?;
                }
                Ok(())
            }
            Self::Not { not } => not.validate(id),
        }
    }
}

/// Kind of an [`InputRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    /// An EVE Online chat-log file or directory of them (UTF-16LE).
    #[default]
    ChatLog,
}

/// Declarative configuration of an input source.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InputRule {
    /// Unique rule identifier (`[A-Za-z0-9_-]`, max 64 chars).
    pub id: String,
    /// Kind of the source.
    #[serde(default)]
    pub kind: InputKind,
    /// Path of the file or directory to read.
    pub path: String,
    /// Channels this input feeds; empty means every channel the source
    /// exposes.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Disabled inputs are not read.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl InputRule {
    fn validate(&self) -> Result<(), PatternError> {
        if !is_valid_id(&self.id) {
            return Err(PatternError::InvalidId(self.id.clone()));
        }
        if self.path.trim().is_empty() {
            return Err(PatternError::InvalidInput {
                id: self.id.clone(),
                reason: "path must not be empty".to_string(),
            });
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(PatternError::TooManyChannels(self.id.clone()));
        }
        for channel in &self.channels {
            if !is_valid_channel(channel) {
                return Err(PatternError::InvalidChannel(channel.clone()));
            }
        }
        Ok(())
    }
}

/// How a [`DetectionRule`] recognizes text.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DetectionKind {
    /// A regular expression evaluated against the text payload of the line.
    Regex {
        /// Regex source (max 1024 chars).
        pattern: String,
    },
    /// A flat list of literal words matched whole-word.
    Dictionary {
        /// Words to match, verbatim (max 4096 words, 128 bytes each).
        words: Vec<String>,
    },
}

/// Declarative configuration of a single detection rule.
///
/// A detection never carries an action: it only says *that* a line matched
/// and *which* [`Tag`]s it carries. What to do with the match is decided
/// later by the [`OutputRule`]s.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DetectionRule {
    /// Unique rule identifier (`[A-Za-z0-9_-]`, max 64 chars). Shares its
    /// namespace with every other detection rule.
    pub id: String,
    /// How the rule matches text.
    pub kind: DetectionKind,
    /// Whether matching is case-insensitive.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Optional channel filter; empty means every channel.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Disabled rules are skipped silently at load time.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Drop the whole line: no detection rule produces a match for it.
    #[serde(default)]
    pub drop: bool,
    /// What the match contributes to the map tooltip summary of its line.
    #[serde(default)]
    pub category: Option<IntelCategory>,
    /// Named capture group holding the reported solar system (regex rules
    /// only). Dictionary rules use the matched word itself.
    #[serde(default)]
    pub system_group: Option<String>,
    /// Tags emitted with each match; the [`OutputRule`]s subscribe to these.
    #[serde(default)]
    pub tags: Vec<Tag>,
}

impl DetectionRule {
    fn validate(&self) -> Result<(), PatternError> {
        if !is_valid_id(&self.id) {
            return Err(PatternError::InvalidId(self.id.clone()));
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(PatternError::TooManyChannels(self.id.clone()));
        }
        for channel in &self.channels {
            if !is_valid_channel(channel) {
                return Err(PatternError::InvalidChannel(channel.clone()));
            }
        }
        if self.tags.len() > MAX_TAGS {
            return Err(PatternError::TooManyTags(self.id.clone()));
        }
        for tag in &self.tags {
            if !is_valid_tag(tag) {
                return Err(PatternError::InvalidTag {
                    id: self.id.clone(),
                    tag: tag.clone(),
                });
            }
        }
        if let Some(group) = &self.system_group
            && !is_valid_group_name(group)
        {
            return Err(PatternError::InvalidSystemGroup {
                id: self.id.clone(),
                group: group.clone(),
            });
        }
        match &self.kind {
            DetectionKind::Regex { pattern } => {
                if pattern.is_empty() || pattern.len() > MAX_PATTERN_LEN {
                    return Err(PatternError::PatternTooLong(self.id.clone()));
                }
            }
            DetectionKind::Dictionary { words } => {
                if self.category == Some(IntelCategory::Count) {
                    return Err(PatternError::MissingCountGroup(self.id.clone()));
                }
                if words.is_empty() || words.len() > MAX_DICTIONARY_WORDS {
                    return Err(PatternError::InvalidDictionarySize(self.id.clone()));
                }
                for word in words {
                    if word.is_empty() || word.len() > MAX_DICTIONARY_WORD_LEN {
                        return Err(PatternError::InvalidDictionaryWord {
                            id: self.id.clone(),
                            word: word.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// Which queue an [`OutputRule`] feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// Highlight the reported system on the maps.
    Visual,
    /// Play the alarm.
    Sound,
    /// Append a line to the status log.
    Log,
}

/// Whether an output emits or withholds its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Fire the action when the condition holds.
    #[default]
    Emit,
    /// Withhold the action when the condition holds (an explicit veto).
    Suppress,
}

/// Declarative configuration of a single output rule.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct OutputRule {
    /// Unique rule identifier (`[A-Za-z0-9_-]`, max 64 chars).
    pub id: String,
    /// Queue this rule feeds.
    pub kind: OutputKind,
    /// Whether the rule emits or suppresses.
    #[serde(default)]
    pub mode: OutputMode,
    /// Condition over the batch's tags.
    #[serde(default)]
    pub when: Condition,
    /// Optional channel filter; empty means every channel.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Disabled rules are skipped silently.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl OutputRule {
    fn validate(&self) -> Result<(), PatternError> {
        if !is_valid_id(&self.id) {
            return Err(PatternError::InvalidId(self.id.clone()));
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(PatternError::TooManyChannels(self.id.clone()));
        }
        for channel in &self.channels {
            if !is_valid_channel(channel) {
                return Err(PatternError::InvalidChannel(channel.clone()));
            }
        }
        self.when.validate(&self.id)
    }
}

/// The whole three-class rule configuration.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct RulesConfig {
    /// Input sources.
    #[serde(default)]
    pub inputs: Vec<InputRule>,
    /// Detection rules.
    #[serde(default)]
    pub detections: Vec<DetectionRule>,
    /// Output rules.
    #[serde(default)]
    pub outputs: Vec<OutputRule>,
}

impl RulesConfig {
    /// Validates every rule without compiling the engine. Returns one error
    /// per invalid rule (a rule with several problems reports the first).
    pub fn validate(&self) -> Vec<PatternError> {
        let mut errors = Vec::new();

        let mut seen_inputs = HashSet::new();
        for input in &self.inputs {
            if !seen_inputs.insert(input.id.clone()) {
                errors.push(PatternError::DuplicateId(input.id.clone()));
                continue;
            }
            if let Err(error) = input.validate() {
                errors.push(error);
            }
        }

        let mut seen_detections = HashSet::new();
        for rule in &self.detections {
            if !rule.enabled {
                continue;
            }
            if !seen_detections.insert(rule.id.clone()) {
                errors.push(PatternError::DuplicateId(rule.id.clone()));
                continue;
            }
            if let Err(error) = rule.validate() {
                errors.push(error);
            }
        }

        if self.outputs.len() > MAX_OUTPUTS {
            errors.push(PatternError::TooManyRules(self.outputs.len()));
        }
        let mut seen_outputs = HashSet::new();
        for output in &self.outputs {
            if !seen_outputs.insert(output.id.clone()) {
                errors.push(PatternError::DuplicateId(output.id.clone()));
                continue;
            }
            if let Err(error) = output.validate() {
                errors.push(error);
            }
        }

        errors
    }

    /// Builds a rules configuration from a legacy `patterns.toml` config:
    /// every pattern/dictionary becomes a [`DetectionRule`], and every action
    /// becomes the matching [`OutputRule`]s (`notify` -> log, `map_alert` ->
    /// visual + sound, `ignore` -> `drop` on the detection).
    ///
    /// Each detection also gets a `rule_<id>` tag so the generated outputs
    /// subscribe to exactly that rule, preserving the per-rule behavior of
    /// the old configuration.
    pub fn from_pattern_config(config: &PatternConfig) -> RulesConfig {
        let mut detections = Vec::new();
        let mut outputs = Vec::new();

        for rule in &config.patterns {
            let tags = detection_tags(&rule.id, rule.category);
            let (drop, system_group) = match &rule.action {
                ActionConfig::Notify => (false, None),
                ActionConfig::MapAlert { system_group } => (false, Some(system_group.clone())),
                ActionConfig::Ignore => (true, None),
            };
            detections.push(DetectionRule {
                id: rule.id.clone(),
                kind: DetectionKind::Regex {
                    pattern: rule.pattern.clone(),
                },
                case_insensitive: rule.case_insensitive,
                channels: rule.channels.clone(),
                enabled: rule.enabled,
                drop,
                category: rule.category,
                system_group,
                tags: tags.clone(),
            });
            push_outputs(
                &mut outputs,
                &rule.id,
                &rule.action,
                &tags,
                &rule.channels,
                rule.enabled,
            );
        }

        for rule in &config.dictionaries {
            let tags = detection_tags(&rule.id, rule.category);
            let (system_group, action) = match rule.action {
                DictionaryActionConfig::Notify => (None, ActionConfig::Notify),
                DictionaryActionConfig::MapAlert => (
                    Some("word".to_string()),
                    ActionConfig::MapAlert {
                        system_group: "word".to_string(),
                    },
                ),
            };
            detections.push(DetectionRule {
                id: rule.id.clone(),
                kind: DetectionKind::Dictionary {
                    words: rule.words.clone(),
                },
                case_insensitive: rule.case_insensitive,
                channels: rule.channels.clone(),
                enabled: rule.enabled,
                drop: false,
                category: rule.category,
                system_group,
                tags: tags.clone(),
            });
            push_outputs(
                &mut outputs,
                &rule.id,
                &action,
                &tags,
                &rule.channels,
                rule.enabled,
            );
        }

        RulesConfig {
            inputs: Vec::new(),
            detections,
            outputs,
        }
    }
}

fn category_tag(category: IntelCategory) -> &'static str {
    match category {
        IntelCategory::Ship => "ship",
        IntelCategory::Count => "count",
        IntelCategory::Clear => "clear",
        IntelCategory::Keyword => "keyword",
        IntelCategory::Query => "query",
    }
}

fn detection_tags(id: &str, category: Option<IntelCategory>) -> Vec<Tag> {
    let mut tags = Vec::new();
    if let Some(category) = category {
        tags.push(category_tag(category).to_string());
    }
    tags.push(format!("rule_{id}"));
    tags
}

fn push_outputs(
    outputs: &mut Vec<OutputRule>,
    id: &str,
    action: &ActionConfig,
    tags: &[Tag],
    channels: &[String],
    enabled: bool,
) {
    // A `query` line raises no map alert at all, and a `clear` report raises
    // no sound (only the visual "clear" entry). Both are encoded as extra
    // `NoneTrue` guards so the migrated outputs behave like the old actions.
    let fires = Condition::AllTrue {
        tags: tags.to_vec(),
    };
    let not_query = Condition::NoneTrue {
        tags: vec![String::from("query")],
    };
    let not_query_or_clear = Condition::NoneTrue {
        tags: vec![String::from("query"), String::from("clear")],
    };
    match action {
        ActionConfig::Notify => outputs.push(OutputRule {
            id: format!("{id}_log"),
            kind: OutputKind::Log,
            mode: OutputMode::Emit,
            when: fires,
            channels: channels.to_vec(),
            enabled,
        }),
        ActionConfig::MapAlert { .. } => {
            outputs.push(OutputRule {
                id: format!("{id}_visual"),
                kind: OutputKind::Visual,
                mode: OutputMode::Emit,
                when: Condition::And {
                    all: vec![fires.clone(), not_query],
                },
                channels: channels.to_vec(),
                enabled,
            });
            outputs.push(OutputRule {
                id: format!("{id}_sound"),
                kind: OutputKind::Sound,
                mode: OutputMode::Emit,
                when: Condition::And {
                    all: vec![fires, not_query_or_clear],
                },
                channels: channels.to_vec(),
                enabled,
            });
        }
        ActionConfig::Ignore => {}
    }
}

/// A single rule match over a parsed line, carrying its tags.
///
/// Produced by [`DetectionEngine::evaluate_line`]. All captured text is
/// already sanitized.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Id of the detection rule that matched.
    pub rule_id: String,
    /// Named capture groups of the match, sanitized with
    /// [`sanitize_display`]. Dictionary matches expose the matched word under
    /// the `word` key.
    pub captures: HashMap<String, String>,
    /// Category configured for the matching rule.
    pub category: Option<IntelCategory>,
    /// Named capture group holding the reported solar system, for a rule that
    /// declares one; `None` for a dictionary match (its `word` capture is the
    /// system) or a rule with no system.
    pub system_group: Option<String>,
    /// Tags emitted by the matching rule.
    pub tags: Vec<Tag>,
    /// Byte range of the relevant text in the line's payload: the
    /// `system_group` for a rule that declares one, the whole match otherwise.
    pub span: Range<usize>,
    /// The text at [`Self::span`], sanitized with [`sanitize_display`].
    pub matched: String,
}

/// The detections of one parsed input line.
///
/// The routing stage joins the detections of an input line into a batch before
/// the output rules decide what to do with it.
#[derive(Debug, Clone)]
pub struct DetectionBatch {
    /// The parsed line the detections belong to.
    pub line: IntelLine,
    /// Detections of that line, in evaluation order.
    pub detections: Vec<Detection>,
}

impl DetectionBatch {
    /// The union of the tags of every detection in the batch.
    pub fn tags(&self) -> HashSet<Tag> {
        self.detections
            .iter()
            .flat_map(|detection| detection.tags.iter().cloned())
            .collect()
    }
}

/// A rule with its regex already compiled.
struct CompiledRule {
    id: String,
    regex: Regex,
    /// `None` means the rule applies to every channel.
    channels: Option<HashSet<String>>,
    category: Option<IntelCategory>,
    system_group: Option<String>,
    tags: Vec<Tag>,
    drop: bool,
}

/// A dictionary rule with its [`AhoCorasick`] automaton already built.
struct CompiledDictionary {
    id: String,
    automaton: AhoCorasick,
    /// `None` means the rule applies to every channel.
    channels: Option<HashSet<String>>,
    category: Option<IntelCategory>,
    tags: Vec<Tag>,
    drop: bool,
}

fn channels_set(channels: &[String]) -> Option<HashSet<String>> {
    if channels.is_empty() {
        None
    } else {
        Some(channels.iter().cloned().collect())
    }
}

/// Compiled detection engine.
///
/// Building an engine compiles every regex once; reuse the same instance for
/// every chunk of intel data.
pub struct DetectionEngine {
    set: RegexSet,
    rules: Vec<CompiledRule>,
    dictionaries: Vec<CompiledDictionary>,
}

impl DetectionEngine {
    /// Builds an engine from a rules configuration.
    ///
    /// Returns the engine plus one [`PatternError`] per skipped invalid rule.
    /// Fails only on structural problems: more than 64 regex rules
    /// ([`PatternError::TooManyRules`]), more than 16 dictionaries
    /// ([`PatternError::TooManyDictionaries`]) or a combined regex set that
    /// exceeds the size limits.
    pub fn from_config(config: &RulesConfig) -> Result<(Self, Vec<PatternError>), PatternError> {
        let enabled: Vec<&DetectionRule> =
            config.detections.iter().filter(|rule| rule.enabled).collect();
        let regex_count = enabled
            .iter()
            .filter(|rule| matches!(rule.kind, DetectionKind::Regex { .. }))
            .count();
        let dictionary_count = enabled.len() - regex_count;
        if regex_count > MAX_RULES {
            return Err(PatternError::TooManyRules(regex_count));
        }
        if dictionary_count > MAX_DICTIONARIES {
            return Err(PatternError::TooManyDictionaries(dictionary_count));
        }

        let mut errors = Vec::new();
        // Shared across regex and dictionary rules: ids must be unique in the
        // whole detection namespace.
        let mut seen_ids = HashSet::new();
        let mut rules = Vec::new();
        let mut set_patterns = Vec::new();

        for rule in &enabled {
            if let DetectionKind::Regex { pattern } = &rule.kind {
                match Self::compile_rule(rule, pattern, &mut seen_ids) {
                    Ok(compiled) => {
                        let set_pattern = if rule.case_insensitive {
                            format!("(?i:{pattern})")
                        } else {
                            pattern.clone()
                        };
                        set_patterns.push(set_pattern);
                        rules.push(compiled);
                    }
                    Err(error) => errors.push(error),
                }
            }
        }

        let set = if set_patterns.is_empty() {
            RegexSet::empty()
        } else {
            RegexSetBuilder::new(set_patterns)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_SIZE_LIMIT)
                .build()
                .map_err(|e| PatternError::InvalidPattern {
                    id: String::from("<regex_set>"),
                    reason: e.to_string(),
                })?
        };

        let mut dictionaries = Vec::new();
        for rule in &enabled {
            if let DetectionKind::Dictionary { words } = &rule.kind {
                match Self::compile_dictionary(rule, words, &mut seen_ids) {
                    Ok(compiled) => dictionaries.push(compiled),
                    Err(error) => errors.push(error),
                }
            }
        }

        let engine = Self {
            set,
            rules,
            dictionaries,
        };
        Ok((engine, errors))
    }

    fn compile_rule(
        rule: &DetectionRule,
        pattern: &str,
        seen_ids: &mut HashSet<String>,
    ) -> Result<CompiledRule, PatternError> {
        rule.validate()?;
        if !seen_ids.insert(rule.id.clone()) {
            return Err(PatternError::DuplicateId(rule.id.clone()));
        }
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(rule.case_insensitive)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| PatternError::InvalidPattern {
                id: rule.id.clone(),
                reason: e.to_string(),
            })?;
        if let Some(group) = &rule.system_group
            && !regex.capture_names().flatten().any(|name| name == group)
        {
            return Err(PatternError::InvalidSystemGroup {
                id: rule.id.clone(),
                group: group.clone(),
            });
        }
        if rule.category == Some(IntelCategory::Count)
            && !regex
                .capture_names()
                .flatten()
                .any(|name| name == COUNT_GROUP)
        {
            return Err(PatternError::MissingCountGroup(rule.id.clone()));
        }
        Ok(CompiledRule {
            id: rule.id.clone(),
            regex,
            channels: channels_set(&rule.channels),
            category: rule.category,
            system_group: rule.system_group.clone(),
            tags: rule.tags.clone(),
            drop: rule.drop,
        })
    }

    fn compile_dictionary(
        rule: &DetectionRule,
        words: &[String],
        seen_ids: &mut HashSet<String>,
    ) -> Result<CompiledDictionary, PatternError> {
        rule.validate()?;
        if !seen_ids.insert(rule.id.clone()) {
            return Err(PatternError::DuplicateId(rule.id.clone()));
        }
        let automaton = AhoCorasickBuilder::new()
            .ascii_case_insensitive(rule.case_insensitive)
            .match_kind(MatchKind::LeftmostLongest)
            .build(words)
            .map_err(|e| PatternError::DictionaryBuildFailed {
                id: rule.id.clone(),
                reason: e.to_string(),
            })?;
        Ok(CompiledDictionary {
            id: rule.id.clone(),
            automaton,
            channels: channels_set(&rule.channels),
            category: rule.category,
            tags: rule.tags.clone(),
            drop: rule.drop,
        })
    }

    /// Evaluates a single parsed line from a given channel and returns every
    /// detection, without running any action.
    ///
    /// A rule marked `drop` that applies to the channel drops the whole line
    /// before any other rule runs on it. At most 100 detections are reported
    /// per line.
    #[tracing::instrument(skip(self, line))]
    pub fn evaluate_line(&self, channel: &str, line: &IntelLine) -> Vec<Detection> {
        let mut results = Vec::new();
        let candidates = self.set.matches(&line.text);
        let dropped = candidates.iter().any(|index| {
            let rule = &self.rules[index];
            rule.drop
                && rule
                    .channels
                    .as_ref()
                    .is_none_or(|channels| channels.contains(channel))
        }) || self.dictionaries.iter().any(|dict| {
            dict.drop
                && dict
                    .channels
                    .as_ref()
                    .is_none_or(|channels| channels.contains(channel))
                && dict
                    .automaton
                    .find_iter(&line.text)
                    .any(|m| has_word_boundaries(&line.text, m.start(), m.end()))
        });
        if dropped {
            return results;
        }
        for index in candidates.iter() {
            if results.len() >= MAX_MATCHES_PER_CHUNK {
                break;
            }
            let rule = &self.rules[index];
            if let Some(channels) = &rule.channels
                && !channels.contains(channel)
            {
                continue;
            }
            let candidate_count = if rule.system_group.is_some() {
                MAX_SYSTEM_CANDIDATES
            } else {
                1
            };
            for caps in rule.regex.captures_iter(&line.text).take(candidate_count) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    break;
                }
                let span_group = match &rule.system_group {
                    Some(group) => caps.name(group),
                    None => caps.get(0),
                };
                let span = span_group.map_or(0..0, |m| m.range());
                let captures = rule
                    .regex
                    .capture_names()
                    .flatten()
                    .filter_map(|name| {
                        caps.name(name)
                            .map(|m| (name.to_string(), sanitize_display(m.as_str())))
                    })
                    .collect();
                results.push(Detection {
                    rule_id: rule.id.clone(),
                    captures,
                    category: rule.category,
                    system_group: rule.system_group.clone(),
                    tags: rule.tags.clone(),
                    matched: sanitize_display(&line.text[span.clone()]),
                    span,
                });
            }
        }
        for dict in &self.dictionaries {
            if results.len() >= MAX_MATCHES_PER_CHUNK {
                break;
            }
            if let Some(channels) = &dict.channels
                && !channels.contains(channel)
            {
                continue;
            }
            for m in dict.automaton.find_iter(&line.text) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    break;
                }
                if !has_word_boundaries(&line.text, m.start(), m.end()) {
                    continue;
                }
                let matched = sanitize_display(&line.text[m.start()..m.end()]);
                let mut captures = HashMap::new();
                captures.insert("word".to_string(), matched.clone());
                results.push(Detection {
                    rule_id: dict.id.clone(),
                    captures,
                    category: dict.category,
                    system_group: None,
                    tags: dict.tags.clone(),
                    matched,
                    span: m.range(),
                });
            }
        }
        results
    }

    /// Parses and evaluates a chunk of log data from a given channel and
    /// returns every detection, without running any action. Lines are
    /// truncated to 2 KiB and at most 100 detections are reported per chunk.
    #[tracing::instrument(skip(self, data))]
    pub fn evaluate(&self, channel: &str, data: &str) -> Vec<Detection> {
        let mut results = Vec::new();
        for raw_line in data.lines() {
            if results.len() >= MAX_MATCHES_PER_CHUNK {
                break;
            }
            let Some(line) = parse_line(truncate_str(raw_line, MAX_LINE_LEN)) else {
                continue;
            };
            for detection in self.evaluate_line(channel, &line) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    break;
                }
                results.push(detection);
            }
        }
        results
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

/// The rules configuration of the shipped template, for tests elsewhere in
/// the crate.
#[cfg(test)]
pub(crate) fn template_rules_config() -> RulesConfig {
    use crate::patterns::DEFAULT_PATTERNS_TOML;
    let legacy: PatternConfig =
        toml::from_str(DEFAULT_PATTERNS_TOML).expect("the shipped template must parse");
    RulesConfig::from_pattern_config(&legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(items: &[&str]) -> HashSet<Tag> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn list(items: &[&str]) -> Vec<Tag> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn regex_rule(id: &str, pattern: &str, rule_tags: &[&str]) -> DetectionRule {
        DetectionRule {
            id: id.to_string(),
            kind: DetectionKind::Regex {
                pattern: pattern.to_string(),
            },
            case_insensitive: false,
            channels: Vec::new(),
            enabled: true,
            drop: false,
            category: None,
            system_group: None,
            tags: rule_tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn condition_quantifiers() {
        let present = tags(&["ship", "hostile"]);
        assert!(Condition::Always.evaluate(&present));
        assert!(
            Condition::AllTrue {
                tags: list(&["ship", "hostile"])
            }
            .evaluate(&present)
        );
        assert!(
            !Condition::AllTrue {
                tags: list(&["ship", "capital"])
            }
            .evaluate(&present)
        );
        assert!(
            Condition::AnyTrue {
                tags: list(&["capital", "hostile"])
            }
            .evaluate(&present)
        );
        assert!(
            Condition::NoneTrue {
                tags: list(&["capital"])
            }
            .evaluate(&present)
        );
        assert!(
            Condition::AtLeastOneFalse {
                tags: list(&["ship", "capital"])
            }
            .evaluate(&present)
        );
        assert!(
            Condition::AtLeastNTrue {
                n: 2,
                tags: list(&["ship", "hostile", "capital"])
            }
            .evaluate(&present)
        );
        assert!(
            !Condition::AtLeastNTrue {
                n: 3,
                tags: list(&["ship", "hostile", "capital"])
            }
            .evaluate(&present)
        );
        assert!(
            Condition::Or {
                any: vec![
                    Condition::NoneTrue {
                        tags: list(&["ship"])
                    },
                    Condition::AllTrue {
                        tags: list(&["ship"])
                    },
                ]
            }
            .evaluate(&present)
        );
        assert!(
            Condition::Not {
                not: Box::new(Condition::NoneTrue {
                    tags: list(&["ship"])
                })
            }
            .evaluate(&present)
        );
    }

    #[test]
    fn invalid_condition_quantifier_is_reported() {
        let config = RulesConfig {
            inputs: Vec::new(),
            detections: Vec::new(),
            outputs: vec![OutputRule {
                id: "out".to_string(),
                kind: OutputKind::Log,
                mode: OutputMode::Emit,
                when: Condition::AtLeastNTrue {
                    n: 3,
                    tags: list(&["a"]),
                },
                channels: Vec::new(),
                enabled: true,
            }],
        };
        let errors = config.validate();
        assert!(matches!(errors[..], [PatternError::InvalidCondition { .. }]));
    }

    #[test]
    fn engine_reports_detections_with_tags() {
        let config = RulesConfig {
            inputs: Vec::new(),
            detections: vec![regex_rule(
                "system_reported",
                r"(?P<system>[A-Z0-9]{1,5}-[A-Z0-9]{1,4})",
                &["ship"],
            )],
            outputs: Vec::new(),
        };
        let (engine, errors) = DetectionEngine::from_config(&config).unwrap();
        assert!(errors.is_empty());
        let detections = engine.evaluate("intel", "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A");
        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].rule_id, "system_reported");
        assert_eq!(detections[0].captures["system"], "1DQ1-A");
        assert_eq!(detections[0].tags, vec!["ship".to_string()]);
    }

    #[test]
    fn drop_rule_suppresses_the_whole_line() {
        let mut drop_rule = regex_rule("motd", "message of the day", &[]);
        drop_rule.drop = true;
        let config = RulesConfig {
            inputs: Vec::new(),
            detections: vec![drop_rule, regex_rule("any", ".+", &["all"])],
            outputs: Vec::new(),
        };
        let (engine, _) = DetectionEngine::from_config(&config).unwrap();
        let detections = engine.evaluate(
            "intel",
            "[ 2021.09.08 22:56:47 ] Some Pilot > message of the day",
        );
        assert!(detections.is_empty());
    }

    #[test]
    fn dictionary_detection_exposes_the_word() {
        let config = RulesConfig {
            inputs: Vec::new(),
            detections: vec![DetectionRule {
                id: "ships".to_string(),
                kind: DetectionKind::Dictionary {
                    words: vec!["Sabre".to_string()],
                },
                case_insensitive: true,
                channels: Vec::new(),
                enabled: true,
                drop: false,
                category: None,
                system_group: None,
                tags: vec!["ship".to_string()],
            }],
            outputs: Vec::new(),
        };
        let (engine, _) = DetectionEngine::from_config(&config).unwrap();
        let detections = engine.evaluate("intel", "[ 2021.09.08 22:56:47 ] Some Pilot > sabre");
        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].captures["word"], "sabre");
        assert_eq!(detections[0].matched, "sabre");
    }

    #[test]
    fn from_pattern_config_maps_actions_to_outputs() {
        let toml = r#"
[[patterns]]
id = "clear_report"
pattern = '\bclear\b'
category = "clear"
action = { type = "map_alert", system_group = "system" }

[[patterns]]
id = "chat"
pattern = '.+'
action = { type = "notify" }
"#;
        let legacy: PatternConfig = toml::from_str(toml).unwrap();
        let rules = RulesConfig::from_pattern_config(&legacy);

        assert_eq!(rules.detections.len(), 2);
        assert_eq!(rules.detections[0].tags, vec!["clear", "rule_clear_report"]);
        assert_eq!(rules.detections[0].system_group.as_deref(), Some("system"));
        assert_eq!(rules.detections[1].tags, vec!["rule_chat"]);

        let ids: Vec<&str> = rules.outputs.iter().map(|o| o.id.as_str()).collect();
        assert!(ids.contains(&"clear_report_visual"));
        assert!(ids.contains(&"clear_report_sound"));
        assert!(ids.contains(&"chat_log"));
        assert!(!ids.contains(&"clear_report_log"));
        assert!(rules.validate().is_empty(), "{:?}", rules.validate());
    }

    #[test]
    fn detection_rule_round_trips_through_toml() {
        let toml = r#"
id = "ships"
kind = { type = "dictionary", words = ["Sabre", "Vedmak"] }
case_insensitive = true
tags = ["ship"]
"#;
        let rule: DetectionRule = toml::from_str(toml).unwrap();
        assert_eq!(rule.id, "ships");
        assert!(matches!(rule.kind, DetectionKind::Dictionary { .. }));

        let regex = r#"
id = "sys"
kind = { type = "regex", pattern = '(?P<system>[A-Z0-9-]+)' }
system_group = "system"
tags = ["ship"]
"#;
        let rule: DetectionRule = toml::from_str(regex).unwrap();
        assert_eq!(rule.system_group.as_deref(), Some("system"));
        assert!(matches!(rule.kind, DetectionKind::Regex { .. }));
    }
}
