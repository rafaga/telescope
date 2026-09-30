//! Settings -> Rules: the intel rule graph as a list of input nodes.
//!
//! Each Input node is a card (its switch, description, id, what the rule is
//! made of and the outputs it reaches); opening one shows, in place of the
//! list, the node editor of the **connected component** of that input. Nodes are shared instances of the
//! underlying [`RuleGraph`]: opening another input shows the same nodes.
//!
//! Wires: `input.out -> detection.in` and `detection.t|f -> output.in`. The
//! edges are rebuilt from the graph's wires when the editor closes.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, SettingsPage, Type};
use eframe::egui::{self, Color32, Pos2, RichText, Ui};
use egui_panels::{StatusKind, Variant};
use egui_snarl::{
    InPin, InPinId, NodeId, OutPin, OutPinId, Snarl,
    ui::{BackgroundPattern, Grid, PinInfo, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget},
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
    /// The node the side panel shows: the one last clicked.
    selected: Option<NodeId>,
    /// The side panel is folded away.
    inspector_collapsed: bool,
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
        self.graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect()
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
        self.state.selected = None;
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
        let node_id = self.snarl.insert_node(Pos2::new(node.x, node.y), node);
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

    /// Adds a copy of the node `node_id` (with a new id) next to it, not
    /// connected to anything.
    fn duplicate_node(&mut self, node_id: NodeId) {
        let Some(info) = self.snarl.get_node_info(node_id) else {
            return;
        };
        let (mut node, pos) = (info.value.clone(), info.pos);
        node.id = unique_id(&self.rule_ids(), &format!("{}_copy", node.id));
        let copy = self.snarl.insert_node(pos + egui::vec2(28.0, 28.0), node);
        self.state.selected = Some(copy);
        self.state.dirty = true;
    }

    fn find_node(&self, id: &str) -> Option<NodeId> {
        self.snarl
            .node_ids()
            .find(|(_, node)| node.id == id)
            .map(|(node_id, _)| node_id)
    }

    /// The ids of the component of `input_id` (see [`Self::build_component`]).
    fn component_ids(&self, input_id: &str) -> HashSet<String> {
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
        component
    }

    /// Builds the component of `input_id`: everything reachable forward from
    /// the input (through detections and the special nodes) plus each
    /// detection's other sources. Outputs are shared, so it never traverses
    /// through them.
    fn build_component(&mut self, input_id: &str) {
        self.snarl = Snarl::new();
        self.original_ids.clear();
        let component = self.component_ids(input_id);

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
                OutPinId { node: from, output },
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
        self.graph.edges.sort_by(|a, b| {
            (&a.from, a.from_pin as u8, &a.to).cmp(&(&b.from, b.from_pin as u8, &b.to))
        });
        self.graph.edges.dedup();

        self.state.dirty = true;
        Ok(())
    }

    /// Whether any node of the working graph uses `id`.
    fn rules_has_id(&self, id: &str) -> bool {
        self.graph.nodes.iter().any(|node| node.id == id)
    }

    /// Whether the node editor of a rule is open.
    pub(crate) fn is_open(&self) -> bool {
        self.open_input.is_some()
    }

    /// Whether the rules differ from `live` (the graph running), counting
    /// the edits in the open editor.
    pub(crate) fn is_modified(&self, live: &RuleGraph) -> bool {
        self.loaded && (self.graph != *live || (self.is_open() && self.state.dirty))
    }

    /// Writes the open editor back into the working graph, keeping it open,
    /// so the graph can be applied.
    pub(crate) fn commit_open_editor(&mut self) -> Result<(), String> {
        if let Some(input_id) = self.open_input.clone() {
            self.merge_component()?;
            self.build_component(&input_id);
        }
        Ok(())
    }

    /// The working graph was just applied: nothing is pending.
    pub(crate) fn mark_applied(&mut self) {
        self.state.dirty = false;
    }

    /// What the rule of `input_id` is made of, for its card.
    fn component_summary(&self, input_id: &str) -> ComponentSummary {
        let component = self.component_ids(input_id);
        let mut summary = ComponentSummary::default();
        for node in &self.graph.nodes {
            if node.id == input_id {
                if let NodeKind::Input(input) = &node.kind {
                    summary.channels.clone_from(&input.channels);
                }
                continue;
            }
            if !component.contains(&node.id) {
                continue;
            }
            match &node.kind {
                NodeKind::Input(_) => {}
                NodeKind::Detection(_) => summary.detections += 1,
                NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => {
                    summary.logic += 1;
                }
                NodeKind::Output(output) => {
                    if !summary.outputs.contains(&output.kind) {
                        summary.outputs.push(output.kind);
                    }
                }
            }
        }
        summary
            .outputs
            .sort_by_key(|kind| OUTPUT_KINDS.iter().position(|known| known == kind));
        summary
    }
}

/// What a rule (the component of an input) is made of.
#[derive(Default)]
struct ComponentSummary {
    /// The channels the input reads; empty means every watched channel.
    channels: Vec<String>,
    detections: usize,
    /// Aggregators, gates and formatters.
    logic: usize,
    /// The output kinds reached, in [`OUTPUT_KINDS`] order.
    outputs: Vec<OutputKind>,
}

impl TelescopeApp {
    /// The Rules page: one card per input rule, or the node editor of the
    /// rule opened.
    pub(crate) fn show_patterns_page(&mut self, ui: &mut egui::Ui) {
        let graph = self.intel_graph.clone();
        self.patterns_editor.ensure_loaded(&graph);

        if self.patterns_editor.open_input.is_some() {
            self.show_editor(ui);
            return;
        }

        egui_panels::page(ui, |ui| {
            self.intel_flow_stepper(ui, SettingsPage::Rules);
            egui_panels::page_header(
                ui,
                &SettingsPage::Rules.title(),
                Some(&t!("settings.patterns.description")),
            );
            let has_inputs = !self.patterns_editor.input_ids().is_empty();
            let (mut add, mut open, mut import, mut export) = (false, false, false, false);
            // With little room the buttons share wrapped lines instead of the
            // right-hand group pushing the left-hand one out of view.
            let narrow = ui.available_width() < 560.0;
            let layout = if narrow {
                egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            ui.with_layout(layout, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                add =
                    egui_panels::button(ui, t!("settings.patterns.add_input"), Variant::Secondary)
                        .clicked();
                ui.add_enabled_ui(has_inputs, |ui| {
                    open = egui_panels::button(
                        ui,
                        t!("settings.patterns.open_editor"),
                        Variant::Primary,
                    )
                    .clicked();
                });
                let mut file_buttons = |ui: &mut egui::Ui| {
                    let (first, second) = if narrow { (1, 0) } else { (0, 1) };
                    for which in [first, second] {
                        if which == 0 {
                            export = egui_panels::button(
                                ui,
                                t!("settings.patterns.export"),
                                Variant::Ghost,
                            )
                            .clicked();
                        } else {
                            import = egui_panels::button(
                                ui,
                                t!("settings.patterns.import"),
                                Variant::Ghost,
                            )
                            .clicked();
                        }
                    }
                };
                if narrow {
                    file_buttons(ui);
                } else {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        file_buttons,
                    );
                }
            });
            if add {
                let id = self.patterns_editor.add_input();
                self.patterns_editor.selected_input = Some(id);
            }
            if open {
                let first = self.patterns_editor.input_ids().into_iter().next();
                let target = self.patterns_editor.selected_input.clone().or(first);
                if let Some(id) = target {
                    self.patterns_editor.open_editor(&id);
                }
            }
            if import {
                self.import_rules();
            }
            if export {
                self.export_rules();
            }

            let input_ids = self.patterns_editor.input_ids();
            if input_ids.is_empty() {
                egui_panels::status(ui, StatusKind::Info, &t!("settings.patterns.no_inputs"));
            }
            let mut actions = Vec::new();
            for (index, id) in input_ids.iter().enumerate() {
                let Some((description, enabled)) = self.patterns_editor.input_summary(id) else {
                    continue;
                };
                let summary = self.patterns_editor.component_summary(id);
                let selected = self.patterns_editor.selected_input.as_deref() == Some(id.as_str());
                let card = input_card(ui, index, id, &description, enabled, selected, &summary);
                actions.extend(card.into_iter().map(|action| (id.clone(), action)));
            }
            for (id, action) in actions {
                match action {
                    CardAction::Select => self.patterns_editor.selected_input = Some(id),
                    CardAction::Open => self.patterns_editor.open_editor(&id),
                    CardAction::Remove => self.patterns_editor.remove_input(&id),
                    CardAction::Enable(enabled) => {
                        self.patterns_editor.set_input_enabled(&id, enabled);
                    }
                    CardAction::Describe(description) => {
                        self.patterns_editor.set_input_description(&id, description);
                    }
                    CardAction::Rename(new_id) => {
                        if !self.patterns_editor.rename_input(&id, &new_id) {
                            self.patterns_editor.set_errors(vec![
                                t!("settings.patterns.invalid_id", id = new_id).into_owned(),
                            ]);
                        }
                    }
                }
            }
            self.show_graph_status(ui);
        });
    }

    /// Whether the rule graph being edited is valid, and its errors.
    fn show_graph_status(&mut self, ui: &mut egui::Ui) {
        for error in &self.patterns_editor.errors {
            egui_panels::notice(ui, StatusKind::Error, error);
        }
        let graph = self.patterns_editor.graph.clone();
        let errors = self.settings_ui.validation_errors(&graph);
        if errors.is_empty() {
            egui_panels::notice(ui, StatusKind::Ok, &t!("settings.patterns.valid"));
        } else {
            egui_panels::notice(
                ui,
                StatusKind::Error,
                &t!("settings.patterns.invalid", count = errors.len()),
            );
            for error in errors {
                ui.label(format!("  • {error}"));
            }
        }
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
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let (description, _enabled) = self
            .patterns_editor
            .input_summary(&input_id)
            .unwrap_or_default();
        let summary = self.patterns_editor.component_summary(&input_id);
        let graph = self.patterns_editor.graph.clone();
        let problems = self.settings_ui.validation_errors(&graph).len();
        let mut close = false;
        let wide = ui.available_width() >= INSPECTOR_MIN_VIEWPORT;
        let selected_before = self.patterns_editor.state.selected;
        ui.spacing_mut().item_spacing.y = theme.section_spacing;

        // The toolbar: the way back, the rule, whether its graph is valid
        // and the menu that adds nodes.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if egui_panels::button(ui, t!("settings.patterns.back_to_rules"), Variant::Ghost)
                .on_hover_text(t!("settings.patterns.back_hint"))
                .clicked()
            {
                close = true;
            }
            ui.add(
                egui::Label::new(
                    RichText::new(&input_id)
                        .size(theme.section_title_size + 1.0)
                        .color(palette.strong_text),
                )
                .truncate(),
            )
            .on_hover_text(t!("settings.patterns.editor_hint"));
            ui.add(
                egui::Label::new(
                    RichText::new(&description)
                        .size(theme.small_size)
                        .color(palette.muted_text),
                )
                .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if wide {
                    let collapsed = self.patterns_editor.state.inspector_collapsed;
                    let (arrow, hint) = if collapsed {
                        ("◀", t!("settings.patterns.inspector_show"))
                    } else {
                        ("▶", t!("settings.patterns.inspector_hide"))
                    };
                    if egui_panels::button(ui, arrow, Variant::Ghost)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        self.patterns_editor.state.inspector_collapsed = !collapsed;
                    }
                }
                let missing = self.patterns_editor.missing_output_kinds();
                egui_panels::menu_button(
                    ui,
                    t!("settings.patterns.menu_node"),
                    Variant::Primary,
                    |ui| {
                        ui.menu_button(t!("settings.patterns.menu_detection"), |ui| {
                            for type_name in DETECTION_TYPES {
                                if ui.button(detection_type_label(type_name)).clicked() {
                                    self.patterns_editor.add_detection_of(type_name);
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button(t!("settings.patterns.menu_logic"), |ui| {
                            if ui.button(t!("settings.patterns.kind_aggregator")).clicked() {
                                self.patterns_editor.add_aggregator();
                                ui.close();
                            }
                            ui.separator();
                            for kind in [GateKind::And, GateKind::Or, GateKind::Xor, GateKind::Not]
                            {
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
                        });
                        ui.add_enabled_ui(!missing.is_empty(), |ui| {
                            ui.menu_button(t!("settings.patterns.menu_output"), |ui| {
                                for kind in missing {
                                    if ui.button(output_kind_label(kind)).clicked() {
                                        self.patterns_editor.add_output_of(kind);
                                        ui.close();
                                    }
                                }
                            });
                        });
                    },
                );
                ui.add_space(4.0);
                let (text, kind) = if problems == 0 {
                    (
                        t!(
                            "settings.patterns.editor_valid",
                            detections = summary.detections,
                            logic = summary.logic,
                            outputs = summary.outputs.len()
                        )
                        .into_owned(),
                        StatusKind::Ok,
                    )
                } else {
                    (
                        t!("settings.patterns.invalid", count = problems)
                            .trim_end_matches(':')
                            .to_owned(),
                        StatusKind::Error,
                    )
                };
                ui.add(
                    egui::Label::new(status_text(kind, &palette, &text, theme.small_size))
                        .truncate(),
                );
            });
        });
        egui_panels::divider(ui);
        for error in &self.patterns_editor.errors {
            egui_panels::status(ui, StatusKind::Error, error);
        }

        // The canvas and, when there is room, the panel of the selected node.
        let area = ui.available_rect_before_wrap();
        let show_inspector = area.width() >= INSPECTOR_MIN_VIEWPORT
            && !self.patterns_editor.state.inspector_collapsed;
        let inspector_width = if show_inspector { INSPECTOR_WIDTH } else { 0.0 };
        let canvas = egui::Rect::from_min_max(
            area.min,
            egui::pos2(area.right() - inspector_width, area.bottom()),
        );
        let graph_id = egui::Id::new("rules_graph");
        let snarl_selection = egui_snarl::ui::get_selected_nodes(graph_id, ui.ctx());
        ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
            let PatternsEditor { snarl, state, .. } = &mut self.patterns_editor;
            let mut viewer = RulesViewer { state: &mut *state };
            SnarlWidget::new()
                .id(graph_id)
                .style(graph_style(ui))
                .min_size(canvas.size())
                .show(snarl, &mut viewer, ui);
        });
        // A Shift click or a rectangle selects in the canvas itself: follow it.
        let now = egui_snarl::ui::get_selected_nodes(graph_id, ui.ctx());
        if now != snarl_selection {
            self.patterns_editor.state.selected = now.first().copied();
        }
        if show_inspector {
            let panel = egui::Rect::from_min_max(egui::pos2(canvas.right(), area.top()), area.max);
            ui.painter().vline(
                panel.left(),
                panel.y_range(),
                egui::Stroke::new(1.0, palette.separator),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(panel.width() - 28.0);
                        ui.set_min_height(panel.height() - 28.0);
                        egui::ScrollArea::vertical()
                            .id_salt("rules_inspector")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let selected = self.patterns_editor.state.selected;
                                self.show_inspector(ui, selected);
                            });
                    });
            });
        }
        ui.advance_cursor_after_rect(area);
        // A click selects after the panel was drawn: draw it again.
        if self.patterns_editor.state.selected != selected_before {
            ui.ctx().request_repaint();
        }
        if close {
            self.patterns_editor.close_editor();
        }
    }

    /// The panel at the right of the graph editor: the selected node's
    /// fields, what it is connected to and the actions on it.
    fn show_inspector(&mut self, ui: &mut egui::Ui, selected: Option<NodeId>) {
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        ui.spacing_mut().item_spacing.y = 10.0;
        let Some(node_id) =
            selected.filter(|id| self.patterns_editor.snarl.get_node(*id).is_some())
        else {
            ui.label(
                RichText::new(t!("settings.patterns.inspector_empty"))
                    .size(theme.small_size)
                    .color(palette.muted_text),
            );
            return;
        };
        let mut changed = false;
        let mut delete = false;
        let mut duplicate = false;
        let mut connections = Vec::new();
        {
            let snarl = &self.patterns_editor.snarl;
            for output in 0..output_pin_count(&snarl[node_id].kind) {
                let pin = snarl.out_pin(OutPinId {
                    node: node_id,
                    output,
                });
                let targets: Vec<String> = pin
                    .remotes
                    .iter()
                    .filter_map(|remote| snarl.get_node(remote.node))
                    .map(|node| node.id.clone())
                    .collect();
                connections.push((output, targets));
            }
        }
        {
            let node = &mut self.patterns_editor.snarl[node_id];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (mark, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(mark, 2.0, node_kind_color(&node.kind));
                ui.label(
                    RichText::new(node_kind_label(&node.kind))
                        .size(theme.section_title_size)
                        .color(palette.strong_text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(t!("settings.patterns.inspector_selected"))
                            .size(theme.small_size)
                            .color(palette.muted_text),
                    );
                });
            });
            egui_panels::divider(ui);
            changed |= field_row(ui, &t!("settings.patterns.field_id"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut node.id.clone())
                        .font(egui::TextStyle::Monospace)
                        .interactive(false)
                        .desired_width(ui.available_width()),
                );
                false
            });
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
        }
        if !connections.is_empty() {
            egui_panels::divider(ui);
            ui.label(RichText::new(t!("settings.patterns.connections")).color(palette.strong_text));
            let is_detection = matches!(
                self.patterns_editor.snarl[node_id].kind,
                NodeKind::Detection(_)
            );
            for (output, targets) in &connections {
                let (name, color) = if is_detection {
                    if *output == 1 {
                        ("F ➡", FALSE_WIRE_COLOR)
                    } else {
                        ("T ➡", palette.ok)
                    }
                } else {
                    ("➡", palette.muted_text)
                };
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(name).size(theme.small_size).color(color));
                    let text = if targets.is_empty() {
                        t!("settings.patterns.not_connected").into_owned()
                    } else {
                        targets.join(", ")
                    };
                    ui.label(
                        RichText::new(text)
                            .size(theme.small_size)
                            .color(palette.muted_text),
                    );
                });
            }
        }
        egui_panels::divider(ui);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let can_duplicate =
                !matches!(self.patterns_editor.snarl[node_id].kind, NodeKind::Input(_));
            ui.add_enabled_ui(can_duplicate, |ui| {
                duplicate =
                    egui_panels::button(ui, t!("settings.patterns.duplicate_node"), Variant::Ghost)
                        .clicked();
            });
            delete = egui_panels::button(ui, t!("settings.patterns.delete_node"), Variant::Ghost)
                .clicked();
        });
        if changed {
            self.patterns_editor.state.dirty = true;
        }
        if duplicate {
            self.patterns_editor.duplicate_node(node_id);
        }
        if delete {
            self.patterns_editor.snarl.remove_node(node_id);
            self.patterns_editor.state.selected = None;
            self.patterns_editor.state.dirty = true;
        }
    }
}

/// Width of the panel of the selected node in the graph editor.
const INSPECTOR_WIDTH: f32 = 260.0;

/// The panel is left out when the editor is narrower than this.
const INSPECTOR_MIN_VIEWPORT: f32 = 640.0;

/// The mark and the label of a node kind, in the node headers and the panel.
fn node_kind_color(kind: &NodeKind) -> Color32 {
    match kind {
        NodeKind::Input(_) => Color32::from_rgb(90, 160, 220),
        NodeKind::Detection(_) => Color32::from_rgb(110, 190, 120),
        NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => {
            Color32::from_rgb(225, 170, 80)
        }
        NodeKind::Output(_) => Color32::from_rgb(200, 125, 200),
    }
}

fn node_kind_label(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Input(_) => t!("settings.patterns.node_input").into_owned(),
        NodeKind::Detection(detection) => detection_type_label(detection.kind.type_name()),
        NodeKind::Output(output) => output_kind_label(output.kind),
        NodeKind::Aggregator(_) => t!("settings.patterns.kind_aggregator").into_owned(),
        NodeKind::Gate(gate) => gate_kind_label(gate.kind),
        NodeKind::Formatter(_) => t!("settings.patterns.kind_formatter").into_owned(),
    }
}

/// One line saying what a node does, for its body on the canvas.
fn node_summary(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Input(input) => {
            if input.channels.is_empty() {
                t!("settings.patterns.all_channels").into_owned()
            } else {
                input.channels.join(", ")
            }
        }
        NodeKind::Detection(detection) => match &detection.kind {
            DetectionRuleKind::ClearReport { keywords }
            | DetectionRuleKind::Query { keywords }
            | DetectionRuleKind::Keyword { keywords } => keywords.join(" · "),
            DetectionRuleKind::ShipNames { dictionaries } => dictionaries.join(" · "),
            DetectionRuleKind::Custom { pattern, words, .. } => {
                pattern.clone().unwrap_or_else(|| words.join(" · "))
            }
            DetectionRuleKind::SystemReport
            | DetectionRuleKind::ShipNamesZh
            | DetectionRuleKind::PilotCount => String::new(),
        },
        NodeKind::Output(_) => String::new(),
        NodeKind::Aggregator(aggregator) => {
            t!("settings.patterns.field_inputs").into_owned() + &format!(": {}", aggregator.inputs)
        }
        NodeKind::Gate(gate) => {
            t!("settings.patterns.field_inputs").into_owned() + &format!(": {}", gate.inputs)
        }
        NodeKind::Formatter(formatter) => formatter.template.clone(),
    }
}

/// The text of the graph's validity in the toolbar, in the color of `kind`.
fn status_text(
    kind: StatusKind,
    palette: &egui_panels::Palette,
    text: &str,
    size: f32,
) -> RichText {
    let color = match kind {
        StatusKind::Ok => palette.ok,
        StatusKind::Warning => palette.warning,
        StatusKind::Error => palette.error,
        StatusKind::Info => palette.muted_text,
    };
    RichText::new(text).size(size).color(color)
}

/// The look of the node editor, drawn like egui's own frames: flat nodes with
/// a one pixel border and small corners, a faintly tinted header, thin wires
/// and a plain square grid on the extreme background color. No shadows.
fn graph_style(ui: &egui::Ui) -> SnarlStyle {
    let visuals = ui.visuals();
    let theme = egui_panels::Theme::get(ui.ctx());
    let palette = theme.palette(visuals);
    let radius = theme.radius;
    let top_corners = egui::CornerRadius {
        nw: radius,
        ne: radius,
        sw: 0,
        se: 0,
    };
    SnarlStyle {
        node_frame: Some(
            egui::Frame::new()
                .fill(visuals.window_fill)
                .stroke(egui::Stroke::new(1.0, palette.card_stroke))
                .corner_radius(egui::CornerRadius::same(radius))
                .inner_margin(egui::Margin::same(6)),
        ),
        header_frame: Some(
            egui::Frame::new()
                .fill(palette.hover_fill)
                .corner_radius(top_corners)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        ),
        pin_size: Some(9.0),
        wire_width: Some(1.6),
        bg_frame: Some(egui::Frame::new().fill(visuals.extreme_bg_color)),
        bg_pattern: Some(BackgroundPattern::Grid(Grid::new(
            egui::vec2(24.0, 24.0),
            0.0,
        ))),
        bg_pattern_stroke: Some(egui::Stroke::new(1.0, palette.separator)),
        select_stoke: Some(egui::Stroke::new(1.0, palette.accent_stroke)),
        select_fill: Some(palette.accent.gamma_multiply(0.25)),
        ..SnarlStyle::new()
    }
}

/// What the user did with an input card this frame.
enum CardAction {
    Select,
    Open,
    Remove,
    Enable(bool),
    Describe(String),
    Rename(String),
}

/// Rows at least this wide show their output tags at the right end.
const TAGS_BESIDE_MIN_WIDTH: f32 = 760.0;

/// Width of the column of output tags beside a row's text.
const TAGS_WIDTH: f32 = 300.0;

/// The row of an input rule: a dot that says whether it is on, its id and
/// description, what it is made of and the outputs it reaches. A click
/// selects it and a double click opens its editor; the right-click menu
/// switches it on or off, edits its description and id, and removes it.
fn input_card(
    ui: &mut egui::Ui,
    index: usize,
    id: &str,
    description: &str,
    enabled: bool,
    selected: bool,
    summary: &ComponentSummary,
) -> Vec<CardAction> {
    let mut actions = Vec::new();
    let theme = egui_panels::Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let frame = egui::Frame::new()
        .fill(if selected {
            palette.card_fill_selected
        } else {
            palette.card_fill
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                palette.accent_stroke
            } else {
                palette.card_stroke
            },
        ))
        .corner_radius(egui::CornerRadius::same(theme.radius))
        .inner_margin(egui::Margin::symmetric(16, 12));
    let response = ui
        .scope_builder(
            // Salted with the row's position, not the entry's id: renaming
            // the entry would give every widget inside a new id, and the id
            // field in the menu would lose the focus at each key.
            egui::UiBuilder::new()
                .id_salt(("input_card", index))
                .sense(egui::Sense::click()),
            |ui| {
                frame.show(ui, |ui| {
                    let width = ui.available_width();
                    ui.set_width(width);
                    // Wide enough, the output tags sit at the right end;
                    // otherwise they wrap on a line of their own under the
                    // text instead of covering it.
                    let tags_beside = width >= TAGS_BESIDE_MIN_WIDTH;
                    let tag_labels: Vec<String> = summary
                        .outputs
                        .iter()
                        .map(|kind| output_kind_label(*kind))
                        .collect();
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        let (dot, _) = ui.allocate_exact_size(
                            egui::vec2(8.0, theme.section_title_size),
                            egui::Sense::hover(),
                        );
                        ui.painter().circle_filled(
                            dot.center(),
                            4.0,
                            if enabled {
                                palette.ok
                            } else {
                                palette.muted_text.gamma_multiply(0.6)
                            },
                        );
                        let text_width = if tags_beside {
                            width - 8.0 - TAGS_WIDTH - 2.0 * 14.0
                        } else {
                            width - 8.0 - 14.0
                        };
                        ui.vertical(|ui| {
                            ui.set_width(text_width);
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 9.0;
                                ui.label(
                                    RichText::new(id)
                                        .size(theme.section_title_size)
                                        .color(palette.strong_text),
                                );
                                ui.label(
                                    RichText::new(description)
                                        .size(theme.small_size)
                                        .color(palette.muted_text),
                                );
                            });
                            let channels = if summary.channels.is_empty() {
                                t!("settings.patterns.all_channels").into_owned()
                            } else {
                                summary.channels.join(", ")
                            };
                            ui.label(
                                RichText::new(t!(
                                    "settings.patterns.summary",
                                    channels = channels,
                                    detections = summary.detections,
                                    logic = summary.logic,
                                    outputs = summary.outputs.len()
                                ))
                                .size(theme.small_size)
                                .color(palette.muted_text),
                            );
                            if !tags_beside && !summary.outputs.is_empty() {
                                ui.add_space(3.0);
                                egui_panels::badges(ui, &tag_labels);
                            }
                        });
                        if tags_beside && !summary.outputs.is_empty() {
                            ui.allocate_ui_with_layout(
                                egui::vec2(TAGS_WIDTH, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| egui_panels::badges(ui, &tag_labels),
                            );
                        }
                    });
                });
            },
        )
        .response;
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t!("settings.patterns.row_hint"));
    if response.double_clicked() {
        actions.push(CardAction::Open);
    } else if response.clicked() {
        actions.push(CardAction::Select);
    }
    response.context_menu(|ui| {
        let mut on = enabled;
        if ui
            .checkbox(&mut on, t!("settings.patterns.state_on"))
            .changed()
        {
            actions.push(CardAction::Enable(on));
        }
        ui.separator();
        ui.label(RichText::new(t!("settings.patterns.field_description")).weak());
        let mut text = description.to_string();
        if ui
            .add(egui::TextEdit::singleline(&mut text).desired_width(240.0))
            .changed()
        {
            actions.push(CardAction::Describe(text));
        }
        // The id is edited as a draft and applied when the field is left
        // (Enter, Tab or a click elsewhere): applied at each key, an id
        // being typed is usually invalid (empty, or taken halfway through)
        // and was put back.
        ui.label(RichText::new(t!("settings.patterns.field_id")).weak());
        let edit_id = ui.make_persistent_id("input_id");
        let draft_id = edit_id.with("draft");
        let mut id_text = ui
            .data(|data| data.get_temp::<String>(draft_id))
            .unwrap_or_else(|| id.to_string());
        let field = ui
            .add(
                egui::TextEdit::singleline(&mut id_text)
                    .id(edit_id)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(240.0),
            )
            .on_hover_text(t!("settings.patterns.id_hint"));
        if field.lost_focus() {
            ui.data_mut(|data| data.remove::<String>(draft_id));
            let new_id = id_text.trim();
            if new_id != id {
                actions.push(CardAction::Rename(new_id.to_string()));
            }
        } else if field.has_focus() {
            ui.data_mut(|data| data.insert_temp(draft_id, id_text));
        }
        ui.separator();
        if ui.button(t!("settings.patterns.remove_input")).clicked() {
            actions.push(CardAction::Remove);
            ui.close();
        }
    });
    actions
}

/// The arrow that points along the flow of a node's pins: into an input, out
/// of an output.
const PIN_ARROW: &str = "➡";

/// The widest a node's body gets on the canvas; longer text is cut short.
const NODE_BODY_MAX_WIDTH: f32 = 170.0;

/// The snarl viewer of the graph editor.
struct RulesViewer<'a> {
    state: &'a mut GraphState,
}

impl RulesViewer<'_> {
    /// Selects `node` when `rect` (a part of it) is clicked: the canvas only
    /// selects with Shift or a rectangle, which is not how anyone expects a
    /// click on a node to work.
    fn select_on_click(&mut self, ui: &mut Ui, node: NodeId, rect: egui::Rect) {
        let id = ui.id().with(("select_node", node));
        if ui.interact(rect, id, egui::Sense::click()).clicked() {
            self.state.selected = Some(node);
        }
    }
}

/// The border of the selected node.
fn selection_color() -> Color32 {
    Color32::from_rgb(70, 150, 200)
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
                ui.label(format!(
                    "{PIN_ARROW} {}",
                    t!("settings.patterns.node_input")
                ));
            }
            NodeKind::Output(output) => {
                ui.label(format!("{PIN_ARROW} {}", output_kind_label(output.kind)));
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
                ui.label(format!(
                    "{} {PIN_ARROW}",
                    t!("settings.patterns.node_input")
                ));
                false
            }
            NodeKind::Detection(_) => {
                let false_pin = pin.id.output == 1;
                ui.label(format!("{} {PIN_ARROW}", if false_pin { "F" } else { "T" }));
                false_pin
            }
            NodeKind::Output(_) => false,
            NodeKind::Aggregator(_) | NodeKind::Gate(_) | NodeKind::Formatter(_) => {
                ui.label(format!(
                    "{} {PIN_ARROW}",
                    t!("settings.patterns.node_output")
                ));
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

    fn node_frame(
        &mut self,
        default: egui::Frame,
        node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        _snarl: &Snarl<Node>,
    ) -> egui::Frame {
        // The node the side panel shows gets the selection's border.
        if self.state.selected == Some(node) {
            default.stroke(egui::Stroke::new(1.5, selection_color()))
        } else {
            default
        }
    }

    fn show_header(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) {
        let node = &snarl[node_id];
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (mark, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(mark, 2.0, node_kind_color(&node.kind));
            ui.label(RichText::new(&node.id).strong());
            ui.label(RichText::new(self.title(node)).small().weak());
        });
        self.select_on_click(ui, node_id, row.response.rect);
    }

    fn has_body(&mut self, node: &Node) -> bool {
        !node_summary(&node.kind).is_empty() || !node.enabled
    }

    fn show_body(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) {
        let node = &snarl[node_id];
        ui.set_max_width(NODE_BODY_MAX_WIDTH);
        let summary = node_summary(&node.kind);
        let body = ui.vertical(|ui| {
            if !summary.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(summary).monospace().small().weak()).truncate(),
                );
            }
            if !node.enabled {
                ui.weak(t!("settings.patterns.state_off"));
            }
        });
        self.select_on_click(ui, node_id, body.response.rect);
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
            if ui.button(t!("settings.patterns.kind_aggregator")).clicked() {
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
    // Narrow places (the side panel) put the name above its value.
    if ui.available_width() < FIELD_NAME_WIDTH + 150.0 {
        return ui
            .vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(name);
                value(ui)
            })
            .inner;
    }
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

fn category_editor(ui: &mut Ui, category: &mut Option<webb::intel::IntelCategory>) -> bool {
    use webb::intel::IntelCategory;
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
