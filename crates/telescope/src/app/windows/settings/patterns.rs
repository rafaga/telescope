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

    /// Every id in use: the working graph's and the open editor's canvas,
    /// where nodes added since it opened are not merged into the graph yet.
    fn rule_ids(&self) -> HashSet<String> {
        let mut ids = collect_node_ids(&self.snarl);
        ids.extend(self.graph.nodes.iter().map(|node| node.id.clone()));
        ids
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

#[cfg(test)]
mod tests {
    use super::*;

    // Builders

    fn node(id: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind,
        }
    }

    fn input(id: &str, channels: &[&str]) -> Node {
        node(
            id,
            NodeKind::Input(InputNode {
                description: format!("{id} description"),
                kind: InputKind::ChatLog,
                path: String::new(),
                channels: channels.iter().map(|c| c.to_string()).collect(),
                exclude_motd: true,
            }),
        )
    }

    fn detection(id: &str) -> Node {
        node(
            id,
            NodeKind::Detection(DetectionNode {
                kind: DetectionRuleKind::SystemReport,
                case_insensitive: false,
            }),
        )
    }

    fn output(kind: OutputKind) -> Node {
        node(
            kind.type_name(),
            NodeKind::Output(OutputNode {
                kind,
                tooltip: TooltipConfig { emojis: true },
                log: LogConfig::default(),
            }),
        )
    }

    fn edge(from: &str, from_pin: Pin, to: &str, to_pin: u8) -> Edge {
        Edge {
            from: from.to_string(),
            from_pin,
            to: to.to_string(),
            to_pin,
        }
    }

    /// `in1 -> d1 -T-> visual`, `d1 -F-> log`; `in2 -> d2 -T-> visual` (the
    /// output is shared); `in3` alone.
    fn graph() -> RuleGraph {
        RuleGraph {
            nodes: vec![
                input("in1", &["Intel"]),
                detection("d1"),
                input("in2", &[]),
                detection("d2"),
                input("in3", &[]),
                output(OutputKind::Visual),
                output(OutputKind::Log),
            ],
            edges: vec![
                edge("in1", Pin::Out, "d1", 0),
                edge("d1", Pin::T, "visual", 0),
                edge("d1", Pin::F, "log", 0),
                edge("in2", Pin::Out, "d2", 0),
                edge("d2", Pin::T, "visual", 0),
            ],
        }
    }

    fn editor() -> PatternsEditor {
        let mut editor = PatternsEditor::default();
        editor.ensure_loaded(&graph());
        editor
    }

    fn ids(editor: &PatternsEditor) -> Vec<String> {
        editor.graph.nodes.iter().map(|n| n.id.clone()).collect()
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    // Free functions

    #[test]
    fn unique_id_takes_the_first_free_index() {
        assert_eq!(unique_id(&HashSet::new(), "input"), "input_1");
        assert_eq!(unique_id(&set(&["input_1", "input_2"]), "input"), "input_3");
        // A gap is filled, and other prefixes do not interfere.
        assert_eq!(unique_id(&set(&["input_2", "gate_1"]), "input"), "input_1");
    }

    #[test]
    fn valid_id_follows_the_rule_id_charset_and_length() {
        for id in ["a", "in_1", "Rule-2", &"x".repeat(64)] {
            assert!(valid_id(id), "{id:?} should be valid");
        }
        for id in [
            "",
            "has space",
            "dot.dot",
            "\u{f1}and\u{fa}",
            &"x".repeat(65),
        ] {
            assert!(!valid_id(id), "{id:?} should be invalid");
        }
    }

    #[test]
    fn every_offered_detection_type_has_a_default_of_that_type() {
        for name in DETECTION_TYPES {
            assert_eq!(default_detection_kind(name).type_name(), name);
        }
        assert_eq!(default_detection_kind("unknown").type_name(), "custom");
    }

    #[test]
    fn default_detections_are_valid_where_they_can_be() {
        let dictionaries = Dictionaries::defaults();
        for name in [
            "system_report",
            "clear_report",
            "ship_names",
            "ship_names_zh",
            "pilot_count",
            "query",
        ] {
            assert_eq!(
                default_detection_kind(name).validate("id", &dictionaries),
                Ok(()),
                "{name}"
            );
        }
        // A fresh keyword or custom rule still needs the user's words.
        assert!(
            default_detection_kind("keyword")
                .validate("id", &dictionaries)
                .is_err()
        );
        assert!(
            default_detection_kind("custom")
                .validate("id", &dictionaries)
                .is_err()
        );
    }

    #[test]
    fn default_ship_names_use_every_built_in_dictionary() {
        match default_detection_kind("ship_names") {
            DetectionRuleKind::ShipNames { dictionaries } => {
                assert_eq!(dictionaries, Dictionaries::defaults().names());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn output_kinds_are_offered_without_repeats() {
        let unique: HashSet<&str> = OUTPUT_KINDS.iter().map(|kind| kind.type_name()).collect();
        assert_eq!(unique.len(), OUTPUT_KINDS.len());
    }

    #[test]
    fn node_colors_group_the_special_nodes() {
        let gate = node(
            "g",
            NodeKind::Gate(GateNode {
                kind: GateKind::And,
                inputs: 2,
            }),
        );
        let aggregator = node("a", NodeKind::Aggregator(AggregatorNode { inputs: 2 }));
        assert_eq!(
            node_kind_color(&gate.kind),
            node_kind_color(&aggregator.kind)
        );
        assert_ne!(
            node_kind_color(&input("i", &[]).kind),
            node_kind_color(&detection("d").kind)
        );
    }

    #[test]
    fn node_summary_shows_what_the_node_matches() {
        let keywords = |list: &[&str]| DetectionRuleKind::Keyword {
            keywords: list.iter().map(|w| w.to_string()).collect(),
        };
        let summary = |kind: DetectionRuleKind| {
            node_summary(&NodeKind::Detection(DetectionNode {
                kind,
                case_insensitive: false,
            }))
        };
        assert_eq!(summary(keywords(&["a", "b"])), "a \u{b7} b");
        assert_eq!(summary(DetectionRuleKind::SystemReport), "");
        assert_eq!(
            summary(DetectionRuleKind::Custom {
                pattern: Some(String::from("x+")),
                words: vec![String::from("w")],
                category: None,
                system_group: None,
            }),
            "x+"
        );
        assert_eq!(
            summary(DetectionRuleKind::Custom {
                pattern: None,
                words: vec![String::from("w1"), String::from("w2")],
                category: None,
                system_group: None,
            }),
            "w1 \u{b7} w2"
        );
        assert_eq!(
            node_summary(&NodeKind::Formatter(FormatterNode {
                template: String::from("{all}"),
                inputs: 2
            })),
            "{all}"
        );
        assert_eq!(node_summary(&output(OutputKind::Log).kind), "");
        assert_eq!(node_summary(&input("i", &["A", "B"]).kind), "A, B");
    }

    // Loading

    #[test]
    fn ensure_loaded_copies_the_graph_only_once() {
        let mut editor = PatternsEditor::default();
        editor.ensure_loaded(&graph());
        editor.add_input();
        editor.ensure_loaded(&RuleGraph::default());
        assert_eq!(editor.graph.nodes.len(), graph().nodes.len() + 1);
    }

    #[test]
    fn reset_reloads_and_drops_all_editing_state() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.selected_input = Some(String::from("in2"));
        editor.set_errors(vec![String::from("boom")]);

        editor.reset(&RuleGraph::default());

        assert!(editor.graph.nodes.is_empty());
        assert!(!editor.is_open());
        assert!(editor.selected_input.is_none());
        assert!(editor.errors.is_empty());
        assert!(editor.original_ids.is_empty());
        assert!(!editor.state.dirty);
        assert_eq!(editor.snarl.nodes().count(), 0);
        assert_eq!(editor.to_graph(), RuleGraph::default());
    }

    #[test]
    fn errors_can_be_set_and_cleared() {
        let mut editor = editor();
        editor.set_errors(vec![String::from("a"), String::from("b")]);
        assert_eq!(editor.errors, ["a", "b"]);
        editor.clear_errors();
        assert!(editor.errors.is_empty());
    }

    // Inputs

    #[test]
    fn input_ids_and_summaries_only_cover_inputs() {
        let editor = editor();
        assert_eq!(editor.input_ids(), ["in1", "in2", "in3"]);
        assert_eq!(
            editor.input_summary("in1"),
            Some((String::from("in1 description"), true))
        );
        assert_eq!(editor.input_summary("d1"), None);
        assert_eq!(editor.input_summary("nope"), None);
    }

    #[test]
    fn add_input_makes_a_unique_enabled_input_and_marks_the_graph_dirty() {
        let mut editor = editor();
        assert!(!editor.state.dirty);
        let first = editor.add_input();
        let second = editor.add_input();
        assert_eq!((first.as_str(), second.as_str()), ("input_1", "input_2"));
        assert!(editor.state.dirty);
        let (description, enabled) = editor.input_summary("input_1").unwrap();
        assert_eq!(description, "input");
        assert!(enabled);
    }

    #[test]
    fn remove_input_drops_its_edges_and_any_open_or_selected_state() {
        let mut editor = editor();
        editor.selected_input = Some(String::from("in1"));
        editor.open_input = Some(String::from("in1"));

        editor.remove_input("in1");

        assert!(!ids(&editor).contains(&String::from("in1")));
        assert!(
            editor
                .graph
                .edges
                .iter()
                .all(|e| e.from != "in1" && e.to != "in1")
        );
        assert!(editor.open_input.is_none());
        assert!(editor.selected_input.is_none());
        assert!(editor.state.dirty);
        // The detection it fed stays, now without a source.
        assert!(ids(&editor).contains(&String::from("d1")));
    }

    #[test]
    fn remove_input_keeps_an_unrelated_open_editor() {
        let mut editor = editor();
        editor.open_input = Some(String::from("in2"));
        editor.selected_input = Some(String::from("in2"));
        editor.remove_input("in3");
        assert_eq!(editor.open_input.as_deref(), Some("in2"));
        assert_eq!(editor.selected_input.as_deref(), Some("in2"));
    }

    #[test]
    fn rename_input_updates_nodes_edges_and_selection() {
        let mut editor = editor();
        editor.selected_input = Some(String::from("in1"));

        assert!(editor.rename_input("in1", "front_door"));

        assert!(ids(&editor).contains(&String::from("front_door")));
        assert!(!ids(&editor).contains(&String::from("in1")));
        assert!(
            editor
                .graph
                .edges
                .contains(&edge("front_door", Pin::Out, "d1", 0))
        );
        assert_eq!(editor.selected_input.as_deref(), Some("front_door"));
        assert!(editor.state.dirty);
    }

    #[test]
    fn rename_input_to_itself_succeeds_without_marking_dirty() {
        let mut editor = editor();
        assert!(editor.rename_input("in1", "in1"));
        assert!(!editor.state.dirty);
        assert_eq!(editor.to_graph(), graph());
    }

    #[test]
    fn rename_input_rejects_invalid_and_taken_ids() {
        let mut editor = editor();
        for new in ["", "bad id", "d1", "in2", "visual"] {
            assert!(!editor.rename_input("in1", new), "{new:?}");
        }
        assert_eq!(editor.to_graph(), graph());
        assert!(!editor.state.dirty);
    }

    #[test]
    fn description_and_enabled_flag_are_editable() {
        let mut editor = editor();
        editor.set_input_description("in1", String::from("new text"));
        assert_eq!(editor.input_summary("in1").unwrap().0, "new text");
        assert!(editor.state.dirty);

        editor.state.dirty = false;
        editor.set_input_enabled("in1", false);
        assert!(!editor.input_summary("in1").unwrap().1);
        assert!(editor.state.dirty);
    }

    #[test]
    fn description_of_a_non_input_or_missing_node_is_left_alone() {
        let mut editor = editor();
        editor.set_input_description("d1", String::from("nope"));
        editor.set_input_description("missing", String::from("nope"));
        editor.set_input_enabled("missing", false);
        assert_eq!(editor.to_graph(), graph());
        assert!(!editor.state.dirty);
    }

    // Components

    #[test]
    fn component_follows_the_wires_forward_but_not_through_outputs() {
        let editor = editor();
        // in2 shares the `visual` output with in1, but outputs are not crossed.
        assert_eq!(
            editor.component_ids("in1"),
            set(&["in1", "d1", "visual", "log"])
        );
        assert_eq!(editor.component_ids("in2"), set(&["in2", "d2", "visual"]));
        assert_eq!(editor.component_ids("in3"), set(&["in3"]));
    }

    #[test]
    fn component_pulls_in_the_other_sources_of_a_shared_detection() {
        let mut editor = editor();
        editor.graph.edges.push(edge("in3", Pin::Out, "d1", 1));
        assert_eq!(
            editor.component_ids("in1"),
            set(&["in1", "in3", "d1", "visual", "log"])
        );
        assert!(editor.component_ids("in3").contains("in1"));
    }

    #[test]
    fn component_of_an_unknown_id_is_just_that_id() {
        assert_eq!(editor().component_ids("ghost"), set(&["ghost"]));
    }

    #[test]
    fn component_summary_counts_parts_and_orders_outputs() {
        let mut editor = editor();
        editor.graph.nodes.push(node(
            "gate_1",
            NodeKind::Gate(GateNode {
                kind: GateKind::And,
                inputs: 2,
            }),
        ));
        editor.graph.edges.push(edge("d1", Pin::T, "gate_1", 0));

        let summary = editor.component_summary("in1");

        assert_eq!(summary.channels, ["Intel"]);
        assert_eq!(summary.detections, 1);
        assert_eq!(summary.logic, 1);
        // Offered order: visual before log, whatever the graph's order.
        assert_eq!(summary.outputs, [OutputKind::Visual, OutputKind::Log]);
        assert!(editor.component_summary("in3").outputs.is_empty());
    }

    #[test]
    fn build_component_places_nodes_in_columns_and_wires_the_pins() {
        let mut editor = editor();
        editor.build_component("in1");

        assert_eq!(editor.snarl.nodes().count(), 4);
        assert_eq!(editor.original_ids, set(&["in1", "d1", "visual", "log"]));
        let x_of = |id: &str| {
            let node_id = editor.find_node(id).unwrap();
            editor.snarl.get_node_info(node_id).unwrap().pos.x
        };
        assert_eq!(x_of("in1"), 0.0);
        assert_eq!(x_of("d1"), 360.0);
        assert_eq!(x_of("visual"), 900.0);
        assert_eq!(x_of("log"), 900.0);

        // T is the detection's first output pin, F the second.
        let mut wires: Vec<(String, usize, String)> = editor
            .snarl
            .wires()
            .map(|(from, to)| {
                (
                    editor.snarl[from.node].id.clone(),
                    from.output,
                    editor.snarl[to.node].id.clone(),
                )
            })
            .collect();
        wires.sort();
        assert_eq!(
            wires,
            [
                (String::from("d1"), 0, String::from("visual")),
                (String::from("d1"), 1, String::from("log")),
                (String::from("in1"), 0, String::from("d1")),
            ]
        );
    }

    #[test]
    fn build_component_keeps_saved_positions() {
        let mut editor = editor();
        editor.graph.nodes[1].x = 123.0;
        editor.graph.nodes[1].y = 45.0;
        editor.build_component("in1");
        let info = editor
            .snarl
            .get_node_info(editor.find_node("d1").unwrap())
            .unwrap();
        assert_eq!((info.pos.x, info.pos.y), (123.0, 45.0));
    }

    #[test]
    fn opening_and_closing_without_changes_keeps_the_graph() {
        let mut editor = editor();
        editor.open_editor("in1");
        assert!(editor.is_open());
        assert!(editor.state.selected.is_none());

        editor.close_editor();

        assert!(!editor.is_open());
        assert!(editor.errors.is_empty());
        // Only the layout positions were filled in.
        assert_eq!(ids(&editor).len(), graph().nodes.len());
        let mut edges = editor.graph.edges.clone();
        let mut expected = graph().edges;
        let key = |e: &Edge| (e.from.clone(), e.from_pin as u8, e.to.clone());
        edges.sort_by_key(key);
        expected.sort_by_key(key);
        assert_eq!(edges, expected);
    }

    #[test]
    fn close_editor_with_nothing_open_does_nothing() {
        let mut editor = editor();
        editor.close_editor();
        assert_eq!(editor.to_graph(), graph());
        assert!(!editor.state.dirty);
    }

    #[test]
    fn deleting_a_node_in_the_editor_removes_it_and_its_edges_on_close() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();
        editor.snarl.remove_node(d1);

        editor.close_editor();

        assert!(!ids(&editor).contains(&String::from("d1")));
        assert!(
            editor
                .graph
                .edges
                .iter()
                .all(|e| e.from != "d1" && e.to != "d1")
        );
        // The other component is untouched.
        assert!(
            editor
                .graph
                .edges
                .contains(&edge("d2", Pin::T, "visual", 0))
        );
        assert!(editor.graph.edges.contains(&edge("in2", Pin::Out, "d2", 0)));
        assert!(editor.state.dirty);
    }

    #[test]
    fn new_wires_in_the_editor_become_edges_on_close() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.add_detection_of("query");
        editor.close_editor();

        let detection = editor
            .graph
            .nodes
            .iter()
            .find(|n| n.id == "detection_1")
            .expect("the new detection was merged");
        assert!(matches!(detection.kind, NodeKind::Detection(_)));
        assert!(
            editor
                .graph
                .edges
                .contains(&edge("in1", Pin::Out, "detection_1", 0))
        );
    }

    #[test]
    fn duplicate_ids_in_the_editor_keep_it_open_with_an_error() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();
        editor.snarl.get_node_mut(d1).unwrap().id = String::from("in1");
        let before = editor.graph.clone();

        editor.close_editor();

        assert!(editor.is_open());
        assert_eq!(editor.errors.len(), 1);
        assert!(editor.errors[0].contains("duplicate node id 'in1'"));
        assert_eq!(editor.graph, before, "a failed merge writes nothing");
    }

    #[test]
    fn a_new_node_cannot_take_an_id_used_by_another_component() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();
        editor.snarl.get_node_mut(d1).unwrap().id = String::from("d2");

        assert_eq!(
            editor.merge_component(),
            Err(String::from("node id 'd2' is already in use"))
        );
    }

    #[test]
    fn an_output_of_another_component_can_be_reused() {
        let mut editor = editor();
        editor.open_editor("in3");
        // `visual` lives in the other components' graph, not in this one.
        editor.add_output_of(OutputKind::Visual);
        assert_eq!(editor.merge_component(), Ok(()));
        assert_eq!(
            editor
                .graph
                .nodes
                .iter()
                .filter(|n| n.id == "visual")
                .count(),
            1
        );
    }

    #[test]
    fn edges_to_nodes_outside_the_component_survive_a_merge() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.close_editor();
        // in2's edge into the shared output was not part of in1's component.
        assert!(
            editor
                .graph
                .edges
                .contains(&edge("d2", Pin::T, "visual", 0))
        );
    }

    #[test]
    fn merge_records_the_canvas_positions() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();
        editor.snarl.get_node_info_mut(d1).unwrap().pos = Pos2::new(11.0, 22.0);
        editor.close_editor();
        let node = editor.graph.nodes.iter().find(|n| n.id == "d1").unwrap();
        assert_eq!((node.x, node.y), (11.0, 22.0));
    }

    // Adding nodes to the open component

    #[test]
    fn add_detection_connects_it_to_the_open_input_and_stacks_downwards() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.add_detection_of("keyword");
        editor.add_detection_of("query");

        let first = editor.find_node("detection_1").unwrap();
        let second = editor.find_node("detection_2").unwrap();
        // d1 was already there, so the first new one is the second detection.
        let y = |id| editor.snarl.get_node_info(id).unwrap().pos.y;
        assert_eq!(y(first), 260.0);
        assert_eq!(y(second), 520.0);
        let from_input = editor
            .snarl
            .wires()
            .filter(|(from, _)| editor.snarl[from.node].id == "in1")
            .count();
        assert_eq!(from_input, 3);
        assert!(editor.state.dirty);
    }

    #[test]
    fn add_detection_without_an_open_input_stays_unconnected() {
        let mut editor = editor();
        editor.add_detection_of("custom");
        assert_eq!(editor.snarl.wires().count(), 0);
        assert_eq!(editor.detection_count(), 1);
    }

    #[test]
    fn each_output_kind_can_be_added_once() {
        let mut editor = editor();
        assert_eq!(editor.missing_output_kinds(), OUTPUT_KINDS);

        editor.add_output_of(OutputKind::Sound);
        editor.add_output_of(OutputKind::Sound);

        assert_eq!(editor.output_count(), 1);
        assert!(!editor.missing_output_kinds().contains(&OutputKind::Sound));
        assert_eq!(editor.missing_output_kinds().len(), OUTPUT_KINDS.len() - 1);
        let sound = editor.find_node("sound").expect("id is the output kind");
        assert!(
            matches!(&editor.snarl[sound].kind, NodeKind::Output(o) if o.kind == OutputKind::Sound)
        );
    }

    #[test]
    fn special_nodes_get_unique_ids_and_count_together() {
        let mut editor = editor();
        editor.add_aggregator();
        editor.add_aggregator();
        editor.add_gate(GateKind::Xor);
        editor.add_formatter();

        for id in ["aggregator_1", "aggregator_2", "xor_1", "formatter_1"] {
            assert!(editor.find_node(id).is_some(), "{id}");
        }
        assert_eq!(editor.aggregator_count(), 2);
        assert_eq!(editor.special_count(), 4);
        assert_eq!(editor.detection_count(), 0);
        // Each new one is placed below the previous ones.
        let y_of = |id: &str| {
            let node_id = editor.find_node(id).unwrap();
            editor.snarl.get_node_info(node_id).unwrap().pos.y
        };
        assert_eq!(y_of("aggregator_1"), 0.0);
        assert_eq!(y_of("aggregator_2"), 260.0);
        assert_eq!(y_of("xor_1"), 520.0);
        assert_eq!(y_of("formatter_1"), 780.0);
    }

    #[test]
    fn ids_of_nodes_added_in_the_open_editor_are_not_reused() {
        // The toolbar adds several nodes before the editor is closed, when
        // none of them is in the graph yet.
        let mut editor = editor();
        editor.open_editor("in1");
        editor.add_aggregator();
        editor.add_aggregator();
        editor.add_detection_of("keyword");
        editor.add_detection_of("keyword");
        let d1 = editor.find_node("d1").unwrap();
        editor.duplicate_node(d1);
        editor.duplicate_node(d1);

        let canvas: Vec<String> = editor
            .snarl
            .node_ids()
            .map(|(_, node)| node.id.clone())
            .collect();
        assert_eq!(canvas.len(), canvas.iter().collect::<HashSet<_>>().len());
        assert_eq!(editor.merge_component(), Ok(()));
    }

    #[test]
    fn new_special_nodes_start_with_two_or_four_inputs() {
        let mut editor = editor();
        editor.add_aggregator();
        editor.add_gate(GateKind::And);
        editor.add_formatter();
        let kind = |id: &str| editor.snarl[editor.find_node(id).unwrap()].kind.clone();
        assert!(matches!(kind("aggregator_1"), NodeKind::Aggregator(a) if a.inputs == 4));
        assert!(matches!(kind("and_1"), NodeKind::Gate(g) if g.inputs == 2));
        assert!(
            matches!(kind("formatter_1"), NodeKind::Formatter(f) if f.template == "{all}" && f.inputs == 2)
        );
    }

    #[test]
    fn duplicate_node_copies_it_unconnected_with_a_new_id() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();

        editor.duplicate_node(d1);

        let copy = editor.find_node("d1_copy_1").expect("copy exists");
        assert_eq!(editor.state.selected, Some(copy));
        assert!(matches!(editor.snarl[copy].kind, NodeKind::Detection(_)));
        assert!(
            editor
                .snarl
                .wires()
                .all(|(from, to)| from.node != copy && to.node != copy)
        );
        let original = editor.snarl.get_node_info(d1).unwrap().pos;
        let moved = editor.snarl.get_node_info(copy).unwrap().pos;
        assert_eq!(moved, original + egui::vec2(28.0, 28.0));

        editor.duplicate_node(d1);
        assert!(editor.find_node("d1_copy_2").is_some());
    }

    // Applying

    #[test]
    fn is_modified_needs_a_loaded_graph_that_differs_or_a_dirty_open_editor() {
        let live = graph();
        assert!(!PatternsEditor::default().is_modified(&live));

        let mut editor = editor();
        assert!(!editor.is_modified(&live));

        editor.add_input();
        assert!(editor.is_modified(&live));
        assert!(editor.is_modified(&RuleGraph::default()));
    }

    #[test]
    fn dirty_edits_in_an_open_editor_count_as_modified() {
        let live = graph();
        let mut editor = editor();
        editor.open_editor("in1");
        assert!(!editor.is_modified(&live));
        editor.add_aggregator();
        assert!(editor.is_modified(&live));
        editor.mark_applied();
        assert!(!editor.is_modified(&live));
    }

    #[test]
    fn commit_open_editor_saves_the_edits_and_keeps_the_editor_open() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.add_aggregator();

        assert_eq!(editor.commit_open_editor(), Ok(()));

        assert!(editor.is_open());
        assert!(ids(&editor).contains(&String::from("aggregator_1")));
        // Not asserted: the rebuilt canvas only shows what is wired to the
        // input, so a node still unconnected is in the graph but not on it.
    }

    #[test]
    fn commit_open_editor_reports_a_bad_id_and_does_not_rebuild() {
        let mut editor = editor();
        editor.open_editor("in1");
        let d1 = editor.find_node("d1").unwrap();
        editor.snarl.get_node_mut(d1).unwrap().id = String::from("in1");

        assert!(editor.commit_open_editor().is_err());
        assert!(editor.is_open());
        assert_eq!(editor.snarl.nodes().count(), 4);
    }

    #[test]
    fn commit_without_an_open_editor_is_a_no_op() {
        let mut editor = editor();
        assert_eq!(editor.commit_open_editor(), Ok(()));
        assert_eq!(editor.to_graph(), graph());
    }
}
