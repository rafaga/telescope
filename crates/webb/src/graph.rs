//! Node-graph intel model: a DAG of `Input -> Detection -> ... -> Output`.
//!
//! A [`RuleGraph`] holds typed [`Node`]s and [`Edge`]s. The executor walks the
//! graph once per input line: an **Input** node emits the raw line as a
//! [`Mensaje`], every **Detection** node matches it, and every **Output** node
//! turns the messages that reach it into an [`Activation`].
//!
//! # Flow
//!
//! The graph is read from left to right. What travels on a wire for one
//! line is that line's *event* going on, together with what the nodes it went
//! through have found in it (their [`Mensaje`]s). An event either reaches a
//! node or it does not; there is nothing else to tell.
//!
//! * An **Input** lets the events of its source through, with nothing found
//!   yet (channel and MOTD filters apply here).
//! * A **Detection** reads the event itself, never what another node wrote,
//!   so chaining detections means *and*: with mode `require` the event goes on
//!   when the detection finds something (and what it found is added), with mode
//!   `exclude` it goes on when it finds nothing (*and not*).
//! * A **Union** (the aggregator) lets an event through when it reaches any of
//!   its inputs, once, with everything found on the way (*or*).
//! * A **Formatter** renders what reached it through a template.
//! * An **Output** fires when an event reaches it, with what was found.
//!
//! A disabled node lets nothing through. [`Mensaje`]s never repeat the line
//! metadata: the shared [`IntelEvent`] travels once per line alongside them.
//!
//! # Text and facts
//!
//! A Detection reads the text of the event by default. The ones that report
//! something a source can know by itself (`system_report`, `ship_names`,
//! `pilot_count`, see [`DetectionType::fact`]) read the source's
//! [`Facts`](crate::event::Facts) instead when it gave them, so a source
//! with structured data does not have to be searched as text; the node can
//! also ask for only one of the two ([`DetectionSource`]).

use crate::channel_memory::ChannelMemory;
use crate::event::{Facts, IntelEvent, ShipCount, SystemFact};
use crate::intel::{
    COUNT_GROUP, IntelCategory, MAX_CHANNELS, MAX_MATCHES_PER_CHUNK, MAX_SYSTEM_CANDIDATES,
    PatternError, REGEX_SIZE_LIMIT, has_word_boundaries, is_valid_channel, is_valid_id,
    sanitize_display,
};
use crate::rules::{
    DetectionMatcher, DetectionRuleKind, DetectionType, Dictionaries, FactKind, InputKind,
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

/// Structured payload a detection attaches to its [`Mensaje`], typed per
/// detection kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Data {
    /// Resolved solar-system ids (`system_report`).
    Systems(Vec<usize>),
    /// Ship names with how many times each was named (`ship_names`).
    Ships(Vec<ShipCount>),
    /// Number of pilots reported (`pilot_count`).
    Count(u32),
    /// Plain words (`clear_report`, `keyword`, `query`).
    Words(Vec<String>),
    /// Free text (the input line, `custom`).
    Text(String),
}

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

    /// The ship types the resolver knows, if it knows any. Without them the
    /// ships a chat line names stay known by name only.
    fn ship_resolver(&self) -> Option<&dyn ShipResolver> {
        None
    }
}

/// Resolves a ship name to its SDE type id, so what a player types in chat
/// is the same ship as what a killmail carries.
pub trait ShipResolver {
    /// The type id of the ship called `name` (ignoring case), if there is one.
    fn resolve_ship(&self, name: &str) -> Option<u32>;
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

/// What a Detection node reads. Only the detections of a type that can read
/// a fact (see [`DetectionType::fact`](crate::rules::DetectionType::fact))
/// have a choice; the others always read text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionSource {
    /// The fact the event's source gave, and the text of the event when it
    /// gave none.
    #[default]
    Auto,
    /// Only the facts of the event. An event whose source gave no such fact
    /// has nothing for it.
    Facts,
    /// Only the text of the event, even when the source gave the fact.
    Text,
}

impl DetectionSource {
    /// Every choice, in the order they are offered.
    pub const ALL: [Self; 3] = [Self::Auto, Self::Facts, Self::Text];

    /// The name stored in the graph.
    pub fn type_name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Facts => "facts",
            Self::Text => "text",
        }
    }
}

/// What a Detection does with the event it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionMode {
    /// The event goes on when the detection finds something, and what it
    /// found goes with it.
    #[default]
    Require,
    /// The event goes on when the detection finds nothing ("and not").
    Exclude,
}

impl DetectionMode {
    /// Both modes, in the order they are offered.
    pub const ALL: [Self; 2] = [Self::Require, Self::Exclude];

    /// The name stored in the graph.
    pub fn type_name(self) -> &'static str {
        match self {
            Self::Require => "require",
            Self::Exclude => "exclude",
        }
    }
}

/// Parameters of a Detection node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectionNode {
    /// The detection type and its parameters.
    pub kind: DetectionRuleKind,
    /// Whether matching is case-insensitive.
    #[serde(default)]
    pub case_insensitive: bool,
    /// What the detection reads: facts, text, or whatever fits (the default).
    #[serde(default)]
    pub source: DetectionSource,
    /// Whether the event goes on when the detection finds something (the
    /// default) or when it does not.
    #[serde(default)]
    pub mode: DetectionMode,
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

/// A directed edge between two node pins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id.
    pub from: String,
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
        NodeKind::Detection(_) | NodeKind::Output(_) => 1,
        NodeKind::Aggregator(node) => node.inputs as usize,
        NodeKind::Formatter(formatter) => formatter.inputs as usize,
    }
}

/// Whether a node has an output pin (every node but an Output has exactly
/// one).
pub fn has_output_pin(kind: &NodeKind) -> bool {
    !matches!(kind, NodeKind::Output(_))
}

impl RuleGraph {
    /// Validates the graph: unique/valid ids, per-kind parameters, one cable
    /// per input (an Output takes any number), exactly one Output per kind,
    /// valid pins and no cycles.
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
        let mut cabled_pins: HashSet<(&str, u8)> = HashSet::new();
        for edge in &self.edges {
            let from = self.nodes.iter().find(|node| node.id == edge.from);
            let to = self.nodes.iter().find(|node| node.id == edge.to);
            match (from, to) {
                (Some(from), Some(to)) => {
                    if !has_output_pin(&from.kind) {
                        errors.push(PatternError::InvalidPin {
                            node: from.id.clone(),
                            pin: String::from("out"),
                        });
                    }
                    if edge.to_pin as usize >= input_pin_count(&to.kind) {
                        errors.push(PatternError::InvalidPin {
                            node: to.id.clone(),
                            pin: edge.to_pin.to_string(),
                        });
                    }
                    *incoming.entry(edge.to.as_str()).or_default() += 1;
                    // An output takes the cables of any number of rules.
                    if !cabled_pins.insert((edge.to.as_str(), edge.to_pin))
                        && !matches!(to.kind, NodeKind::Output(_))
                    {
                        errors.push(PatternError::MultipleCables(to.id.clone()));
                    }
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
                    if incoming.get(node.id.as_str()).copied().unwrap_or(0) == 0 {
                        errors.push(PatternError::MissingInput(node.id.clone()));
                    }
                    if let Err(error) = detection.kind.validate(&node.id, dictionaries) {
                        errors.push(error);
                    }
                    if detection.source == DetectionSource::Facts && detection.kind.fact().is_none()
                    {
                        errors.push(PatternError::NoFactsToRead(node.id.clone()));
                    }
                }
                NodeKind::Output(output) => {
                    if incoming.get(node.id.as_str()).copied().unwrap_or(0) == 0 {
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
                NodeKind::Formatter(_) => {
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

    /// The ids of the nodes that make the zKillboard rules: every node a
    /// zKillboard input reaches, up to the outputs (which are not part of
    /// them). The ids are whatever the graph says, so the rules can be renamed
    /// without touching the code.
    pub fn zkillboard_chain(&self) -> HashSet<String> {
        let is_output = |id: &str| {
            self.nodes
                .iter()
                .any(|node| node.id == id && matches!(node.kind, NodeKind::Output(_)))
        };
        let mut chain: HashSet<String> = self
            .nodes
            .iter()
            .filter(|node| {
                matches!(&node.kind, NodeKind::Input(input) if input.kind == InputKind::ZKillboard)
            })
            .map(|node| node.id.clone())
            .collect();
        let mut pending: Vec<String> = chain.iter().cloned().collect();
        while let Some(from) = pending.pop() {
            for edge in self.edges.iter().filter(|edge| edge.from == from) {
                if !is_output(&edge.to) && chain.insert(edge.to.clone()) {
                    pending.push(edge.to.clone());
                }
            }
        }
        chain
    }

    /// This graph with the default zKillboard rules added: the input, the
    /// detections that read the facts of its killmails and their cables to
    /// the outputs (see `rules.toml`). What the graph already has is kept as
    /// it is.
    ///
    /// Nothing is added when the graph has a zKillboard input already (the
    /// user has their own rules for it) or a node with one of the ids the
    /// rules use. An output takes the cables of any number of rules, so the
    /// new ones go straight to it; an output the graph lacks gets none.
    pub fn with_zkillboard_rules(mut self) -> Self {
        let template = Self::default_graph();
        let chain = template.zkillboard_chain();
        let has_input = self.nodes.iter().any(|node| {
            matches!(&node.kind, NodeKind::Input(input) if input.kind == InputKind::ZKillboard)
        });
        let id_taken = self.nodes.iter().any(|node| chain.contains(&node.id));
        if has_input || id_taken {
            return self;
        }
        let in_chain = |id: &str| chain.contains(id);
        let has_output = |graph: &Self, id: &str| {
            graph
                .nodes
                .iter()
                .any(|node| node.id == id && matches!(node.kind, NodeKind::Output(_)))
        };
        let edges: Vec<Edge> = template
            .edges
            .iter()
            .filter(|edge| {
                in_chain(&edge.from) && (in_chain(&edge.to) || has_output(&self, &edge.to))
            })
            .cloned()
            .collect();
        self.nodes.extend(
            template
                .nodes
                .iter()
                .filter(|node| in_chain(&node.id))
                .cloned(),
        );
        self.edges.extend(edges);
        self
    }
}

/// A detection compiled once and reused for every line.
struct CompiledDetection {
    kind: DetectionRuleKind,
    source: DetectionSource,
    mode: DetectionMode,
    regexes: Vec<Regex>,
    automaton: Option<AhoCorasick>,
    /// The dictionary ignores case (Unicode, not only ASCII): the automaton
    /// holds the lowercased words and runs over the lowercased line.
    fold: bool,
}

/// What a detection makes of the facts of an event.
enum FromFacts {
    /// The source did not give the fact the detection reads: read the text.
    NotGiven,
    /// The source gave it and it holds nothing for this detection. That is
    /// final: the text is not searched.
    Nothing,
    /// The message the fact makes.
    Found(Mensaje),
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
        let (patterns, words) = match node.kind.matcher(dictionaries) {
            DetectionMatcher::Regex(patterns) => (patterns, None),
            DetectionMatcher::Dictionary(words) => (Vec::new(), Some(words)),
            DetectionMatcher::Mixed { patterns, words } => (patterns, Some(words)),
        };
        let regexes = Self::build_regexes(node, id, &patterns)?;
        let automaton = match words {
            Some(words) => Some(Self::build_automaton(node, id, words)?),
            None => None,
        };
        Ok(Self {
            kind: node.kind.clone(),
            source: node.source,
            mode: node.mode,
            regexes,
            automaton,
            fold: node.case_insensitive,
        })
    }

    fn build_regexes(
        node: &DetectionNode,
        id: &str,
        patterns: &[String],
    ) -> Result<Vec<Regex>, PatternError> {
        let mut regexes = Vec::new();
        for pattern in patterns {
            let regex = RegexBuilder::new(pattern)
                .case_insensitive(node.case_insensitive)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_SIZE_LIMIT)
                .build()
                .map_err(|error| PatternError::InvalidPattern {
                    id: id.to_string(),
                    reason: error.to_string(),
                })?;
            let has_group = |group: &str| regex.capture_names().flatten().any(|name| name == group);
            if let Some(group) = node.kind.system_group()
                && !has_group(&group)
            {
                return Err(PatternError::InvalidSystemGroup {
                    id: id.to_string(),
                    group,
                });
            }
            if node.kind.category() == Some(IntelCategory::Count) && !has_group(COUNT_GROUP) {
                return Err(PatternError::MissingCountGroup(id.to_string()));
            }
            regexes.push(regex);
        }
        Ok(regexes)
    }

    fn build_automaton(
        node: &DetectionNode,
        id: &str,
        words: Vec<String>,
    ) -> Result<AhoCorasick, PatternError> {
        let words: Vec<String> = if node.case_insensitive {
            words.iter().map(|word| fold(word)).collect()
        } else {
            words
        };
        AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&words)
            .map_err(|error| PatternError::DictionaryBuildFailed {
                id: id.to_string(),
                reason: error.to_string(),
            })
    }

    fn matches(&self, text: &str, resolver: &dyn SystemResolver) -> Vec<MatchData> {
        let mut results = Vec::new();
        let system_group = self.kind.system_group();
        for regex in &self.regexes {
            for caps in regex.captures_iter(text) {
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
            // With the case ignored the line is searched lowercased, and each
            // match is mapped back to the bytes it came from.
            let folded;
            let (haystack, origin) = if self.fold {
                folded = fold_with_map(text);
                (folded.0.as_str(), Some(folded.1.as_slice()))
            } else {
                (text, None)
            };
            for matched in automaton.find_iter(haystack) {
                if results.len() >= MAX_MATCHES_PER_CHUNK {
                    break;
                }
                let (start, end) = match origin {
                    Some(origin) => (origin[matched.start()].0, origin[matched.end() - 1].1),
                    None => (matched.start(), matched.end()),
                };
                if !has_word_boundaries(text, start, end) {
                    continue;
                }
                let found = sanitize_display(&text[start..end]);
                let mut captures = HashMap::new();
                captures.insert("word".to_string(), found.clone());
                results.push(MatchData {
                    captures,
                    matched: found.clone(),
                    word: Some(found),
                    span: start..end,
                });
            }
        }
        if let DetectionRuleKind::SystemReport { ignore_words } = &self.kind {
            // The code-like names came from the pattern; the names made of
            // words are looked up (they only count if the universe knows them).
            let taken: Vec<Range<usize>> = results
                .iter()
                .filter(|found| {
                    found
                        .captures
                        .get("system")
                        .is_some_and(|name| resolver.resolve(name).is_some())
                })
                .map(|found| found.span.clone())
                .collect();
            let named = named_systems(text, resolver, &taken);
            results.extend(Self::without_ignored(
                text,
                named,
                ignore_words,
                taken.len(),
            ));
            results.sort_by_key(|found| found.span.start);
        }
        results
    }

    /// The names made of words that are not on the node's ignore list, plus
    /// the ignored ones that the line marks as a report: followed by `*`
    /// (`Van*`), written in capitals (`VAN`), or the only candidate of a line
    /// that says `nv`, `clr`, `clear`, `status` or a count. `code_like` is how
    /// many code-like systems the line already has: with one of those, a
    /// word that is not marked is taken for a pilot name.
    fn without_ignored(
        text: &str,
        named: Vec<MatchData>,
        ignore_words: &[String],
        code_like: usize,
    ) -> Vec<MatchData> {
        if ignore_words.is_empty() {
            return named;
        }
        static CONTEXT: OnceLock<Regex> = OnceLock::new();
        let context = CONTEXT.get_or_init(|| {
            Regex::new(r"(?i)(?:^|\s)(?:nv|clr|clear|status|[+x]\d{1,3}|\d{1,3}\+)(?:\s|$)")
                .expect("the context pattern must compile")
        });
        let ignored: Vec<String> = ignore_words.iter().map(|word| fold(word)).collect();
        let is_ignored = |found: &MatchData| ignored.contains(&fold(&found.matched));
        let others = code_like + named.iter().filter(|found| !is_ignored(found)).count();
        let line_reports = others == 0 && context.is_match(text);
        named
            .into_iter()
            .filter(|found| {
                if !is_ignored(found) {
                    return true;
                }
                let starred = text[found.span.end..].starts_with('*');
                let capitals = found.matched.chars().count() >= 3
                    && found.matched.chars().any(char::is_uppercase)
                    && !found.matched.chars().any(char::is_lowercase);
                starred || capitals || line_reports
            })
            .collect()
    }

    /// What this detection finds in `event`, according to what the node
    /// reads (see [`DetectionSource`]).
    fn read(&self, event: &IntelEvent, resolver: &dyn SystemResolver) -> Option<Mensaje> {
        let text = &event.line.text;
        match self.source {
            DetectionSource::Facts => match self.read_facts(&event.facts, resolver) {
                FromFacts::Found(found) => Some(found),
                FromFacts::Nothing | FromFacts::NotGiven => None,
            },
            DetectionSource::Auto => self.evaluate(text, &event.facts, resolver),
            DetectionSource::Text => self.process(&self.matches(text, resolver), resolver),
        }
    }

    /// What this detection finds in an event: what the source says about it
    /// when it gave the fact this detection reads, and what the text says
    /// otherwise.
    fn evaluate(
        &self,
        text: &str,
        facts: &Facts,
        resolver: &dyn SystemResolver,
    ) -> Option<Mensaje> {
        match self.read_facts(facts, resolver) {
            FromFacts::Found(message) => Some(message),
            FromFacts::Nothing => None,
            FromFacts::NotGiven => self.process(&self.matches(text, resolver), resolver),
        }
    }

    /// The message for the fact this detection reads, if `facts` has it.
    fn read_facts(&self, facts: &Facts, resolver: &dyn SystemResolver) -> FromFacts {
        let message = match self.kind.fact() {
            Some(FactKind::Systems) => match &facts.systems {
                Some(systems) => self.systems_fact(systems),
                None => return FromFacts::NotGiven,
            },
            Some(FactKind::Ships) => match &facts.ships {
                Some(ships) => self.ships_fact(ships, resolver),
                None => return FromFacts::NotGiven,
            },
            Some(FactKind::Pilots) => match facts.pilots {
                Some(pilots) => self.pilots_fact(pilots),
                None => return FromFacts::NotGiven,
            },
            None => return FromFacts::NotGiven,
        };
        match message {
            Some(mut message) => {
                if !matches!(message.data, Data::Systems(_)) {
                    message.category = self.kind.category();
                }
                FromFacts::Found(message)
            }
            None => FromFacts::Nothing,
        }
    }

    /// A `system_report` for the systems a source reported. Like the text
    /// path it stops at the limit of systems per line, but it does not ask the
    /// resolver: the source already knows the ids.
    fn systems_fact(&self, systems: &[SystemFact]) -> Option<Mensaje> {
        let mut ids = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut spans = Vec::new();
        for system in systems.iter().take(MAX_SYSTEM_CANDIDATES) {
            spans.extend(system.span.clone());
            if !ids.contains(&system.id) {
                ids.push(system.id);
            }
            if !names
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&system.name))
            {
                names.push(system.name.clone());
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

    /// The ships a source reported, read the way the text would be: the
    /// detection's dictionaries find the ship names in each name (so the node
    /// chooses which ships count, whatever the source) and every one found
    /// counts as many times as the source says.
    fn ships_fact(&self, ships: &[ShipCount], resolver: &dyn SystemResolver) -> Option<Mensaje> {
        let mut known: Vec<ShipCount> = Vec::new();
        for ship in ships {
            let found = self.matches(&ship.name, resolver);
            // The id is only kept when the dictionary found the whole name:
            // a name with several ships in it is not one ship.
            let whole =
                matches!(found.as_slice(), [only] if fold(&only.matched) == fold(&ship.name));
            for found in found {
                let mut counted = ShipCount::new(found.matched, ship.times);
                counted.type_id = ship.type_id.filter(|_| whole);
                ShipCount::tally(&mut known, Self::identify(counted, resolver));
            }
        }
        (!known.is_empty()).then(|| Mensaje {
            tag: self.kind.type_name().to_string(),
            text: format_ships(&known),
            data: Data::Ships(known),
            spans: Vec::new(),
            category: None,
        })
    }

    /// A `pilot_count` for the pilots a source reported. None are no report.
    fn pilots_fact(&self, pilots: u32) -> Option<Mensaje> {
        (pilots > 0).then(|| Mensaje {
            tag: self.kind.type_name().to_string(),
            text: pilots.to_string(),
            data: Data::Count(pilots),
            spans: Vec::new(),
            category: None,
        })
    }

    /// `ship` with its type id, when it has none and the resolver knows the
    /// name.
    fn identify(mut ship: ShipCount, resolver: &dyn SystemResolver) -> ShipCount {
        if ship.type_id.is_none() {
            ship.type_id = resolver
                .ship_resolver()
                .and_then(|ships| ships.resolve_ship(&ship.name));
        }
        ship
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
                let mut ships: Vec<ShipCount> = Vec::new();
                for found in matches {
                    ShipCount::tally(
                        &mut ships,
                        Self::identify(ShipCount::new(found.matched.clone(), 1), resolver),
                    );
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
            // The limit counts the systems the line reports (a system named
            // twice counts twice), not the words that were tried.
            if spans.len() >= MAX_SYSTEM_CANDIDATES {
                break;
            }
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

/// `text` lowercased, character by character, the way the dictionaries ignore
/// case (Unicode: `CÁPSULA` is `cápsula`, not only `RIFTER` and `rifter`).
pub(crate) fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// [`fold`] of `text`, and for every byte of the result the bytes of `text`
/// its character came from, so a match in the folded line can be mapped back.
fn fold_with_map(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut folded = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (start, c) in text.char_indices() {
        let end = start + c.len_utf8();
        for lower in c.to_lowercase() {
            let before = folded.len();
            folded.push(lower);
            origin.extend(std::iter::repeat_n((start, end), folded.len() - before));
        }
    }
    (folded, origin)
}

/// Most words a system name is made of (`Old Man Star`).
const MAX_SYSTEM_WORDS: usize = 3;

/// The systems named with words (`Jita`, `Old Man Star`, `Tash-Murkon Prime`),
/// whatever their case, that the universe knows.
///
/// Once the case is ignored a name cannot be told from any other word by its
/// shape, so every run of up to three words is tried against `resolver`,
/// longest first at each position: `hostile in jita` finds `jita` instead of
/// being taken for one three-word name, and `old man star` is found whole.
/// What is already `taken` (code-like names) is skipped, and the words of a
/// name found are not tried again.
fn named_systems(
    text: &str,
    resolver: &dyn SystemResolver,
    taken: &[Range<usize>],
) -> Vec<MatchData> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| {
        Regex::new("[A-Za-z]{2,14}(?:-[A-Za-z]{2,14}){0,2}").expect("the word pattern must compile")
    });
    let words: Vec<Range<usize>> = word
        .find_iter(text)
        .map(|found| found.range())
        .filter(|range| {
            !taken
                .iter()
                .any(|used| range.start < used.end && used.start < range.end)
        })
        .collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let mut matched = None;
        for length in (1..=MAX_SYSTEM_WORDS.min(words.len() - index)).rev() {
            let run = &words[index..index + length];
            // The words of a name are separated by blanks, nothing else.
            let joined = run.windows(2).all(|pair| {
                let gap = &text[pair[0].end..pair[1].start];
                !gap.is_empty() && gap.chars().all(char::is_whitespace)
            });
            if !joined {
                continue;
            }
            let span = run[0].start..run[length - 1].end;
            let name = run
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>()
                .join(" ");
            if resolver.resolve(&name).is_some() {
                matched = Some((span, name, length));
                break;
            }
        }
        match matched {
            Some((span, name, length)) => {
                let mut captures = HashMap::new();
                captures.insert(String::from("system"), sanitize_display(&name));
                found.push(MatchData {
                    captures,
                    matched: sanitize_display(&name),
                    word: None,
                    span,
                });
                index += length;
            }
            None => index += 1,
        }
    }
    found
}

/// `"nave1 (x3) nave2 (x4)"` (a single mention has no count).
fn format_ships(ships: &[ShipCount]) -> String {
    ships
        .iter()
        .map(|ship| {
            if ship.times > 1 {
                format!("{} (x{})", ship.name, ship.times)
            } else {
                ship.name.clone()
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
        for edge in graph.edges {
            edges_out.entry(edge.from.clone()).or_default().push(edge);
        }

        (
            Self {
                nodes,
                compiled,
                edges_out,
            },
            errors,
        )
    }

    /// Runs the graph over one event and returns the Output nodes it
    /// reached, in graph order.
    pub fn run(&self, event: &IntelEvent, resolver: &dyn SystemResolver) -> Vec<Activation> {
        // What reached each input pin: what the nodes upstream found. A pin
        // with an entry is a pin the event reached.
        let mut incoming: HashMap<(String, u8), Vec<Mensaje>> = HashMap::new();
        let mut activations = Vec::new();
        for node in &self.nodes {
            let reached: Vec<Option<Vec<Mensaje>>> = (0..input_pin_count(&node.kind))
                .map(|pin| incoming.remove(&(node.id.clone(), pin as u8)))
                .collect();
            if !node.enabled {
                continue;
            }
            match &node.kind {
                NodeKind::Input(input) => {
                    if input.kind != event.kind {
                        continue;
                    }
                    // Channels and the channel MOTD only exist in chat logs.
                    if input.kind == InputKind::ChatLog {
                        if !input.channels.is_empty()
                            && !input.channels.iter().any(|name| name == &event.channel)
                        {
                            continue;
                        }
                        if input.exclude_motd && motd_regex().is_match(&event.line.text) {
                            continue;
                        }
                    }
                    self.route(&node.id, Vec::new(), &mut incoming);
                }
                NodeKind::Detection(_) => {
                    let Some(Some(found_so_far)) = reached.into_iter().next() else {
                        continue;
                    };
                    let Some(compiled) = self.compiled.get(&node.id) else {
                        continue;
                    };
                    let found = compiled.read(event, resolver);
                    let flow = match (compiled.mode, found) {
                        (DetectionMode::Require, Some(found)) => {
                            let mut flow = found_so_far;
                            flow.push(found);
                            flow
                        }
                        (DetectionMode::Exclude, None) => found_so_far,
                        _ => continue,
                    };
                    self.route(&node.id, flow, &mut incoming);
                }
                NodeKind::Aggregator(_) => {
                    let flows: Vec<Vec<Mensaje>> = reached.into_iter().flatten().collect();
                    if flows.is_empty() {
                        continue;
                    }
                    self.route(&node.id, Self::merge(flows), &mut incoming);
                }
                NodeKind::Formatter(formatter) => {
                    if reached.iter().all(Option::is_none) {
                        continue;
                    }
                    let per_pin: Vec<Vec<Mensaje>> = reached
                        .iter()
                        .map(|flow| flow.clone().unwrap_or_default())
                        .collect();
                    let all = Self::merge(reached.into_iter().flatten().collect());
                    let text = format_template(&formatter.template, &per_pin, &all);
                    let message = Mensaje {
                        tag: String::from(FORMATTED_TAG),
                        text: text.clone(),
                        data: Data::Text(text),
                        spans: Vec::new(),
                        category: None,
                    };
                    self.route(&node.id, vec![message], &mut incoming);
                }
                NodeKind::Output(output) => {
                    if let Some(Some(found)) = reached.into_iter().next() {
                        activations.push(Activation {
                            output_id: node.id.clone(),
                            kind: output.kind,
                            tooltip: output.tooltip.clone(),
                            log: output.log.clone(),
                            messages: found,
                        });
                    }
                }
            }
        }
        activations
    }

    /// The tags of the detections whose finding makes a line a follow-up of
    /// the last report (see [`Self::run_remembering`]): a clear, a number of
    /// pilots, or a keyword such as `nv`. A ship name alone does not: killmail
    /// links and chatter name ships too.
    const FOLLOW_UP_TAGS: [&'static str; 3] = ["clear_report", "pilot_count", "keyword"];

    /// Runs the graph over one event like [`Self::run`], but remembering what
    /// the channel reported.
    ///
    /// A line that reports exactly one system is remembered in `memory`
    /// (unless it says the system is clear: that report is over). A line that
    /// reports none, and that only says something about the situation (a
    /// clear, `+4`, `nv`; see [`Self::FOLLOW_UP_TAGS`]), is evaluated again as
    /// if it named the last system its channel reported, when that was not
    /// long ago ([`ChannelMemory::window`]). The system travels as a fact of
    /// the event, so every detection and output downstream sees it as if the
    /// line had said it. Anything else is evaluated as it is.
    pub fn run_remembering(
        &self,
        event: &IntelEvent,
        resolver: &dyn SystemResolver,
        memory: &mut ChannelMemory,
    ) -> Vec<Activation> {
        let activations = self.run(event, resolver);
        if event.facts.systems.is_some() {
            return activations;
        }
        let reported = Self::reported_systems(&activations);
        if !reported.is_empty() {
            if let [system] = reported.as_slice()
                && !Self::found(&activations, &["clear_report"])
            {
                memory.remember(&event.channel, system.clone(), event.at());
            }
            return activations;
        }
        // A line that names something shaped like a system the universe does
        // not know is about that one, not about the last report.
        if Self::looks_like_a_system(&event.line.text) {
            return activations;
        }
        let Some(system) = memory.recall(&event.channel, event.at()) else {
            return activations;
        };
        let attributed = event.clone().with_facts(Facts {
            systems: Some(vec![system.clone()]),
            ..event.facts.clone()
        });
        let attributed_activations = self.run(&attributed, resolver);
        if Self::found(&attributed_activations, &Self::FOLLOW_UP_TAGS) {
            attributed_activations
        } else {
            activations
        }
    }

    /// Whether `text` has a word shaped like a system's name (`H-5GUI`,
    /// `J123456`, `AD001`), whether or not the universe has it.
    fn looks_like_a_system(text: &str) -> bool {
        static SHAPE: OnceLock<Regex> = OnceLock::new();
        SHAPE
            .get_or_init(|| {
                Regex::new(crate::rules::DEFAULT_SYSTEM_PATTERN)
                    .expect("the default system pattern must compile")
            })
            .is_match(text)
    }

    /// The systems the messages that reached the outputs report, once each.
    /// The name is the message's text, so it is only a system's own name when
    /// the line reports one system.
    fn reported_systems(activations: &[Activation]) -> Vec<SystemFact> {
        let mut systems: Vec<SystemFact> = Vec::new();
        for message in activations
            .iter()
            .flat_map(|activation| &activation.messages)
        {
            if let Data::Systems(ids) = &message.data {
                for id in ids {
                    if !systems.iter().any(|known| known.id == *id) {
                        systems.push(SystemFact {
                            id: *id,
                            name: message.text.clone(),
                            span: None,
                        });
                    }
                }
            }
        }
        systems
    }

    /// Whether any message that reached an output has one of `tags`.
    fn found(activations: &[Activation], tags: &[&str]) -> bool {
        activations
            .iter()
            .flat_map(|activation| &activation.messages)
            .any(|message| tags.contains(&message.tag.as_str()))
    }

    /// What several flows found, together: the same finding reaching by two
    /// paths counts once, in order of appearance.
    fn merge(flows: Vec<Vec<Mensaje>>) -> Vec<Mensaje> {
        let mut merged: Vec<Mensaje> = Vec::new();
        for message in flows.into_iter().flatten() {
            if !merged.contains(&message) {
                merged.push(message);
            }
        }
        merged
    }

    fn route(
        &self,
        from: &str,
        found: Vec<Mensaje>,
        incoming: &mut HashMap<(String, u8), Vec<Mensaje>>,
    ) {
        for edge in self.edges_out.get(from).into_iter().flatten() {
            // Only an Output has several cables on a pin (see `validate`):
            // what they bring is one flow.
            incoming
                .entry((edge.to.clone(), edge.to_pin))
                .and_modify(|flow| *flow = Self::merge(vec![std::mem::take(flow), found.clone()]))
                .or_insert_with(|| found.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::IntelLine;

    struct FixedResolver;

    impl SystemResolver for FixedResolver {
        fn resolve(&self, name: &str) -> Option<usize> {
            (name == "Jita").then_some(30000142)
        }
    }

    fn line(text: &str) -> IntelEvent {
        IntelEvent::text(
            IntelLine {
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                author: String::from("Pilot"),
                text: text.to_string(),
            },
            "intel",
            InputKind::ChatLog,
        )
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
                        source: DetectionSource::Auto,
                        mode: DetectionMode::Require,
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
                    to: String::from("det"),
                    to_pin: 0,
                },
                Edge {
                    from: String::from("det"),
                    to: String::from("out"),
                    to_pin: 0,
                },
            ],
        }
    }

    #[test]
    fn a_valid_graph_passes() {
        let graph = graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        });
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
    }

    #[test]
    fn a_graph_round_trips_through_json() {
        let graph = graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        });
        let json = serde_json::to_string(&graph).unwrap();
        let back: RuleGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, back);
    }

    #[test]
    fn a_detection_without_input_is_an_error() {
        let mut graph = graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        });
        graph.edges.retain(|edge| edge.from != "in");
        assert!(matches!(
            graph.validate().as_slice(),
            [PatternError::MissingInput(id)] if id == "det"
        ));
    }

    #[test]
    fn two_cables_on_one_input_are_not_reported_as_a_missing_input() {
        let mut graph = graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        });
        graph.nodes.push(input_node("in2"));
        graph.edges.push(Edge {
            from: String::from("in2"),
            to: String::from("det"),
            to_pin: 0,
        });
        assert!(matches!(
            graph.validate().as_slice(),
            [PatternError::MultipleCables(id)] if id == "det"
        ));
    }

    /// Two systems, `Jita` and `Amarr`.
    struct TwoSystems;

    impl SystemResolver for TwoSystems {
        fn resolve(&self, name: &str) -> Option<usize> {
            match name.to_lowercase().as_str() {
                "jita" => Some(30000142),
                "amarr" => Some(30002187),
                _ => None,
            }
        }
    }

    /// The default rules over a channel that writes lines one after another.
    struct Channel {
        executor: Executor,
        memory: ChannelMemory,
    }

    impl Channel {
        fn new() -> Self {
            let (executor, errors) = Executor::new(RuleGraph::default_graph());
            assert!(errors.is_empty(), "{errors:?}");
            Self {
                executor,
                memory: ChannelMemory::with_default_window(),
            }
        }

        /// The line `text`, written in `channel` `seconds` after the start.
        fn event(channel: &str, seconds: i64, text: &str) -> IntelEvent {
            let mut event = line(text);
            event.channel = channel.to_string();
            event.line.timestamp =
                chrono::DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap();
            event
        }

        /// What the line makes the outputs do.
        fn says(&mut self, channel: &str, seconds: i64, text: &str) -> Vec<Activation> {
            self.executor.run_remembering(
                &Self::event(channel, seconds, text),
                &TwoSystems,
                &mut self.memory,
            )
        }

        fn kinds(activations: &[Activation]) -> Vec<OutputKind> {
            activations
                .iter()
                .map(|activation| activation.kind)
                .collect()
        }

        /// The systems the outputs were given.
        fn systems(activations: &[Activation]) -> Vec<usize> {
            let mut ids = Vec::new();
            for message in activations
                .iter()
                .flat_map(|activation| &activation.messages)
            {
                if let Data::Systems(found) = &message.data {
                    for id in found {
                        if !ids.contains(id) {
                            ids.push(*id);
                        }
                    }
                }
            }
            ids
        }
    }

    #[test]
    fn a_count_after_a_report_is_about_the_reported_system() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        let follow_up = channel.says("intel", 10, "+4");
        assert!(Channel::kinds(&follow_up).contains(&OutputKind::Visual));
        assert!(Channel::kinds(&follow_up).contains(&OutputKind::Sound));
        assert_eq!(Channel::systems(&follow_up), [30000142]);
        // Without the memory the same line alerts nothing.
        let alone = channel
            .executor
            .run(&Channel::event("intel", 10, "+4"), &TwoSystems);
        assert!(!Channel::kinds(&alone).contains(&OutputKind::Visual));
    }

    #[test]
    fn a_number_with_a_plus_after_a_report_is_about_the_reported_system() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        for (seconds, text) in [(4, "8+"), (9, "15+ Captain Ginjuic"), (12, "30+kkmora")] {
            let follow_up = channel.says("intel", seconds, text);
            assert_eq!(Channel::systems(&follow_up), [30000142], "{text:?}");
            assert!(
                Channel::kinds(&follow_up).contains(&OutputKind::Visual),
                "{text:?}"
            );
        }
        // A sum is not a count.
        let sum = channel.says("intel", 14, "1+1");
        assert!(Channel::systems(&sum).is_empty());
    }

    #[test]
    fn nv_and_clr_after_a_report_are_about_the_reported_system() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        let nv = channel.says("intel", 5, "nv");
        assert_eq!(Channel::systems(&nv), [30000142]);
        assert!(Channel::kinds(&nv).contains(&OutputKind::Visual));
        let clear = channel.says("intel", 8, "clr");
        assert_eq!(Channel::systems(&clear), [30000142]);
        assert!(Channel::kinds(&clear).contains(&OutputKind::Suppress));
        assert!(Channel::kinds(&clear).contains(&OutputKind::Tooltip));
    }

    #[test]
    fn a_follow_up_after_the_window_is_not_attributed() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        let late = channel.says("intel", ChannelMemory::DEFAULT_WINDOW_SECS + 1, "+4");
        assert!(!Channel::kinds(&late).contains(&OutputKind::Visual));
        assert!(Channel::systems(&late).is_empty());
    }

    #[test]
    fn a_channel_does_not_borrow_the_report_of_another() {
        let mut channel = Channel::new();
        channel.says("vale", 0, "Jita*  Some Pilot");
        let other = channel.says("imperium", 5, "+4");
        assert!(Channel::systems(&other).is_empty());
    }

    #[test]
    fn chatter_and_ships_are_not_follow_ups() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        for text in ["gj", "who is that?", "Rifter", "Kill: Someone (Capsule)"] {
            let activations = channel.says("intel", 5, text);
            assert!(
                !Channel::kinds(&activations).contains(&OutputKind::Visual),
                "{text:?} alerted"
            );
        }
    }

    #[test]
    fn a_line_with_a_system_of_its_own_is_not_attributed() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        // Another system it knows: its own.
        let known = channel.says("intel", 5, "Amarr +4");
        assert_eq!(Channel::systems(&known), [30002187]);
        // One it does not know: not the last report either.
        let unknown = channel.says("intel", 6, "HE-4V4 +4");
        assert!(Channel::systems(&unknown).is_empty());
        assert!(!Channel::kinds(&unknown).contains(&OutputKind::Visual));
    }

    #[test]
    fn a_clear_report_does_not_become_the_system_to_follow() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        channel.says("intel", 5, "Amarr clr");
        // Amarr is clear, so the count that comes next is still about Jita.
        let follow_up = channel.says("intel", 8, "+4");
        assert_eq!(Channel::systems(&follow_up), [30000142]);
    }

    #[test]
    fn a_report_of_several_systems_is_not_followed() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita Amarr  Some Pilot");
        let follow_up = channel.says("intel", 5, "+4");
        assert!(Channel::systems(&follow_up).is_empty());
    }

    #[test]
    fn a_source_that_knows_its_system_is_left_alone() {
        let mut channel = Channel::new();
        channel.says("intel", 0, "Jita*  Some Pilot");
        let mut event = Channel::event("intel", 5, "+4");
        event.facts.systems = Some(vec![SystemFact {
            id: 99,
            name: String::from("Elsewhere"),
            span: None,
        }]);
        let activations =
            channel
                .executor
                .run_remembering(&event, &TwoSystems, &mut channel.memory);
        assert_eq!(Channel::systems(&activations), [99]);
    }

    #[test]
    fn a_cycle_is_rejected() {
        let mut graph = graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        });
        graph.edges.push(Edge {
            from: String::from("out"),
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
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        }));
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
    fn only_the_candidates_that_resolve_keep_their_span() {
        // "Floris" is shaped like a system and is not one. Its bytes are not
        // claimed by the detection, so they stay in the leftover text.
        let message = only_message(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            "Floris at Jita",
        );
        assert_eq!(message.data, Data::Systems(vec![30000142]));
        assert_eq!(message.text, "Jita");
        assert_eq!(message.spans, vec![10..14]);
        assert_eq!(
            crate::map_alerts::leftover("Floris at Jita", &[&message]),
            "Floris at"
        );
    }

    #[test]
    fn a_line_whose_candidates_all_fail_to_resolve_gives_no_message() {
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        }));
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            executor
                .run(&line("Floris at Dodixie"), &FixedResolver)
                .is_empty()
        );
    }

    /// Resolves every `ADnnn` to its number.
    struct Numbered;

    impl SystemResolver for Numbered {
        fn resolve(&self, name: &str) -> Option<usize> {
            name.strip_prefix("AD")?.parse().ok()
        }
    }

    #[test]
    fn a_system_detection_reads_at_most_the_candidate_limit() {
        let text = (1..=MAX_SYSTEM_CANDIDATES + 4)
            .map(|n| format!("AD{n:03}"))
            .collect::<Vec<_>>()
            .join(" ");
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        }));
        assert!(errors.is_empty(), "{errors:?}");
        let message = executor
            .run(&line(&text), &Numbered)
            .remove(0)
            .messages
            .remove(0);
        let expected: Vec<usize> = (1..=MAX_SYSTEM_CANDIDATES).collect();
        assert_eq!(message.data, Data::Systems(expected));
        assert_eq!(message.spans.len(), MAX_SYSTEM_CANDIDATES);
        // The ones past the limit were not read: they stay in the leftover.
        assert_eq!(
            crate::map_alerts::leftover(&text, &[&message]),
            "AD009 AD010 AD011 AD012"
        );
    }

    /// Knows a few systems of every style, in any case, like the resolver of
    /// the application.
    struct AnyCase;

    impl SystemResolver for AnyCase {
        fn resolve(&self, name: &str) -> Option<usize> {
            match name.to_lowercase().as_str() {
                "jita" => Some(1),
                "old man star" => Some(2),
                "tash-murkon prime" => Some(3),
                "h-5gui" => Some(4),
                "j105443" => Some(5),
                "ad001" => Some(6),
                _ => None,
            }
        }
    }

    /// The systems a line reports, in order. The system detection does not
    /// depend on the node's case switch (it is off here).
    fn systems_in(text: &str) -> Vec<usize> {
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        }));
        assert!(errors.is_empty(), "{errors:?}");
        match executor.run(&line(text), &AnyCase).first() {
            Some(activation) => match &activation.messages[0].data {
                Data::Systems(ids) => ids.clone(),
                other => panic!("{other:?}"),
            },
            None => Vec::new(),
        }
    }

    /// `AnyCase` plus two systems whose names are also pilot names.
    struct WithPilotNames;

    impl SystemResolver for WithPilotNames {
        fn resolve(&self, name: &str) -> Option<usize> {
            match name.to_lowercase().as_str() {
                "van" => Some(7),
                "jan" => Some(8),
                _ => AnyCase.resolve(name),
            }
        }
    }

    /// The systems a line reports when `Van` and `Jan` are on the ignore list.
    fn systems_ignoring_pilot_names(text: &str) -> Vec<usize> {
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: vec![String::from("van"), String::from("Jan")],
        }));
        assert!(errors.is_empty(), "{errors:?}");
        match executor.run(&line(text), &WithPilotNames).first() {
            Some(activation) => match &activation.messages[0].data {
                Data::Systems(ids) => ids.clone(),
                other => panic!("{other:?}"),
            },
            None => Vec::new(),
        }
    }

    #[test]
    fn an_ignored_word_is_not_a_system() {
        assert!(systems_ignoring_pilot_names("Van Gogh in the area").is_empty());
        assert!(systems_ignoring_pilot_names("jan and friends").is_empty());
        // The other systems of the line stay.
        assert_eq!(systems_ignoring_pilot_names("Jita  Van Gogh"), [1]);
        // Without a list the word is a system, as before.
        assert_eq!(systems_in("Jita"), [1]);
    }

    #[test]
    fn an_ignored_word_that_is_marked_is_a_system() {
        assert_eq!(systems_ignoring_pilot_names("Van*  Some Pilot"), [7]);
        assert_eq!(systems_ignoring_pilot_names("VAN  Some Pilot"), [7]);
    }

    #[test]
    fn an_ignored_word_alone_with_a_report_context_is_a_system() {
        assert_eq!(systems_ignoring_pilot_names("Van nv"), [7]);
        assert_eq!(systems_ignoring_pilot_names("jan clr"), [8]);
        assert_eq!(systems_ignoring_pilot_names("Van +4"), [7]);
        // With another system in the line it is a pilot, context or not.
        assert_eq!(systems_ignoring_pilot_names("H-5GUI*  Van Gogh  nv"), [4]);
        assert_eq!(systems_ignoring_pilot_names("Jita  Van  nv"), [1]);
    }

    #[test]
    fn code_like_names_are_never_ignored() {
        let (executor, _) = Executor::new(graph(DetectionRuleKind::SystemReport {
            ignore_words: vec![String::from("h-5gui")],
        }));
        let found = executor.run(&line("H-5GUI*  Some Pilot"), &AnyCase);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn systems_are_read_whatever_their_case() {
        for (text, expected) in [
            ("Jita", 1),
            ("jita", 1),
            ("JITA", 1),
            ("jItA", 1),
            ("Old Man Star", 2),
            ("old man star", 2),
            ("OLD MAN STAR", 2),
            ("Old   Man   Star", 2),
            ("Tash-Murkon Prime", 3),
            ("tash-murkon prime", 3),
            ("TASH-MURKON PRIME", 3),
            ("H-5GUI", 4),
            ("h-5gui", 4),
            ("H-5gui", 4),
            ("J105443", 5),
            ("j105443", 5),
            ("AD001", 6),
            ("ad001", 6),
        ] {
            assert_eq!(systems_in(text), [expected], "{text:?}");
        }
    }

    #[test]
    fn a_system_is_found_among_ordinary_words() {
        // Not taken for one long name made of the words around it.
        assert_eq!(systems_in("hostile in jita now"), [1]);
        assert_eq!(systems_in("gate camp in old man star now"), [2]);
        assert_eq!(systems_in("tash-murkon prime to jita"), [3, 1]);
        assert_eq!(systems_in("jita old man star h-5gui"), [1, 2, 4]);
    }

    #[test]
    fn words_that_are_not_systems_are_not_read() {
        for text in [
            "hostile in the area",
            "Floris Saucus",
            "man star old",
            "old man",
            "x-up",
            "re-ship",
            "+5",
            "5gui",
        ] {
            assert_eq!(systems_in(text), Vec::<usize>::new(), "{text:?}");
        }
    }

    #[test]
    fn the_words_of_a_name_are_separated_by_blanks_only() {
        // Punctuation between the words is not part of any name.
        assert_eq!(systems_in("old, man star"), Vec::<usize>::new());
        assert_eq!(systems_in("old - man star"), Vec::<usize>::new());
        assert_eq!(systems_in("old\tman star"), [2]);
    }

    /// The ships a line reports with the dictionaries `names`, ignoring case.
    fn ships_in(text: &str, names: &[&str]) -> Vec<(String, u32)> {
        let kind = DetectionRuleKind::ShipNames {
            dictionaries: names.iter().map(|name| name.to_string()).collect(),
        };
        let mut graph = graph(kind);
        for node in &mut graph.nodes {
            if let NodeKind::Detection(detection) = &mut node.kind {
                detection.case_insensitive = true;
            }
        }
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        match executor.run(&line(text), &FixedResolver).first() {
            Some(activation) => match &activation.messages[0].data {
                Data::Ships(ships) => ships
                    .iter()
                    .map(|ship| (ship.name.clone(), ship.times))
                    .collect(),
                other => panic!("{other:?}"),
            },
            None => Vec::new(),
        }
    }

    #[test]
    fn ships_are_read_whatever_their_case() {
        for text in ["Rifter", "rifter", "RIFTER", "rIfTeR"] {
            let ships = ships_in(text, &["ship_report_en"]);
            assert_eq!(ships.len(), 1, "{text:?}");
            assert_eq!(ships[0].1, 1, "{text:?}");
        }
        // The three spellings are one ship, named as it was first typed.
        assert_eq!(
            ships_in("rifter RIFTER Rifter", &["ship_report_en"]),
            [(String::from("rifter"), 3)]
        );
    }

    #[test]
    fn ships_with_accents_are_read_whatever_their_case() {
        // Only ASCII letters change case under `ascii_case_insensitive`: an
        // upper case `Á` used to hide the ship.
        for text in ["Cápsula", "cápsula", "CÁPSULA", "cÁpSuLa"] {
            let ships = ships_in(text, &["ship_report_es"]);
            assert_eq!(ships.len(), 1, "{text:?}");
        }
        assert_eq!(
            ships_in("Cápsula cápsula CÁPSULA", &["ship_report_es"]),
            [(String::from("Cápsula"), 3)]
        );
        // A word that merely contains one is not the ship.
        assert!(ships_in("CÁPSULAS", &["ship_report_es"]).is_empty());
    }

    #[test]
    fn a_ship_keeps_its_place_in_a_line_after_characters_that_change_length() {
        // `İ` is two bytes and lowercases to three (`i` and a combining dot):
        // the match in the lowercased line must map back to the right bytes.
        let (folded, origin) = fold_with_map("İ rifter");
        assert_eq!(folded.len(), origin.len());
        let start = folded.find("rifter").unwrap();
        assert_eq!(origin[start].0, 3);
        assert_eq!(origin[folded.len() - 1].1, 9);

        let kind = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let mut graph = graph(kind);
        for node in &mut graph.nodes {
            if let NodeKind::Detection(detection) = &mut node.kind {
                detection.case_insensitive = true;
            }
        }
        let (executor, _) = Executor::new(graph);
        let activations = executor.run(&line("İ rifter"), &FixedResolver);
        assert_eq!(activations[0].messages[0].spans, vec![3..9]);
    }

    #[test]
    fn folding_is_unicode_lowercasing() {
        assert_eq!(fold("RIFTER"), "rifter");
        assert_eq!(fold("CÁPSULA Ñandú"), "cápsula ñandú");
        assert_eq!(fold(""), "");
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
                ShipCount::new("Rifter", 2),
                ShipCount::new("Merlin", 1)
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
                source: DetectionSource::Auto,
                mode: DetectionMode::Require,
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

    fn edge(from: &str, to: &str, to_pin: u8) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            to_pin,
        }
    }

    fn zkillboard_input_node(id: &str) -> Node {
        Node {
            kind: NodeKind::Input(InputNode {
                description: String::from("kills"),
                kind: InputKind::ZKillboard,
                path: String::new(),
                // Chat-log options that mean nothing here: they must not
                // filter a killmail out.
                channels: vec![String::from("Local")],
                exclude_motd: true,
            }),
            ..input_node(id)
        }
    }

    /// `input -> system report -> log` over the given Input node.
    fn system_graph(input: Node) -> RuleGraph {
        let id = input.id.clone();
        RuleGraph {
            nodes: vec![
                input,
                detection_node(
                    "sys",
                    DetectionRuleKind::SystemReport {
                        ignore_words: Vec::new(),
                    },
                ),
                output_node("out"),
            ],
            edges: vec![edge(&id, "sys", 0), edge("sys", "out", 0)],
        }
    }

    fn killmail_line(text: &str) -> IntelEvent {
        IntelEvent {
            channel: String::from("zkillboard"),
            kind: InputKind::ZKillboard,
            ..line(text)
        }
    }

    #[test]
    fn a_zkillboard_input_receives_killmail_lines() {
        let graph = system_graph(zkillboard_input_node("kills"));
        assert!(graph.validate().is_empty(), "{:?}", graph.validate());
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        let activations = executor.run(&killmail_line("Jita Rifter destroyed"), &FixedResolver);
        assert_eq!(activations.len(), 1);
        assert_eq!(
            activations[0].messages[0].data,
            Data::Systems(vec![30000142])
        );
    }

    #[test]
    fn a_zkillboard_input_ignores_chat_lines() {
        let (executor, _) = Executor::new(system_graph(zkillboard_input_node("kills")));
        assert!(
            executor
                .run(&line("hostile in Jita"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn a_chat_log_input_ignores_killmail_lines() {
        // Not even with "every channel" selected (no channels listed).
        let (executor, _) = Executor::new(system_graph(input_node("chat")));
        assert!(
            executor
                .run(&killmail_line("Jita Rifter destroyed"), &FixedResolver)
                .is_empty()
        );
    }

    /// `zkillboard input -> detection -> log`: the first message `detection`
    /// sends for `event`.
    fn message_of(detection: DetectionRuleKind, event: &IntelEvent) -> Option<Mensaje> {
        message_with(detection, event, &FixedResolver)
    }

    fn message_with(
        detection: DetectionRuleKind,
        event: &IntelEvent,
        resolver: &dyn SystemResolver,
    ) -> Option<Mensaje> {
        let graph = RuleGraph {
            nodes: vec![
                zkillboard_input_node("in"),
                detection_node("det", detection),
                output_node("out"),
            ],
            edges: vec![edge("in", "det", 0), edge("det", "out", 0)],
        };
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        executor
            .run(event, resolver)
            .first()
            .map(|activation| activation.messages[0].clone())
    }

    fn killmail_event(text: &str, facts: Facts) -> IntelEvent {
        killmail_line(text).with_facts(facts)
    }

    fn system_fact(id: usize, name: &str, span: Option<Range<usize>>) -> SystemFact {
        SystemFact {
            id,
            name: name.to_string(),
            span,
        }
    }

    #[test]
    fn a_system_report_reads_the_systems_the_source_gave() {
        // The text names no system and the resolver does not know this one:
        // the source's id is enough.
        let event = killmail_event(
            "Nowhere: Rifter destroyed",
            Facts {
                systems: Some(vec![system_fact(77, "Nowhere", Some(0..7))]),
                ..Facts::default()
            },
        );
        let message = message_of(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            &event,
        )
        .unwrap();
        assert_eq!(message.tag, "system_report");
        assert_eq!(message.text, "Nowhere");
        assert_eq!(message.data, Data::Systems(vec![77]));
        assert_eq!(message.spans, vec![0..7]);
        assert_eq!(message.category, None);
        // The tooltip's leftover text leaves the system out, as it does for
        // one found in the text.
        assert_eq!(
            crate::map_alerts::leftover(&event.line.text, &[&message]),
            "Rifter destroyed"
        );
    }

    #[test]
    fn the_systems_of_a_source_are_listed_once_and_capped() {
        let systems = (0..20)
            .map(|index| system_fact(index % 12, &format!("S{}", index % 12), None))
            .collect();
        let event = killmail_event(
            "x",
            Facts {
                systems: Some(systems),
                ..Facts::default()
            },
        );
        let message = message_of(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            &event,
        )
        .unwrap();
        // 20 reported, the first MAX_SYSTEM_CANDIDATES read, repeats once.
        let Data::Systems(ids) = message.data else {
            panic!("not systems");
        };
        let expected: Vec<usize> = (0..MAX_SYSTEM_CANDIDATES).collect();
        assert_eq!(ids, expected);
    }

    fn with_source(mut node: Node, source: DetectionSource) -> Node {
        if let NodeKind::Detection(detection) = &mut node.kind {
            detection.source = source;
        }
        node
    }

    /// The first message of the Output node for `event` in
    /// `input -> [keyword detection ->] system report -> output`, where the
    /// system report reads `source`.
    fn system_report_reading(
        source: DetectionSource,
        chained: bool,
        event: &IntelEvent,
    ) -> Option<Mensaje> {
        let report = with_source(
            detection_node(
                "sys",
                DetectionRuleKind::SystemReport {
                    ignore_words: Vec::new(),
                },
            ),
            source,
        );
        let mut nodes = vec![zkillboard_input_node("in"), report, output_node("out")];
        let mut edges = vec![edge("sys", "out", 0)];
        if chained {
            nodes.push(detection_node(
                "key",
                DetectionRuleKind::Keyword {
                    keywords: vec![String::from("hostile")],
                },
            ));
            edges.push(edge("in", "key", 0));
            edges.push(edge("key", "sys", 0));
        } else {
            edges.push(edge("in", "sys", 0));
        }
        let (executor, errors) = Executor::new(RuleGraph { nodes, edges });
        assert!(errors.is_empty(), "{errors:?}");
        executor
            .run(event, &FixedResolver)
            .first()
            .map(|activation| activation.messages[0].clone())
    }

    /// A killmail event whose text says Jita and whose facts say system 99.
    fn jita_in_the_text_99_in_the_facts() -> IntelEvent {
        killmail_event(
            "hostile in Jita",
            Facts {
                systems: Some(vec![system_fact(99, "Nowhere", None)]),
                ..Facts::default()
            },
        )
    }

    #[test]
    fn a_detection_reads_the_facts_or_the_text_as_the_node_says() {
        let event = jita_in_the_text_99_in_the_facts();
        let systems =
            |source| system_report_reading(source, false, &event).map(|message| message.data);
        assert_eq!(
            systems(DetectionSource::Auto),
            Some(Data::Systems(vec![99]))
        );
        assert_eq!(
            systems(DetectionSource::Facts),
            Some(Data::Systems(vec![99]))
        );
        // The text says Jita, whatever the source knows.
        assert_eq!(
            systems(DetectionSource::Text),
            Some(Data::Systems(vec![30000142]))
        );
    }

    #[test]
    fn reading_facts_finds_nothing_in_an_event_without_them_but_auto_reads_the_text() {
        let event = killmail_line("hostile in Jita");
        let read = |source| system_report_reading(source, false, &event);
        assert!(read(DetectionSource::Facts).is_none());
        assert_eq!(
            read(DetectionSource::Auto).map(|message| message.data),
            Some(Data::Systems(vec![30000142]))
        );
    }

    fn hostile() -> DetectionRuleKind {
        DetectionRuleKind::Keyword {
            keywords: vec![String::from("hostile")],
        }
    }

    fn status() -> DetectionRuleKind {
        DetectionRuleKind::Keyword {
            keywords: vec![String::from("status")],
        }
    }

    fn with_mode(mut node: Node, mode: DetectionMode) -> Node {
        if let NodeKind::Detection(detection) = &mut node.kind {
            detection.mode = mode;
        }
        node
    }

    /// The messages that reach the Output `out` of `graph` for `event`, or
    /// `None` when it does not fire.
    fn reaching_out(graph: RuleGraph, event: &IntelEvent) -> Option<Vec<Mensaje>> {
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        executor
            .run(event, &FixedResolver)
            .into_iter()
            .find(|activation| activation.output_id == "out")
            .map(|activation| activation.messages)
    }

    /// `input -> a -> b -> out`.
    fn chain(a: Node, b: Node) -> RuleGraph {
        RuleGraph {
            nodes: vec![zkillboard_input_node("in"), a, b, output_node("out")],
            edges: vec![edge("in", "a", 0), edge("a", "b", 0), edge("b", "out", 0)],
        }
    }

    fn tags(messages: &[Mensaje]) -> Vec<&str> {
        messages
            .iter()
            .map(|message| message.tag.as_str())
            .collect()
    }

    #[test]
    fn chaining_detections_means_and_on_the_same_event() {
        let graph = || {
            chain(
                detection_node("a", hostile()),
                detection_node(
                    "b",
                    DetectionRuleKind::SystemReport {
                        ignore_words: Vec::new(),
                    },
                ),
            )
        };
        // Both find something in the event itself, and both findings arrive.
        let both = reaching_out(graph(), &killmail_line("hostile in Jita")).unwrap();
        assert_eq!(tags(&both), ["keyword", "system_report"]);
        // The second reads the event, not what the first found: the first's
        // word `hostile` names no system, the line does.
        assert!(reaching_out(graph(), &killmail_line("hostile elsewhere")).is_none());
        assert!(reaching_out(graph(), &killmail_line("in Jita")).is_none());
    }

    #[test]
    fn a_chained_detection_reads_the_facts_of_the_event_too() {
        let event = jita_in_the_text_99_in_the_facts();
        let chained = |source| {
            chain(
                detection_node("a", hostile()),
                with_source(
                    detection_node(
                        "b",
                        DetectionRuleKind::SystemReport {
                            ignore_words: Vec::new(),
                        },
                    ),
                    source,
                ),
            )
        };
        let systems = |source| {
            reaching_out(chained(source), &event).and_then(|messages| {
                messages
                    .into_iter()
                    .find(|message| message.tag == "system_report")
                    .map(|message| message.data)
            })
        };
        assert_eq!(
            systems(DetectionSource::Auto),
            Some(Data::Systems(vec![99]))
        );
        assert_eq!(
            systems(DetectionSource::Facts),
            Some(Data::Systems(vec![99]))
        );
        assert_eq!(
            systems(DetectionSource::Text),
            Some(Data::Systems(vec![30000142]))
        );
    }

    #[test]
    fn an_excluding_detection_lets_the_event_through_when_it_finds_nothing() {
        let graph = || {
            chain(
                detection_node("a", hostile()),
                with_mode(detection_node("b", status()), DetectionMode::Exclude),
            )
        };
        // "hostile and not status", with what the first found going on.
        let passed = reaching_out(graph(), &killmail_line("hostile spotted")).unwrap();
        assert_eq!(tags(&passed), ["keyword"]);
        assert!(reaching_out(graph(), &killmail_line("hostile status?")).is_none());
        assert!(reaching_out(graph(), &killmail_line("all quiet")).is_none());
    }

    #[test]
    fn a_union_lets_an_event_through_once_with_everything_found() {
        let graph = || RuleGraph {
            nodes: vec![
                zkillboard_input_node("in"),
                detection_node("a", hostile()),
                detection_node(
                    "b",
                    DetectionRuleKind::SystemReport {
                        ignore_words: Vec::new(),
                    },
                ),
                Node {
                    kind: NodeKind::Aggregator(AggregatorNode { inputs: 2 }),
                    ..output_node("any")
                },
                output_node("out"),
            ],
            edges: vec![
                edge("in", "a", 0),
                edge("in", "b", 0),
                edge("a", "any", 0),
                edge("b", "any", 1),
                edge("any", "out", 0),
            ],
        };
        // Either is enough.
        let only_a = reaching_out(graph(), &killmail_line("hostile elsewhere")).unwrap();
        assert_eq!(tags(&only_a), ["keyword"]);
        let only_b = reaching_out(graph(), &killmail_line("in Jita")).unwrap();
        assert_eq!(tags(&only_b), ["system_report"]);
        let both = reaching_out(graph(), &killmail_line("hostile in Jita")).unwrap();
        assert_eq!(tags(&both), ["keyword", "system_report"]);
        assert!(reaching_out(graph(), &killmail_line("all quiet")).is_none());
    }

    #[test]
    fn a_finding_that_reaches_a_union_by_two_paths_counts_once() {
        let graph = RuleGraph {
            nodes: vec![
                zkillboard_input_node("in"),
                detection_node("a", hostile()),
                detection_node("b1", status()),
                detection_node("b2", status()),
                Node {
                    kind: NodeKind::Aggregator(AggregatorNode { inputs: 2 }),
                    ..output_node("any")
                },
                output_node("out"),
            ],
            edges: vec![
                edge("in", "a", 0),
                edge("a", "b1", 0),
                edge("a", "b2", 0),
                edge("b1", "any", 0),
                edge("b2", "any", 1),
                edge("any", "out", 0),
            ],
        };
        let found = reaching_out(graph, &killmail_line("hostile status")).unwrap();
        // `hostile` came by both paths and `status` is found by both
        // detections: each is there once.
        let texts: Vec<&str> = found.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts, ["hostile", "status"]);
    }

    #[test]
    fn a_disabled_detection_lets_nothing_through() {
        let mut a = detection_node("a", hostile());
        a.enabled = false;
        let graph = chain(a, detection_node("b", status()));
        assert!(reaching_out(graph, &killmail_line("hostile status")).is_none());
    }

    #[test]
    fn the_mode_of_a_detection_is_stored_and_defaults_to_require() {
        let old: DetectionNode =
            toml::from_str("[kind]\ntype = \"system_report\"\n").expect("a node without mode");
        assert_eq!(old.mode, DetectionMode::Require);
        for mode in DetectionMode::ALL {
            let node = DetectionNode {
                kind: DetectionRuleKind::SystemReport {
                    ignore_words: Vec::new(),
                },
                case_insensitive: false,
                source: DetectionSource::Auto,
                mode,
            };
            let text = toml::to_string(&node).unwrap();
            assert!(text.contains(mode.type_name()), "{text}");
            assert_eq!(toml::from_str::<DetectionNode>(&text).unwrap(), node);
        }
    }

    #[test]
    fn the_source_of_a_detection_is_stored_and_defaults_to_auto() {
        let old: DetectionNode =
            toml::from_str("[kind]\ntype = \"system_report\"\n").expect("a node without source");
        assert_eq!(old.source, DetectionSource::Auto);
        for source in DetectionSource::ALL {
            let node = DetectionNode {
                kind: DetectionRuleKind::SystemReport {
                    ignore_words: Vec::new(),
                },
                case_insensitive: false,
                source,
                mode: DetectionMode::Require,
            };
            let text = toml::to_string(&node).unwrap();
            assert!(text.contains(source.type_name()), "{text}");
            assert_eq!(toml::from_str::<DetectionNode>(&text).unwrap(), node);
        }
    }

    #[test]
    fn only_a_type_with_facts_can_be_set_to_read_them() {
        let graph = |kind| RuleGraph {
            nodes: vec![
                input_node("in"),
                with_source(detection_node("det", kind), DetectionSource::Facts),
                output_node("out"),
            ],
            edges: vec![edge("in", "det", 0), edge("det", "out", 0)],
        };
        assert!(
            graph(DetectionRuleKind::SystemReport {
                ignore_words: Vec::new()
            })
            .validate()
            .is_empty()
        );
        let errors = graph(DetectionRuleKind::Keyword {
            keywords: vec![String::from("hostile")],
        })
        .validate();
        assert_eq!(errors, [PatternError::NoFactsToRead(String::from("det"))]);
    }

    #[test]
    fn a_fact_the_source_gave_is_final_even_when_it_is_empty() {
        // The text would report Jita; the source says there are no systems.
        let event = killmail_event(
            "hostile in Jita",
            Facts {
                systems: Some(Vec::new()),
                ..Facts::default()
            },
        );
        assert!(
            message_of(
                DetectionRuleKind::SystemReport {
                    ignore_words: Vec::new()
                },
                &event
            )
            .is_none()
        );
    }

    #[test]
    fn without_the_fact_a_detection_reads_the_text() {
        // The source gave ships but no systems: the systems come from the
        // text, as for a chat line.
        let event = killmail_event(
            "hostile in Jita",
            Facts {
                ships: Some(Vec::new()),
                ..Facts::default()
            },
        );
        let message = message_of(
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
            &event,
        )
        .unwrap();
        assert_eq!(message.data, Data::Systems(vec![30000142]));
    }

    #[test]
    fn ship_names_read_the_ships_the_source_gave_that_its_dictionaries_know() {
        let ships = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let event = killmail_event(
            "no ships written here",
            Facts {
                ships: Some(vec![
                    ShipCount::new("Sabre", 2),
                    ShipCount::new("Zzyzx Hauler", 5),
                    ShipCount::new("Rifter", 1),
                    ShipCount::new("Sabre", 1),
                ]),
                ..Facts::default()
            },
        );
        let message = message_of(ships, &event).unwrap();
        // Only the ships of the dictionary count, and one ship is one entry.
        assert_eq!(
            message.data,
            Data::Ships(vec![
                ShipCount::new("Sabre", 3),
                ShipCount::new("Rifter", 1)
            ])
        );
        assert_eq!(message.text, "Sabre (x3) Rifter");
        assert_eq!(message.category, Some(IntelCategory::Ship));
        assert!(message.spans.is_empty());
    }

    #[test]
    fn ship_names_report_nothing_when_no_ship_of_the_source_is_in_the_dictionaries() {
        let ships = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let event = killmail_event(
            "Rifter",
            Facts {
                ships: Some(vec![ShipCount::new("Zzyzx Hauler", 5)]),
                ..Facts::default()
            },
        );
        assert!(message_of(ships, &event).is_none());
    }

    /// Knows two ship types, like the SDE index of the application.
    struct WithShips;

    impl ShipResolver for WithShips {
        fn resolve_ship(&self, name: &str) -> Option<u32> {
            match fold(name).as_str() {
                "rifter" => Some(587),
                "sabre" => Some(22456),
                _ => None,
            }
        }
    }

    impl SystemResolver for WithShips {
        fn resolve(&self, _name: &str) -> Option<usize> {
            None
        }

        fn ship_resolver(&self) -> Option<&dyn ShipResolver> {
            Some(self)
        }
    }

    fn ships_data(event: &IntelEvent, resolver: &dyn SystemResolver) -> Vec<ShipCount> {
        let (executor, errors) = Executor::new(graph(DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        }));
        assert!(errors.is_empty(), "{errors:?}");
        match executor.run(event, resolver).first() {
            Some(activation) => match &activation.messages[0].data {
                Data::Ships(ships) => ships.clone(),
                other => panic!("{other:?}"),
            },
            None => Vec::new(),
        }
    }

    #[test]
    fn the_ships_of_a_chat_line_get_their_type_id_when_the_resolver_knows_them() {
        let ships = ships_data(&line("Rifter Rifter Sabre"), &WithShips);
        assert_eq!(
            ships,
            [
                ShipCount::new("Rifter", 2).with_type_id(587),
                ShipCount::new("Sabre", 1).with_type_id(22456)
            ]
        );
    }

    #[test]
    fn without_a_ship_resolver_the_ships_of_a_chat_line_have_no_id() {
        let ships = ships_data(&line("Rifter"), &FixedResolver);
        assert_eq!(ships, [ShipCount::new("Rifter", 1)]);
    }

    #[test]
    fn a_ship_without_id_in_the_facts_is_resolved_and_one_with_id_keeps_it() {
        let event = killmail_event(
            "no ships written here",
            Facts {
                ships: Some(vec![
                    ShipCount::new("Rifter", 1),
                    ShipCount::new("Sabre", 2).with_type_id(22456),
                    ShipCount::new("Rifter", 1).with_type_id(587),
                ]),
                ..Facts::default()
            },
        );
        let ships = DetectionRuleKind::ShipNames {
            dictionaries: vec![String::from("ship_report_en")],
        };
        let message = message_with(ships, &event, &WithShips).unwrap();
        assert_eq!(
            message.data,
            Data::Ships(vec![
                ShipCount::new("Rifter", 2).with_type_id(587),
                ShipCount::new("Sabre", 2).with_type_id(22456)
            ])
        );
    }

    #[test]
    fn a_pilot_count_reads_the_pilots_the_source_gave() {
        // A number the pattern could never read from the text.
        let event = killmail_event(
            "no number written here",
            Facts {
                pilots: Some(1500),
                ..Facts::default()
            },
        );
        let message = message_of(DetectionRuleKind::PilotCount, &event).unwrap();
        assert_eq!(message.data, Data::Count(1500));
        assert_eq!(message.text, "1500");
        assert_eq!(message.category, Some(IntelCategory::Count));

        // No pilots is no report, whatever the text says.
        let none = killmail_event(
            "5 pilots",
            Facts {
                pilots: Some(0),
                ..Facts::default()
            },
        );
        assert!(message_of(DetectionRuleKind::PilotCount, &none).is_none());

        // Without the fact the text is read.
        let text = killmail_event("5 pilots", Facts::default());
        let message = message_of(DetectionRuleKind::PilotCount, &text).unwrap();
        assert_eq!(message.data, Data::Count(5));
    }

    #[test]
    fn the_detections_that_only_read_text_ignore_the_facts() {
        let facts = Facts {
            systems: Some(vec![system_fact(77, "Nowhere", None)]),
            ships: Some(vec![ShipCount::new("Rifter", 1)]),
            pilots: Some(9),
        };
        let event = killmail_event("all clear", facts);
        let clear = DetectionRuleKind::ClearReport {
            keywords: vec![String::from("clear")],
        };
        let message = message_of(clear, &event).unwrap();
        assert_eq!(message.data, Data::Words(vec![String::from("clear")]));
        let custom = DetectionRuleKind::Custom {
            pattern: Some(String::from(r"(?P<count>\d+) hostiles")),
            words: Vec::new(),
            category: Some(IntelCategory::Count),
            system_group: None,
        };
        // A custom count reads its own pattern, not the source's pilots.
        assert!(message_of(custom, &event).is_none());
    }

    #[test]
    fn a_detection_after_a_formatter_still_reads_the_event() {
        // `input -> formatter -> pilot count`: the detection reads the
        // formatter's text, not the pilots of the killmail.
        let graph = RuleGraph {
            nodes: vec![
                zkillboard_input_node("in"),
                Node {
                    id: String::from("fmt"),
                    enabled: true,
                    x: 0.0,
                    y: 0.0,
                    kind: NodeKind::Formatter(FormatterNode {
                        template: String::from("no number here"),
                        inputs: 1,
                    }),
                },
                detection_node("det", DetectionRuleKind::PilotCount),
                output_node("out"),
            ],
            edges: vec![
                edge("in", "fmt", 0),
                edge("fmt", "det", 0),
                edge("det", "out", 0),
            ],
        };
        let (executor, errors) = Executor::new(graph);
        assert!(errors.is_empty(), "{errors:?}");
        let event = killmail_event(
            "x",
            Facts {
                pilots: Some(9),
                ..Facts::default()
            },
        );
        assert_eq!(executor.run(&event, &FixedResolver).len(), 1);
    }

    #[test]
    fn a_disabled_zkillboard_input_receives_nothing() {
        let mut input = zkillboard_input_node("kills");
        input.enabled = false;
        let (executor, _) = Executor::new(system_graph(input));
        assert!(
            executor
                .run(&killmail_line("Jita Rifter destroyed"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn a_zkillboard_input_round_trips_through_json() {
        let graph = system_graph(zkillboard_input_node("kills"));
        let json = serde_json::to_string(&graph).unwrap();
        assert!(json.contains("\"kind\":\"zkillboard\""), "{json}");
        let back: RuleGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, back);
    }

    /// `in -> sys (system report) -> not -> log`, for the tri-state tests.
    #[test]
    fn a_formatter_renders_its_template() {
        let mut graph = RuleGraph::default();
        graph.nodes.push(input_node("in"));
        graph.nodes.push(detection_node(
            "sys",
            DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            },
        ));
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
        graph.edges.push(edge("in", "sys", 0));
        graph.edges.push(edge("in", "ship", 0));
        graph.edges.push(edge("sys", "fmt", 0));
        graph.edges.push(edge("ship", "fmt", 1));
        graph.edges.push(edge("fmt", "log", 0));

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
        graph.edges.push(edge("in", "a", 0));
        graph.edges.push(edge("a", "b", 0));
        graph.edges.push(edge("b", "a", 1));
        graph.edges.push(edge("a", "log", 0));

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

    /// `in -> sys, ship -> gate -> log`, the gate being of `kind`.
    #[test]
    fn disabled_nodes_stop_the_flow() {
        for disabled in ["in", "det", "out"] {
            let mut rules = graph(DetectionRuleKind::SystemReport {
                ignore_words: Vec::new(),
            });
            let (executor, _) = Executor::new(rules.clone());
            assert_eq!(executor.run(&line("Jita"), &FixedResolver).len(), 1);

            rules
                .nodes
                .iter_mut()
                .find(|node| node.id == disabled)
                .unwrap()
                .enabled = false;
            let (executor, errors) = Executor::new(rules);
            assert!(errors.is_empty(), "{errors:?}");
            assert!(
                executor.run(&line("Jita"), &FixedResolver).is_empty(),
                "disabling {disabled} should stop the line"
            );
        }
    }

    #[test]
    fn a_clear_report_detection_matches_its_keywords() {
        let kind = || DetectionRuleKind::ClearReport {
            keywords: vec![String::from("clear"), String::from("nv")],
        };
        let message = only_message(kind(), "Jita clear");
        assert!(matches!(message.data, Data::Words(_)), "{:?}", message.data);
        assert_eq!(message.category, Some(IntelCategory::Clear));
        let (executor, _) = Executor::new(graph(kind()));
        assert!(
            executor
                .run(&line("Jita hostile"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn a_keyword_detection_matches_its_words() {
        let kind = || DetectionRuleKind::Keyword {
            keywords: vec![String::from("bubble")],
        };
        let message = only_message(kind(), "gate has a bubble");
        assert!(matches!(message.data, Data::Words(_)), "{:?}", message.data);
        let (executor, _) = Executor::new(graph(kind()));
        assert!(
            executor
                .run(&line("gate is quiet"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn a_pilot_count_detection_reads_both_count_shapes() {
        assert_eq!(
            only_message(DetectionRuleKind::PilotCount, "Jita +5").data,
            Data::Count(5)
        );
        assert_eq!(
            only_message(DetectionRuleKind::PilotCount, "Jita 12 reds").data,
            Data::Count(12)
        );
        let (executor, _) = Executor::new(graph(DetectionRuleKind::PilotCount));
        assert!(executor.run(&line("Jita quiet"), &FixedResolver).is_empty());
    }

    #[test]
    fn a_chinese_ship_detection_needs_the_class_suffix() {
        let message = only_message(
            DetectionRuleKind::ShipNames {
                dictionaries: Vec::new(),
            },
            "破坏级 在 Jita",
        );
        assert!(matches!(message.data, Data::Ships(_)), "{:?}", message.data);
        assert_eq!(message.category, Some(IntelCategory::Ship));
        let (executor, _) = Executor::new(graph(DetectionRuleKind::ShipNames {
            dictionaries: Vec::new(),
        }));
        // Han characters without the suffix are not a ship.
        assert!(
            executor
                .run(&line("破坏者 在 Jita"), &FixedResolver)
                .is_empty()
        );
        assert!(
            executor
                .run(&line("only latin text"), &FixedResolver)
                .is_empty()
        );
    }

    #[test]
    fn a_custom_detection_produces_the_data_of_its_category() {
        let ships = only_message(
            custom(None, &["Proteus"], IntelCategory::Ship),
            "2x Proteus proteus",
        );
        assert_eq!(ships.data, Data::Ships(vec![ShipCount::new("Proteus", 1)]));
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
        let context = IntelEvent {
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

    /// The default graph as it was before the zKillboard rules: the same
    /// without their nodes and cables.
    fn graph_without_zkillboard_rules() -> RuleGraph {
        let mut graph = RuleGraph::default_graph();
        let chain = graph.zkillboard_chain();
        graph.nodes.retain(|node| !chain.contains(&node.id));
        graph
            .edges
            .retain(|edge| !chain.contains(&edge.from) && !chain.contains(&edge.to));
        graph
    }

    #[test]
    fn the_zkillboard_chain_ends_at_the_outputs() {
        let graph = RuleGraph::default_graph();
        let chain = graph.zkillboard_chain();
        assert!(chain.contains("zkillboard"));
        assert!(chain.contains("z_ship_names"));
        // The aggregator that gathers the detections is in it, whatever its id.
        assert!(graph.nodes.iter().any(|node| {
            chain.contains(&node.id) && matches!(node.kind, NodeKind::Aggregator(_))
        }));
        assert!(graph.nodes.iter().all(|node| {
            !matches!(node.kind, NodeKind::Output(_)) || !chain.contains(&node.id)
        }));
        // The chat rules are not part of it.
        assert!(!chain.contains("log_in"));
    }

    #[test]
    fn the_default_graph_reads_zkillboard_and_is_left_alone_by_the_rules_for_it() {
        let graph = RuleGraph::default_graph();
        assert!(graph.nodes.iter().any(|node| {
            node.enabled
                && matches!(&node.kind, NodeKind::Input(input) if input.kind == InputKind::ZKillboard)
        }));
        assert_eq!(graph.clone().with_zkillboard_rules(), graph);
    }

    #[test]
    fn the_zkillboard_rules_are_added_to_a_graph_that_lacks_them() {
        let old = graph_without_zkillboard_rules();
        assert!(old.validate().is_empty());
        let migrated = old.clone().with_zkillboard_rules();
        assert!(migrated.validate().is_empty());
        assert_eq!(migrated, RuleGraph::default_graph());
        // What the user had is as it was: only nodes and cables were added.
        assert_eq!(migrated.nodes[..old.nodes.len()], old.nodes[..]);
        assert_eq!(migrated.edges[..old.edges.len()], old.edges[..]);
    }

    #[test]
    fn the_zkillboard_cables_go_to_the_outputs_the_graph_has() {
        let mut old = graph_without_zkillboard_rules();
        old.nodes.retain(|node| node.id != "visual");
        old.edges.retain(|edge| edge.to != "visual");
        let migrated = old.with_zkillboard_rules();
        assert!(migrated.validate().is_empty());
        assert!(migrated.edges.iter().all(|edge| edge.to != "visual"));
        let chain = RuleGraph::default_graph().zkillboard_chain();
        assert!(
            migrated
                .edges
                .iter()
                .any(|edge| chain.contains(&edge.from) && edge.to == "sound")
        );
    }

    #[test]
    fn the_zkillboard_rules_are_not_added_over_the_users_own() {
        let mut graph = graph_without_zkillboard_rules();
        let mut input = graph
            .nodes
            .iter()
            .find(|node| node.id == "chat_logs")
            .cloned()
            .unwrap();
        input.id = String::from("mine");
        if let NodeKind::Input(node) = &mut input.kind {
            node.kind = InputKind::ZKillboard;
        }
        graph.nodes.push(input);
        assert_eq!(graph.clone().with_zkillboard_rules(), graph);
        // Nor over a node that already has one of their ids.
        let mut graph = graph_without_zkillboard_rules();
        let mut taken = graph.nodes[0].clone();
        taken.id = String::from("z_pilot_count");
        graph.nodes.push(taken);
        assert_eq!(graph.clone().with_zkillboard_rules(), graph);
    }

    #[test]
    fn an_output_takes_the_cables_of_several_rules_and_nothing_else_does() {
        let graph = RuleGraph::default_graph();
        let to_visual = graph
            .edges
            .iter()
            .filter(|edge| edge.to == "visual")
            .count();
        assert_eq!(to_visual, 2);
        assert!(graph.validate().is_empty());
        // A detection still takes a single cable.
        let mut crossed = graph.clone();
        crossed.edges.push(Edge {
            from: String::from("zkillboard"),
            to: String::from("system_reported"),
            to_pin: 0,
        });
        assert!(
            crossed
                .validate()
                .contains(&PatternError::MultipleCables(String::from(
                    "system_reported"
                )))
        );
    }

    #[test]
    fn what_the_cables_of_an_output_bring_is_one_activation() {
        let (executor, errors) = Executor::new(RuleGraph::default_graph());
        assert!(errors.is_empty(), "{errors:?}");
        let mut event = line("Jita");
        event.kind = InputKind::ZKillboard;
        event.facts = Facts {
            systems: Some(vec![SystemFact {
                id: 30000142,
                name: String::from("Jita"),
                span: None,
            }]),
            ships: Some(vec![ShipCount {
                type_id: Some(587),
                name: String::from("Rifter"),
                times: 2,
            }]),
            pilots: Some(3),
        };
        let activations = executor.run(&event, &FixedResolver);
        // The three detections of the killmail rule meet in an aggregator that
        // reaches the log once, with what each found.
        let log: Vec<_> = activations
            .iter()
            .filter(|activation| activation.kind == OutputKind::Log)
            .collect();
        assert_eq!(log.len(), 1);
        let mut tags: Vec<&str> = log[0]
            .messages
            .iter()
            .map(|message| message.tag.as_str())
            .collect();
        tags.sort_unstable();
        assert_eq!(tags, vec!["pilot_count", "ship_names", "system_report"]);
        // The visual alert and the sound are reached once too, by the same
        // aggregator.
        for kind in [OutputKind::Visual, OutputKind::Sound] {
            let reached: Vec<_> = activations
                .iter()
                .filter(|activation| activation.kind == kind)
                .collect();
            assert_eq!(reached.len(), 1, "{kind:?}");
            let mut tags: Vec<&str> = reached[0]
                .messages
                .iter()
                .map(|message| message.tag.as_str())
                .collect();
            tags.sort_unstable();
            assert_eq!(
                tags,
                vec!["pilot_count", "ship_names", "system_report"],
                "{kind:?}"
            );
        }
    }

    #[test]
    fn the_default_zkillboard_rules_read_the_facts_of_the_killmails() {
        let graph = RuleGraph::default_graph();
        let input = graph
            .nodes
            .iter()
            .find(|node| node.id == "zkillboard")
            .unwrap();
        let NodeKind::Input(input) = &input.kind else {
            panic!("not an input")
        };
        assert_eq!(input.kind, InputKind::ZKillboard);
        for id in ["z_system_reported", "z_pilot_count", "z_ship_names"] {
            let node = graph.nodes.iter().find(|node| node.id == id).unwrap();
            let NodeKind::Detection(detection) = &node.kind else {
                panic!("not a detection")
            };
            assert_eq!(detection.source, DetectionSource::Facts, "{id}");
            assert!(detection.case_insensitive, "{id}");
        }
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
