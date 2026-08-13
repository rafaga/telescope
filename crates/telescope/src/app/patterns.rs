//! Pattern matching engine for intel text streams.
//!
//! Telescope monitors EVE Online chat log files; this module evaluates the
//! text flowing from those files against a configurable set of regex rules,
//! producing notifications or map alerts for the player.
//!
//! # Architecture
//!
//! The engine works in two stages:
//!
//! 1. **Line parsing**: a single anchored regex converts each raw log line
//!    into an [`IntelLine`] (timestamp, author, text payload).
//! 2. **Rule evaluation**: all enabled rules are combined into a
//!    [`regex::RegexSet`] so candidate rules are found in a single automaton
//!    pass per line. Only candidate rules run their individual regex to
//!    extract capture groups.
//!
//! Regexes are compiled once at startup (see [`PatternEngine`]) and each
//! chunk is evaluated single-threaded, keeping the per-line cost minimal.
//!
//! # Rule file format (`patterns.toml`)
//!
//! Rules are declared in an external `patterns.toml` file located next to
//! the application configuration. Each rule is a `[[patterns]]` table with
//! the following fields:
//!
//! | Field | Type | Default | Description |
//! |-------|------|---------|-------------|
//! | `id` | string | *(required)* | Unique rule id. Allowed chars: `[A-Za-z0-9_-]`, max 64. |
//! | `pattern` | string | *(required)* | [Rust regex](https://docs.rs/regex) evaluated against the text payload of each parsed log line. Max 1024 chars. TOML literal strings (`'...'`) are recommended so backslashes need no escaping. |
//! | `case_insensitive` | bool | `false` | Match the pattern case-insensitively. |
//! | `channels` | list of strings | `[]` | Restrict the rule to the given channel names; empty means every monitored channel. |
//! | `enabled` | bool | `true` | Disabled rules are skipped silently at load time. |
//! | `action` | table | `{ type = "notify" }` | What to do when the rule matches; see *Available actions*. |
//!
//! Unknown fields are rejected at load time (`deny_unknown_fields`).
//!
//! ```toml
//! [[patterns]]
//! id = "clear_report"
//! pattern = '\b(clear|clr)\b'
//! case_insensitive = true
//! channels = ["my-intel-channel"]
//! action = { type = "notify" }
//! ```
//!
//! # Available actions
//!
//! | `type` | Extra fields | Effect |
//! |--------|--------------|--------|
//! | `notify` | — | Sends the matched line to the application notification log. |
//! | `map_alert` | `system_group` | Resolves the named capture group as a solar system and highlights it on the maps. The group must exist in the pattern (validated at load time). |
//!
//! # Default rules shipped in the template
//!
//! The embedded template (the repository's own `patterns.toml`) ships these
//! rules:
//!
//! | Rule id | Matches | Action | State |
//! |---------|---------|--------|-------|
//! | `intel_line` | Every parsed intel line. | `notify` | active |
//! | `system_reported` | Any EVE solar system name; the pattern covers 100% of the 8436 `solarSystemName` values in `assets/sde.db` (wormholes `J\d{6}`, nullsec-style `1DQ1-A`/`B-R5RB`, coded `AD001`, named systems like `Jita` or `Tash-Murkon Prime`). | `map_alert` | active |
//! | `clear_report` | `clear` / `clr` keywords (case-insensitive). | `notify` | active |
//! | `hostile_report` | `hostile` / `neut` / `red` keywords (case-insensitive). | `notify` | commented out |
//! | `capital_report` | Capital hull / `cyno` keywords (case-insensitive). | `notify` | commented out |
//!
//! # Validation and sanitization
//!
//! Every input read from `patterns.toml` is validated before use:
//!
//! * Rule ids, channel names and capture group names are checked against
//!   strict character allowlists and length limits.
//! * Patterns are compiled with bounded `size_limit`/`dfa_size_limit` so a
//!   huge pattern cannot exhaust memory at compile time. The `regex` crate
//!   guarantees linear-time matching, so classic ReDoS is not exploitable.
//! * `map_alert` actions are validated at load time against the compiled
//!   regex capture names, so a missing group fails fast instead of at
//!   runtime.
//! * At evaluation time, lines are truncated to 2 KiB, matches per chunk
//!   are capped at 100, and all captured text is sanitized with
//!   [`sanitize_display`] (control characters stripped, length capped)
//!   before it reaches the UI.
//!
//! Invalid individual rules are skipped and reported through
//! [`PatternError`]; the remaining rules keep loading.
//!
//! # File regeneration
//!
//! [`PatternEngine::load_or_create`] regenerates `patterns.toml` from an
//! embedded template when it is missing or corrupted. A corrupted file is
//! backed up as `patterns.toml.bak` before being replaced; a missing file
//! (normal on first run) is recreated silently. If the file cannot be
//! written, the engine falls back to the embedded default rules
//! ([`PatternEngine::with_defaults`]).

use chrono::{DateTime, NaiveDateTime, Utc};
use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Maximum number of rules allowed in a configuration file.
const MAX_RULES: usize = 64;
/// Maximum length of a rule id.
const MAX_ID_LEN: usize = 64;
/// Maximum length of a regex pattern source.
const MAX_PATTERN_LEN: usize = 1024;
/// Maximum number of channels in a single rule filter.
const MAX_CHANNELS: usize = 32;
/// Maximum length of a channel name.
const MAX_CHANNEL_LEN: usize = 64;
/// Compiled regex size limit (bytes) to prevent memory exhaustion.
const REGEX_SIZE_LIMIT: usize = 10 * (1 << 20);
/// Maximum length of a single log line evaluated by the engine.
const MAX_LINE_LEN: usize = 2048;
/// Maximum number of matches reported per evaluated chunk.
const MAX_MATCHES_PER_CHUNK: usize = 100;
/// Maximum length of captured text displayed in notifications.
const MAX_DISPLAY_LEN: usize = 200;

/// Regex that parses a standard EVE Online chat log line:
/// `[ 2021.09.08 22:56:47 ] Character Name > message`
const LINE_PATTERN: &str =
    r"^\[\s(?P<ts>\d{4}\.\d{2}\.\d{2}\s\d{2}:\d{2}:\d{2})\s\]\s(?P<author>.+?)\s>\s(?P<text>.+)$";

/// Timestamp format used by EVE Online chat logs (UTC).
const LINE_TIMESTAMP_FORMAT: &str = "%Y.%m.%d %H:%M:%S";

/// Default configuration used when `patterns.toml` cannot be regenerated.
const DEFAULT_RULE_ID: &str = "intel_line";
const DEFAULT_RULE_PATTERN: &str = ".+";

/// Embedded template used to (re)generate `patterns.toml` when the file is
/// missing or corrupted. This is the repository's own `patterns.toml`, so
/// the regenerated file always matches the shipped template.
const DEFAULT_PATTERNS_TOML: &str = include_str!("../../../../patterns.toml");

fn default_true() -> bool {
    true
}

/// Action executed when a rule matches a line.
///
/// In `patterns.toml` the action is declared as an inline table whose `type`
/// key selects the variant:
///
/// ```toml
/// action = { type = "notify" }
/// action = { type = "map_alert", system_group = "system" }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionConfig {
    /// Send the matched line to the application notification log.
    #[default]
    Notify,
    /// Resolve the named capture group as a solar system and emit a map
    /// notification for it. The group name must exist in the rule pattern;
    /// this is validated when the rule is loaded.
    MapAlert {
        /// Name of the capture group holding the solar system name.
        system_group: String,
    },
}

/// Declarative configuration of a single pattern rule.
///
/// Maps one `[[patterns]]` table of `patterns.toml`; see the
/// [module-level documentation](self) for the full field reference. All
/// fields are validated and sanitized when the engine is built
/// (see [`PatternError`]).
///
/// # Examples
///
/// ```
/// use telescope::patterns::{ActionConfig, PatternRuleConfig};
///
/// let rule = PatternRuleConfig {
///     id: "clear_report".to_string(),
///     pattern: "\\b(clear|clr)\\b".to_string(),
///     case_insensitive: true,
///     channels: vec!["my-intel-channel".to_string()],
///     enabled: true,
///     action: ActionConfig::Notify,
/// };
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternRuleConfig {
    /// Unique rule identifier (`[A-Za-z0-9_-]`, max 64 chars).
    pub id: String,
    /// Regex source evaluated against the text payload of each log line
    /// (max 1024 chars).
    pub pattern: String,
    /// Whether the pattern is matched case-insensitively.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Optional channel filter; empty means the rule applies to every channel.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Disabled rules are skipped silently at load time.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Action executed when the rule matches.
    #[serde(default)]
    pub action: ActionConfig,
}

impl PatternRuleConfig {
    /// Validates all sanitizable fields of the rule. Pattern compilation and
    /// capture group checks are done by the engine because they need the
    /// compiled regex.
    fn validate(&self) -> Result<(), PatternError> {
        if !is_valid_id(&self.id) {
            return Err(PatternError::InvalidId(self.id.clone()));
        }
        if self.pattern.is_empty() || self.pattern.len() > MAX_PATTERN_LEN {
            return Err(PatternError::PatternTooLong(self.id.clone()));
        }
        if self.channels.len() > MAX_CHANNELS {
            return Err(PatternError::TooManyChannels(self.id.clone()));
        }
        for channel in &self.channels {
            if !is_valid_channel(channel) {
                return Err(PatternError::InvalidChannel(channel.clone()));
            }
        }
        if let ActionConfig::MapAlert { system_group } = &self.action
            && !is_valid_group_name(system_group)
        {
            return Err(PatternError::InvalidSystemGroup {
                id: self.id.clone(),
                group: system_group.clone(),
            });
        }
        Ok(())
    }
}

/// Root structure of the `patterns.toml` configuration file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternConfig {
    /// The list of declared rules (max 64; must not be empty).
    pub patterns: Vec<PatternRuleConfig>,
}

/// A parsed EVE Online chat log line.
///
/// Produced by [`PatternEngine::parse_line`] from raw lines such as
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

/// A rule match over a parsed line. All captured text is already sanitized.
///
/// Produced by [`PatternEngine::evaluate`]; at most one match per rule per
/// line is reported.
#[derive(Debug, Clone)]
pub struct PatternMatch {
    /// Id of the rule that matched.
    pub rule_id: String,
    /// The parsed line that matched.
    pub line: IntelLine,
    /// Named capture groups of the match, sanitized with
    /// [`sanitize_display`]. Used by [`ActionConfig::MapAlert`] to locate the
    /// reported solar system.
    pub named: HashMap<String, String>,
    /// Action configured for the matching rule.
    pub action: ActionConfig,
}

/// A rule with its regex already compiled.
struct CompiledRule {
    id: String,
    regex: Regex,
    /// `None` means the rule applies to every channel.
    channels: Option<HashSet<String>>,
    action: ActionConfig,
}

/// Errors produced while loading or validating pattern rules.
///
/// Structural errors ([`IoError`](Self::IoError),
/// [`ParseError`](Self::ParseError), [`EmptyConfig`](Self::EmptyConfig) and
/// [`TooManyRules`](Self::TooManyRules)) abort the load of the whole file;
/// the remaining variants describe individual rules that were skipped while
/// the rest of the file kept loading.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternError {
    /// The patterns file could not be read or written.
    IoError(String),
    /// The patterns file is not valid TOML or violates the schema
    /// (e.g. unknown fields).
    ParseError(String),
    /// The file does not define any rule.
    EmptyConfig,
    /// The file defines more rules than allowed (max 64).
    TooManyRules(usize),
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
}

impl Display for PatternError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "cannot read patterns file: {msg}"),
            Self::ParseError(msg) => write!(f, "invalid patterns file: {msg}"),
            Self::EmptyConfig => write!(f, "patterns file does not define any rule"),
            Self::TooManyRules(count) => {
                write!(f, "too many pattern rules ({count}, max {MAX_RULES})")
            }
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
                "invalid channel name '{name}' (use [A-Za-z0-9_-], max {MAX_CHANNEL_LEN} chars)"
            ),
            Self::InvalidSystemGroup { id, group } => write!(
                f,
                "rule '{id}' references the capture group '{group}' which is invalid or missing in its pattern"
            ),
        }
    }
}

impl Error for PatternError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Report produced by [`PatternEngine::load_or_create`].
pub struct LoadReport {
    /// The compiled engine, ready to use.
    pub engine: PatternEngine,
    /// Errors found while loading: the original load error (if any) plus one
    /// entry per skipped invalid rule.
    pub errors: Vec<PatternError>,
    /// Whether `patterns.toml` was (re)generated from the embedded template.
    pub regenerated: bool,
    /// Path of the backup copy of a corrupted file, when one was made.
    pub backup: Option<PathBuf>,
}

/// Compiled pattern matching engine.
///
/// Building an engine compiles every regex once; reuse the same instance
/// for every chunk of intel data. Use [`load_or_create`](Self::load_or_create)
/// for the standard startup flow (load `patterns.toml`, regenerating it if
/// needed), or [`from_config`](Self::from_config) /
/// [`with_defaults`](Self::with_defaults) for programmatic setups.
///
/// # Examples
///
/// ```
/// use telescope::patterns::PatternEngine;
///
/// // Fallback engine: notifies every parsed intel line.
/// let engine = PatternEngine::with_defaults();
/// let matches = engine.evaluate(
///     "intel",
///     "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear",
/// );
/// assert_eq!(matches.len(), 1);
/// ```
pub struct PatternEngine {
    line_re: Regex,
    set: RegexSet,
    rules: Vec<CompiledRule>,
}

impl PatternEngine {
    /// Loads and compiles the rules from a `patterns.toml` file.
    ///
    /// Structural errors (unreadable file, invalid TOML, no rules, too many
    /// rules) abort the load. Invalid individual rules are skipped and
    /// returned in the error list so the caller can report them.
    ///
    /// Prefer [`load_or_create`](Self::load_or_create) when the file is
    /// expected to be recreated automatically on failure.
    pub fn load(path: &Path) -> Result<(Self, Vec<PatternError>), PatternError> {
        let mut data = String::new();
        let read_result = File::open(path).and_then(|mut file| file.read_to_string(&mut data));
        if read_result.is_err() {
            return Err(PatternError::IoError(path.display().to_string()));
        }
        let config = toml::from_str::<PatternConfig>(&data)
            .map_err(|e| PatternError::ParseError(e.to_string()))?;
        Self::from_config(&config)
    }

    /// Loads the rules from `path`, regenerating the file from the embedded
    /// template when it is missing or corrupted.
    ///
    /// Behavior:
    ///
    /// * **Valid file**: loaded as-is; [`LoadReport::regenerated`] is
    ///   `false`.
    /// * **Missing file** (normal on first run): recreated silently with the
    ///   default content; no error is reported.
    /// * **Corrupted file**: backed up as `<file>.bak`, the original error
    ///   is reported in [`LoadReport::errors`] and the file is regenerated
    ///   with the default content.
    /// * **Unwritable location**: the engine falls back to
    ///   [`with_defaults`](Self::with_defaults) and the I/O error is
    ///   reported.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telescope::patterns::PatternEngine;
    /// use std::path::Path;
    ///
    /// let report = PatternEngine::load_or_create(Path::new("patterns.toml"));
    /// for error in &report.errors {
    ///     eprintln!("pattern rule skipped: {error}");
    /// }
    /// let engine = report.engine;
    /// ```
    pub fn load_or_create(path: &Path) -> LoadReport {
        let load_error = match Self::load(path) {
            Ok((engine, errors)) => {
                return LoadReport {
                    engine,
                    errors,
                    regenerated: false,
                    backup: None,
                };
            }
            Err(error) => error,
        };

        // A missing file is normal on first run, so it is not reported as an
        // error; a corrupted file is backed up and its error is reported.
        let file_existed = path.exists();
        let mut backup = None;
        let mut errors = Vec::new();
        if file_existed {
            errors.push(load_error);
            let backup_path = path.with_extension("toml.bak");
            let _ = std::fs::remove_file(&backup_path);
            if std::fs::rename(path, &backup_path).is_ok() {
                backup = Some(backup_path);
            }
        }
        if let Err(e) = std::fs::write(path, DEFAULT_PATTERNS_TOML) {
            errors.push(PatternError::IoError(format!(
                "cannot write {}: {e}",
                path.display()
            )));
            return LoadReport {
                engine: Self::with_defaults(),
                errors,
                regenerated: false,
                backup,
            };
        }

        match Self::load(path) {
            Ok((engine, mut rule_errors)) => {
                errors.append(&mut rule_errors);
                LoadReport {
                    engine,
                    errors,
                    regenerated: true,
                    backup,
                }
            }
            Err(error) => {
                errors.push(error);
                LoadReport {
                    engine: Self::with_defaults(),
                    errors,
                    regenerated: true,
                    backup,
                }
            }
        }
    }

    /// Builds an engine from an already parsed configuration.
    ///
    /// Returns the engine plus one [`PatternError`] per skipped invalid rule.
    /// Fails only on structural problems: no rules at all
    /// ([`PatternError::EmptyConfig`]), more than 64 rules
    /// ([`PatternError::TooManyRules`]) or a combined regex set that exceeds
    /// the size limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use telescope::patterns::{ActionConfig, PatternConfig, PatternEngine, PatternRuleConfig};
    ///
    /// let config = PatternConfig {
    ///     patterns: vec![PatternRuleConfig {
    ///         id: "clear_report".to_string(),
    ///         pattern: "\\b(clear|clr)\\b".to_string(),
    ///         case_insensitive: true,
    ///         channels: vec![],
    ///         enabled: true,
    ///         action: ActionConfig::Notify,
    ///     }],
    /// };
    /// let (engine, errors) = PatternEngine::from_config(&config).unwrap();
    /// assert!(errors.is_empty());
    /// ```
    pub fn from_config(config: &PatternConfig) -> Result<(Self, Vec<PatternError>), PatternError> {
        if config.patterns.is_empty() {
            return Err(PatternError::EmptyConfig);
        }
        if config.patterns.len() > MAX_RULES {
            return Err(PatternError::TooManyRules(config.patterns.len()));
        }

        let mut errors = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut rules = Vec::new();
        let mut set_patterns = Vec::new();

        for rule_config in &config.patterns {
            if !rule_config.enabled {
                continue;
            }
            match Self::compile_rule(rule_config, &mut seen_ids) {
                Ok(compiled) => {
                    // Inline the case-insensitive flag so each pattern in the
                    // set keeps its own flags.
                    let set_pattern = if rule_config.case_insensitive {
                        format!("(?i:{})", rule_config.pattern)
                    } else {
                        rule_config.pattern.clone()
                    };
                    set_patterns.push(set_pattern);
                    rules.push(compiled);
                }
                Err(error) => errors.push(error),
            }
        }

        let set = RegexSetBuilder::new(set_patterns)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| PatternError::InvalidPattern {
                id: String::from("<regex_set>"),
                reason: e.to_string(),
            })?;

        let engine = Self {
            line_re: Regex::new(LINE_PATTERN).expect("hardcoded line regex must compile"),
            set,
            rules,
        };
        Ok((engine, errors))
    }

    /// Compiles and fully validates a single rule.
    fn compile_rule(
        rule_config: &PatternRuleConfig,
        seen_ids: &mut HashSet<String>,
    ) -> Result<CompiledRule, PatternError> {
        rule_config.validate()?;
        if !seen_ids.insert(rule_config.id.clone()) {
            return Err(PatternError::DuplicateId(rule_config.id.clone()));
        }
        let regex = RegexBuilder::new(&rule_config.pattern)
            .case_insensitive(rule_config.case_insensitive)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|e| PatternError::InvalidPattern {
                id: rule_config.id.clone(),
                reason: e.to_string(),
            })?;
        if let ActionConfig::MapAlert { system_group } = &rule_config.action
            && !regex
                .capture_names()
                .flatten()
                .any(|name| name == system_group)
        {
            return Err(PatternError::InvalidSystemGroup {
                id: rule_config.id.clone(),
                group: system_group.clone(),
            });
        }
        Ok(CompiledRule {
            id: rule_config.id.clone(),
            regex,
            channels: if rule_config.channels.is_empty() {
                None
            } else {
                Some(rule_config.channels.iter().cloned().collect())
            },
            action: rule_config.action.clone(),
        })
    }

    /// Builds an engine with the embedded default rules (notify every parsed
    /// intel line). Used as fallback when the external configuration is
    /// missing or invalid.
    pub fn with_defaults() -> Self {
        let config = PatternConfig {
            patterns: vec![PatternRuleConfig {
                id: String::from(DEFAULT_RULE_ID),
                pattern: String::from(DEFAULT_RULE_PATTERN),
                case_insensitive: false,
                channels: Vec::new(),
                enabled: true,
                action: ActionConfig::Notify,
            }],
        };
        Self::from_config(&config)
            .expect("embedded default rules must compile")
            .0
    }

    /// Parses a raw log line into an [`IntelLine`]. Returns `None` when the
    /// line does not follow the EVE chat log format
    /// (`[ yyyy.MM.dd hh:mm:ss ] Author > message`).
    ///
    /// # Examples
    ///
    /// ```
    /// use telescope::patterns::PatternEngine;
    ///
    /// let engine = PatternEngine::with_defaults();
    /// let line = engine
    ///     .parse_line("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear")
    ///     .unwrap();
    /// assert_eq!(line.author, "Some Pilot");
    /// assert_eq!(line.text, "1DQ1-A clear");
    /// assert!(engine.parse_line("not a log line").is_none());
    /// ```
    pub fn parse_line(&self, raw: &str) -> Option<IntelLine> {
        let caps = self.line_re.captures(raw)?;
        let naive_ts =
            NaiveDateTime::parse_from_str(caps.name("ts")?.as_str(), LINE_TIMESTAMP_FORMAT).ok()?;
        Some(IntelLine {
            timestamp: naive_ts.and_utc(),
            author: caps.name("author")?.as_str().to_string(),
            text: caps.name("text")?.as_str().to_string(),
        })
    }

    /// Evaluates a chunk of log data from a given channel and returns every
    /// rule match.
    ///
    /// Processing is single-threaded and allocation-conscious: the combined
    /// [`regex::RegexSet`] finds candidate rules in one pass per line and
    /// only those candidates extract captures. Lines that do not follow the
    /// EVE chat log format are skipped, lines are truncated to 2 KiB, and at
    /// most 100 matches are reported per chunk.
    ///
    /// # Arguments
    ///
    /// * `channel` - name of the intel channel the data comes from (the file
    ///   name prefix before the first underscore); used by the per-rule
    ///   channel filter.
    /// * `data` - raw text read from the log file, possibly multi-line.
    ///
    /// # Examples
    ///
    /// ```
    /// use telescope::patterns::{ActionConfig, PatternConfig, PatternEngine, PatternRuleConfig};
    ///
    /// let config = PatternConfig {
    ///     patterns: vec![PatternRuleConfig {
    ///         id: "system_reported".to_string(),
    ///         pattern: "(?P<system>[A-Z0-9]{1,5}-[A-Z0-9]{1,4})".to_string(),
    ///         case_insensitive: false,
    ///         channels: vec![],
    ///         enabled: true,
    ///         action: ActionConfig::MapAlert {
    ///             system_group: "system".to_string(),
    ///         },
    ///     }],
    /// };
    /// let (engine, _) = PatternEngine::from_config(&config).unwrap();
    /// let matches = engine.evaluate(
    ///     "intel",
    ///     "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear",
    /// );
    /// assert_eq!(matches[0].named["system"], "1DQ1-A");
    /// ```
    pub fn evaluate(&self, channel: &str, data: &str) -> Vec<PatternMatch> {

        let mut results = Vec::new();
        for raw_line in data.lines() {
            if results.len() >= MAX_MATCHES_PER_CHUNK {
                break;
            }
            let Some(line) = self.parse_line(truncate_str(raw_line, MAX_LINE_LEN)) else {
                continue;
            };
            let candidates = self.set.matches(&line.text);
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
                if let Some(caps) = rule.regex.captures(&line.text) {
                    let named = rule
                        .regex
                        .capture_names()
                        .flatten()
                        .filter_map(|name| {
                            caps.name(name)
                                .map(|m| (name.to_string(), sanitize_display(m.as_str())))
                        })
                        .collect();
                    results.push(PatternMatch {
                        rule_id: rule.id.clone(),
                        line: line.clone(),
                        named,
                        action: rule.action.clone(),
                    });
                }
            }
        }
        results
    }
}

/// Strips control characters (newlines, ANSI escapes, etc.) and truncates the
/// input to 200 characters so captured text can be safely displayed in the UI
/// without forging log lines or breaking the layout.
///
/// # Examples
///
/// ```
/// use telescope::patterns::sanitize_display;
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
fn truncate_str(input: &str, max: usize) -> &str {
    if input.len() <= max {
        return input;
    }
    let mut end = max;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_valid_channel(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CHANNEL_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_valid_group_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LINE: &str = "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear";
    const SAMPLE_LINE_2: &str = "[ 2024.01.15 08:00:01 ] Another Pilot > docked in Jita";

    fn rule(id: &str, pattern: &str) -> PatternRuleConfig {
        PatternRuleConfig {
            id: id.to_string(),
            pattern: pattern.to_string(),
            case_insensitive: false,
            channels: Vec::new(),
            enabled: true,
            action: ActionConfig::Notify,
        }
    }

    fn engine_with(rules: Vec<PatternRuleConfig>) -> (PatternEngine, Vec<PatternError>) {
        PatternEngine::from_config(&PatternConfig { patterns: rules }).expect("engine should build")
    }

    #[test]
    fn parses_valid_eve_line() {
        let (engine, errors) = engine_with(vec![rule("r1", ".+")]);
        assert!(errors.is_empty());
        let line = engine.parse_line(SAMPLE_LINE).expect("line should parse");
        assert_eq!(
            line.timestamp,
            NaiveDateTime::parse_from_str("2021.09.08 22:56:47", LINE_TIMESTAMP_FORMAT)
                .unwrap()
                .and_utc()
        );
        assert_eq!(line.author, "Some Pilot");
        assert_eq!(line.text, "1DQ1-A clear");
    }

    #[test]
    fn rejects_invalid_lines() {
        let (engine, _) = engine_with(vec![rule("r1", ".+")]);
        assert!(engine.parse_line("not a log line").is_none());
        assert!(
            engine
                .parse_line("[ 2021.09.08 22:56:47 ] No message terminator")
                .is_none()
        );
        assert!(
            engine
                .parse_line("[ 99.09.08 22:56:47 ] Bad Timestamp > hello")
                .is_none()
        );
        assert!(engine.parse_line("").is_none());
    }

    #[test]
    fn evaluate_matches_and_extracts_captures() {
        let (engine, errors) = engine_with(vec![rule(
            "system_report",
            r"(?P<system>[A-Z0-9]{1,4}-[A-Z0-9]{1,5})",
        )]);
        assert!(errors.is_empty());
        let matches = engine.evaluate("intel", SAMPLE_LINE);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "system_report");
        assert_eq!(matches[0].named.get("system").unwrap(), "1DQ1-A");
        assert_eq!(matches[0].action, ActionConfig::Notify);
    }

    #[test]
    fn evaluate_is_case_insensitive_when_configured() {
        let mut insensitive = rule("insensitive", "clear");
        insensitive.case_insensitive = true;
        let (engine, _) = engine_with(vec![insensitive]);
        assert_eq!(engine.evaluate("intel", SAMPLE_LINE).len(), 1);

        let (engine, _) = engine_with(vec![rule("sensitive", "CLEAR")]);
        assert!(engine.evaluate("intel", SAMPLE_LINE).is_empty());
    }

    #[test]
    fn channel_filter_limits_rules() {
        let mut filtered = rule("filtered", "clear");
        filtered.channels = vec![String::from("other-channel")];
        let (engine, _) = engine_with(vec![filtered]);
        assert!(engine.evaluate("intel", SAMPLE_LINE).is_empty());
        assert_eq!(engine.evaluate("other-channel", SAMPLE_LINE).len(), 1);
    }

    #[test]
    fn only_matching_rules_are_reported() {
        let (engine, _) = engine_with(vec![rule("hit", "clear"), rule("miss", "hostile tackled")]);
        let matches = engine.evaluate("intel", SAMPLE_LINE);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "hit");
    }

    #[test]
    fn evaluates_multiple_lines_and_skips_garbage() {
        let (engine, _) = engine_with(vec![rule("any", ".+")]);
        let chunk = format!("{SAMPLE_LINE}\ngarbage line\n{SAMPLE_LINE_2}");
        assert_eq!(engine.evaluate("intel", &chunk).len(), 2);
    }

    #[test]
    fn rejects_invalid_ids() {
        let long_id = "x".repeat(MAX_ID_LEN + 1);
        for bad_id in ["", "bad id", "bad$id", long_id.as_str()] {
            let (engine, errors) = engine_with(vec![rule(bad_id, ".+")]);
            assert_eq!(errors, vec![PatternError::InvalidId(bad_id.to_string())]);
            assert_eq!(engine.evaluate("intel", SAMPLE_LINE).len(), 0);
        }
    }

    #[test]
    fn rejects_duplicate_ids() {
        let (_, errors) = engine_with(vec![rule("dup", ".+"), rule("dup", "clear")]);
        assert_eq!(errors, vec![PatternError::DuplicateId(String::from("dup"))]);
    }

    #[test]
    fn rejects_too_long_pattern() {
        let long_pattern = "a".repeat(MAX_PATTERN_LEN + 1);
        let (_, errors) = engine_with(vec![rule("r1", &long_pattern)]);
        assert_eq!(
            errors,
            vec![PatternError::PatternTooLong(String::from("r1"))]
        );
    }

    #[test]
    fn rejects_pattern_that_does_not_compile() {
        let (_, errors) = engine_with(vec![rule("r1", "(unbalanced")]);
        assert!(matches!(
            errors[0],
            PatternError::InvalidPattern { ref id, .. } if id == "r1"
        ));
    }

    #[test]
    fn rejects_invalid_channel_names() {
        let mut bad = rule("r1", ".+");
        bad.channels = vec![String::from("bad channel!")];
        let (_, errors) = engine_with(vec![bad]);
        assert_eq!(
            errors,
            vec![PatternError::InvalidChannel(String::from("bad channel!"))]
        );
    }

    #[test]
    fn rejects_missing_system_group() {
        let mut bad = rule("r1", "clear");
        bad.action = ActionConfig::MapAlert {
            system_group: String::from("system"),
        };
        let (_, errors) = engine_with(vec![bad]);
        assert_eq!(
            errors,
            vec![PatternError::InvalidSystemGroup {
                id: String::from("r1"),
                group: String::from("system"),
            }]
        );
    }

    #[test]
    fn accepts_valid_map_alert_rule() {
        let mut good = rule("r1", r"(?P<system>[A-Z0-9]{1,4}-[A-Z0-9]{1,5})");
        good.action = ActionConfig::MapAlert {
            system_group: String::from("system"),
        };
        let (_, errors) = engine_with(vec![good]);
        assert!(errors.is_empty());
    }

    #[test]
    fn rejects_empty_and_oversized_configs() {
        assert_eq!(
            PatternEngine::from_config(&PatternConfig { patterns: vec![] })
                .err()
                .unwrap(),
            PatternError::EmptyConfig
        );
        let rules = (0..=MAX_RULES)
            .map(|i| rule(&format!("rule_{i}"), ".+"))
            .collect();
        assert!(matches!(
            PatternEngine::from_config(&PatternConfig { patterns: rules })
                .err()
                .unwrap(),
            PatternError::TooManyRules(_)
        ));
    }

    #[test]
    fn toml_rejects_unknown_fields() {
        let toml_data = r#"
            [[patterns]]
            id = "r1"
            pattern = ".+"
            unexpected = true
        "#;
        assert!(toml::from_str::<PatternConfig>(toml_data).is_err());
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let mut disabled = rule("off", "clear");
        disabled.enabled = false;
        let (engine, errors) = engine_with(vec![disabled]);
        assert!(errors.is_empty());
        assert!(engine.evaluate("intel", SAMPLE_LINE).is_empty());
    }

    #[test]
    fn truncates_extremely_long_lines() {
        let (engine, _) = engine_with(vec![rule("any", "needle")]);
        let long_text = format!(
            "[ 2021.09.08 22:56:47 ] Some Pilot > {}needle",
            "x".repeat(MAX_LINE_LEN * 2)
        );
        // The "needle" past the truncation point must not be found, and the
        // engine must not choke on the huge line.
        assert!(engine.evaluate("intel", &long_text).is_empty());

        let long_text = format!(
            "[ 2021.09.08 22:56:47 ] Some Pilot > needle{}",
            "x".repeat(MAX_LINE_LEN * 2)
        );
        assert_eq!(engine.evaluate("intel", &long_text).len(), 1);
    }

    #[test]
    fn caps_matches_per_chunk() {
        let (engine, _) = engine_with(vec![rule("any", ".+")]);
        let line = "[ 2021.09.08 22:56:47 ] Some Pilot > hello\n";
        let chunk = line.repeat(MAX_MATCHES_PER_CHUNK + 50);
        assert_eq!(
            engine.evaluate("intel", &chunk).len(),
            MAX_MATCHES_PER_CHUNK
        );
    }

    #[test]
    fn sanitizes_captured_control_characters() {
        assert_eq!(
            sanitize_display("hello\x1b[31m world\n\r\ttest"),
            "hello[31m worldtest"
        );
        let long = "y".repeat(MAX_DISPLAY_LEN * 2);
        assert_eq!(sanitize_display(&long).len(), MAX_DISPLAY_LEN);
    }

    #[test]
    fn defaults_match_every_parsed_line() {
        let engine = PatternEngine::with_defaults();
        let matches = engine.evaluate("any-channel", SAMPLE_LINE);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, DEFAULT_RULE_ID);
        assert_eq!(matches[0].action, ActionConfig::Notify);
    }

    /// Canonical pattern of the `system_reported` rule documented in
    /// `patterns.toml`. Covers 100% of the 8436 solarSystemName values in
    /// assets/sde.db. Keep in sync with the TOML file.
    const SYSTEM_REPORTED_PATTERN: &str = r"\b(?P<system>J\d{6}|[A-Z0-9]{1,5}-[A-Z0-9]{1,4}|[A-Z]{2}\d{3}|[A-Z][a-z]{1,13}(?:[ -][A-Z][a-z]{1,13}){0,2})\b";

    #[test]
    fn system_reported_pattern_matches_real_system_shapes() {
        let mut map_rule = rule("system_reported", SYSTEM_REPORTED_PATTERN);
        map_rule.action = ActionConfig::MapAlert {
            system_group: String::from("system"),
        };
        let (engine, errors) = engine_with(vec![map_rule]);
        assert!(errors.is_empty());

        let cases = [
            ("[ 2021.09.08 22:56:47 ] Pilot > 1DQ1-A clear", "1DQ1-A"),
            ("[ 2021.09.08 22:56:47 ] Pilot > B-R5RB cyno up", "B-R5RB"),
            ("[ 2021.09.08 22:56:47 ] Pilot > J123456 sig", "J123456"),
            ("[ 2021.09.08 22:56:47 ] Pilot > check AD001", "AD001"),
            ("[ 2021.09.08 22:56:47 ] Pilot > Jita undock camp", "Jita"),
            (
                "[ 2021.09.08 22:56:47 ] Pilot > Old Man Star bubbled",
                "Old Man Star",
            ),
            (
                "[ 2021.09.08 22:56:47 ] Pilot > Tash-Murkon Prime incursion",
                "Tash-Murkon Prime",
            ),
        ];
        for (line, expected) in cases {
            let matches = engine.evaluate("intel", line);
            assert_eq!(matches.len(), 1, "line: {line}");
            assert_eq!(
                matches[0].named.get("system").map(String::as_str),
                Some(expected),
                "line: {line}"
            );
        }

        // lowercase common words must not produce matches (case-sensitive rule)
        assert!(
            engine
                .evaluate("intel", "[ 2021.09.08 22:56:47 ] Pilot > hostiles in jita")
                .is_empty()
        );
        assert!(
            engine
                .evaluate("intel", "[ 2021.09.08 22:56:47 ] Pilot > a van jumped")
                .is_empty()
        );
    }

    fn temp_patterns_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "telescope_patterns_{}_{}_{}.toml",
            tag,
            std::process::id(),
            id
        ))
    }

    #[test]
    fn regenerates_missing_file_from_template() {
        let path = temp_patterns_path("missing");
        let _ = std::fs::remove_file(&path);

        let report = PatternEngine::load_or_create(&path);
        assert!(report.regenerated);
        assert!(report.backup.is_none());
        // The shipped template must load without rule errors.
        assert!(
            report.errors.is_empty(),
            "template errors: {:?}",
            report.errors
        );
        assert!(path.exists());
        // And the regenerated engine must work: on SAMPLE_LINE the template
        // rules intel_line, system_reported (1DQ1-A) and clear_report match.
        assert_eq!(report.engine.evaluate("intel", SAMPLE_LINE).len(), 3);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn regenerates_corrupted_file_and_backs_it_up() {
        let path = temp_patterns_path("corrupted");
        let corrupted = "this is {{{ not valid toml";
        std::fs::write(&path, corrupted).unwrap();

        let report = PatternEngine::load_or_create(&path);
        assert!(report.regenerated);
        let backup = report.backup.expect("a backup must be created");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), corrupted);
        // The first error reports the original parse failure.
        assert!(matches!(report.errors[0], PatternError::ParseError(_)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            DEFAULT_PATTERNS_TOML
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&backup);
    }

    #[test]
    fn valid_file_is_loaded_without_regeneration() {
        let path = temp_patterns_path("valid");
        std::fs::write(
            &path,
            "[[patterns]]\nid = \"only_rule\"\npattern = \"needle\"\n",
        )
        .unwrap();

        let report = PatternEngine::load_or_create(&path);
        assert!(!report.regenerated);
        assert!(report.backup.is_none());
        assert!(report.errors.is_empty());
        let matches = report.engine.evaluate(
            "intel",
            "[ 2021.09.08 22:56:47 ] Some Pilot > found a needle here",
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "only_rule");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn intel_line_display_reproduces_log_format() {
        let (engine, _) = engine_with(vec![rule("r1", ".+")]);
        let line = engine.parse_line(SAMPLE_LINE).unwrap();
        assert_eq!(line.to_string(), SAMPLE_LINE);
    }
}
