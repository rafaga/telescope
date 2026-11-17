//! Settings -> Rules: the intel rule graph as a list of input nodes.
//!
//! Each Input node is a card (its switch, description, what the rule is
//! made of and the outputs it reaches); opening one shows the node editor of
//! the **connected component** of that input in a window of its own (see
//! `show_editor_window`), a modal one: the Settings window waits until it is
//! closed. The editor does not depend on the Settings page: it only needs the
//! working graph in [`PatternsEditor`]. Nodes are shared instances of the
//! underlying [`RuleGraph`]: opening another input shows the same nodes.
//!
//! Wires: `input.out -> detection.in` and `detection.t|f -> output.in`. The
//! edges are rebuilt from the graph's wires when the editor closes.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, SettingsPage, Type};
use eframe::egui::{self, Color32, Pos2, RichText, Ui};
use egui_ide::{PropertyGrid, Toolbar, ToolbarIcon, Toolbox, ToolboxEntry};
use egui_panels::{StatusKind, Variant};
use egui_snarl::{
    InPin, InPinId, NodeId, OutPin, OutPinId, Snarl,
    ui::{BackgroundPattern, Grid, PinInfo, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget},
};
use std::collections::{HashMap, HashSet};
use webb::graph::{
    AggregatorNode, DetectionMode, DetectionNode, DetectionSource, Edge, FormatterNode, InputNode,
    LogConfig, Node, NodeKind, OutputNode, RuleGraph, TooltipConfig, has_output_pin,
    input_pin_count,
};
use webb::rules::{
    DetectionRuleKind, DetectionType, Dictionaries, InputKind, OutputKind, OutputType,
};

mod node_properties;

use node_properties::NodeProperties;

/// File the rules are imported from / exported to (next to the app).
const RULES_FILE: &str = "rules.toml";

/// The detection types, by `type_name()`.
const DETECTION_TYPES: [&str; 7] = [
    "system_report",
    "clear_report",
    "ship_names",
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
    /// The toolbox is folded away.
    toolbox_collapsed: bool,
    /// Sizes of the panels of the editor.
    layout: PanelLayout,
}

/// Sizes of the panels of the graph editor: the width each one starts with
/// (a drag of its edge resizes it) and how wide the editor has to be to show
/// each one.
#[derive(Clone, Copy)]
struct PanelLayout {
    /// Starting width of the toolbox.
    toolbox_width: f32,
    /// The widest the toolbox can be made.
    toolbox_max_width: f32,
    /// Starting width of the properties panel (the selected node).
    inspector_width: f32,
    /// Width of the column of names of the properties panel.
    name_width: f32,
    /// The toolbox is left out when the editor is narrower than this (the
    /// "+ Node" menu of the toolbar adds nodes then).
    toolbox_min_viewport: f32,
    /// The properties panel is left out when the editor is narrower than this.
    inspector_min_viewport: f32,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            toolbox_width: 190.0,
            toolbox_max_width: 360.0,
            inspector_width: 320.0,
            name_width: 120.0,
            toolbox_min_viewport: 860.0,
            inspector_min_viewport: 640.0,
        }
    }
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
    /// Where the editor's window opens, fixed when it does so that it stays
    /// where the user puts it; `None` while it is closed.
    window_place: Option<Pos2>,
    /// The editor's window has to come to the front (the Settings window
    /// behind it was clicked).
    to_front: bool,
    /// The title the editor's window shows, to change it only when the rule
    /// it edits does.
    window_title: String,
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
        self.window_place = None;
        self.to_front = false;
        self.window_title.clear();
    }

    /// Asks for the editor's window to come to the front.
    pub(crate) fn bring_to_front(&mut self) {
        self.to_front = true;
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

    /// Adds an input of the given `kind`. The kind is only chosen here, when
    /// the rule is created; the node editor never changes it.
    fn add_input(&mut self, kind: InputKind) -> String {
        let id = unique_id(&self.rule_ids(), "input");
        self.graph.nodes.push(Node {
            id: id.clone(),
            enabled: true,
            x: 0.0,
            y: 0.0,
            kind: NodeKind::Input(InputNode {
                description: kind.label(),
                kind,
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

    /// Adds the node of `item` (a click on the toolbox, or a drop on the
    /// canvas at `at`, a position of the graph) and selects it, so the
    /// properties panel shows it.
    fn add_toolbox_item(&mut self, item: ToolboxItem, at: Option<Pos2>) {
        let before = collect_node_ids(&self.snarl);
        match item {
            ToolboxItem::Detection(type_name) => self.add_detection_of(type_name),
            ToolboxItem::Aggregator => self.add_aggregator(),
            ToolboxItem::Formatter => self.add_formatter(),
            ToolboxItem::Output(kind) => self.add_output_of(kind),
        }
        let added = self
            .snarl
            .node_ids()
            .find(|(_, node)| !before.contains(&node.id))
            .map(|(id, _)| id);
        if let Some(node_id) = added {
            self.state.selected = Some(node_id);
            // A node dropped on the canvas stays where it was dropped.
            if let Some(at) = at
                && let Some(info) = self.snarl.get_node_info_mut(node_id)
            {
                info.pos = at;
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
                source: DetectionSource::Auto,
                mode: DetectionMode::Require,
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
            .filter(|node| matches!(&node.kind, NodeKind::Aggregator(_) | NodeKind::Formatter(_)))
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
                    // The other sources of a detection (not the detections
                    // that lead to it, which the walk forward already took).
                    let from_other_source = self
                        .graph
                        .nodes
                        .iter()
                        .find(|node| node.id == edge.from)
                        .is_some_and(|node| !matches!(node.kind, NodeKind::Detection(_)));
                    if edge.to == id && from_other_source && component.insert(edge.from.clone()) {
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
                NodeKind::Aggregator(_) | NodeKind::Formatter(_) => 2,
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
            self.snarl.connect(
                OutPinId {
                    node: from,
                    output: 0,
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
            if !has_output_pin(&from_node.kind) {
                continue;
            }
            edges.push(Edge {
                from: from_node.id.clone(),
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
            .sort_by(|a, b| (&a.from, &a.to, a.to_pin).cmp(&(&b.from, &b.to, b.to_pin)));
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
                    summary.source = InputSource::of(input);
                }
                continue;
            }
            if !component.contains(&node.id) {
                continue;
            }
            match &node.kind {
                NodeKind::Input(_) => {}
                NodeKind::Detection(_) => summary.detections += 1,
                NodeKind::Aggregator(_) | NodeKind::Formatter(_) => {
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
    /// What the input reads.
    source: InputSource,
    detections: usize,
    /// Aggregators, gates and formatters.
    logic: usize,
    /// The output kinds reached, in [`OUTPUT_KINDS`] order.
    outputs: Vec<OutputKind>,
}

impl TelescopeApp {
    /// The Rules page: one card per input rule. The node editor of a rule
    /// opened is not part of it (see `show_editor_window`).
    pub(crate) fn show_patterns_page(&mut self, ui: &mut egui::Ui) {
        let graph = self.intel_graph.clone();
        self.patterns_editor.ensure_loaded(&graph);

        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::Rules.title(),
                Some(&t!("settings.patterns.description")),
            );
            let has_inputs = !self.patterns_editor.input_ids().is_empty();
            let (mut new_input, mut open, mut import, mut export) = (None, false, false, false);
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
                egui_panels::menu_button(
                    ui,
                    t!("settings.patterns.add_input"),
                    Variant::Secondary,
                    |ui| {
                        for kind in INPUT_KINDS {
                            if ui.button(kind.label()).clicked() {
                                new_input = Some(kind);
                                ui.close();
                            }
                        }
                    },
                );
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
            if let Some(kind) = new_input {
                let id = self.patterns_editor.add_input(kind);
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
            let show_ids = self.node_ids_visible();
            for (index, id) in input_ids.iter().enumerate() {
                let Some((description, enabled)) = self.patterns_editor.input_summary(id) else {
                    continue;
                };
                let summary = self.patterns_editor.component_summary(id);
                let selected = self.patterns_editor.selected_input.as_deref() == Some(id.as_str());
                let card = input_card(
                    ui,
                    index,
                    show_ids.then_some(id.as_str()),
                    &description,
                    enabled,
                    selected,
                    &summary,
                );
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

    /// Draws the node editor of the rule open, in a window of its own, for as
    /// long as one is open (call it every frame). The window is a modal one
    /// over Settings, which is disabled meanwhile (see
    /// `show_settings_window`). It opens centered over the app's window and
    /// stays where the user puts it. Its close button does what "Done" does:
    /// the nodes are written back to the working graph, and if that fails
    /// (an id that cannot be used) the window stays open with the error.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn show_editor_window(&mut self, ctx: &egui::Context) {
        if self.patterns_editor.open_input.is_none() {
            self.patterns_editor.window_place = None;
            self.patterns_editor.to_front = false;
            return;
        }
        let id = egui::ViewportId::from_hash_of("telescope.rules_editor");
        let size = egui::vec2(1100.0, 720.0);
        let place = *self.patterns_editor.window_place.get_or_insert_with(|| {
            ctx.input(|input| input.viewport().outer_rect)
                .map_or(egui::pos2(80.0, 80.0), |app| {
                    let corner = app.center() - size / 2.0;
                    egui::pos2(corner.x.max(0.0), corner.y.max(0.0))
                })
        });
        let title = self.editor_window_title();
        if self.patterns_editor.window_title != title {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Title(title.clone()));
            self.patterns_editor.window_title = title.clone();
        }
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_position(place);
        if std::mem::take(&mut self.patterns_editor.to_front) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        ctx.show_viewport_immediate(id, builder, |ui, _class| {
            // No margin around the editor: its panels reach the edges of the
            // window, and only the toolbar keeps its own.
            egui::Frame::NONE.show(ui, |ui| self.show_editor(ui));
            if ui.input(|input| input.viewport().close_requested()) {
                self.patterns_editor.close_editor();
                if self.patterns_editor.is_open() {
                    // the nodes could not be written back: keep it open
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            }
        });
    }

    /// The title of the editor's window: the editor, the rule it edits and
    /// what that rule says it is for.
    fn editor_window_title(&self) -> String {
        let name = t!("settings.patterns.editor_title").into_owned();
        let Some(input_id) = self.patterns_editor.open_input.as_deref() else {
            return name;
        };
        let (description, _enabled) = self
            .patterns_editor
            .input_summary(input_id)
            .unwrap_or_default();
        if description.is_empty() || description == input_id {
            format!("{name} \u{2014} {input_id}")
        } else {
            format!("{name} \u{2014} {input_id} ({description})")
        }
    }

    /// Draws the open component's graph editor (in the editor's window).
    fn show_editor(&mut self, ui: &mut egui::Ui) {
        let Some(input_id) = self.patterns_editor.open_input.clone() else {
            return;
        };
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let summary = self.patterns_editor.component_summary(&input_id);
        let graph = self.patterns_editor.graph.clone();
        let problems = self.settings_ui.validation_errors(&graph).len();
        let mut close = false;
        let layout = self.patterns_editor.state.layout;
        let wide = ui.available_width() >= layout.inspector_min_viewport;
        let toolbox_fits = ui.available_width() >= layout.toolbox_min_viewport;
        let show_toolbox = toolbox_fits && !self.patterns_editor.state.toolbox_collapsed;
        let selected_before = self.patterns_editor.state.selected;
        // The bars and the panels sit against each other: the space around
        // them is their own margin.
        ui.spacing_mut().item_spacing.y = 0.0;

        // The toolbar: the way out, the rule, whether its graph is valid
        // and the menu that adds nodes.
        let selected = self.patterns_editor.state.selected;
        let editable = selected.is_some_and(|id| {
            self.patterns_editor
                .snarl
                .get_node_info(id)
                .is_some_and(|info| !matches!(info.value.kind, NodeKind::Input(_)))
        });
        let (mut duplicate, mut delete) = (false, false);
        egui::Panel::top("rules_editor_toolbar")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .resizable(false)
            .show(ui, |ui| {
                Toolbar::panel(ui, |bar| {
                    bar.group(&t!("settings.patterns.toolbar_file"), |bar| {
                        if bar
                            .icon_button(
                                ToolbarIcon::Done,
                                &t!("settings.patterns.editor_done_hint"),
                            )
                            .clicked()
                        {
                            close = true;
                        }
                    });
                    bar.group(&t!("settings.patterns.toolbar_edit"), |bar| {
                        bar.enabled(editable, |bar| {
                            duplicate = bar
                                .icon_button(
                                    ToolbarIcon::Duplicate,
                                    &t!("settings.patterns.duplicate_node"),
                                )
                                .clicked();
                            bar.separator();
                            delete = bar
                                .icon_button(
                                    ToolbarIcon::Delete,
                                    &t!("settings.patterns.delete_node"),
                                )
                                .clicked();
                        });
                    });
                    if toolbox_fits || wide {
                        bar.group(&t!("settings.patterns.toolbar_view"), |bar| {
                            if toolbox_fits {
                                let mut shown = !self.patterns_editor.state.toolbox_collapsed;
                                let hint = if shown {
                                    t!("settings.patterns.toolbox_hide")
                                } else {
                                    t!("settings.patterns.toolbox_show")
                                };
                                if bar
                                    .icon_toggle(&mut shown, ToolbarIcon::Toolbox, &hint)
                                    .changed()
                                {
                                    self.patterns_editor.state.toolbox_collapsed = !shown;
                                }
                            }
                            if wide {
                                let mut shown = !self.patterns_editor.state.inspector_collapsed;
                                let hint = if shown {
                                    t!("settings.patterns.inspector_hide")
                                } else {
                                    t!("settings.patterns.inspector_show")
                                };
                                if bar
                                    .icon_toggle(&mut shown, ToolbarIcon::Properties, &hint)
                                    .changed()
                                {
                                    self.patterns_editor.state.inspector_collapsed = !shown;
                                }
                            }
                        });
                    }
                    // with no room for the toolbox the menu adds the nodes
                    if !show_toolbox {
                        bar.group(&t!("settings.patterns.toolbar_nodes"), |bar| {
                            let missing = self.patterns_editor.missing_output_kinds();
                            egui_panels::menu_button(
                                bar.ui(),
                                t!("settings.patterns.menu_node"),
                                Variant::Primary,
                                |ui| {
                                    ui.menu_button(t!("settings.patterns.menu_detection"), |ui| {
                                        for type_name in DETECTION_TYPES {
                                            if ui.button(detection_type_label(type_name)).clicked()
                                            {
                                                self.patterns_editor.add_detection_of(type_name);
                                                ui.close();
                                            }
                                        }
                                    });
                                    ui.menu_button(t!("settings.patterns.menu_logic"), |ui| {
                                        if ui
                                            .button(t!("settings.patterns.kind_aggregator"))
                                            .clicked()
                                        {
                                            self.patterns_editor.add_aggregator();
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui
                                            .button(t!("settings.patterns.kind_formatter"))
                                            .clicked()
                                        {
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
                        });
                    }
                });
            });
        if let Some(node_id) = selected.filter(|_| editable) {
            if duplicate {
                self.patterns_editor.duplicate_node(node_id);
            }
            if delete {
                self.patterns_editor.snarl.remove_node(node_id);
                self.patterns_editor.state.selected = None;
                self.patterns_editor.state.dirty = true;
            }
        }
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(12, 0))
            .show(ui, |ui| {
                for error in &self.patterns_editor.errors {
                    egui_panels::status(ui, StatusKind::Error, error);
                }
            });

        // The toolbox and, when there is room, the panel of the selected node
        // are side panels the user can resize; the canvas takes what is left.
        let show_inspector = ui.available_width() >= layout.inspector_min_viewport
            && !self.patterns_editor.state.inspector_collapsed;
        let graph_id = egui::Id::unique("rules_graph");
        let snarl_selection = egui_snarl::ui::get_selected_nodes(graph_id, ui.ctx());
        let show_ids = self.node_ids_visible();
        let graph_ids: HashSet<String> = self
            .patterns_editor
            .graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        // The status bar: whether the graph is valid.
        egui::Panel::bottom("rules_editor_status")
            .resizable(false)
            .show(ui, |ui| {
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
                )
                .on_hover_text(t!("settings.patterns.editor_hint"));
            });
        if show_toolbox {
            // The panel never gets narrower than its widest entry.
            let toolbox = self.toolbox();
            let frame = egui::Frame::side_top_panel(ui.style());
            let min_width = toolbox.min_width(ui) + frame.total_margin().sum().x;
            egui::Panel::left("rules_toolbox_panel")
                .frame(frame)
                .resizable(true)
                .default_size(layout.toolbox_width.max(min_width))
                .min_size(min_width)
                .max_size(layout.toolbox_max_width.max(min_width))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("rules_toolbox")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.show_toolbox(ui, toolbox));
                });
        }
        if show_inspector {
            egui::Panel::right("rules_inspector_panel")
                .resizable(true)
                .default_size(layout.inspector_width)
                .min_size(200.0)
                .max_size(520.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("rules_inspector")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let selected = self.patterns_editor.state.selected;
                            self.show_inspector(ui, selected);
                        });
                });
        }
        let central = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let canvas = ui.max_rect();
                let PatternsEditor { snarl, state, .. } = &mut self.patterns_editor;
                let mut viewer = RulesViewer {
                    state: &mut *state,
                    show_ids,
                    graph_ids,
                };
                SnarlWidget::new()
                    .id(graph_id)
                    .style(graph_style(ui))
                    .min_size(canvas.size())
                    .show(snarl, &mut viewer, ui);
                canvas
            });
        // A node dragged from the toolbox is added when it is dropped here.
        let canvas_response = central.response;
        if Toolbox::<ToolboxItem>::hovered(&canvas_response) {
            ui.painter().rect_stroke(
                central.inner.shrink(1.0),
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        if let Some(item) = Toolbox::<ToolboxItem>::dropped(&canvas_response) {
            let at = ui
                .ctx()
                .pointer_latest_pos()
                .and_then(|pointer| egui_snarl::ui::screen_to_graph(graph_id, ui.ctx(), pointer));
            self.patterns_editor.add_toolbox_item(item, at);
        }
        // A Shift click or a rectangle selects in the canvas itself: follow it.
        let now = egui_snarl::ui::get_selected_nodes(graph_id, ui.ctx());
        if now != snarl_selection {
            self.patterns_editor.state.selected = now.first().copied();
        }
        // A click selects after the panel was drawn: draw it again.
        if self.patterns_editor.state.selected != selected_before {
            ui.ctx().request_repaint();
        }
        if close {
            self.patterns_editor.close_editor();
        }
    }

    /// Whether the node editor shows the node ids, which are internal. Only
    /// the Debug window can turn it on, so a release build never does.
    fn node_ids_visible(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            self.debug.show_node_ids
        }
        #[cfg(not(debug_assertions))]
        {
            false
        }
    }

    /// The toolbox of the graph editor: the nodes that can be added to the
    /// rule, in groups.
    fn toolbox(&self) -> Toolbox<ToolboxItem> {
        let missing = self.patterns_editor.missing_output_kinds();
        let present = t!("settings.patterns.toolbox_output_present").into_owned();
        let entry = |item: ToolboxItem| ToolboxEntry::new(item, item.label()).color(item.color());
        let detections: Vec<_> = DETECTION_TYPES
            .iter()
            .map(|&type_name| entry(ToolboxItem::Detection(type_name)))
            .collect();
        let logic = [ToolboxItem::Aggregator, ToolboxItem::Formatter].map(entry);
        let outputs: Vec<_> = OUTPUT_KINDS
            .iter()
            .map(|&kind| {
                entry(ToolboxItem::Output(kind))
                    .enabled(missing.contains(&kind))
                    .disabled_hint(present.clone())
            })
            .collect();
        Toolbox::new("rules_toolbox")
            .group(
                t!("settings.patterns.toolbox_detections").into_owned(),
                detections,
            )
            .group(t!("settings.patterns.toolbox_logic").into_owned(), logic)
            .group(
                t!("settings.patterns.toolbox_outputs").into_owned(),
                outputs,
            )
    }

    /// The panel at the left of the graph editor: its title and `toolbox`. A
    /// click adds the node and selects it; dragging one onto the canvas does
    /// the same (see `show_editor`).
    fn show_toolbox(&mut self, ui: &mut egui::Ui, toolbox: Toolbox<ToolboxItem>) {
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new(t!("settings.patterns.toolbox"))
                .size(theme.section_title_size + 1.0)
                .color(palette.strong_text),
        );
        ui.label(
            RichText::new(t!("settings.patterns.toolbox_hint"))
                .size(theme.small_size)
                .color(palette.muted_text),
        );
        egui_panels::divider(ui);
        if let Some(item) = toolbox.show(ui) {
            self.patterns_editor.add_toolbox_item(item, None);
        }
    }

    /// The properties panel at the right of the graph editor, like the
    /// property window of an IDE: what the selected node is, its properties
    /// in grids of `name | value` rows grouped by category (General,
    /// Parameters, Connections) and the actions on it.
    fn show_inspector(&mut self, ui: &mut egui::Ui, selected: Option<NodeId>) {
        let show_ids = self.node_ids_visible();
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        ui.spacing_mut().item_spacing.y = 10.0;
        ui.label(
            RichText::new(t!("settings.patterns.properties"))
                .size(theme.section_title_size + 1.0)
                .color(palette.strong_text),
        );
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
        let name_width = self.patterns_editor.state.layout.name_width;
        let mut changed = false;
        let mut delete = false;
        let mut duplicate = false;
        // The input is the root of its rule: it is created and removed with
        // the rule on the Rules page, so the panel only edits what is left.
        let is_input = matches!(self.patterns_editor.snarl[node_id].kind, NodeKind::Input(_));
        let mut connections = Vec::new();
        {
            let snarl = &self.patterns_editor.snarl;
            for output in 0..usize::from(has_output_pin(&snarl[node_id].kind)) {
                let pin = snarl.out_pin(OutPinId {
                    node: node_id,
                    output,
                });
                let targets: Vec<String> = pin
                    .remotes
                    .iter()
                    .filter_map(|remote| snarl.get_node(remote.node))
                    .map(|node| {
                        if show_ids {
                            node.id.clone()
                        } else {
                            node_kind_label(&node.kind)
                        }
                    })
                    .collect();
                connections.push((output, targets));
            }
        }
        {
            let node = &mut self.patterns_editor.snarl[node_id];
            // The object the grids below describe: its mark and what it is.
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
            let properties = NodeProperties::new(show_ids, is_input);
            let mut model = properties.model(node, &connections);
            let edited = PropertyGrid::new("rules_properties")
                .name_width(name_width)
                .empty_hint(t!("settings.patterns.properties_hint"))
                .show_model(ui, &mut model);
            if !edited.is_empty() {
                properties.apply(&model, node);
                changed = true;
            }
        }
        if !is_input {
            egui_panels::divider(ui);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                duplicate =
                    egui_panels::button(ui, t!("settings.patterns.duplicate_node"), Variant::Ghost)
                        .clicked();
                delete =
                    egui_panels::button(ui, t!("settings.patterns.delete_node"), Variant::Ghost)
                        .clicked();
            });
        }
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

/// A node the toolbox offers.
#[derive(Clone, Copy, PartialEq)]
enum ToolboxItem {
    /// A detection of the type named (see [`DETECTION_TYPES`]).
    Detection(&'static str),
    Aggregator,
    Formatter,
    Output(OutputKind),
}

impl ToolboxItem {
    /// A node of this kind, to ask the labels and colors of the nodes for.
    fn sample(self) -> NodeKind {
        match self {
            Self::Detection(type_name) => NodeKind::Detection(DetectionNode {
                kind: default_detection_kind(type_name),
                case_insensitive: false,
                source: DetectionSource::Auto,
                mode: DetectionMode::Require,
            }),
            Self::Aggregator => NodeKind::Aggregator(AggregatorNode { inputs: 1 }),
            Self::Formatter => NodeKind::Formatter(FormatterNode {
                template: String::new(),
                inputs: 1,
            }),
            Self::Output(kind) => NodeKind::Output(OutputNode {
                kind,
                tooltip: TooltipConfig { emojis: true },
                log: LogConfig::default(),
            }),
        }
    }

    /// What the node is called, as on the canvas.
    fn label(self) -> String {
        node_kind_label(&self.sample())
    }

    /// The mark of the node's group, as on the canvas.
    fn color(self) -> Color32 {
        node_kind_color(&self.sample())
    }
}

/// The mark and the label of a node kind, in the node headers and the panel.
/// The kinds of source an Input node can read, in the order they are offered.
const INPUT_KINDS: [InputKind; 2] = [InputKind::ChatLog, InputKind::ZKillboard];

/// How the UI names a kind of input.
trait InputKindLabel {
    /// The name the source selector shows.
    fn label(&self) -> String;
}

impl InputKindLabel for InputKind {
    fn label(&self) -> String {
        match self {
            Self::ChatLog => t!("settings.patterns.input_chat_log").into_owned(),
            Self::ZKillboard => t!("settings.patterns.input_zkillboard").into_owned(),
        }
    }
}

/// What an Input node reads, for the one line that sums it up (its card and
/// its body on the canvas): the channels of a chat log, or the name of a feed
/// that has none.
#[derive(Debug, PartialEq, Eq)]
enum InputSource {
    /// Chat logs; empty means every watched channel.
    Channels(Vec<String>),
    /// A feed of the given kind.
    Feed(InputKind),
}

impl Default for InputSource {
    fn default() -> Self {
        Self::Channels(Vec::new())
    }
}

impl InputSource {
    fn of(input: &InputNode) -> Self {
        match input.kind {
            InputKind::ChatLog => Self::Channels(input.channels.clone()),
            kind => Self::Feed(kind),
        }
    }
}

impl std::fmt::Display for InputSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Channels(channels) if channels.is_empty() => {
                f.write_str(&t!("settings.patterns.all_channels"))
            }
            Self::Channels(channels) => f.write_str(&channels.join(", ")),
            Self::Feed(kind) => f.write_str(&kind.label()),
        }
    }
}

fn node_kind_color(kind: &NodeKind) -> Color32 {
    match kind {
        NodeKind::Input(_) => Color32::from_rgb(90, 160, 220),
        NodeKind::Detection(_) => Color32::from_rgb(110, 190, 120),
        NodeKind::Aggregator(_) | NodeKind::Formatter(_) => Color32::from_rgb(225, 170, 80),
        NodeKind::Output(_) => Color32::from_rgb(200, 125, 200),
    }
}

fn node_kind_label(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Input(_) => t!("settings.patterns.node_input").into_owned(),
        NodeKind::Detection(detection) => detection_type_label(detection.kind.type_name()),
        NodeKind::Output(output) => output_kind_label(output.kind),
        NodeKind::Aggregator(_) => t!("settings.patterns.kind_aggregator").into_owned(),
        NodeKind::Formatter(_) => t!("settings.patterns.kind_formatter").into_owned(),
    }
}

/// One line saying what a node does, for its body on the canvas.
fn node_summary(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Input(input) => InputSource::of(input).to_string(),
        NodeKind::Detection(detection) => {
            let summary = detection_summary(&detection.kind);
            match detection.mode {
                DetectionMode::Require => summary,
                DetectionMode::Exclude => {
                    format!("{}: {summary}", t!("settings.patterns.mode_exclude"))
                }
            }
        }
        NodeKind::Output(_) => String::new(),
        NodeKind::Aggregator(aggregator) => {
            t!("settings.patterns.field_inputs").into_owned() + &format!(": {}", aggregator.inputs)
        }
        NodeKind::Formatter(formatter) => formatter.template.clone(),
    }
}

fn detection_summary(kind: &DetectionRuleKind) -> String {
    match kind {
        DetectionRuleKind::ClearReport { keywords }
        | DetectionRuleKind::Query { keywords }
        | DetectionRuleKind::Keyword { keywords } => keywords.join(" · "),
        DetectionRuleKind::ShipNames { dictionaries, .. } => dictionaries.join(" · "),
        DetectionRuleKind::Custom { pattern, words, .. } => {
            pattern.clone().unwrap_or_else(|| words.join(" · "))
        }
        DetectionRuleKind::PilotCount => String::new(),
        DetectionRuleKind::SystemReport { ignore_words } => ignore_words.join(" · "),
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
}

/// The row of an input rule: a dot that says whether it is on, its
/// description, what it is made of and the outputs it reaches. A click
/// selects it and a double click opens its editor; the description is
/// edited in place and the row has its own switch and remove button. The
/// `id` is internal: it is only given (and shown) with the Debug option.
fn input_card(
    ui: &mut egui::Ui,
    index: usize,
    id: Option<&str>,
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
            // Salted with the row's position, not the entry's id.
            egui::UiBuilder::new()
                .id_salt(("input_card", index))
                .sense(egui::Sense::click()),
            |ui| {
                frame.show(ui, |ui| {
                    let width = ui.available_width();
                    ui.set_width(width);
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
                        let text_width = width - 8.0 - 14.0;
                        ui.vertical(|ui| {
                            ui.set_width(text_width);
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 9.0;
                                if let Some(id) = id {
                                    ui.label(
                                        RichText::new(id)
                                            .size(theme.section_title_size)
                                            .color(palette.strong_text),
                                    );
                                }
                                let mut text = description.to_string();
                                // The hint is also the control's accessible name.
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut text)
                                            .hint_text(t!("settings.patterns.field_description"))
                                            .desired_width(240.0),
                                    )
                                    .changed()
                                {
                                    actions.push(CardAction::Describe(text));
                                }
                            });
                            ui.label(
                                RichText::new(t!(
                                    "settings.patterns.summary",
                                    channels = summary.source.to_string(),
                                    detections = summary.detections,
                                    logic = summary.logic,
                                    outputs = summary.outputs.len()
                                ))
                                .size(theme.small_size)
                                .color(palette.muted_text),
                            );
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let mut on = enabled;
                                let switch = egui_panels::switch(ui, &mut on);
                                if switch.changed() {
                                    actions.push(CardAction::Enable(on));
                                }
                                let state = ui.label(
                                    RichText::new(if enabled {
                                        t!("settings.patterns.state_on")
                                    } else {
                                        t!("settings.patterns.state_off")
                                    })
                                    .size(theme.small_size)
                                    .color(palette.muted_text),
                                );
                                // The state text is the switch's accessible name.
                                let _ = switch.labelled_by(state.id);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if egui_panels::button(
                                            ui,
                                            t!("settings.patterns.remove_input"),
                                            Variant::Ghost,
                                        )
                                        .clicked()
                                        {
                                            actions.push(CardAction::Remove);
                                        }
                                    },
                                );
                            });
                        });
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
    /// Show each node's internal id in its header (a Debug window option).
    show_ids: bool,
    /// The ids of the whole working graph: the canvas shows only one rule, but
    /// the ids of the others must not be reused.
    graph_ids: HashSet<String>,
}

impl RulesViewer<'_> {
    /// An id that no node of the graph or of the canvas has.
    fn fresh_id(&self, snarl: &Snarl<Node>, prefix: &str) -> String {
        let mut taken = collect_node_ids(snarl);
        taken.extend(self.graph_ids.iter().cloned());
        unique_id(&taken, prefix)
    }

    /// Selects `node` when `rect` (a part of it) is clicked: the canvas only
    /// selects with Shift or a rectangle, which is not how anyone expects a
    /// click on a node to work.
    fn select_on_click(&mut self, ui: &mut Ui, node: NodeId, rect: egui::Rect) {
        let id = ui.scope_id().with(("select_node", node));
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
            NodeKind::Formatter(_) => t!("settings.patterns.kind_formatter").into_owned(),
        }
    }

    fn inputs(&mut self, node: &Node) -> usize {
        input_pin_count(&node.kind)
    }

    fn outputs(&mut self, node: &Node) -> usize {
        usize::from(has_output_pin(&node.kind))
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
            NodeKind::Aggregator(_) | NodeKind::Formatter(_) => {}
        }
        PinInfo::circle().with_fill(Color32::LIGHT_GREEN)
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<Node>,
    ) -> impl SnarlPin + 'static {
        match &snarl[pin.id.node].kind {
            NodeKind::Input(_) => {
                ui.label(format!(
                    "{} {PIN_ARROW}",
                    t!("settings.patterns.node_input")
                ));
            }
            NodeKind::Output(_) => {}
            NodeKind::Detection(_) | NodeKind::Aggregator(_) | NodeKind::Formatter(_) => {
                ui.label(format!(
                    "{} {PIN_ARROW}",
                    t!("settings.patterns.node_output")
                ));
            }
        }
        PinInfo::circle().with_fill(Color32::LIGHT_BLUE)
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
            if self.show_ids {
                ui.label(RichText::new(&node.id).strong());
                ui.label(RichText::new(self.title(node)).small().weak());
            } else {
                ui.label(RichText::new(self.title(node)).strong());
            }
        });
        self.select_on_click(ui, node_id, row.response.rect);
    }

    fn has_body(&mut self, node: &Node) -> bool {
        !node_summary(&node.kind).is_empty()
            || !node.enabled
            || matches!(&node.kind, NodeKind::Detection(detection) if detection_reads(detection).is_some())
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
            if let NodeKind::Detection(detection) = &node.kind
                && let Some(reads) = detection_reads(detection)
            {
                ui.add(egui::Label::new(RichText::new(reads).small().weak()).truncate());
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
                    let id = self.fresh_id(snarl, "detection");
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
                                source: DetectionSource::Auto,
                                mode: DetectionMode::Require,
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
                let id = self.fresh_id(snarl, "aggregator");
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
            if ui.button(t!("settings.patterns.kind_formatter")).clicked() {
                let id = self.fresh_id(snarl, "formatter");
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
        // An input pin takes a single cable: a new one replaces the old. An
        // output takes the cables of any number of rules.
        let is_output = snarl
            .get_node(to.id.node)
            .is_some_and(|node| matches!(node.kind, NodeKind::Output(_)));
        if !is_output {
            snarl.drop_inputs(to.id);
        }
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

fn detection_mode_label(mode: DetectionMode) -> String {
    match mode {
        DetectionMode::Require => t!("settings.patterns.mode_require").into_owned(),
        DetectionMode::Exclude => t!("settings.patterns.mode_exclude").into_owned(),
    }
}

fn detection_source_label(source: DetectionSource) -> String {
    match source {
        DetectionSource::Auto => t!("settings.patterns.source_auto").into_owned(),
        DetectionSource::Facts => t!("settings.patterns.source_facts").into_owned(),
        DetectionSource::Text => t!("settings.patterns.source_text").into_owned(),
    }
}

/// What a detection reads, for the node, if its type can read facts at all
/// (the others only ever read text, so there is nothing to tell).
fn detection_reads(detection: &DetectionNode) -> Option<String> {
    detection.kind.fact()?;
    Some(format!(
        "{}: {}",
        t!("settings.patterns.field_reads"),
        detection_source_label(detection.source)
    ))
}

fn default_detection_kind(name: &str) -> DetectionRuleKind {
    match name {
        "system_report" => DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        },
        "clear_report" => DetectionRuleKind::ClearReport {
            keywords: vec![String::from("clear"), String::from("clr")],
        },
        "ship_names" => DetectionRuleKind::ShipNames {
            dictionaries: Dictionaries::defaults().names(),
            sum_quantities: false,
        },
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
                kind: DetectionRuleKind::SystemReport {
                    ignore_words: Vec::new(),
                },
                case_insensitive: false,
                source: DetectionSource::Auto,
                mode: DetectionMode::Require,
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

    fn edge(from: &str, to: &str, to_pin: u8) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            to_pin,
        }
    }

    /// `in1 -> d1 -> visual`, `d1 -> log`; `in2 -> d2 -T-> visual` (the
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
                edge("in1", "d1", 0),
                edge("d1", "visual", 0),
                edge("d1", "log", 0),
                edge("in2", "d2", 0),
                edge("d2", "visual", 0),
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
            DetectionRuleKind::ShipNames { dictionaries, .. } => {
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
    fn an_excluding_detection_says_so_in_its_summary() {
        let summary = |mode| {
            node_summary(&NodeKind::Detection(DetectionNode {
                kind: DetectionRuleKind::Query {
                    keywords: vec![String::from("status")],
                },
                case_insensitive: false,
                source: DetectionSource::Auto,
                mode,
            }))
        };
        assert_eq!(summary(DetectionMode::Require), "status");
        assert!(summary(DetectionMode::Exclude).ends_with(": status"));
        assert_ne!(summary(DetectionMode::Exclude), "status");
    }

    #[test]
    fn node_colors_group_the_special_nodes() {
        let formatter = node(
            "f",
            NodeKind::Formatter(FormatterNode {
                template: String::from("{all}"),
                inputs: 2,
            }),
        );
        let aggregator = node("a", NodeKind::Aggregator(AggregatorNode { inputs: 2 }));
        assert_eq!(
            node_kind_color(&formatter.kind),
            node_kind_color(&aggregator.kind)
        );
        assert_ne!(
            node_kind_color(&input("i", &[]).kind),
            node_kind_color(&detection("d").kind)
        );
    }

    #[test]
    fn a_detection_that_can_read_facts_says_what_it_reads() {
        let detection = |kind, source| DetectionNode {
            kind,
            case_insensitive: false,
            source,
            mode: DetectionMode::Require,
        };
        let reads = |kind, source| detection_reads(&detection(kind, source));
        let system = || DetectionRuleKind::SystemReport {
            ignore_words: Vec::new(),
        };
        let auto = reads(system(), DetectionSource::Auto).unwrap();
        let facts = reads(system(), DetectionSource::Facts).unwrap();
        let text = reads(system(), DetectionSource::Text).unwrap();
        assert!(auto != facts && facts != text && auto != text);
        // The rest only ever read text: nothing to tell.
        let keyword = DetectionRuleKind::Keyword {
            keywords: vec![String::from("hostile")],
        };
        assert_eq!(reads(keyword, DetectionSource::Auto), None);
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
                source: DetectionSource::Auto,
                mode: DetectionMode::Require,
            }))
        };
        assert_eq!(summary(keywords(&["a", "b"])), "a \u{b7} b");
        assert_eq!(
            summary(DetectionRuleKind::SystemReport {
                ignore_words: Vec::new()
            }),
            ""
        );
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
        editor.add_input(InputKind::ChatLog);
        editor.ensure_loaded(&RuleGraph::default());
        assert_eq!(editor.graph.nodes.len(), graph().nodes.len() + 1);
    }

    #[test]
    fn reset_reloads_and_drops_all_editing_state() {
        let mut editor = editor();
        editor.open_editor("in1");
        editor.selected_input = Some(String::from("in2"));
        editor.set_errors(vec![String::from("boom")]);
        editor.window_place = Some(Pos2::new(10.0, 20.0));
        editor.bring_to_front();

        editor.reset(&RuleGraph::default());

        assert!(editor.graph.nodes.is_empty());
        assert!(!editor.is_open());
        assert!(editor.selected_input.is_none());
        assert!(editor.errors.is_empty());
        assert!(editor.original_ids.is_empty());
        assert!(!editor.state.dirty);
        assert!(editor.window_place.is_none());
        assert!(!editor.to_front);
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
        let first = editor.add_input(InputKind::ChatLog);
        let second = editor.add_input(InputKind::ChatLog);
        assert_eq!((first.as_str(), second.as_str()), ("input_1", "input_2"));
        assert!(editor.state.dirty);
        let (description, enabled) = editor.input_summary("input_1").unwrap();
        assert_eq!(description, InputKind::ChatLog.label());
        assert!(enabled);
    }

    #[test]
    fn add_input_creates_the_kind_that_was_chosen() {
        let mut editor = editor();
        let id = editor.add_input(InputKind::ZKillboard);
        let node = editor
            .graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap();
        let NodeKind::Input(input) = &node.kind else {
            panic!("an input was expected");
        };
        assert_eq!(input.kind, InputKind::ZKillboard);
        assert_eq!(input.description, InputKind::ZKillboard.label());
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
        editor.graph.edges.push(edge("in3", "d1", 1));
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
    fn a_feed_input_is_summed_up_by_its_name_not_by_channels() {
        let mut editor = editor();
        let set_kind = |editor: &mut PatternsEditor, kind: InputKind| {
            for node in &mut editor.graph.nodes {
                if node.id == "in1"
                    && let NodeKind::Input(input) = &mut node.kind
                {
                    input.kind = kind;
                }
            }
        };
        // Channels that were set before the source changed mean nothing now.
        set_kind(&mut editor, InputKind::ZKillboard);

        let summary = editor.component_summary("in1");
        assert_eq!(summary.source, InputSource::Feed(InputKind::ZKillboard));
        assert_eq!(summary.source.to_string(), InputKind::ZKillboard.label());
        let node = editor.graph.nodes.iter().find(|node| node.id == "in1");
        assert_eq!(
            node_summary(&node.unwrap().kind),
            InputKind::ZKillboard.label()
        );

        // Back to chat logs: the channels are still there.
        set_kind(&mut editor, InputKind::ChatLog);
        assert_eq!(
            editor.component_summary("in1").source,
            InputSource::Channels(vec![String::from("Intel")])
        );
    }

    #[test]
    fn every_input_kind_is_offered_once() {
        assert!(INPUT_KINDS.contains(&InputKind::ChatLog));
        assert!(INPUT_KINDS.contains(&InputKind::ZKillboard));
        let labels: HashSet<String> = INPUT_KINDS.iter().map(InputKindLabel::label).collect();
        assert_eq!(labels.len(), INPUT_KINDS.len());
    }

    #[test]
    fn component_summary_counts_parts_and_orders_outputs() {
        let mut editor = editor();
        editor.graph.nodes.push(node(
            "union_1",
            NodeKind::Aggregator(AggregatorNode { inputs: 2 }),
        ));
        editor.graph.edges.push(edge("d1", "union_1", 0));

        let summary = editor.component_summary("in1");

        assert_eq!(
            summary.source,
            InputSource::Channels(vec![String::from("Intel")])
        );
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

        // Every node has a single output pin.
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
                (String::from("d1"), 0, String::from("log")),
                (String::from("d1"), 0, String::from("visual")),
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
        let key = |e: &Edge| (e.from.clone(), e.to.clone(), e.to_pin);
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
        assert!(editor.graph.edges.contains(&edge("d2", "visual", 0)));
        assert!(editor.graph.edges.contains(&edge("in2", "d2", 0)));
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
        assert!(editor.graph.edges.contains(&edge("in1", "detection_1", 0)));
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
        assert!(editor.graph.edges.contains(&edge("d2", "visual", 0)));
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
        editor.add_formatter();

        for id in ["aggregator_1", "aggregator_2", "formatter_1"] {
            assert!(editor.find_node(id).is_some(), "{id}");
        }
        assert_eq!(editor.aggregator_count(), 2);
        assert_eq!(editor.special_count(), 3);
        assert_eq!(editor.detection_count(), 0);
        // Each new one is placed below the previous ones.
        let y_of = |id: &str| {
            let node_id = editor.find_node(id).unwrap();
            editor.snarl.get_node_info(node_id).unwrap().pos.y
        };
        assert_eq!(y_of("aggregator_1"), 0.0);
        assert_eq!(y_of("aggregator_2"), 260.0);
        assert_eq!(y_of("formatter_1"), 520.0);
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
        editor.add_formatter();
        let kind = |id: &str| editor.snarl[editor.find_node(id).unwrap()].kind.clone();
        assert!(matches!(kind("aggregator_1"), NodeKind::Aggregator(a) if a.inputs == 4));
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

    #[test]
    fn toolbox_items_add_a_node_and_select_it() {
        let mut editor = editor();
        editor.open_editor("in1");
        let before = editor.snarl.node_ids().count();

        editor.add_toolbox_item(ToolboxItem::Aggregator, None);
        let selected = editor.state.selected.expect("the new node is selected");
        assert!(matches!(
            editor.snarl[selected].kind,
            NodeKind::Aggregator(_)
        ));

        editor.add_toolbox_item(ToolboxItem::Formatter, None);
        let selected = editor.state.selected.expect("the new node is selected");
        assert!(matches!(
            editor.snarl[selected].kind,
            NodeKind::Formatter(_)
        ));
        assert_eq!(editor.snarl.node_ids().count(), before + 2);
    }

    #[test]
    fn a_dropped_toolbox_item_stays_where_it_was_dropped() {
        let mut editor = editor();
        editor.open_editor("in1");

        editor.add_toolbox_item(ToolboxItem::Aggregator, Some(egui::pos2(123.0, 45.0)));

        let node = editor.state.selected.expect("the new node is selected");
        assert_eq!(
            editor.snarl.get_node_info(node).unwrap().pos,
            egui::pos2(123.0, 45.0)
        );
    }

    // Applying

    #[test]
    fn is_modified_needs_a_loaded_graph_that_differs_or_a_dirty_open_editor() {
        let live = graph();
        assert!(!PatternsEditor::default().is_modified(&live));

        let mut editor = editor();
        assert!(!editor.is_modified(&live));

        editor.add_input(InputKind::ChatLog);
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
