//! Node-graph intel model: a DAG of `Input -> Detection -> ... -> Output`.
//!
//! A [`RuleGraph`] holds typed [`Node`]s and [`Edge`]s. The executor walks the
//! graph once per input line: an **Input** node emits the raw line as a
//! [`Mensaje`], every **Detection** node matches it, and every **Output** node
//! turns the messages that reach it into an [`Activation`].
//!
//! # Signals
//!
//! Each wire carries, for one line, one of three states:
//!
//! * **true**, with messages: the node found something. An Output fires only
//!   on this.
//! * **false**: the node was evaluated and found nothing.
//! * **absent**: the node was not evaluated -- it is disabled, its Input
//!   filtered the line out (channel, MOTD), or nothing upstream reached it.
//!
//! A Detection that matches sends true (its messages) on **T** and false on
//! **F**; one that doesn't sends false on **T** and true (no messages) on
//! **F**, so **F** means "not found". A false input makes both pins false.
//! Gates treat an absent input as false, but emit nothing when every input is
//! absent, and `not` of an absent input is absent: a rule that was never
//! evaluated can't fire an output through a `not`.
//!
//! [`Mensaje`]s never repeat the line metadata: the shared [`LineContext`]
//! travels once per line alongside them.

use crate::intel::{
    COUNT_GROUP, IntelCategory, MAX_CHANNELS, MAX_MATCHES_PER_CHUNK, MAX_SYSTEM_CANDIDATES,
    PatternError, REGEX_SIZE_LIMIT, has_word_boundaries, is_valid_channel, is_valid_id,
    sanitize_display,
};
use crate::rules::{
    DetectionMatcher, DetectionRuleKind, DetectionType, Dictionaries, InputKind, IntelLine,
    OutputKind, OutputType,
};
use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

/// Pattern of the channel message-of-the-day line EVE writes when joining a
/// channel; an input with `exclude_motd` drops these lines.
const MOTD_PATTERN: &str = r"^Channel MOTD:";

/// The built-in default graph, embedded from the repository's `rules.toml`. It
/// seeds a fresh or rebuilt database and is used whenever there are no rules.
const DEFAULT_RULES_TOML: &str = include_str!("../../../rules.toml");

fn default_true() -> bool {
    true
}

/// The line a message belongs to, shared by every message of one input line.
#[derive(Debug, Clone)]
pub struct LineContext {
    /// The parsed line (its timestamp, author and text).
    pub line: IntelLine,
    /// Channel the line came from.
    pub channel: String,
}

/// Structured payload a detection attaches to its [`Mensaje`], typed per
/// detection kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Data {
    /// Resolved solar-system ids (`system_report`).
    Systems(Vec<usize>),
    /// Ship names with how many times each was named (`ship_names`).
    Ships(Vec<(String, u32)>),
    /// Number of pilots reported (`pilot_count`).
    Count(u32),
    /// Plain words (`clear_report`, `keyword`, `query`).
    Words(Vec<String>),
    /// Free text (the input line, `custom`).
    Text(String),
}

/// Tag of the message an Input node emits: the raw line text.
pub const LINE_TAG: &str = "line";
/// Tag of the message a Formatter node emits: its rendered template.
pub const FORMATTED_TAG: &str = "formatted";

/// One detection result flowing through the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mensaje {
    /// The detection kind that produced it (`system_report`, `ship_names`, ...).
    pub tag: String,
    /// The processed text.
    pub text: String,
    /// The structured payload.
    pub data: Data,
    /// Byte ranges of the line text this message stands for (for a system
    /// report, only the candidates that resolved). Empty when the detection
    /// did not read the line itself (e.g. it was fed by a formatter). Used to
    /// compute what is left of the line for the tooltip (see
    /// [`crate::map_alerts::leftover`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Range<usize>>,
    /// What the message tells the tooltip (see [`IntelCategory`]): set from
    /// the detection type, or chosen for a `custom` detection. `None` for a
    /// system report, the input line and a formatter's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<IntelCategory>,
}

/// Resolves a solar-system name to its SDE id. Injected into the executor so
/// this crate does not depend on `sde`.
pub trait SystemResolver {
    /// The system id for `name`, if it is a real solar system.
    fn resolve(&self, name: &str) -> Option<usize>;
}

/// Parameters of an Input node (its id and enabled flag live on the
/// [`Node`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputNode {
    /// Human-readable description.
    pub description: String,
    /// Kind of the source.
    #[serde(default)]
    pub kind: InputKind,
    /// Path of the file or directory to read.
    pub path: String,
    /// Channels this input feeds; empty means every channel.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Drop the channel's message of the day.
    #[serde(default = "default_true")]
    pub exclude_motd: bool,
}

/// Parameters of a Detection node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectionNode {
    /// The detection type and its parameters.
    pub kind: DetectionRuleKind,
    /// Whether matching is case-insensitive.
    #[serde(default)]
    pub case_insensitive: bool,
}

/// What the Tooltip output shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TooltipConfig {
    /// Add an emoji per message type.
    #[serde(default = "default_true")]
    pub emojis: bool,
}

impl Default for TooltipConfig {
    fn default() -> Self {
        Self { emojis: true }
    }
}

/// Parameters of a Log node.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogConfig {
    /// Stamp each entry with the current time instead of the line's own.
    #[serde(default)]
    pub use_current_time: bool,
}

/// Parameters of an Output node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputNode {
    /// Which queue the node feeds.
    pub kind: OutputKind,
    /// Tooltip display options (only meaningful for [`OutputKind::Tooltip`]).
    #[serde(default)]
    pub tooltip: TooltipConfig,
    /// Log options (only meaningful for [`OutputKind::Log`]).
    #[serde(default)]
    pub log: LogConfig,
}

/// Parameters of an Aggregator node: it merges the messages of its input pins
/// into one `Some(Vec<Mensaje>)`, so several detections can feed a single
/// output without sharing a pin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregatorNode {
    /// Number of input pins.
    #[serde(default = "default_aggregator_inputs")]
    pub inputs: u8,
}

fn default_aggregator_inputs() -> u8 {
    4
}

/// The boolean operation of a Gate node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// True when every input is true.
    And,
    /// True when at least one input is true.
    Or,
    /// True when exactly one input is true.
    Xor,
    /// True when its single input is false.
    Not,
}

impl GateKind {
    /// A short name for the type.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
            Self::Xor => "xor",
            Self::Not => "not",
        }
    }
}

/// Parameters of a Gate node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateNode {
    /// Which boolean operation it applies.
    pub kind: GateKind,
    /// Number of input pins (`Not` always uses one).
    #[serde(default = "default_aggregator_inputs")]
    pub inputs: u8,
}

/// Parameters of a Formatter node: it renders its inputs through a template
/// into one text. `{0}`, `{1}`, ... are the messages of each input pin and
/// `{all}` is every message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatterNode {
    /// The template.
    #[serde(default)]
    pub template: String,
    /// Number of input pins.
    #[serde(default = "default_aggregator_inputs")]
    pub inputs: u8,
}

/// The typed payload of a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum NodeKind {
    /// A chat-log source.
    Input(InputNode),
    /// A detection.
    Detection(DetectionNode),
    /// An output.
    Output(OutputNode),
    /// A many-to-one aggregator.
    Aggregator(AggregatorNode),
    /// A boolean gate.
    Gate(GateNode),
    /// A template formatter.
    Formatter(FormatterNode),
}

/// A graph node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Unique id (`[A-Za-z0-9_-]`, max 64 chars).
    pub id: String,
    /// Disabled nodes are skipped.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Canvas position.
    #[serde(default)]
    pub x: f32,
    /// Canvas position.
    #[serde(default)]
    pub y: f32,
    /// The node payload.
    pub kind: NodeKind,
}

/// A node's *output* pin. Inputs and aggregators expose `Out`; detections
/// expose `T` and `F`. Input pins are indexed by position (`Edge::to_pin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pin {
    /// A node's single output.
    Out,
    /// A Detection's "matched" output: true with its messages when it
    /// found something, false otherwise.
    T,
    /// A Detection's "not matched" output: true (no messages) when it found
    /// nothing, false otherwise.
    F,
}

impl Pin {
    fn as_str(self) -> &'static str {
        match self {
            Self::Out => "out",
            Self::T => "t",
            Self::F => "f",
        }
    }
}

/// A directed edge between two node pins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id.
    pub from: String,
    /// Source (output) pin.
    pub from_pin: Pin,
    /// Target node id.
    pub to: String,
    /// Index of the target node's input pin.
    pub to_pin: u8,
}

/// The whole node graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleGraph {
    /// The nodes.
    #[serde(default)]
    pub nodes: Vec<Node>,
    /// The edges.
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// One Output node that fired for a line, with the messages that reached it.
pub struct Activation {
    /// Id of the Output node.
    pub output_id: String,
    /// Which queue it feeds.
    pub kind: OutputKind,
    /// Tooltip display options.
    pub tooltip: TooltipConfig,
    /// Log options.
    pub log: LogConfig,
    /// Messages that reached the output.
    pub messages: Vec<Mensaje>,
}

/// What travels on a wire during one line's evaluation (an absent signal,
/// the third state, is a wire nothing was routed to; see the module docs).
#[derive(Debug, Clone)]
enum Signal {
    True(Vec<Mensaje>),
    False,
}

impl Signal {
    fn is_true(&self) -> bool {
        matches!(self, Self::True(_))
    }

    fn messages(&self) -> &[Mensaje] {
        match self {
            Self::True(messages) => messages,
            Self::False => &[],
        }
    }
}

/// Renders a Formatter's template: `{0}`, `{1}`, ... are the messages of each
/// input pin, `{<tag>}` (e.g. `{ship_names}`) the messages of that tag and
/// `{all}` every message; the texts of several messages are joined with
/// ` · `. A placeholder that matches nothing is kept as written.
///
/// One pass over the template: the inserted texts come from chat lines, and
/// a `{0}` or `{all}` typed in the chat must stay literal, not be expanded by
/// a later replacement.
fn format_template(template: &str, per_pin: &[Vec<Mensaje>], all: &[Mensaje]) -> String {
    fn joined<'a>(messages: impl Iterator<Item = &'a Mensaje>) -> String {
        messages
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    }
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        rendered.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            rendered.push_str(&rest[open..]);
            return rendered;
        };
        let key = &after[..close];
        let value = if key == "all" {
            Some(joined(all.iter()))
        } else if let Ok(pin) = key.parse::<usize>() {
            per_pin.get(pin).map(|messages| joined(messages.iter()))
        } else if all.iter().any(|message| message.tag == key) {
            Some(joined(all.iter().filter(|message| message.tag == key)))
        } else {
            None
        };
        match value {
            Some(value) => rendered.push_str(&value),
            None => {
                rendered.push('{');
                rendered.push_str(key);
                rendered.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    rendered.push_str(rest);
    rendered
}

/// Number of input pins a node exposes.
pub fn input_pin_count(kind: &NodeKind) -> usize {
    match kind {
        NodeKind::Input(_) => 0,
        NodeKind::Detection(_) => 1,
        NodeKind::Output(_) => 1,
        NodeKind::Aggregator(node) => node.inputs as usize,
        NodeKind::Gate(gate) => match gate.kind {
            GateKind::Not => 1,
            _ => gate.inputs as usize,
        },
        NodeKind::Formatter(formatter) => formatter.inputs as usize,
    }
}

/// Number of output pins a node exposes.
pub fn output_pin_count(kind: &NodeKind) -> usize {
    match kind {
        NodeKind::Input(_)
        | NodeKind::Aggregator(_)
        | NodeKind::Gate(_)
        | NodeKind::Formatter(_) => 1,
        NodeKind::Detection(_) => 2,
        NodeKind::Output(_) => 0,
    }
}

fn pin_valid_from(kind: &NodeKind, pin: Pin) -> bool {
    match kind {
        NodeKind::Input(_)
        | NodeKind::Aggregator(_)
        | NodeKind::Gate(_)
        | NodeKind::Formatter(_) => pin == Pin::Out,
        NodeKind::Detection(_) => matches!(pin, Pin::T | Pin::F),
        NodeKind::Output(_) => false,
    }
}

impl RuleGraph {
    /// Validates the graph: unique/valid ids, per-kind parameters, one cable
    /// per input, exactly one Output per kind, valid pins and no cycles.
    pub fn validate(&self) -> Vec<PatternError> {
        let mut errors = Vec::new();
        let dictionaries = Dictionaries::shared();

        let mut seen = HashSet::new();
        for node in &self.nodes {
            if !is_valid_id(&node.id) {
                errors.push(PatternError::InvalidId(node.id.clone()));
                continue;
            }
            if !seen.insert(node.id.clone()) {
                errors.push(PatternError::DuplicateId(node.id.clone()));
            }
        }

        let mut incoming: HashMap<&str, usize> = HashMap::new();
        for edge in &self.edges {
            let from = self.nodes.iter().find(|node| node.id == edge.from);
            let to = self.nodes.iter().find(|node| node.id == edge.to);
            match (from, to) {
                (Some(from), Some(to)) => {
                    if !pin_valid_from(&from.kind, edge.from_pin) {
                        errors.push(PatternError::InvalidPin {
                            node: from.id.clone(),
                            pin: edge.from_pin.as_str().to_string(),
                        });
                    }
                    if edge.to_pin as usize >= input_pin_count(&to.kind) {
                        errors.push(PatternError::InvalidPin {
                            node: to.id.clone(),
                            pin: edge.to_pin.to_string(),
                        });
                    }
                    *incoming.entry(edge.to.as_str()).or_default() += 1;
                }
                _ => errors.push(PatternError::InvalidSource {
                    id: edge.to.clone(),
                    source: edge.from.clone(),
                }),
            }
        }

        let mut output_kinds = HashSet::new();
        for node in &self.nodes {
            match &node.kind {
                NodeKind::Input(input) => {
                    if incoming.get(node.id.as_str()).copied().unwrap_or(0) != 0 {
                        errors.push(PatternError::InvalidPin {
                            node: node.id.clone(),
                            pin: String::from("in"),
                        });
                    }
                    if input.description.trim().is_empty() {
                        errors.push(PatternError::InvalidInput {
                            id: node.id.clone(),
                            reason: "description must not be empty".to_string(),
                        });
                    }
                    if input.channels.len() > MAX_CHANNELS {
                        errors.push(PatternError::TooManyChannels(node.id.clone()));
                    }
                    for channel in &input.channels {
                        if !is_valid_channel(channel) {
                            errors.push(PatternError::InvalidChannel(channel.clone()));
                        }
                    }
                }
                NodeKind::Detection(detection) => {
                    if incoming.get(node.id.as_str()).copied().unwrap_or(0) != 1 {
                        errors.push(PatternError::MissingInput(node.id.clone()));
                    }
                    if let Err(error) = detection.kind.validate(&node.id, dictionaries) {
                        errors.push(error);
                    }
                }
                NodeKind::Output(output) => {
                    if incoming.get(node.id.as_str()).copied().unwrap_or(0) != 1 {
                        errors.push(PatternError::MissingInput(node.id.clone()));
                    }
                    if !output_kinds.insert(output.kind.type_name()) {
                        errors.push(PatternError::DuplicateOutputKind(
                            output.kind.type_name().to_string(),
                        ));
                    }
                }
                NodeKind::Aggregator(aggregator) => {
                    let count = incoming.get(node.id.as_str()).copied().unwrap_or(0);
                    if count > aggregator.inputs as usize {
                        errors.push(PatternError::TooManyInputs(node.id.clone()));
                    }
                }
                NodeKind::Gate(_) | NodeKind::Formatter(_) => {
                    let count = incoming.get(node.id.as_str()).copied().unwrap_or(0);
                    if count > input_pin_count(&node.kind) {
                        errors.push(PatternError::TooManyInputs(node.id.clone()));
                    }
                }
            }
        }

        if let Some(cycle) = self.cycle() {
            errors.push(PatternError::GraphCycle(cycle));
        }

        errors
    }

    /// The nodes caught in a cycle (and those only reachable through one),
    /// sorted and capped at 8, if the graph has a cycle.
    fn cycle(&self) -> Option<Vec<String>> {
        let (_, mut stuck) = self.kahn();
        if stuck.is_empty() {
            return None;
        }
        stuck.sort_unstable();
        stuck.truncate(8);
        Some(stuck)
    }

    /// A topological order of the nodes, or `None` when the graph has a cycle.
    pub fn topological_order(&self) -> Option<Vec<String>> {
        let (order, stuck) = self.kahn();
        (stuck.is_empty() && order.len() == self.nodes.len()).then_some(order)
    }

    /// Kahn's algorithm: the nodes in topological order, and the ones left
    /// with an unresolved incoming edge (a cycle and what hangs from it).
    fn kahn(&self) -> (Vec<String>, Vec<String>) {
        let mut indegree: HashMap<&str, usize> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), 0))
            .collect();
        for edge in &self.edges {
            if let Some(degree) = indegree.get_mut(edge.to.as_str()) {
                *degree += 1;
            }
        }
        let mut queue: Vec<&str> = indegree
            .iter()
            .filter(|(_, degree)| **degree == 0)
            .map(|(id, _)| *id)
            .collect();
        queue.sort_unstable();
        let mut order = Vec::new();
        while let Some(id) = queue.pop() {
            order.push(id.to_string());
            for edge in self.edges.iter().filter(|edge| edge.from == id) {
                if let Some(degree) = indegree.get_mut(edge.to.as_str()) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push(edge.to.as_str());
                    }
                }
            }
        }
        let stuck = indegree
            .into_iter()
            .filter(|(_, degree)| *degree > 0)
            .map(|(id, _)| id.to_string())
            .collect();
        (order, stuck)
    }

    /// The built-in default graph, from the embedded `rules.toml`. It seeds a
    /// fresh or rebuilt database and is used whenever there are no rules.
    pub fn default_graph() -> Self {
        let mut graph: Self =
            toml::from_str(DEFAULT_RULES_TOML).expect("the embedded default rules must parse");
        // Input paths are machine-specific; leave them empty so the app fills
        // them from the settings.
        for node in &mut graph.nodes {
            if let NodeKind::Input(input) = &mut node.kind {
                input.path.clear();
            }
        }
        graph
    }
}

/// A detection compiled once and reused for every line.
struct CompiledDetection {
    kind: DetectionRuleKind,
    regexes: Vec<Regex>,
    automaton: Option<AhoCorasick>,
}

/// One raw match of a detection over a line.
struct MatchData {
    captures: HashMap<String, String>,
    matched: String,
    word: Option<String>,
    /// Byte range of the match (the system group for a system report).
    span: Range<usize>,
}

impl CompiledDetection {
    fn compile(
        node: &DetectionNode,
        id: &str,
        dictionaries: &Dictionaries,
    ) -> Result<Self, PatternError> {
        match node.kind.matcher(dictionaries) {
            DetectionMatcher::Regex(patterns) => {
                let mut regexes = Vec::new();
                for pattern in &patterns {
                    let regex = RegexBuilder::new(pattern)
                        .case_insensitive(node.case_insensitive)
                        .size_limit(REGEX_SIZE_LIMIT)
                        .dfa_size_limit(REGEX_SIZE_LIMIT)
                        .build()
                        .map_err(|error| PatternError::InvalidPattern {
                            id: id.to_string(),
                            reason: error.to_string(),
                        })?;
                    let has_group =
                        |group: &str| regex.capture_names().flatten().any(|name| name == group);
                    if let Some(group) = node.kind.system_group()
                        && !has_group(&group)
                    {
                        return Err(PatternError::InvalidSystemGroup {
                            id: id.to_string(),
                            group,
                        });
                    }
                    if node.kind.category() == Some(IntelCategory::Count) && !has_group(COUNT_GROUP)
                    {
                        return Err(PatternError::MissingCountGroup(id.to_string()));
                    }
                    regexes.push(regex);
                }
                Ok(Self {
                    kind: node.kind.clone(),
                    regexes,
                    automaton: None,
                })
            }
            DetectionMatcher::Dictionary(words) => {
                let automaton = AhoCorasickBuilder::new()
                    .ascii_case_insensitive(node.case_insensitive)
                    .match_kind(MatchKind::LeftmostLongest)
                    .build(&words)
                    .map_err(|error| PatternError::DictionaryBuildFailed {
                        id: id.to_string(),
                        reason: error.to_string(),
                    })?;
                Ok(Self {
                    kind: node.kind.clone(),
                    regexes: Vec::new(),
                    automaton: Some(automaton),
                })
            }
        }
    }

    fn matches(&self, text: &str) -> Vec<MatchData> {
        let mut results = Vec::new();
        let system_group = self.kind.system_group();
        let per_regex = if system_group.is_some() {
            MAX_SYSTEM_CANDIDATES
        } else {
            MAX_MATCHES_PER_CHUNK
        };
        for regex in &self.regexes {
            for caps in regex.captures_iter(text).take(per_regex) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    return results;
                }
                let span: Range<usize> = system_group
                    .as_deref()
                    .and_then(|group| caps.name(group))
                    .or_else(|| caps.get(0))
                    .map(|matched| matched.range())
                    .unwrap_or(0..0);
                let captures = regex
                    .capture_names()
                    .flatten()
                    .filter_map(|name| {
                        caps.name(name)
                            .map(|matched| (name.to_string(), sanitize_display(matched.as_str())))
                    })
                    .collect();
                results.push(MatchData {
                    captures,
                    matched: sanitize_display(&text[span.clone()]),
                    word: None,
                    span,
                });
            }
        }
        if let Some(automaton) = &self.automaton {
            for matched in automaton.find_iter(text) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    break;
                }
                if !has_word_boundaries(text, matched.start(), matched.end()) {
                    continue;
                }
                let text = sanitize_display(&text[matched.start()..matched.end()]);
                let mut captures = HashMap::new();
                captures.insert("word".to_string(), text.clone());
                results.push(MatchData {
                    captures,
                    matched: text.clone(),
                    word: Some(text),
                    span: matched.range(),
                });
            }
        }
        results
    }

    fn process(&self, matches: &[MatchData], resolver: &dyn SystemResolver) -> Option<Mensaje> {
        let mut message = self.process_matches(matches, resolver)?;
        if !matches!(message.data, Data::Systems(_)) {
            message.category = self.kind.category();
        }
        // A system report already kept the spans of the candidates that
        // resolved; every other kind stands for all of its matches.
        if message.spans.is_empty() && !matches!(message.data, Data::Systems(_)) {
            message.spans = matches.iter().map(|found| found.span.clone()).collect();
        }
        Some(message)
    }

    fn process_matches(
        &self,
        matches: &[MatchData],
        resolver: &dyn SystemResolver,
    ) -> Option<Mensaje> {
        let tag = self.kind.type_name().to_string();
        // What the detection produces follows from its category (a `custom`
        // detection chooses one), or a system group when it names systems.
        if self.kind.system_group().is_some() {
            return self.systems(matches, resolver);
        }
        match self.kind.category() {
            Some(IntelCategory::Clear) => matches.first().map(|found| Mensaje {
                tag,
                text: found.matched.clone(),
                data: Data::Words(vec![found.matched.clone()]),
                spans: Vec::new(),
                category: None,
            }),
            Some(IntelCategory::Ship) => {
                let mut ships: Vec<(String, u32)> = Vec::new();
                for found in matches {
                    let name = found.matched.clone();
                    match ships
                        .iter_mut()
                        .find(|(known, _)| known.eq_ignore_ascii_case(&name))
                    {
                        Some((_, times)) => *times += 1,
                        None => ships.push((name, 1)),
                    }
                }
                (!ships.is_empty()).then(|| Mensaje {
                    tag,
                    text: format_ships(&ships),
                    data: Data::Ships(ships),
                    spans: Vec::new(),
                    category: None,
                })
            }
            Some(IntelCategory::Count) => matches
                .iter()
                .find_map(|found| {
                    found
                        .captures
                        .get(COUNT_GROUP)
                        .and_then(|count| count.parse::<u32>().ok())
                })
                .map(|count| Mensaje {
                    tag,
                    text: count.to_string(),
                    data: Data::Count(count),
                    spans: Vec::new(),
                    category: None,
                }),
            Some(IntelCategory::Keyword | IntelCategory::Query) | None => {
                let words: Vec<String> =
                    matches.iter().map(|found| found.matched.clone()).collect();
                (!words.is_empty()).then(|| Mensaje {
                    tag,
                    text: words.join(", "),
                    data: Data::Words(words),
                    spans: Vec::new(),
                    category: None,
                })
            }
        }
    }

    /// Resolves every reported system of `matches`, producing a `system_report`
    /// message (only when at least one resolves).
    fn systems(&self, matches: &[MatchData], resolver: &dyn SystemResolver) -> Option<Mensaje> {
        let group = self.kind.system_group();
        let mut ids = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut spans = Vec::new();
        for found in matches {
            let name = group
                .as_deref()
                .and_then(|group| found.captures.get(group))
                .cloned()
                .or_else(|| found.word.clone())
                .unwrap_or_else(|| found.matched.clone());
            if let Some(id) = resolver.resolve(&name) {
                spans.push(found.span.clone());
                if !ids.contains(&id) {
                    ids.push(id);
                }
                if !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                    names.push(name);
                }
            }
        }
        (!ids.is_empty()).then(|| Mensaje {
            tag: self.kind.type_name().to_string(),
            text: names.join(", "),
            data: Data::Systems(ids),
            spans,
            category: None,
        })
    }
}

/// `"nave1 (x3) nave2 (x4)"` (a single mention has no count).
fn format_ships(ships: &[(String, u32)]) -> String {
    ships
        .iter()
        .map(|(name, times)| {
            if *times > 1 {
                format!("{name} (x{times})")
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn motd_regex() -> &'static Regex {
    static MOTD: OnceLock<Regex> = OnceLock::new();
    MOTD.get_or_init(|| Regex::new(MOTD_PATTERN).expect("hardcoded MOTD regex must compile"))
}

/// Compiled graph, ready to evaluate lines.
pub struct Executor {
    nodes: Vec<Node>,
    compiled: HashMap<String, CompiledDetection>,
    edges_out: HashMap<String, Vec<Edge>>,
    /// For each node, the input pins that have an incoming edge.
    connected_inputs: HashMap<String, Vec<u8>>,
}

impl Executor {
    /// Compiles the graph. Returns the executor plus one error per skipped
    /// invalid detection node.
    ///
    /// The graph is validated first ([`RuleGraph::validate`]) and every problem
    /// is returned. A graph with a cycle has no valid evaluation order, so its
    /// executor evaluates nothing; any other problem only affects the nodes
    /// involved.
    pub fn new(graph: RuleGraph) -> (Self, Vec<PatternError>) {
        let dictionaries = Dictionaries::shared();
        let mut errors = graph.validate();
        let mut compiled = HashMap::new();
        for node in &graph.nodes {
            if let NodeKind::Detection(detection) = &node.kind {
                match CompiledDetection::compile(detection, &node.id, dictionaries) {
                    Ok(compiled_detection) => {
                        compiled.insert(node.id.clone(), compiled_detection);
                    }
                    Err(error) => {
                        if !errors.contains(&error) {
                            errors.push(error);
                        }
                    }
                }
            }
        }

        let order = graph.topological_order();
        let mut nodes = graph.nodes;
        match order {
            Some(order) => {
                let position: HashMap<&str, usize> = order
                    .iter()
                    .enumerate()
                    .map(|(index, id)| (id.as_str(), index))
                    .collect();
                nodes.sort_by_key(|node| {
                    position
                        .get(node.id.as_str())
                        .copied()
                        .unwrap_or(usize::MAX)
                });
            }
            // `validate` already reported the cycle.
            None => nodes.clear(),
        }
        let mut edges_out: HashMap<String, Vec<Edge>> = HashMap::new();
        let mut connected_inputs: HashMap<String, Vec<u8>> = HashMap::new();
        for edge in graph.edges {
            connected_inputs
                .entry(edge.to.clone())
                .or_default()
                .push(edge.to_pin);
            edges_out.entry(edge.from.clone()).or_default().push(edge);
        }

        (
            Self {
                nodes,
                compiled,
                edges_out,
                connected_inputs,
            },
            errors,
        )
    }

    /// Runs the graph over one input line and returns the Output nodes that
    /// fired, in graph order.
    pub fn run(&self, context: &LineContext, resolver: &dyn SystemResolver) -> Vec<Activation> {
        let mut incoming: HashMap<(String, u8), Signal> = HashMap::new();
        let mut activations = Vec::new();
        for node in &self.nodes {
            let signals: Vec<Option<Signal>> = (0..input_pin_count(&node.kind))
                .map(|pin| incoming.remove(&(node.id.clone(), pin as u8)))
                .collect();
            match &node.kind {
                NodeKind::Input(input) => {
                    if !node.enabled {
                        continue;
                    }
                    if !input.channels.is_empty()
                        && !input.channels.iter().any(|name| name == &context.channel)
                    {
                        continue;
                    }
                    if input.exclude_motd && motd_regex().is_match(&context.line.text) {
                        continue;
                    }
                    let message = Mensaje {
                        tag: String::from(LINE_TAG),
                        text: context.line.text.clone(),
                        data: Data::Text(context.line.text.clone()),
                        spans: Vec::new(),
                        category: None,
                    };
                    self.route(
                        &node.id,
                        Pin::Out,
                        Signal::True(vec![message]),
                        &mut incoming,
                    );
                }
                NodeKind::Detection(_) => {
                    if !node.enabled {
                        continue;
                    }
                    let evaluated = match signals.first().and_then(|signal| signal.as_ref()) {
                        Some(Signal::True(messages)) => {
                            let Some(compiled) = self.compiled.get(&node.id) else {
                                continue;
                            };
                            let mut matched = Vec::new();
                            for message in messages {
                                let found = compiled.matches(&message.text);
                                if let Some(mut result) = compiled.process(&found, resolver) {
                                    // Spans only mean something relative to
                                    // the line itself.
                                    if message.tag != LINE_TAG {
                                        result.spans.clear();
                                    }
                                    matched.push(result);
                                }
                            }
                            Some(matched)
                        }
                        // Nothing to look at: neither found nor not found.
                        Some(Signal::False) => None,
                        None => continue,
                    };
                    let (t, f) = match evaluated {
                        Some(matched) if !matched.is_empty() => {
                            (Signal::True(matched), Signal::False)
                        }
                        Some(_) => (Signal::False, Signal::True(Vec::new())),
                        None => (Signal::False, Signal::False),
                    };
                    self.route(&node.id, Pin::T, t, &mut incoming);
                    self.route(&node.id, Pin::F, f, &mut incoming);
                }
                NodeKind::Aggregator(_) => {
                    if !node.enabled {
                        continue;
                    }
                    let present: Vec<&Signal> = signals.iter().flatten().collect();
                    if present.is_empty() {
                        continue;
                    }
                    let merged: Vec<Mensaje> = present
                        .iter()
                        .flat_map(|signal| signal.messages().iter().cloned())
                        .collect();
                    let signal = if present.iter().any(|signal| signal.is_true()) {
                        Signal::True(merged)
                    } else {
                        Signal::False
                    };
                    self.route(&node.id, Pin::Out, signal, &mut incoming);
                }
                NodeKind::Gate(gate) => {
                    if !node.enabled {
                        continue;
                    }
                    let pins = self
                        .connected_inputs
                        .get(&node.id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    // Absent inputs (see the module docs): `None`.
                    let inputs: Vec<Option<bool>> = pins
                        .iter()
                        .map(|pin| {
                            signals
                                .get(*pin as usize)
                                .and_then(|signal| signal.as_ref())
                                .map(Signal::is_true)
                        })
                        .collect();
                    if inputs.iter().all(Option::is_none) {
                        continue;
                    }
                    let truth = || inputs.iter().map(|value| value.unwrap_or(false));
                    let fired = match gate.kind {
                        GateKind::And => truth().all(|value| value),
                        GateKind::Or => truth().any(|value| value),
                        GateKind::Xor => truth().filter(|value| *value).count() == 1,
                        GateKind::Not => inputs.first().copied().flatten() == Some(false),
                    };
                    let merged: Vec<Mensaje> = signals
                        .iter()
                        .flatten()
                        .flat_map(|signal| signal.messages().iter().cloned())
                        .collect();
                    let signal = if fired {
                        Signal::True(merged)
                    } else {
                        Signal::False
                    };
                    self.route(&node.id, Pin::Out, signal, &mut incoming);
                }
                NodeKind::Formatter(formatter) => {
                    if !node.enabled {
                        continue;
                    }
                    let present: Vec<&Signal> = signals.iter().flatten().collect();
                    if present.is_empty() {
                        continue;
                    }
                    if !present.iter().any(|signal| signal.is_true()) {
                        self.route(&node.id, Pin::Out, Signal::False, &mut incoming);
                        continue;
                    }
                    let per_pin: Vec<Vec<Mensaje>> = signals
                        .iter()
                        .map(|signal| {
                            signal
                                .as_ref()
                                .map_or_else(Vec::new, |signal| signal.messages().to_vec())
                        })
                        .collect();
                    let all: Vec<Mensaje> = present
                        .iter()
                        .flat_map(|signal| signal.messages().iter().cloned())
                        .collect();
                    let text = format_template(&formatter.template, &per_pin, &all);
                    let message = Mensaje {
                        tag: String::from(FORMATTED_TAG),
                        text: text.clone(),
                        data: Data::Text(text),
                        spans: Vec::new(),
                        category: None,
                    };
                    self.route(
                        &node.id,
                        Pin::Out,
                        Signal::True(vec![message]),
                        &mut incoming,
                    );
                }
                NodeKind::Output(output) => {
                    if !node.enabled {
                        continue;
                    }
                    if let Some(Signal::True(messages)) =
                        signals.first().and_then(|signal| signal.as_ref())
                    {
                        activations.push(Activation {
                            output_id: node.id.clone(),
                            kind: output.kind,
                            tooltip: output.tooltip.clone(),
                            log: output.log.clone(),
                            messages: messages.clone(),
                        });
                    }
                }
            }
        }
        activations
    }

    fn route(
        &self,
        from: &str,
        pin: Pin,
        signal: Signal,
        incoming: &mut HashMap<(String, u8), Signal>,
    ) {
        for edge in self.edges_out.get(from).into_iter().flatten() {
            if edge.from_pin == pin {
                incoming.insert((edge.to.clone(), edge.to_pin), signal.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedResolver;

    impl SystemResolver for FixedResolver {
        fn resolve(&self, name: &str) -> Option<usize> {
            (name == "Jita").then_some(30000142)
        }
    }

    fn line(text: &str) -> LineContext {
        LineContext {
            line: IntelLine {
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                author: String::from("Pilot"),
                text: text.to_string(),
            },
            channel: String::from("intel"),
        }
    }

    fn graph(detection: DetectionRuleKind) -> RuleGraph {
        RuleGraph {
            nodes: vec![
                Node {
                    id: String::from("in"),
                    enabled: true,
                    x: 0.0,
                    y: 0.0,
                    kind: NodeKind::Input(InputNode {
                        description: String::from("chat"),
                        kind: InputKind::ChatLog,
                        path: String::new(),
                        channels: Vec::new(),
                        exclude_motd: true,
                    }),
                },
                Node {
                    id: String::from("det"),
                    enabled: true,
                    x: 0.0,
                    y: 0.0,
                    kind: NodeKind::Detection(DetectionNode {
                        kind: detection,
                        case_insensitive: false,
                    }),
                },
                Node {
                    id: String::from("out"),
                    enabled: true,
                    x: 0.0,
                    y: 0.0,
                    kind: NodeKind::Output(OutputNode {
                        kind: OutputKind::Log,
                        tooltip: TooltipConfig::default(),
                        log: LogConfig::default(),
                    }),
                },
            ],
            edges: vec![
                Edge {
                    from: String::from("in"),
                    from_pin: Pin::Out,
                    to: String::from("det"),
                    to_pin: 0,
                },
                Edge {
                    from: String::from("det"),
                    from_pin: Pin::T,
                    to: String::from("out"),
                    to_pin: 0,
                },
            ],
        }
    }

    #[test]
    fn a_valid_graph_passes() {
        let graph = graph(DetectionRuleKind::SystemReport);
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    }

    #[test]
    fn a_graph_round_trips_through_json() {
        let graph = graph(DetectionRuleKind::SystemReport);
        let json = serde_json::to_string(&graph).unwrap();
        let back: RuleGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, back);
    }

    #[test]
    fn a_detection_without_input_is_an_error() {
        let mut graph = graph(DetectionRuleKind::SystemReport);
        graph.edges.retain(|edge| edge.from != "in");
        assert!(matches!(
            graph.validate().as_slice(),
            [PatternError::MissingInput(id)] if id == "det"
        ));
    }

    #[test]
    fn a_cycle_is_rejected() {
        let mut graph = graph(DetectionRuleKind::SystemReport);
        graph.edges.push(Edge {
            from: String::from("out"),
            from_pin: Pin::Out,
            to: String::from("det"),
            to_pin: 0,
        });
        assert!(
            graph
                .validate()
                .iter()
                .any(|error| matches!(error, PatternError::GraphCycle(_)))
        );
    }

    #[test]
    fn a_system_detection_resolves_and_reaches_the_output() {
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport));
        assert!(errors.is_empty(), "{errors:?}");
        let activations = executor.run(&line("hostile in Jita"), &FixedResolver);
        assert_eq!(activations.len(), 1);
        assert_eq!(activations[0].kind, OutputKind::Log);
        assert_eq!(activations[0].messages.len(), 1);
        assert_eq!(
            activations[0].messages[0].data,
            Data::Systems(vec![30000142])
        );
    }

    #[test]
    fn ship_names_are_grouped() {
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let (executor, _) = Executor::new(graph(kind));
        let activations = executor.run(&line("Rifter Rifter Merlin"), &FixedResolver);
        let message = &activations[0].messages[0];
        assert_eq!(message.tag, "ship_names");
        assert_eq!(
            message.data,
            Data::Ships(vec![
                (String::from("Rifter"), 2),
                (String::from("Merlin"), 1)
            ])
        );
        assert_eq!(message.text, "Rifter (x2) Merlin");
    }

    fn input_node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Input(InputNode {
                description: String::from("chat"),
                kind: InputKind::ChatLog,
                path: String::new(),
                channels: Vec::new(),
                exclude_motd: true,
            }),
        }
    }

    fn detection_node(id: &str, kind: DetectionRuleKind) -> Node {
        Node {
            id: id.to_string(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Detection(DetectionNode {
                kind,
                case_insensitive: false,
            }),
        }
    }

    fn output_node(id: &str) -> Node {
        Node {
            id: id.to_string(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Output(OutputNode {
                kind: OutputKind::Log,
                tooltip: TooltipConfig::default(),
                log: LogConfig::default(),
            }),
        }
    }

    fn edge(from: &str, from_pin: Pin, to: &str, to_pin: u8) -> Edge {
        Edge {
            from: from.to_string(),
            from_pin,
            to: to.to_string(),
            to_pin,
        }
    }

    #[test]
    fn an_and_gate_needs_every_input() {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph
            .nodes
            .push(detection_node("sys", DetectionRuleKind::SystemReport));
        graph.nodes.push(detection_node(
            "ship",
            DetectionRuleKind::ShipNames {
                dictionaries: vec![String::from("ship_report_en")],
            },
        ));
        graph.nodes.push(Node {
            id: String::from("and"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Gate(GateNode {
                kind: GateKind::And,
                inputs: 2,
            }),
        });
        graph.nodes.push(output_node("log"));
        graph.edges.push(edge("in", Pin::Out, "sys", 0));
        graph.edges.push(edge("in", Pin::Out, "ship", 0));
        graph.edges.push(edge("sys", Pin::T, "and", 0));
        graph.edges.push(edge("ship", Pin::T, "and", 1));
        graph.edges.push(edge("and", Pin::Out, "log", 0));

        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(executor.run(&line("Jita, Rifter"), &FixedResolver).len(), 1);
        assert!(executor.run(&line("Jita"), &FixedResolver).is_empty());
    }

    #[test]
    fn a_not_gate_inverts_its_input() {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph
            .nodes
            .push(detection_node("sys", DetectionRuleKind::SystemReport));
        graph.nodes.push(Node {
            id: String::from("not"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Gate(GateNode {
                kind: GateKind::Not,
                inputs: 1,
            }),
        });
        graph.nodes.push(output_node("log"));
        graph.edges.push(edge("in", Pin::Out, "sys", 0));
        graph.edges.push(edge("sys", Pin::T, "not", 0));
        graph.edges.push(edge("not", Pin::Out, "log", 0));

        let (executor, _) = Executor::new(graph);
        assert!(executor.run(&line("Jita"), &FixedResolver).is_empty());
        assert_eq!(
            executor.run(&line("no system here"), &FixedResolver).len(),
            1
        );
    }

    /// `in -> sys (system report) -> not -> log`, for the tri-state tests.
    fn not_graph() -> RuleGraph {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph
            .nodes
            .push(detection_node("sys", DetectionRuleKind::SystemReport));
        graph.nodes.push(Node {
            id: String::from("not"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Gate(GateNode {
                kind: GateKind::Not,
                inputs: 1,
            }),
        });
        graph.nodes.push(output_node("log"));
        graph.edges.push(edge("in", Pin::Out, "sys", 0));
        graph.edges.push(edge("sys", Pin::T, "not", 0));
        graph.edges.push(edge("not", Pin::Out, "log", 0));
        graph
    }

    #[test]
    fn a_not_gate_of_an_unevaluated_detection_fires_nothing() {
        // Disabled detection: not evaluated, so `not` has nothing to invert.
        let mut graph = not_graph();
        graph.nodes[1].enabled = false;
        let (executor, _) = Executor::new(graph);
        assert!(
            executor
                .run(&line("no system here"), &FixedResolver)
                .is_empty()
        );

        // The line is the channel MOTD, dropped by the input.
        let (executor, _) = Executor::new(not_graph());
        assert!(
            executor
                .run(&line("Channel MOTD: welcome"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn the_f_pin_fires_when_nothing_is_found() {
        let mut graph = graph(DetectionRuleKind::SystemReport);
        graph.edges.retain(|edge| edge.from != "det");
        graph.edges.push(Edge {
            from: String::from("det"),
            from_pin: Pin::F,
            to: String::from("out"),
            to_pin: 0,
        });
        let (executor, _) = Executor::new(graph);
        assert_eq!(executor.run(&line("nothing here"), &FixedResolver).len(), 1);
        assert!(executor.run(&line("Jita"), &FixedResolver).is_empty());
    }

    #[test]
    fn a_formatter_renders_its_template() {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph
            .nodes
            .push(detection_node("sys", DetectionRuleKind::SystemReport));
        graph.nodes.push(detection_node(
            "ship",
            DetectionRuleKind::ShipNames {
                dictionaries: vec![String::from("ship_report_en")],
            },
        ));
        graph.nodes.push(Node {
            id: String::from("fmt"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Formatter(FormatterNode {
                template: String::from("{0} @ {1}"),
                inputs: 2,
            }),
        });
        graph.nodes.push(output_node("log"));
        graph.edges.push(edge("in", Pin::Out, "sys", 0));
        graph.edges.push(edge("in", Pin::Out, "ship", 0));
        graph.edges.push(edge("sys", Pin::T, "fmt", 0));
        graph.edges.push(edge("ship", Pin::T, "fmt", 1));
        graph.edges.push(edge("fmt", Pin::Out, "log", 0));

        let (executor, _) = Executor::new(graph);
        let activations = executor.run(&line("Jita, Rifter"), &FixedResolver);
        assert_eq!(activations[0].messages[0].text, "Jita @ Rifter");
    }

    fn text_message(tag: &str, text: &str) -> Mensaje {
        Mensaje {
            tag: String::from(tag),
            text: String::from(text),
            data: Data::Text(String::from(text)),
            spans: Vec::new(),
            category: None,
        }
    }

    #[test]
    fn placeholders_typed_in_the_chat_are_not_expanded() {
        let pin0 = vec![text_message("keyword", "hi {1} {all}")];
        let pin1 = vec![text_message("ship_names", "Rifter")];
        let all: Vec<Mensaje> = pin0.iter().chain(&pin1).cloned().collect();
        let rendered = format_template("{0} | {ship_names} | {nope} | {", &[pin0, pin1], &all);
        assert_eq!(rendered, "hi {1} {all} | Rifter | {nope} | {");
    }

    #[test]
    fn a_cycle_reports_its_nodes_and_the_executor_runs_nothing() {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph.nodes.push(Node {
            id: String::from("a"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Aggregator(AggregatorNode { inputs: 2 }),
        });
        graph.nodes.push(Node {
            id: String::from("b"),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Aggregator(AggregatorNode { inputs: 1 }),
        });
        graph.nodes.push(output_node("log"));
        graph.edges.push(edge("in", Pin::Out, "a", 0));
        graph.edges.push(edge("a", Pin::Out, "b", 0));
        graph.edges.push(edge("b", Pin::Out, "a", 1));
        graph.edges.push(edge("a", Pin::Out, "log", 0));

        let (executor, errors) = Executor::new(graph);
        let cycle = errors.iter().find_map(|error| match error {
            PatternError::GraphCycle(nodes) => Some(nodes.clone()),
            _ => None,
        });
        assert_eq!(
            cycle,
            Some(vec![
                String::from("a"),
                String::from("b"),
                String::from("log")
            ])
        );
        assert!(executor.run(&line("Jita"), &FixedResolver).is_empty());
    }

    fn custom(pattern: Option<&str>, words: &[&str], category: IntelCategory) -> DetectionRuleKind {
        DetectionRuleKind::Custom {
            pattern: pattern.map(String::from),
            words: words.iter().map(|word| word.to_string()).collect(),
            category: Some(category),
            system_group: None,
        }
    }

    /// The single message a one-detection graph produces for `text`.
    fn only_message(kind: DetectionRuleKind, text: &str) -> Mensaje {
        let (executor, errors) = Executor::new(graph(kind));
        assert!(errors.is_empty(), "{errors:?}");
        let mut activations = executor.run(&line(text), &FixedResolver);
        assert_eq!(activations.len(), 1);
        activations.remove(0).messages.remove(0)
    }

    #[test]
    fn a_custom_detection_produces_the_data_of_its_category() {
        let ships = only_message(
            custom(None, &["Proteus"], IntelCategory::Ship),
            "2x Proteus proteus",
        );
        assert_eq!(ships.data, Data::Ships(vec![(String::from("Proteus"), 1)]));
        assert_eq!(ships.category, Some(IntelCategory::Ship));

        let count = only_message(
            custom(Some(r"(?P<count>\d+) reds"), &[], IntelCategory::Count),
            "12 reds in local",
        );
        assert_eq!(count.data, Data::Count(12));

        let clear = only_message(custom(None, &["empty"], IntelCategory::Clear), "gate empty");
        assert_eq!(clear.category, Some(IntelCategory::Clear));
        let summary = crate::map_alerts::AlertSummary::from_messages(&[&clear]);
        assert_eq!(summary.clear.as_deref(), Some("empty"));
    }

    #[test]
    fn a_custom_count_needs_a_count_group() {
        for kind in [
            custom(None, &["reds"], IntelCategory::Count),
            custom(Some(r"\d+ reds"), &[], IntelCategory::Count),
        ] {
            let (_, errors) = Executor::new(graph(kind));
            assert!(
                errors.iter().any(
                    |error| matches!(error, PatternError::MissingCountGroup(id) if id == "det")
                ),
                "{errors:?}"
            );
        }
    }

    /// Resolves only `H-5GUI`, like the SDE would for these lines.
    struct H5gui;

    impl SystemResolver for H5gui {
        fn resolve(&self, name: &str) -> Option<usize> {
            name.eq_ignore_ascii_case("H-5GUI").then_some(1)
        }
    }

    /// The output kinds the shipped default graph fires for `text`, and the
    /// tooltip's leftover text.
    fn default_outputs(text: &str) -> (Vec<OutputKind>, Option<String>) {
        let (executor, errors) = Executor::new(RuleGraph::default_graph());
        assert!(errors.is_empty(), "{errors:?}");
        let context = LineContext {
            channel: String::from("wc.Vale+Tribute"),
            ..line(text)
        };
        let activations = executor.run(&context, &H5gui);
        let tooltip = activations
            .iter()
            .find(|activation| activation.kind == OutputKind::Tooltip)
            .map(|activation| {
                let messages: Vec<&Mensaje> = activation.messages.iter().collect();
                crate::map_alerts::leftover(text, &messages)
            });
        (
            activations
                .iter()
                .map(|activation| activation.kind)
                .collect(),
            tooltip,
        )
    }

    #[test]
    fn the_default_graph_is_valid() {
        assert!(RuleGraph::default_graph().validate().is_empty());
    }

    #[test]
    fn a_default_sighting_alerts_logs_and_lists_the_pilots() {
        let (kinds, tooltip) = default_outputs("H-5GUI*  Floris Saucus  nv");
        for kind in [OutputKind::Visual, OutputKind::Sound, OutputKind::Log] {
            assert!(kinds.contains(&kind), "{kind:?} missing from {kinds:?}");
        }
        assert!(!kinds.contains(&OutputKind::Suppress));
        assert_eq!(tooltip.as_deref(), Some("Floris Saucus"));
    }

    #[test]
    fn a_default_status_question_is_suppressed_and_not_listed() {
        let (kinds, tooltip) = default_outputs("H-5GUI status?");
        assert!(kinds.contains(&OutputKind::Suppress), "{kinds:?}");
        assert_eq!(tooltip, None);
    }

    #[test]
    fn a_default_clear_report_is_suppressed_but_listed() {
        let (kinds, tooltip) = default_outputs("H-5GUI Clear");
        assert!(kinds.contains(&OutputKind::Suppress), "{kinds:?}");
        assert_eq!(tooltip.as_deref(), Some(""));
    }
}
