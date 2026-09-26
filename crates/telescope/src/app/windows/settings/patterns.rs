//! Settings -> Patterns: visual editor of the three-class intel rules
//! (input -> detection -> output).
//!
//! The graph is a view over a [`RulesConfig`]: each rule is a node and each
//! wire goes from a detection's tag output to an output's single input,
//! meaning "this output fires when this tag is present". Connecting a wire
//! rewrites the output's condition to `all_true` over its connected tags;
//! the full boolean condition (combinators, `not`, quantifiers) is edited in
//! the inspector below the graph.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use eframe::egui::{self, Color32, Pos2, RichText, Ui};
use egui_snarl::{
    InPin, InPinId, NodeId, OutPin, OutPinId, Snarl,
    ui::{PinInfo, SnarlPin, SnarlViewer, SnarlWidget},
};
use webb::patterns::IntelCategory;
use webb::rules::{
    Condition, DetectionKind, DetectionRule, InputKind, InputRule, OutputKind, OutputMode,
    OutputRule, RulesConfig, Tag,
};

/// A node of the rules graph.
enum RuleNode {
    Input(InputRule),
    Detection(DetectionRule),
    Output(OutputRule),
}

/// In-memory editing state of the Rules page.
#[derive(Default)]
pub(crate) struct PatternsEditor {
    snarl: Snarl<RuleNode>,
    state: GraphState,
    loaded: bool,
}

#[derive(Default)]
struct GraphState {
    dirty: bool,
    errors: Vec<String>,
    selected: Option<NodeId>,
}

impl PatternsEditor {
    fn ensure_loaded(&mut self, rules: &RulesConfig) {
        if self.loaded {
            return;
        }
        self.rebuild(rules);
        self.loaded = true;
    }

    fn rebuild(&mut self, rules: &RulesConfig) {
        self.snarl = Snarl::new();
        self.state.selected = None;
        self.state.dirty = false;

        let mut y = 0.0;
        for input in &rules.inputs {
            self.snarl
                .insert_node(Pos2::new(0.0, y), RuleNode::Input(input.clone()));
            y += 180.0;
        }
        y = 0.0;
        for detection in &rules.detections {
            self.snarl
                .insert_node(Pos2::new(360.0, y), RuleNode::Detection(detection.clone()));
            y += 260.0;
        }
        y = 0.0;
        for output in &rules.outputs {
            let node = self
                .snarl
                .insert_node(Pos2::new(780.0, y), RuleNode::Output(output.clone()));
            if let Condition::AllTrue { tags } = &output.when {
                for tag in tags {
                    if let Some((detection_node, pin)) = self.find_tag_output(tag) {
                        self.snarl.connect(
                            OutPinId {
                                node: detection_node,
                                output: pin,
                            },
                            InPinId { node, input: 0 },
                        );
                    }
                }
            }
            y += 260.0;
        }
    }

    fn find_tag_output(&self, tag: &str) -> Option<(NodeId, usize)> {
        for (id, node) in self.snarl.node_ids() {
            if let RuleNode::Detection(detection) = node
                && let Some(index) = detection.tags.iter().position(|known| known == tag)
            {
                return Some((id, index));
            }
        }
        None
    }

    fn to_config(&self) -> RulesConfig {
        let mut config = RulesConfig::default();
        for node in self.snarl.nodes() {
            match node {
                RuleNode::Input(input) => config.inputs.push(input.clone()),
                RuleNode::Detection(detection) => config.detections.push(detection.clone()),
                RuleNode::Output(output) => config.outputs.push(output.clone()),
            }
        }
        config
    }
}

/// File the rules are imported from / exported to (next to the app, same as
/// the legacy `patterns.toml`).
const RULES_FILE: &str = "rules.toml";

impl TelescopeApp {
    /// Draws the Rules page: the graph plus the inspector of the selected node.
    pub(crate) fn show_patterns_page(&mut self, ui: &mut egui::Ui) {
        let rules = self.intel_rules.clone();
        self.patterns_editor.ensure_loaded(&rules);

        ui.horizontal(|ui| {
            if ui.button(t!("settings.patterns.save")).clicked() {
                self.save_patterns();
            }
            if ui.button(t!("settings.patterns.discard")).clicked() {
                let rules = self.intel_rules.clone();
                self.patterns_editor.rebuild(&rules);
            }
            if ui.button(t!("settings.patterns.import")).clicked() {
                self.import_rules();
            }
            if ui.button(t!("settings.patterns.export")).clicked() {
                self.export_rules();
            }
            if self.patterns_editor.state.dirty {
                ui.colored_label(Color32::YELLOW, t!("settings.patterns.unsaved"));
            }
        });
        ui.label(t!("settings.patterns.graph_hint"));
        for error in &self.patterns_editor.state.errors {
            ui.colored_label(Color32::RED, error);
        }
        ui.separator();

        {
            let PatternsEditor { snarl, state, .. } = &mut self.patterns_editor;
            let mut viewer = RulesViewer { state };
            SnarlWidget::new()
                .id(egui::Id::new("rules_graph"))
                .min_size(egui::vec2(640.0, 380.0))
                .show(snarl, &mut viewer, ui);
        }

        let selected = self.patterns_editor.state.selected;
        if let Some(node) = selected {
            ui.separator();
            ui.label(RichText::new(t!("settings.patterns.selected")).strong());
            let mut changed = false;
            if let Some(RuleNode::Output(output)) = self.patterns_editor.snarl.get_node_mut(node) {
                ui.label(t!("settings.patterns.condition_help"));
                changed |= condition_editor(ui, &mut output.when);
            }
            if changed {
                self.patterns_editor.state.dirty = true;
            }
        }
    }

    /// Validates the edited rules, persists them to the database and reloads
    /// the live engine/router. On any error nothing is written and the errors
    /// are shown on the page.
    pub(crate) fn save_patterns(&mut self) {
        let rules = self.patterns_editor.to_config();
        let errors = rules.validate();
        if !errors.is_empty() {
            self.patterns_editor.state.errors =
                errors.iter().map(|error| error.to_string()).collect();
            return;
        }
        self.patterns_editor.state.errors.clear();
        self.patterns_editor.state.dirty = false;
        self.apply_rules(rules);
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("Intel"),
            String::from("save_rules"),
            String::from("rules were saved and the engine reloaded"),
        )));
    }

    /// Loads `rules.toml` (a serialized [`RulesConfig`]) into the editor.
    pub(crate) fn import_rules(&mut self) {
        let text = match std::fs::read_to_string(RULES_FILE) {
            Ok(text) => text,
            Err(error) => {
                self.patterns_editor.state.errors = vec![format!("cannot read {RULES_FILE}: {error}")];
                return;
            }
        };
        let rules = match toml::from_str::<RulesConfig>(&text) {
            Ok(rules) => rules,
            Err(error) => {
                self.patterns_editor.state.errors = vec![error.to_string()];
                return;
            }
        };
        let errors = rules.validate();
        if !errors.is_empty() {
            self.patterns_editor.state.errors =
                errors.iter().map(|error| error.to_string()).collect();
            return;
        }
        self.patterns_editor.state.errors.clear();
        self.patterns_editor.rebuild(&rules);
        self.patterns_editor.loaded = true;
        self.patterns_editor.state.dirty = true;
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("Intel"),
            String::from("import_rules"),
            format!("{RULES_FILE} was loaded into the editor; press Save rules to apply it"),
        )));
    }

    /// Writes the editor's current rules to `rules.toml`.
    pub(crate) fn export_rules(&mut self) {
        let rules = self.patterns_editor.to_config();
        let text = match toml::to_string(&rules) {
            Ok(text) => text,
            Err(error) => {
                self.patterns_editor.state.errors = vec![error.to_string()];
                return;
            }
        };
        if let Err(error) = std::fs::write(RULES_FILE, text) {
            self.patterns_editor.state.errors = vec![format!("cannot write {RULES_FILE}: {error}")];
            return;
        }
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("Intel"),
            String::from("export_rules"),
            format!("{RULES_FILE} was written"),
        )));
    }
}

struct RulesViewer<'a> {
    state: &'a mut GraphState,
}

impl SnarlViewer<RuleNode> for RulesViewer<'_> {
    fn title(&mut self, node: &RuleNode) -> String {
        match node {
            RuleNode::Input(input) => {
                format!("{} · {}", t!("settings.patterns.node_input"), input.id)
            }
            RuleNode::Detection(detection) => format!(
                "{} · {}",
                t!("settings.patterns.node_detection"),
                detection.id
            ),
            RuleNode::Output(output) => format!(
                "{} · {} · {}",
                t!("settings.patterns.node_output"),
                output.id,
                output_kind_label(output.kind)
            ),
        }
    }

    fn inputs(&mut self, node: &RuleNode) -> usize {
        match node {
            RuleNode::Output(_) => 1,
            _ => 0,
        }
    }

    fn outputs(&mut self, node: &RuleNode) -> usize {
        match node {
            RuleNode::Detection(detection) => detection.tags.len(),
            _ => 0,
        }
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut Ui,
        snarl: &mut Snarl<RuleNode>,
    ) -> impl SnarlPin + 'static {
        if let RuleNode::Output(output) = &snarl[pin.id.node] {
            ui.label(condition_summary(&output.when));
        }
        PinInfo::circle().with_fill(Color32::LIGHT_GREEN)
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<RuleNode>,
    ) -> impl SnarlPin + 'static {
        if let RuleNode::Detection(detection) = &snarl[pin.id.node] {
            let tag = detection
                .tags
                .get(pin.id.output)
                .cloned()
                .unwrap_or_default();
            ui.label(tag);
        }
        PinInfo::circle().with_fill(Color32::LIGHT_BLUE)
    }

    fn has_body(&mut self, _node: &RuleNode) -> bool {
        true
    }

    fn show_body(
        &mut self,
        node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<RuleNode>,
    ) {
        self.state.selected = Some(node);
        let mut changed = false;
        match &mut snarl[node] {
            RuleNode::Input(input) => changed |= input_body(ui, input),
            RuleNode::Detection(detection) => changed |= detection_body(ui, detection),
            RuleNode::Output(output) => changed |= output_body(ui, output),
        }
        if changed {
            self.state.dirty = true;
        }
    }

    fn has_graph_menu(&mut self, _pos: Pos2, _snarl: &mut Snarl<RuleNode>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: Pos2, ui: &mut Ui, snarl: &mut Snarl<RuleNode>) {
        if ui.button(t!("settings.patterns.add_input")).clicked() {
            snarl.insert_node(pos, RuleNode::Input(default_input()));
            self.state.dirty = true;
            ui.close();
        }
        if ui.button(t!("settings.patterns.add_detection")).clicked() {
            snarl.insert_node(pos, RuleNode::Detection(default_detection()));
            self.state.dirty = true;
            ui.close();
        }
        if ui.button(t!("settings.patterns.add_output")).clicked() {
            snarl.insert_node(pos, RuleNode::Output(default_output()));
            self.state.dirty = true;
            ui.close();
        }
    }

    fn has_node_menu(&mut self, _node: &RuleNode) -> bool {
        true
    }

    fn show_node_menu(
        &mut self,
        node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<RuleNode>,
    ) {
        if ui.button(t!("settings.patterns.delete_node")).clicked() {
            snarl.remove_node(node);
            if self.state.selected == Some(node) {
                self.state.selected = None;
            }
            self.state.dirty = true;
            ui.close();
        }
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<RuleNode>) {
        snarl.connect(from.id, to.id);
        sync_output_condition(to.id.node, snarl);
        self.state.dirty = true;
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<RuleNode>) {
        snarl.disconnect(from.id, to.id);
        sync_output_condition(to.id.node, snarl);
        self.state.dirty = true;
    }
}

/// Rewrites `output_node`'s condition as `all_true` over the tags of the
/// detection outputs wired into its input (or `always` when nothing is
/// connected).
fn sync_output_condition(output_node: NodeId, snarl: &mut Snarl<RuleNode>) {
    let tags: Vec<Tag> = snarl
        .wires()
        .filter(|(_, to)| to.node == output_node && to.input == 0)
        .filter_map(|(from, _)| match &snarl[from.node] {
            RuleNode::Detection(detection) => detection.tags.get(from.output).cloned(),
            _ => None,
        })
        .collect();
    if let RuleNode::Output(output) = &mut snarl[output_node] {
        output.when = if tags.is_empty() {
            Condition::Always
        } else {
            Condition::AllTrue { tags }
        };
    }
}

fn input_body(ui: &mut Ui, input: &mut InputRule) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.field_id"));
        changed |= ui.text_edit_singleline(&mut input.id).changed();
    });
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.field_path"));
        changed |= ui.text_edit_singleline(&mut input.path).changed();
    });
    ui.label(format!(
        "{}: {}",
        t!("settings.patterns.field_kind"),
        input_kind_label(input.kind)
    ));
    changed |= string_list_editor(
        ui,
        &t!("settings.patterns.field_channels"),
        &mut input.channels,
    );
    changed |= ui
        .checkbox(&mut input.enabled, t!("settings.patterns.field_enabled"))
        .changed();
    changed
}

fn detection_body(ui: &mut Ui, detection: &mut DetectionRule) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.field_id"));
        changed |= ui.text_edit_singleline(&mut detection.id).changed();
    });
    ui.horizontal(|ui| {
        let is_dictionary = matches!(detection.kind, DetectionKind::Dictionary { .. });
        if ui
            .selectable_label(!is_dictionary, t!("settings.patterns.kind_regex"))
            .clicked()
            && is_dictionary
        {
            detection.kind = DetectionKind::Regex {
                pattern: String::new(),
            };
            changed = true;
        }
        if ui
            .selectable_label(is_dictionary, t!("settings.patterns.kind_dictionary"))
            .clicked()
            && !is_dictionary
        {
            detection.kind = DetectionKind::Dictionary { words: Vec::new() };
            changed = true;
        }
    });
    match &mut detection.kind {
        DetectionKind::Regex { pattern } => {
            ui.horizontal(|ui| {
                ui.label(t!("settings.patterns.field_pattern"));
                changed |= ui.text_edit_singleline(pattern).changed();
            });
        }
        DetectionKind::Dictionary { words } => {
            changed |= string_list_editor(ui, &t!("settings.patterns.field_words"), words);
        }
    }
    changed |= ui
        .checkbox(
            &mut detection.case_insensitive,
            t!("settings.patterns.field_case_insensitive"),
        )
        .changed();
    changed |= string_list_editor(
        ui,
        &t!("settings.patterns.field_channels"),
        &mut detection.channels,
    );
    changed |= ui
        .checkbox(&mut detection.enabled, t!("settings.patterns.field_enabled"))
        .changed();
    changed |= ui
        .checkbox(&mut detection.drop, t!("settings.patterns.field_drop"))
        .changed();
    changed |= category_editor(ui, &mut detection.category);
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.field_system_group"));
        let mut group = detection.system_group.clone().unwrap_or_default();
        if ui.text_edit_singleline(&mut group).changed() {
            detection.system_group = if group.is_empty() { None } else { Some(group) };
            changed = true;
        }
    });
    changed |= string_list_editor(ui, &t!("settings.patterns.field_tags"), &mut detection.tags);
    changed
}

fn output_body(ui: &mut Ui, output: &mut OutputRule) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.field_id"));
        changed |= ui.text_edit_singleline(&mut output.id).changed();
    });
    changed |= output_kind_editor(ui, &mut output.kind);
    changed |= output_mode_editor(ui, &mut output.mode);
    changed |= string_list_editor(
        ui,
        &t!("settings.patterns.field_channels"),
        &mut output.channels,
    );
    changed |= ui
        .checkbox(&mut output.enabled, t!("settings.patterns.field_enabled"))
        .changed();
    ui.label(format!(
        "{}: {}",
        t!("settings.patterns.field_when"),
        condition_summary(&output.when)
    ));
    changed
}

fn string_list_editor(ui: &mut Ui, label: &str, values: &mut Vec<String>) -> bool {
    let mut text = values.join(", ");
    let changed = ui
        .horizontal(|ui| {
            ui.label(label);
            ui.text_edit_singleline(&mut text).changed()
        })
        .inner;
    if changed {
        *values = text
            .split(',')
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
    }
    changed
}

fn category_editor(ui: &mut Ui, category: &mut Option<IntelCategory>) -> bool {
    let mut changed = false;
    egui::ComboBox::from_label(t!("settings.patterns.field_category"))
        .selected_text(category_label(*category))
        .show_ui(ui, |ui| {
            for option in [
                None,
                Some(IntelCategory::Ship),
                Some(IntelCategory::Count),
                Some(IntelCategory::Clear),
                Some(IntelCategory::Keyword),
                Some(IntelCategory::Query),
            ] {
                if ui
                    .selectable_label(*category == option, category_label(option))
                    .clicked()
                {
                    *category = option;
                    changed = true;
                }
            }
        });
    changed
}

fn output_kind_editor(ui: &mut Ui, kind: &mut OutputKind) -> bool {
    let mut changed = false;
    egui::ComboBox::from_label(t!("settings.patterns.field_kind"))
        .selected_text(output_kind_label(*kind))
        .show_ui(ui, |ui| {
            for option in [OutputKind::Visual, OutputKind::Sound, OutputKind::Log] {
                if ui
                    .selectable_label(*kind == option, output_kind_label(option))
                    .clicked()
                {
                    *kind = option;
                    changed = true;
                }
            }
        });
    changed
}

fn output_mode_editor(ui: &mut Ui, mode: &mut OutputMode) -> bool {
    let mut changed = false;
    egui::ComboBox::from_label(t!("settings.patterns.field_mode"))
        .selected_text(output_mode_label(*mode))
        .show_ui(ui, |ui| {
            for option in [OutputMode::Emit, OutputMode::Suppress] {
                if ui
                    .selectable_label(*mode == option, output_mode_label(option))
                    .clicked()
                {
                    *mode = option;
                    changed = true;
                }
            }
        });
    changed
}

fn condition_editor(ui: &mut Ui, condition: &mut Condition) -> bool {
    let mut changed = false;
    let current = condition_op(condition);
    egui::ComboBox::from_label(t!("settings.patterns.field_condition"))
        .selected_text(op_label(current))
        .show_ui(ui, |ui| {
            for op in [
                "always",
                "all_true",
                "any_true",
                "none_true",
                "at_least_one_false",
                "at_least_n_true",
                "and",
                "or",
                "not",
            ] {
                if ui
                    .selectable_label(current == op, op_label(op))
                    .clicked()
                    && current != op
                {
                    *condition = default_condition(op);
                    changed = true;
                }
            }
        });
    match condition {
        Condition::Always => {}
        Condition::AllTrue { tags }
        | Condition::AnyTrue { tags }
        | Condition::NoneTrue { tags }
        | Condition::AtLeastOneFalse { tags } => {
            changed |= string_list_editor(ui, &t!("settings.patterns.field_tags"), tags);
        }
        Condition::AtLeastNTrue { n, tags } => {
            ui.horizontal(|ui| {
                ui.label("n");
                changed |= ui.add(egui::DragValue::new(n)).changed();
            });
            changed |= string_list_editor(ui, &t!("settings.patterns.field_tags"), tags);
        }
        Condition::And { all } => changed |= conditions_list_editor(ui, all),
        Condition::Or { any } => changed |= conditions_list_editor(ui, any),
        Condition::Not { not } => changed |= condition_editor(ui, not),
    }
    changed
}

fn conditions_list_editor(ui: &mut Ui, list: &mut Vec<Condition>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, child) in list.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= condition_editor(ui, child);
                if ui.button(t!("settings.patterns.remove")).clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        list.remove(index);
        changed = true;
    }
    if ui.button(t!("settings.patterns.add_subcondition")).clicked() {
        list.push(Condition::AllTrue { tags: Vec::new() });
        changed = true;
    }
    changed
}

fn condition_op(condition: &Condition) -> &'static str {
    match condition {
        Condition::Always => "always",
        Condition::AllTrue { .. } => "all_true",
        Condition::AnyTrue { .. } => "any_true",
        Condition::NoneTrue { .. } => "none_true",
        Condition::AtLeastOneFalse { .. } => "at_least_one_false",
        Condition::AtLeastNTrue { .. } => "at_least_n_true",
        Condition::And { .. } => "and",
        Condition::Or { .. } => "or",
        Condition::Not { .. } => "not",
    }
}

fn default_condition(op: &str) -> Condition {
    match op {
        "all_true" => Condition::AllTrue { tags: Vec::new() },
        "any_true" => Condition::AnyTrue { tags: Vec::new() },
        "none_true" => Condition::NoneTrue { tags: Vec::new() },
        "at_least_one_false" => Condition::AtLeastOneFalse { tags: Vec::new() },
        "at_least_n_true" => Condition::AtLeastNTrue {
            n: 1,
            tags: Vec::new(),
        },
        "and" => Condition::And { all: Vec::new() },
        "or" => Condition::Or { any: Vec::new() },
        "not" => Condition::Not {
            not: Box::new(Condition::Always),
        },
        _ => Condition::Always,
    }
}

fn condition_summary(condition: &Condition) -> String {
    let op = op_label(condition_op(condition));
    match condition {
        Condition::Always => op,
        Condition::AllTrue { tags }
        | Condition::AnyTrue { tags }
        | Condition::NoneTrue { tags }
        | Condition::AtLeastOneFalse { tags } => format!("{op} [{}]", tags.join(", ")),
        Condition::AtLeastNTrue { n, tags } => format!("{op}({n}) [{}]", tags.join(", ")),
        Condition::And { all } => format!("{op} ({})", all.len()),
        Condition::Or { any } => format!("{op} ({})", any.len()),
        Condition::Not { .. } => format!("{op} (...)"),
    }
}

fn op_label(op: &str) -> String {
    match op {
        "always" => t!("settings.patterns.op_always").into_owned(),
        "all_true" => t!("settings.patterns.op_all_true").into_owned(),
        "any_true" => t!("settings.patterns.op_any_true").into_owned(),
        "none_true" => t!("settings.patterns.op_none_true").into_owned(),
        "at_least_one_false" => t!("settings.patterns.op_at_least_one_false").into_owned(),
        "at_least_n_true" => t!("settings.patterns.op_at_least_n_true").into_owned(),
        "and" => t!("settings.patterns.op_and").into_owned(),
        "or" => t!("settings.patterns.op_or").into_owned(),
        "not" => t!("settings.patterns.op_not").into_owned(),
        other => other.to_string(),
    }
}

fn category_label(category: Option<IntelCategory>) -> String {
    match category {
        None => t!("settings.patterns.category_none").into_owned(),
        Some(IntelCategory::Ship) => t!("settings.patterns.category_ship").into_owned(),
        Some(IntelCategory::Count) => t!("settings.patterns.category_count").into_owned(),
        Some(IntelCategory::Clear) => t!("settings.patterns.category_clear").into_owned(),
        Some(IntelCategory::Keyword) => t!("settings.patterns.category_keyword").into_owned(),
        Some(IntelCategory::Query) => t!("settings.patterns.category_query").into_owned(),
    }
}

fn output_kind_label(kind: OutputKind) -> String {
    match kind {
        OutputKind::Visual => t!("settings.patterns.kind_visual").into_owned(),
        OutputKind::Sound => t!("settings.patterns.kind_sound").into_owned(),
        OutputKind::Log => t!("settings.patterns.kind_log").into_owned(),
    }
}

fn output_mode_label(mode: OutputMode) -> String {
    match mode {
        OutputMode::Emit => t!("settings.patterns.mode_emit").into_owned(),
        OutputMode::Suppress => t!("settings.patterns.mode_suppress").into_owned(),
    }
}

fn input_kind_label(kind: InputKind) -> String {
    match kind {
        InputKind::ChatLog => t!("settings.patterns.input_kind_chat_log").into_owned(),
    }
}

fn default_input() -> InputRule {
    InputRule {
        id: String::from("new_input"),
        kind: InputKind::ChatLog,
        path: String::new(),
        channels: Vec::new(),
        enabled: true,
    }
}

fn default_detection() -> DetectionRule {
    DetectionRule {
        id: String::from("new_detection"),
        kind: DetectionKind::Regex {
            pattern: String::new(),
        },
        case_insensitive: false,
        channels: Vec::new(),
        enabled: true,
        drop: false,
        category: None,
        system_group: None,
        tags: Vec::new(),
    }
}

fn default_output() -> OutputRule {
    OutputRule {
        id: String::from("new_output"),
        kind: OutputKind::Log,
        mode: OutputMode::Emit,
        when: Condition::Always,
        channels: Vec::new(),
        enabled: true,
    }
}
