//! Settings -> Patterns: the intel rule graph as a list of input nodes.
//!
//! Each Input node is a card; opening one shows a floating editor with the
//! **connected component** of that input. Nodes are shared instances of the
//! underlying [`RuleGraph`]: opening another input shows the same nodes.
//!
//! Wires: `input.out -> detection.in` and `detection.t|f -> output.in`. The
//! edges are rebuilt from the graph's wires when the editor closes.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use eframe::egui::{self, Color32, Pos2, RichText, Ui};
use egui_snarl::{
    InPin, InPinId, NodeId, OutPin, OutPinId, Snarl,
    ui::{PinInfo, SnarlPin, SnarlViewer, SnarlWidget},
};
use std::collections::{HashMap, HashSet};
use webb::graph::{
    AggregatorNode, DetectionNode, Edge, FormatterNode, GateKind, GateNode, InputNode, LogConfig,
    Node, NodeKind, OutputNode, Pin, RuleGraph, TooltipConfig, input_pin_count, output_pin_count,
};
use webb::rules::{
    DetectionRuleKind, DetectionType, Dictionaries, InputKind, OutputKind, OutputType,
};

/// File the rules are imported from / exported to (next to the app).
const RULES_FILE: &str = "rules.toml";

/// The detection types, by `type_name()`.
const DETECTION_TYPES: [&str; 8] = [
    "system_report",
    "clear_report",
    "ship_names",
    "ship_names_zh",
    "pilot_count",
    "keyword",
    "query",
    "custom",
];

/// The output kinds, in the order they are offered.
const OUTPUT_KINDS: [OutputKind; 5] = [
    OutputKind::Visual,
    OutputKind::Sound,
    OutputKind::Log,
    OutputKind::Suppress,
    OutputKind::Tooltip,
];

#[derive(Default)]
struct GraphState {
    dirty: bool,
    selected: Option<NodeId>,
}

/// In-memory editing state of the Rules page: the working copy of the graph
/// (applied only on Accept) plus the currently open floating editor.
#[derive(Default)]
pub(crate) struct PatternsEditor {
    graph: RuleGraph,
    loaded: bool,
    /// Input id whose component is open in the editor, if any.
    open_input: Option<String>,
    /// Input id selected in the cards list.
    selected_input: Option<String>,
    snarl: Snarl<Node>,
    /// Node ids present when the editor opened, to detect deletions on merge.
    original_ids: HashSet<String>,
    state: GraphState,
    errors: Vec<String>,
}

impl PatternsEditor {
    pub(crate) fn ensure_loaded(&mut self, graph: &RuleGraph) {
        if self.loaded {
            return;
        }
        self.graph = graph.clone();
        self.loaded = true;
    }

    pub(crate) fn reset(&mut self, graph: &RuleGraph) {
        self.graph = graph.clone();
        self.loaded = true;
        self.open_input = None;
        self.selected_input = None;
        self.snarl = Snarl::new();
        self.original_ids.clear();
        self.state = GraphState::default();
        self.errors.clear();
    }

    pub(crate) fn to_graph(&self) -> RuleGraph {
        self.graph.clone()
    }

    pub(crate) fn set_errors(&mut self, errors: Vec<String>) {
        self.errors = errors;
    }

    pub(crate) fn clear_errors(&mut self) {
        self.errors.clear();
    }

    fn input_ids(&self) -> Vec<String> {
        self.graph
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::Input(_) => Some(node.id.clone()),
                _ => None,
            })
            .collect()
    }

    fn input_summary(&self, id: &str) -> Option<(String, bool)> {
        self.graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| match &node.kind {
                NodeKind::Input(input) => Some((input.description.clone(), node.enabled)),
                _ => None,
            })
    }

    fn rule_ids(&self) -> HashSet<String> {
        self.graph.nodes.iter().map(|node| node.id.clone()).collect()
    }

    fn add_input(&mut self) -> String {
        let id = unique_id(&self.rule_ids(), "input");
        self.graph.nodes.push(Node {
            id: id.clone(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Input(InputNode {
                description: String::from("input"),
                kind: InputKind::ChatLog,
                path: String::new(),
                channels: Vec::new(),
                exclude_motd: true,
            }),
        });
        self.state.dirty = true;
        id
    }

    fn remove_input(&mut self, id: &str) {
        self.graph.nodes.retain(|node| node.id != id);
        self.graph
            .edges
            .retain(|edge| edge.from != id && edge.to != id);
        if self.open_input.as_deref() == Some(id) {
            self.open_input = None;
        }
        if self.selected_input.as_deref() == Some(id) {
            self.selected_input = None;
        }
        self.state.dirty = true;
    }

    /// Renames the input `old` to `new`, updating every edge that referenced
    /// it. Returns whether the new id was applied (valid and unused).
    fn rename_input(&mut self, old: &str, new: &str) -> bool {
        if new == old {
            return true;
        }
        if !valid_id(new) || self.rules_has_id(new) {
            return false;
        }
        if let Some(node) = self.graph.nodes.iter_mut().find(|node| node.id == old) {
            node.id = new.to_string();
        }
        for edge in &mut self.graph.edges {
            if edge.from == old {
                edge.from = new.to_string();
            }
            if edge.to == old {
                edge.to = new.to_string();
            }
        }
        if self.selected_input.as_deref() == Some(old) {
            self.selected_input = Some(new.to_string());
        }
        self.state.dirty = true;
        true
    }

    fn set_input_description(&mut self, id: &str, description: String) {
        if let Some(Node {
            kind: NodeKind::Input(input),
            ..
        }) = self.graph.nodes.iter_mut().find(|node| node.id == id)
        {
            input.description = description;
            self.state.dirty = true;
        }
    }

    fn set_input_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(node) = self.graph.nodes.iter_mut().find(|node| node.id == id) {
            node.enabled = enabled;
            self.state.dirty = true;
        }
    }

    fn open_editor(&mut self, input_id: &str) {
        self.build_component(input_id);
        self.open_input = Some(input_id.to_string());
    }

    fn close_editor(&mut self) {
        if self.open_input.is_some() {
            match self.merge_component() {
                Ok(()) => {
                    self.open_input = None;
                    self.errors.clear();
                }
                // Keep the editor open so the user can fix the id.
                Err(error) => self.errors = vec![error],
            }
        }
    }

    fn add_detection_of(&mut self, type_name: &str) {
        let id = unique_id(&self.rule_ids(), "detection");
        let node = Node {
            id,
            enabled: true,
            x: 360.0,
            y: self.detection_count() as f32 * 260.0,
            kind: NodeKind::Detection(DetectionNode {
                kind: default_detection_kind(type_name),
                case_insensitive: false,
            }),
        };
        let node_id = self
            .snarl
            .insert_node(Pos2::new(node.x, node.y), node);
        if let Some(input_id) = self.open_input.clone()
            && let Some(input_node) = self.find_node(&input_id)
        {
            self.snarl.connect(
                OutPinId {
                    node: input_node,
                    output: 0,
                },
                InPinId {
                    node: node_id,
                    input: 0,
                },
            );
        }
        self.state.dirty = true;
    }

    fn add_output_of(&mut self, kind: OutputKind) {
        if self
            .snarl
            .nodes()
            .any(|node| matches!(&node.kind, NodeKind::Output(output) if output.kind == kind))
        {
            return;
        }
        let node = Node {
            id: kind.type_name().to_string(),
            enabled: true,
            x: 760.0,
            y: self.output_count() as f32 * 260.0,
            kind: NodeKind::Output(OutputNode {
                kind,
                tooltip: TooltipConfig { emojis: true },
                log: LogConfig::default(),
            }),
        };
        self.snarl.insert_node(Pos2::new(node.x, node.y), node);
        self.state.dirty = true;
    }

    /// The output kinds not yet present in the open component.
    fn missing_output_kinds(&self) -> Vec<OutputKind> {
        let present: Vec<OutputKind> = self
            .snarl
            .nodes()
            .filter_map(|node| match &node.kind {
                NodeKind::Output(output) => Some(output.kind),
                _ => None,
            })
            .collect();
        OUTPUT_KINDS
            .into_iter()
            .filter(|kind| !present.contains(kind))
            .collect()
    }

    fn add_aggregator(&mut self) {
        let id = unique_id(&self.rule_ids(), "aggregator");
        let node = Node {
            id,
            enabled: true,
            x: 540.0,
            y: self.aggregator_count() as f32 * 260.0,
            kind: NodeKind::Aggregator(AggregatorNode { inputs: 4 }),
        };
        self.snarl.insert_node(Pos2::new(node.x, node.y), node);
        self.state.dirty = true;
    }

    fn aggregator_count(&self) -> usize {
        self.snarl
            .nodes()
            .filter(|node| matches!(&node.kind, NodeKind::Aggregator(_)))
            .count()
    }

    fn add_gate(&mut self, kind: GateKind) {
        let id = unique_id(&self.rule_ids(), kind.type_name());
        let node = Node {
            id,
            enabled: true,
            x: 540.0,
            y: self.special_count() as f32 * 260.0,
            kind: NodeKind::Gate(GateNode { kind, inputs: 2 }),
        };
        self.snarl.insert_node(Pos2::new(node.x, node.y), node);
        self.state.dirty = true;
    }

    fn add_formatter(&mut self) {
        let id = unique_id(&self.rule_ids(), "formatter");
        let node = Node {
            id,
            enabled: true,
            x: 540.0,
            y: self.special_count() as f32 * 260.0,
            kind: NodeKind::Formatter(FormatterNode {
                template: String::from("{all}"),
                inputs: 2,
            }),
        };
        self.snarl.insert_node(Pos2::new(node.x, node.y), node);
        self.state.dirty = true;
    }

    fn special_count(&self) -> usize {
        self.snarl
            .nodes()
            .filter(|node| {
                matches!(
                    &node.kind,
                    NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_)
                )
            })
            .count()
    }

    fn detection_count(&self) -> usize {
        self.snarl
            .nodes()
            .filter(|node| matches!(&node.kind, NodeKind::Detection(_)))
            .count()
    }

    fn output_count(&self) -> usize {
        self.snarl
            .nodes()
            .filter(|node| matches!(&node.kind, NodeKind::Output(_)))
            .count()
    }

    fn find_node(&self, id: &str) -> Option<NodeId> {
        self.snarl
            .node_ids()
            .find(|(_, node)| node.id == id)
            .map(|(node_id, _)| node_id)
    }

    /// Builds the component of `input_id`: everything reachable forward from
    /// the input (through detections and the special nodes) plus each
    /// detection's other sources. Outputs are shared, so it never traverses
    /// through them.
    fn build_component(&mut self, input_id: &str) {
        self.snarl = Snarl::new();
        self.original_ids.clear();

        let mut component: HashSet<String> = HashSet::new();
        component.insert(input_id.to_string());
        let mut queue: Vec<String> = vec![input_id.to_string()];
        while let Some(id) = queue.pop() {
            let Some(node) = self.graph.nodes.iter().find(|node| node.id == id) else {
                continue;
            };
            for edge in &self.graph.edges {
                if edge.from == id && component.insert(edge.to.clone()) {
                    queue.push(edge.to.clone());
                }
            }
            if matches!(&node.kind, NodeKind::Detection(_)) {
                for edge in &self.graph.edges {
                    if edge.to == id
                        && edge.from_pin == Pin::Out
                        && component.insert(edge.from.clone())
                    {
                        queue.push(edge.from.clone());
                    }
                }
            }
        }

        let mut node_of: HashMap<String, NodeId> = HashMap::new();
        let mut next_y: HashMap<u8, f32> = HashMap::new();
        for node in &self.graph.nodes {
            if !component.contains(&node.id) {
                continue;
            }
            let column = match node.kind {
                NodeKind::Input(_) => 0u8,
                NodeKind::Detection(_) => 1,
                NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => 2,
                NodeKind::Output(_) => 3,
            };
            let pos = if node.x != 0.0 || node.y != 0.0 {
                Pos2::new(node.x, node.y)
            } else {
                let x = match column {
                    0 => 0.0,
                    1 => 360.0,
                    2 => 620.0,
                    _ => 900.0,
                };
                let y = next_y.entry(column).or_insert(0.0);
                let slot = Pos2::new(x, *y);
                *y += 280.0;
                slot
            };
            node_of.insert(node.id.clone(), self.snarl.insert_node(pos, node.clone()));
        }

        for edge in &self.graph.edges {
            let (Some(&from), Some(&to)) = (node_of.get(&edge.from), node_of.get(&edge.to)) else {
                continue;
            };
            let output = match edge.from_pin {
                Pin::Out => 0,
                Pin::T => 0,
                Pin::F => 1,
            };
            self.snarl.connect(
                OutPinId {
                    node: from,
                    output,
                },
                InPinId {
                    node: to,
                    input: edge.to_pin as usize,
                },
            );
        }

        self.original_ids = component;
    }

    /// Writes the editor's component back into the working graph.
    fn merge_component(&mut self) -> Result<(), String> {
        let mut present: HashSet<String> = HashSet::new();
        let mut nodes: Vec<Node> = Vec::new();
        for (node_id, node) in self.snarl.node_ids() {
            if !present.insert(node.id.clone()) {
                return Err(format!("duplicate node id '{}'", node.id));
            }
            let mut node = node.clone();
            if let Some(info) = self.snarl.get_node_info(node_id) {
                node.x = info.pos.x;
                node.y = info.pos.y;
            }
            nodes.push(node);
        }
        for id in &present {
            if !self.original_ids.contains(id) && self.rules_has_id(id) {
                // Outputs are shared (their id is their kind), so reusing one
                // that lives in another component is intended.
                let existing_is_output = self
                    .graph
                    .nodes
                    .iter()
                    .find(|node| &node.id == id)
                    .is_some_and(|node| matches!(node.kind, NodeKind::Output(_)));
                if !existing_is_output {
                    return Err(format!("node id '{id}' is already in use"));
                }
            }
        }

        // Rebuild the component's edges from its wires.
        let mut edges = Vec::new();
        for (from, to) in self.snarl.wires() {
            let from_node = &self.snarl[from.node];
            let from_pin = match &from_node.kind {
                NodeKind::Input(_) => Pin::Out,
                NodeKind::Detection(_) => {
                    if from.output == 0 {
                        Pin::T
                    } else {
                        Pin::F
                    }
                }
                NodeKind::Output(_) => continue,
                NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => Pin::Out,
            };
            edges.push(Edge {
                from: from_node.id.clone(),
                from_pin,
                to: self.snarl[to.node].id.clone(),
                to_pin: to.input as u8,
            });
        }

        // Drop the component's nodes and its internal edges (edges to nodes
        // outside the component are kept).
        self.graph
            .nodes
            .retain(|node| !self.original_ids.contains(&node.id) || present.contains(&node.id));
        self.graph.edges.retain(|edge| {
            !(self.original_ids.contains(&edge.from) && self.original_ids.contains(&edge.to))
        });

        for node in nodes {
            match self
                .graph
                .nodes
                .iter_mut()
                .find(|existing| existing.id == node.id)
            {
                Some(existing) => *existing = node,
                None => self.graph.nodes.push(node),
            }
        }
        self.graph.edges.extend(edges);
        self.graph
            .edges
            .sort_by(|a, b| (&a.from, a.from_pin as u8, &a.to).cmp(&(&b.from, b.from_pin as u8, &b.to)));
        self.graph.edges.dedup();

        self.state.dirty = true;
        Ok(())
    }

    /// Whether any node of the working graph uses `id`.
    fn rules_has_id(&self, id: &str) -> bool {
        self.graph.nodes.iter().any(|node| node.id == id)
    }
}

impl TelescopeApp {
    /// Draws the Rules page: the list of input cards and, if one is open, the
    /// floating editor of its component.
    pub(crate) fn show_patterns_page(&mut self, ui: &mut egui::Ui) {
        let graph = self.intel_graph.clone();
        self.patterns_editor.ensure_loaded(&graph);

        if self.patterns_editor.open_input.is_some() {
            self.show_editor(ui);
            return;
        }

        ui.label(RichText::new(t!("settings.patterns.heading")).strong());
        ui.label(t!("settings.patterns.graph_hint"));
        self.patterns_toolbar(ui);
        for error in &self.patterns_editor.errors {
            ui.colored_label(Color32::RED, error);
        }
        ui.separator();

        let input_ids = self.patterns_editor.input_ids();
        if input_ids.is_empty() {
            ui.label(t!("settings.patterns.no_inputs"));
        }
        let mut select = None;
        let mut toggle = None;
        let mut rename = None;
        let mut set_description = None;
        for id in &input_ids {
            let Some((description, enabled)) = self.patterns_editor.input_summary(id) else {
                continue;
            };
            let selected = self.patterns_editor.selected_input.as_deref() == Some(id.as_str());
            let card = input_card(ui, id, &description, enabled, selected);
            if card.clicked {
                select = Some(id.clone());
            }
            if card.toggled {
                toggle = Some((id.clone(), !enabled));
            }
            if let Some(new_id) = card.renamed {
                rename = Some((id.clone(), new_id));
            }
            if let Some(description) = card.description {
                set_description = Some((id.clone(), description));
            }
            ui.add_space(4.0);
        }
        if let Some((old, new)) = rename {
            self.patterns_editor.rename_input(&old, &new);
        }
        if let Some((id, description)) = set_description {
            self.patterns_editor.set_input_description(&id, description);
        }
        if let Some((id, enabled)) = toggle {
            self.patterns_editor.set_input_enabled(&id, enabled);
        }
        if let Some(id) = select {
            self.patterns_editor.selected_input = Some(id);
        }
    }

    /// The Rules page toolbar: add a rule, open or remove the selected one, and
    /// import / export the whole graph.
    fn patterns_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(t!("settings.patterns.add_input")).clicked() {
                let id = self.patterns_editor.add_input();
                self.patterns_editor.selected_input = Some(id);
            }
            let selected = self.patterns_editor.selected_input.clone();
            let open = ui.add_enabled(
                selected.is_some(),
                egui::Button::new(t!("settings.patterns.open")),
            );
            if open.clicked()
                && let Some(id) = &selected
            {
                self.patterns_editor.open_editor(id);
            }
            let remove = ui.add_enabled(
                selected.is_some(),
                egui::Button::new(t!("settings.patterns.remove_input")),
            );
            if remove.clicked()
                && let Some(id) = &selected
            {
                self.patterns_editor.remove_input(id);
            }
            ui.separator();
            if ui.button(t!("settings.patterns.import")).clicked() {
                self.import_rules();
            }
            if ui.button(t!("settings.patterns.export")).clicked() {
                self.export_rules();
            }
        });
    }

    /// Loads `rules.toml` (a serialized [`RuleGraph`]) into the editor.
    pub(crate) fn import_rules(&mut self) {
        let text = match std::fs::read_to_string(RULES_FILE) {
            Ok(text) => text,
            Err(error) => {
                self.patterns_editor
                    .set_errors(vec![format!("cannot read {RULES_FILE}: {error}")]);
                return;
            }
        };
        let graph = match toml::from_str::<RuleGraph>(&text) {
            Ok(graph) => graph,
            Err(error) => {
                self.patterns_editor.set_errors(vec![error.to_string()]);
                return;
            }
        };
        let errors = graph.validate();
        if !errors.is_empty() {
            self.patterns_editor
                .set_errors(errors.iter().map(|error| error.to_string()).collect());
            return;
        }
        self.patterns_editor.clear_errors();
        self.patterns_editor.reset(&graph);
        self.patterns_editor.state.dirty = true;
    }

    /// Writes the editor's graph to `rules.toml`.
    pub(crate) fn export_rules(&mut self) {
        let graph = self.patterns_editor.to_graph();
        let errors = graph.validate();
        if !errors.is_empty() {
            self.patterns_editor
                .set_errors(errors.iter().map(|error| error.to_string()).collect());
            return;
        }
        match toml::to_string(&graph) {
            Ok(text) => {
                if let Err(error) = std::fs::write(RULES_FILE, text) {
                    self.patterns_editor
                        .set_errors(vec![format!("cannot write {RULES_FILE}: {error}")]);
                    return;
                }
                self.patterns_editor.clear_errors();
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Info,
                    String::from("Patterns"),
                    String::from("export_rules"),
                    format!("{RULES_FILE} was written"),
                )));
            }
            Err(error) => self.patterns_editor.set_errors(vec![error.to_string()]),
        }
    }

    /// Draws the open component's graph editor in place of the input cards.
    fn show_editor(&mut self, ui: &mut egui::Ui) {
        let Some(input_id) = self.patterns_editor.open_input.clone() else {
            return;
        };
        let mut close = false;
        ui.horizontal(|ui| {
            if ui
                .button("⬅")
                .on_hover_text(t!("settings.patterns.back"))
                .clicked()
            {
                close = true;
            }
            ui.label(
                RichText::new(format!(
                    "{} · {}",
                    t!("settings.patterns.node_input"),
                    input_id
                ))
                .strong(),
            );
            ui.separator();
            ui.menu_button("🔎", |ui| {
                for type_name in DETECTION_TYPES {
                    if ui.button(detection_type_label(type_name)).clicked() {
                        self.patterns_editor.add_detection_of(type_name);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(t!("settings.patterns.add_detection"));
            let missing = self.patterns_editor.missing_output_kinds();
            ui.menu_button("📤", |ui| {
                for kind in missing {
                    if ui.button(output_kind_label(kind)).clicked() {
                        self.patterns_editor.add_output_of(kind);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text(t!("settings.patterns.add_output"));
            ui.menu_button("🧩", |ui| {
                if ui
                    .button(t!("settings.patterns.kind_aggregator"))
                    .clicked()
                {
                    self.patterns_editor.add_aggregator();
                    ui.close();
                }
                ui.separator();
                for kind in [GateKind::And, GateKind::Or, GateKind::Xor, GateKind::Not] {
                    if ui.button(gate_kind_label(kind)).clicked() {
                        self.patterns_editor.add_gate(kind);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(t!("settings.patterns.kind_formatter")).clicked() {
                    self.patterns_editor.add_formatter();
                    ui.close();
                }
            })
            .response
            .on_hover_text(t!("settings.patterns.add_logic"));
        });
        for error in &self.patterns_editor.errors {
            ui.colored_label(Color32::RED, error);
        }
        ui.separator();
        {
            let PatternsEditor { snarl, state, .. } = &mut self.patterns_editor;
            let mut viewer = RulesViewer { state: &mut *state };
            SnarlWidget::new()
                .id(egui::Id::new("rules_graph"))
                .min_size(ui.available_size())
                .show(snarl, &mut viewer, ui);
        }
        if close {
            self.patterns_editor.close_editor();
        }
    }
}

/// What the user did with an input card this frame.
#[derive(Default)]
struct InputCard {
    clicked: bool,
    toggled: bool,
    renamed: Option<String>,
    description: Option<String>,
}

fn input_card(
    ui: &mut egui::Ui,
    id: &str,
    description: &str,
    enabled: bool,
    selected: bool,
) -> InputCard {
    let mut card = InputCard::default();
    let visuals = ui.visuals();
    let (fill, stroke) = if selected {
        (
            visuals.selection.bg_fill.gamma_multiply(0.35),
            visuals.selection.stroke,
        )
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    let frame = egui::Frame::group(ui.style()).fill(fill).stroke(stroke);

    let response = ui
        .push_id(id, |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut description_text = description.to_string();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut description_text)
                                .font(egui::FontId::proportional(16.0))
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        card.description = Some(description_text);
                    }
                    let mut id_text = id.to_string();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut id_text)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                        && id_text != id
                    {
                        card.renamed = Some(id_text);
                    }
                    // Bottom-left: a red/green LED and the state.
                    ui.horizontal(|ui| {
                        if led_widget(ui, enabled).clicked() {
                            card.toggled = true;
                        }
                        ui.label(if enabled {
                            t!("settings.patterns.state_on")
                        } else {
                            t!("settings.patterns.state_off")
                        });
                        ui.label(if enabled { "✅" } else { "⛔" });
                    });
                })
                .response
        })
        .inner;

    if response.interact(egui::Sense::click()).clicked() {
        card.clicked = true;
    }
    card
}

/// A small green (enabled) / red (disabled) LED the user can click to toggle.
fn led_widget(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::click());
    let color = if enabled {
        Color32::from_rgb(60, 200, 60)
    } else {
        Color32::from_rgb(220, 60, 60)
    };
    let radius = rect.width() * 0.5;
    let center = rect.center();
    ui.painter().circle_filled(center, radius, color);
    ui.painter()
        .circle_stroke(center, radius, egui::Stroke::new(1.0, Color32::BLACK));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The snarl viewer of the graph editor.
struct RulesViewer<'a> {
    state: &'a mut GraphState,
}

impl SnarlViewer<Node> for RulesViewer<'_> {
    fn title(&mut self, node: &Node) -> String {
        match &node.kind {
            NodeKind::Input(input) => input.description.clone(),
            NodeKind::Detection(detection) => detection_type_label(detection.kind.type_name()),
            NodeKind::Output(output) => output_kind_label(output.kind),
            NodeKind::Aggregator(_) => t!("settings.patterns.kind_aggregator").into_owned(),
            NodeKind::Gate(gate) => gate_kind_label(gate.kind),
            NodeKind::Formatter(_) => t!("settings.patterns.kind_formatter").into_owned(),
        }
    }

    fn inputs(&mut self, node: &Node) -> usize {
        input_pin_count(&node.kind)
    }

    fn outputs(&mut self, node: &Node) -> usize {
        output_pin_count(&node.kind)
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) -> impl SnarlPin + 'static {
        match &snarl[pin.id.node].kind {
            NodeKind::Input(_) => {}
            NodeKind::Detection(_) => {
                ui.label(t!("settings.patterns.node_input"));
            }
            NodeKind::Output(output) => {
                ui.label(output_kind_label(output.kind));
            }
            NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => {}
        }
        let from_false = pin.remotes.iter().any(|remote| {
            remote.output == 1 && matches!(&snarl[remote.node].kind, NodeKind::Detection(_))
        });
        let info = PinInfo::circle().with_fill(Color32::LIGHT_GREEN);
        if from_false {
            info.with_wire_color(FALSE_WIRE_COLOR)
        } else {
            info
        }
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) -> impl SnarlPin + 'static {
        let false_pin = match &snarl[pin.id.node].kind {
            NodeKind::Input(_) => {
                ui.label(t!("settings.patterns.node_input"));
                false
            }
            NodeKind::Detection(_) => {
                let false_pin = pin.id.output == 1;
                ui.label(if false_pin { "F" } else { "T" });
                false_pin
            }
            NodeKind::Output(_) => false,
            NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => {
                ui.label(t!("settings.patterns.node_output"));
                false
            }
        };
        if false_pin {
            PinInfo::circle()
                .with_fill(FALSE_WIRE_COLOR)
                .with_wire_color(FALSE_WIRE_COLOR)
        } else {
            PinInfo::circle().with_fill(Color32::LIGHT_BLUE)
        }
    }

    fn has_body(&mut self, _node: &Node) -> bool {
        true
    }

    fn show_body(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) {
        self.state.selected = Some(node_id);
        let mut changed = false;
        let mut delete = false;
        {
            let node = &mut snarl[node_id];
            ui.vertical(|ui| {
                changed |= field_row(ui, &t!("settings.patterns.field_enabled"), |ui| {
                    ui.checkbox(&mut node.enabled, "").changed()
                });
                changed |= match &mut node.kind {
                    NodeKind::Input(input) => input_body(ui, input),
                    NodeKind::Detection(detection) => detection_body(ui, detection),
                    NodeKind::Output(output) => output_body(ui, output),
                    NodeKind::Aggregator(aggregator) => aggregator_body(ui, aggregator),
                    NodeKind::Gate(gate) => gate_body(ui, gate),
                    NodeKind::Formatter(formatter) => formatter_body(ui, formatter),
                };
                if ui.button(t!("settings.patterns.delete_node")).clicked() {
                    delete = true;
                }
            });
        }
        if changed {
            self.state.dirty = true;
        }
        if delete {
            snarl.remove_node(node_id);
            self.state.selected = None;
            self.state.dirty = true;
        }
    }

    fn has_graph_menu(&mut self, _pos: Pos2, _snarl: &mut Snarl<Node>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: Pos2, ui: &mut Ui, snarl: &mut Snarl<Node>) {
        ui.menu_button(t!("settings.patterns.add_detection"), |ui| {
            for type_name in DETECTION_TYPES {
                if ui.button(detection_type_label(type_name)).clicked() {
                    let id = unique_id(&collect_node_ids(snarl), "detection");
                    snarl.insert_node(
                        pos,
                        Node {
                            id,
                            enabled: true,
                            x: pos.x,
                            y: pos.y,
                            kind: NodeKind::Detection(DetectionNode {
                                kind: default_detection_kind(type_name),
                                case_insensitive: false,
                            }),
                        },
                    );
                    self.state.dirty = true;
                    ui.close();
                }
            }
        });
        let present: Vec<OutputKind> = snarl
            .nodes()
            .filter_map(|node| match &node.kind {
                NodeKind::Output(output) => Some(output.kind),
                _ => None,
            })
            .collect();
        ui.menu_button(t!("settings.patterns.add_output"), |ui| {
            for kind in OUTPUT_KINDS {
                if present.contains(&kind) {
                    continue;
                }
                if ui.button(output_kind_label(kind)).clicked() {
                    snarl.insert_node(
                        pos,
                        Node {
                            id: kind.type_name().to_string(),
                            enabled: true,
                            x: pos.x,
                            y: pos.y,
                            kind: NodeKind::Output(OutputNode {
                                kind,
                                tooltip: TooltipConfig { emojis: true },
                                log: LogConfig::default(),
                            }),
                        },
                    );
                    self.state.dirty = true;
                    ui.close();
                }
            }
        });
        ui.menu_button(t!("settings.patterns.add_logic"), |ui| {
            if ui
                .button(t!("settings.patterns.kind_aggregator"))
                .clicked()
            {
                let id = unique_id(&collect_node_ids(snarl), "aggregator");
                snarl.insert_node(
                    pos,
                    Node {
                        id,
                        enabled: true,
                        x: pos.x,
                        y: pos.y,
                        kind: NodeKind::Aggregator(AggregatorNode { inputs: 4 }),
                    },
                );
                self.state.dirty = true;
                ui.close();
            }
            ui.separator();
            for kind in [GateKind::And, GateKind::Or, GateKind::Xor, GateKind::Not] {
                if ui.button(gate_kind_label(kind)).clicked() {
                    let id = unique_id(&collect_node_ids(snarl), kind.type_name());
                    snarl.insert_node(
                        pos,
                        Node {
                            id,
                            enabled: true,
                            x: pos.x,
                            y: pos.y,
                            kind: NodeKind::Gate(GateNode { kind, inputs: 2 }),
                        },
                    );
                    self.state.dirty = true;
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(t!("settings.patterns.kind_formatter")).clicked() {
                let id = unique_id(&collect_node_ids(snarl), "formatter");
                snarl.insert_node(
                    pos,
                    Node {
                        id,
                        enabled: true,
                        x: pos.x,
                        y: pos.y,
                        kind: NodeKind::Formatter(FormatterNode {
                            template: String::from("{all}"),
                            inputs: 2,
                        }),
                    },
                );
                self.state.dirty = true;
                ui.close();
            }
        });
    }

    fn has_node_menu(&mut self, _node: &Node) -> bool {
        false
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<Node>) {
        snarl.connect(from.id, to.id);
        self.state.dirty = true;
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<Node>) {
        snarl.disconnect(from.id, to.id);
        self.state.dirty = true;
    }
}

fn collect_node_ids(snarl: &Snarl<Node>) -> HashSet<String> {
    snarl.node_ids().map(|(_, node)| node.id.clone()).collect()
}

fn unique_id(existing: &HashSet<String>, prefix: &str) -> String {
    let mut index = 1;
    loop {
        let id = format!("{prefix}_{index}");
        if !existing.contains(&id) {
            return id;
        }
        index += 1;
    }
}

/// Whether `id` is a valid rule id (`[A-Za-z0-9_-]`, max 64 chars).
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Width of the parameter-name column of a node body, so the rows line up like
/// a table (`name | value`).
const FIELD_NAME_WIDTH: f32 = 110.0;

/// Color of a detection's `F` (false) pin and of the cables leaving it.
const FALSE_WIRE_COLOR: Color32 = Color32::from_rgb(230, 60, 60);

/// One parameter row: a fixed-width name on the left and its value/options on
/// the right. The value is laid out vertically so multi-line values (lists)
/// stack instead of running along the row.
fn field_row(ui: &mut Ui, name: &str, value: impl FnOnce(&mut Ui) -> bool) -> bool {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [FIELD_NAME_WIDTH, ui.spacing().interact_size.y],
            egui::Label::new(name),
        );
        ui.vertical(|ui| value(ui)).inner
    })
    .inner
}

fn input_body(ui: &mut Ui, input: &mut InputNode) -> bool {
    let mut changed = false;
    changed |= field_row(ui, &t!("settings.patterns.field_description"), |ui| {
        ui.text_edit_singleline(&mut input.description).changed()
    });
    changed |= field_row(ui, &t!("settings.patterns.field_path"), |ui| {
        ui.text_edit_singleline(&mut input.path).changed()
    });
    changed |= field_row(ui, &t!("settings.patterns.field_channels"), |ui| {
        string_list_editor(ui, &mut input.channels)
    });
    changed |= field_row(ui, &t!("settings.patterns.field_exclude_motd"), |ui| {
        ui.checkbox(&mut input.exclude_motd, "").changed()
    });
    changed
}

fn detection_body(ui: &mut Ui, detection: &mut DetectionNode) -> bool {
    let mut changed = false;
    changed |= detection_params_editor(ui, &mut detection.kind);
    changed |= field_row(ui, &t!("settings.patterns.field_case_insensitive"), |ui| {
        ui.checkbox(&mut detection.case_insensitive, "").changed()
    });
    changed
}

fn output_body(ui: &mut Ui, output: &mut OutputNode) -> bool {
    let mut changed = false;
    if output.kind == OutputKind::Tooltip {
        changed |= field_row(ui, &t!("settings.patterns.field_emojis"), |ui| {
            ui.checkbox(&mut output.tooltip.emojis, "").changed()
        });
    }
    if output.kind == OutputKind::Log {
        changed |= field_row(ui, &t!("settings.patterns.field_current_time"), |ui| {
            ui.checkbox(&mut output.log.use_current_time, "").changed()
        });
    }
    changed
}

fn aggregator_body(ui: &mut Ui, aggregator: &mut AggregatorNode) -> bool {
    field_row(ui, &t!("settings.patterns.field_inputs"), |ui| {
        let mut inputs = aggregator.inputs as usize;
        if ui
            .add(egui::DragValue::new(&mut inputs).range(1..=16))
            .changed()
        {
            aggregator.inputs = inputs as u8;
            true
        } else {
            false
        }
    })
}

fn gate_kind_label(kind: GateKind) -> String {
    match kind {
        GateKind::And => t!("settings.patterns.gate_and").into_owned(),
        GateKind::Or => t!("settings.patterns.gate_or").into_owned(),
        GateKind::Xor => t!("settings.patterns.gate_xor").into_owned(),
        GateKind::Not => t!("settings.patterns.gate_not").into_owned(),
    }
}

fn gate_body(ui: &mut Ui, gate: &mut GateNode) -> bool {
    let mut changed = false;
    changed |= field_row(ui, &t!("settings.patterns.field_gate"), |ui| {
        let mut changed = false;
        egui::ComboBox::from_id_salt("gate_kind")
            .selected_text(gate_kind_label(gate.kind))
            .show_ui(ui, |ui| {
                for kind in [GateKind::And, GateKind::Or, GateKind::Xor, GateKind::Not] {
                    if ui
                        .selectable_label(gate.kind == kind, gate_kind_label(kind))
                        .clicked()
                    {
                        gate.kind = kind;
                        changed = true;
                    }
                }
            });
        changed
    });
    if gate.kind != GateKind::Not {
        changed |= field_row(ui, &t!("settings.patterns.field_inputs"), |ui| {
            let mut inputs = gate.inputs as usize;
            if ui
                .add(egui::DragValue::new(&mut inputs).range(1..=16))
                .changed()
            {
                gate.inputs = inputs as u8;
                true
            } else {
                false
            }
        });
    }
    changed
}

fn formatter_body(ui: &mut Ui, formatter: &mut FormatterNode) -> bool {
    let mut changed = false;
    changed |= field_row(ui, &t!("settings.patterns.field_template"), |ui| {
        ui.text_edit_singleline(&mut formatter.template).changed()
    });
    changed |= field_row(ui, &t!("settings.patterns.field_inputs"), |ui| {
        let mut inputs = formatter.inputs as usize;
        if ui
            .add(egui::DragValue::new(&mut inputs).range(1..=16))
            .changed()
        {
            formatter.inputs = inputs as u8;
            true
        } else {
            false
        }
    });
    changed
}

fn detection_params_editor(ui: &mut Ui, kind: &mut DetectionRuleKind) -> bool {
    let mut changed = false;
    match kind {
        DetectionRuleKind::SystemReport
        | DetectionRuleKind::PilotCount
        | DetectionRuleKind::ShipNamesZh => {}
        DetectionRuleKind::ClearReport { keywords } | DetectionRuleKind::Query { keywords } => {
            // Telescope defines these words: shown read-only.
            changed |= field_row(ui, &t!("settings.patterns.field_keywords"), |ui| {
                for keyword in keywords.iter() {
                    ui.weak(keyword);
                }
                false
            });
        }
        DetectionRuleKind::Keyword { keywords } => {
            changed |= field_row(ui, &t!("settings.patterns.field_keywords"), |ui| {
                string_list_editor(ui, keywords)
            });
        }
        DetectionRuleKind::ShipNames { dictionaries } => {
            changed |= field_row(ui, &t!("settings.patterns.field_dictionaries"), |ui| {
                for name in dictionaries.iter() {
                    ui.weak(name);
                }
                false
            });
        }
        DetectionRuleKind::Custom {
            pattern,
            words,
            category,
            system_group,
        } => {
            let mut has_pattern = pattern.is_some();
            let toggled = field_row(ui, &t!("settings.patterns.field_custom_pattern"), |ui| {
                ui.checkbox(&mut has_pattern, "").changed()
            });
            if toggled {
                *pattern = has_pattern.then(String::new);
            }
            changed |= toggled;
            if let Some(pattern) = pattern {
                changed |= field_row(ui, &t!("settings.patterns.field_pattern"), |ui| {
                    ui.text_edit_singleline(pattern).changed()
                });
            } else {
                changed |= field_row(ui, &t!("settings.patterns.field_keywords"), |ui| {
                    string_list_editor(ui, words)
                });
            }
            changed |= field_row(ui, &t!("settings.patterns.field_category"), |ui| {
                category_editor(ui, category)
            });
            changed |= field_row(ui, &t!("settings.patterns.field_system_group"), |ui| {
                let mut group = system_group.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut group).changed() {
                    *system_group = (!group.is_empty()).then_some(group);
                    true
                } else {
                    false
                }
            });
        }
    }
    changed
}

fn category_editor(ui: &mut Ui, category: &mut Option<webb::patterns::IntelCategory>) -> bool {
    use webb::patterns::IntelCategory;
    let options = [
        (None, t!("settings.patterns.category_none").into_owned()),
        (
            Some(IntelCategory::Ship),
            t!("settings.patterns.category_ship").into_owned(),
        ),
        (
            Some(IntelCategory::Count),
            t!("settings.patterns.category_count").into_owned(),
        ),
        (
            Some(IntelCategory::Clear),
            t!("settings.patterns.category_clear").into_owned(),
        ),
        (
            Some(IntelCategory::Keyword),
            t!("settings.patterns.category_keyword").into_owned(),
        ),
        (
            Some(IntelCategory::Query),
            t!("settings.patterns.category_query").into_owned(),
        ),
    ];
    let selected = options
        .iter()
        .find(|(value, _)| value == category)
        .map(|(_, label)| label.clone())
        .unwrap_or_default();
    let mut changed = false;
    egui::ComboBox::from_id_salt("detection_category")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (value, label) in options {
                if ui.selectable_label(*category == value, label).clicked() {
                    *category = value;
                    changed = true;
                }
            }
        });
    changed
}

/// Edits a list of strings with one item per row: a remove button, the value
/// and an "add" button at the end.
fn string_list_editor(ui: &mut Ui, values: &mut Vec<String>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, value) in values.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.button(t!("settings.patterns.remove")).clicked() {
                remove = Some(index);
            }
            changed |= ui.text_edit_singleline(value).changed();
        });
    }
    if let Some(index) = remove {
        values.remove(index);
        changed = true;
    }
    if ui.button(t!("settings.patterns.add")).clicked() {
        values.push(String::new());
        changed = true;
    }
    changed
}

fn default_detection_kind(name: &str) -> DetectionRuleKind {
    match name {
        "system_report" => DetectionRuleKind::SystemReport,
        "clear_report" => DetectionRuleKind::ClearReport {
            keywords: vec![String::from("clear"), String::from("clr")],
        },
        "ship_names" => DetectionRuleKind::ShipNames {
            dictionaries: Dictionaries::defaults().names(),
        },
        "ship_names_zh" => DetectionRuleKind::ShipNamesZh,
        "pilot_count" => DetectionRuleKind::PilotCount,
        "keyword" => DetectionRuleKind::Keyword {
            keywords: Vec::new(),
        },
        "query" => DetectionRuleKind::Query {
            keywords: vec![String::from("status")],
        },
        _ => DetectionRuleKind::Custom {
            pattern: Some(String::new()),
            words: Vec::new(),
            category: None,
            system_group: None,
        },
    }
}

fn detection_type_label(name: &str) -> String {
    match name {
        "system_report" => t!("settings.patterns.type_system_report").into_owned(),
        "clear_report" => t!("settings.patterns.type_clear_report").into_owned(),
        "ship_names" => t!("settings.patterns.type_ship_names").into_owned(),
        "ship_names_zh" => t!("settings.patterns.type_ship_names_zh").into_owned(),
        "pilot_count" => t!("settings.patterns.type_pilot_count").into_owned(),
        "keyword" => t!("settings.patterns.type_keyword").into_owned(),
        "query" => t!("settings.patterns.type_query").into_owned(),
        _ => t!("settings.patterns.type_custom").into_owned(),
    }
}

fn output_kind_label(kind: OutputKind) -> String {
    match kind {
        OutputKind::Visual => t!("settings.patterns.kind_visual").into_owned(),
        OutputKind::Sound => t!("settings.patterns.kind_sound").into_owned(),
        OutputKind::Log => t!("settings.patterns.kind_log").into_owned(),
        OutputKind::Suppress => t!("settings.patterns.kind_suppress").into_owned(),
        OutputKind::Tooltip => t!("settings.patterns.kind_tooltip").into_owned(),
    }
}
