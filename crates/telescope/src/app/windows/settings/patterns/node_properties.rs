//! The properties of a node of the rule graph as the model that the property
//! panel shows: [`NodeProperties::model`] reads them from the node and
//! [`NodeProperties::apply`] writes the edited ones back.

use super::{PIN_ARROW, detection_mode_label, detection_source_label};
use egui_ide::{
    PropertyDescriptor, PropertyId, PropertyKind, PropertyLabels, PropertyModel, PropertyValue,
};
use webb::graph::{DetectionMode, DetectionSource, Node, NodeKind};
use webb::intel::IntelCategory;
use webb::rules::{DetectionRuleKind, DetectionType, InputKind, OutputKind};

/// A property and the value the node has for it.
type Entry = (PropertyDescriptor, PropertyValue);

/// What the panel shows of the selected node, and how it is edited.
pub(super) struct NodeProperties {
    /// The internal id of the node is shown too (the Debug option).
    show_ids: bool,
    /// The node is the input of its rule, which is switched on and off on the
    /// Rules page.
    is_input: bool,
}

impl NodeProperties {
    /// The properties of a node that is, or is not, the input of its rule.
    pub(super) fn new(show_ids: bool, is_input: bool) -> Self {
        Self { show_ids, is_input }
    }

    /// The model of `node`: General, Parameters and, for what the node is
    /// wired to, Connections. A group with nothing in it is left out.
    pub(super) fn model(&self, node: &Node, connections: &[(usize, Vec<String>)]) -> PropertyModel {
        let mut model = PropertyModel::new().labels(PropertyLabels {
            ok: t!("settings.patterns.prop_ok").into_owned(),
            cancel: t!("settings.patterns.prop_cancel").into_owned(),
            items: t!("settings.patterns.prop_items").into_owned(),
            key: t!("settings.patterns.prop_key").into_owned(),
            value: t!("settings.patterns.prop_value").into_owned(),
            reset: t!("settings.patterns.prop_reset").into_owned(),
            yes: t!("settings.patterns.prop_true").into_owned(),
            no: t!("settings.patterns.prop_false").into_owned(),
            ..PropertyLabels::default()
        });
        let wired: Vec<Entry> = connections
            .iter()
            .enumerate()
            .map(|(position, (_, targets))| {
                let text = self.connection_text(targets);
                (
                    PropertyDescriptor::new(
                        format!("connection_{position}"),
                        PropertyKind::ReadOnly,
                    )
                    .label(PIN_ARROW)
                    .default_to(PropertyValue::Text(text.clone())),
                    PropertyValue::Text(text),
                )
            })
            .collect();
        let groups = [
            (t!("settings.patterns.prop_general"), self.general(node)),
            (
                t!("settings.patterns.prop_parameters"),
                self.parameters(node),
            ),
            (t!("settings.patterns.connections"), wired),
        ];
        for (title, entries) in groups {
            if entries.is_empty() {
                continue;
            }
            model.group(title, |group| {
                for (descriptor, _) in &entries {
                    group.property(descriptor.clone());
                }
            });
            for (descriptor, value) in entries {
                // The kind accepts the value it was built with.
                let _ = model.set(descriptor.id(), value);
            }
        }
        model
    }

    /// Writes the values of `model` into `node`.
    pub(super) fn apply(&self, model: &PropertyModel, node: &mut Node) {
        if !self.is_input {
            if let Some(enabled) = Self::flag(model, "enabled") {
                node.enabled = enabled;
            }
        }
        match &mut node.kind {
            NodeKind::Input(input) => {
                if input.kind == InputKind::ChatLog {
                    if let Some(channels) = Self::list(model, "channels") {
                        input.channels = channels;
                    }
                    if let Some(motd) = Self::flag(model, "exclude_motd") {
                        input.exclude_motd = motd;
                    }
                }
            }
            NodeKind::Detection(detection) => {
                self.apply_rule(model, &mut detection.kind);
                if let Some(index) = Self::choice(model, "reads") {
                    detection.source = DetectionSource::ALL
                        .get(index)
                        .copied()
                        .unwrap_or(detection.source);
                }
                if let Some(index) = Self::choice(model, "mode") {
                    detection.mode = DetectionMode::ALL
                        .get(index)
                        .copied()
                        .unwrap_or(detection.mode);
                }
                if let Some(flag) = Self::flag(model, "case_insensitive") {
                    detection.case_insensitive = flag;
                }
            }
            NodeKind::Output(output) => {
                if let Some(flag) = Self::flag(model, "emojis") {
                    output.tooltip.emojis = flag;
                }
                if let Some(flag) = Self::flag(model, "current_time") {
                    output.log.use_current_time = flag;
                }
            }
            NodeKind::Aggregator(aggregator) => {
                if let Some(inputs) = Self::pins(model) {
                    aggregator.inputs = inputs;
                }
            }
            NodeKind::Formatter(formatter) => {
                if let Some(template) = Self::text(model, "template") {
                    formatter.template = template;
                }
                if let Some(inputs) = Self::pins(model) {
                    formatter.inputs = inputs;
                }
            }
        }
    }

    /// What a node is wired to, as in "name ×2" when the same kind of node is
    /// there more than once.
    fn connection_text(&self, targets: &[String]) -> String {
        if targets.is_empty() {
            return t!("settings.patterns.not_connected").into_owned();
        }
        let mut counted: Vec<(&str, usize)> = Vec::new();
        for target in targets {
            match counted
                .iter_mut()
                .find(|(name, _)| *name == target.as_str())
            {
                Some((_, times)) => *times += 1,
                None => counted.push((target.as_str(), 1)),
            }
        }
        counted
            .iter()
            .map(|&(name, times)| {
                if times > 1 {
                    format!("{name} ×{times}")
                } else {
                    name.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The properties every node has.
    fn general(&self, node: &Node) -> Vec<Entry> {
        let mut entries = Vec::new();
        if self.show_ids {
            entries.push(Self::read_only(
                "id",
                "settings.patterns.field_id",
                node.id.clone(),
            ));
        }
        if !self.is_input {
            entries.push((
                PropertyDescriptor::new("enabled", PropertyKind::Bool)
                    .label(t!("settings.patterns.field_enabled"))
                    .description(t!("settings.patterns.desc_enabled"))
                    .default_to(PropertyValue::Bool(true)),
                PropertyValue::Bool(node.enabled),
            ));
        }
        entries
    }

    /// The properties of what the node is.
    fn parameters(&self, node: &Node) -> Vec<Entry> {
        let mut entries = Vec::new();
        match &node.kind {
            NodeKind::Input(input) => {
                entries.push(Self::read_only(
                    "description",
                    "settings.patterns.field_description",
                    input.description.clone(),
                ));
                match input.kind {
                    InputKind::ChatLog => {
                        entries.push((
                            PropertyDescriptor::new("channels", PropertyKind::TextList)
                                .label(t!("settings.patterns.field_channels")),
                            PropertyValue::TextList(input.channels.clone()),
                        ));
                        entries.push((
                            PropertyDescriptor::new("exclude_motd", PropertyKind::Bool)
                                .label(t!("settings.patterns.field_exclude_motd"))
                                .default_to(PropertyValue::Bool(true)),
                            PropertyValue::Bool(input.exclude_motd),
                        ));
                    }
                    InputKind::ZKillboard => {
                        entries.push(Self::read_only(
                            "hint",
                            "",
                            t!("settings.patterns.zkillboard_hint").into_owned(),
                        ));
                    }
                }
            }
            NodeKind::Detection(detection) => {
                self.rule_entries(&detection.kind, &mut entries);
                if detection.kind.fact().is_some() {
                    let sources: Vec<String> = DetectionSource::ALL
                        .iter()
                        .map(|source| detection_source_label(*source))
                        .collect();
                    entries.push((
                        PropertyDescriptor::new("reads", PropertyKind::Choice(sources))
                            .label(t!("settings.patterns.field_reads"))
                            .description(t!("settings.patterns.desc_reads"))
                            .default_to(PropertyValue::Choice(Self::position(
                                &DetectionSource::ALL,
                                DetectionSource::Auto,
                            ))),
                        PropertyValue::Choice(Self::position(
                            &DetectionSource::ALL,
                            detection.source,
                        )),
                    ));
                }
                let modes: Vec<String> = DetectionMode::ALL
                    .iter()
                    .map(|mode| detection_mode_label(*mode))
                    .collect();
                entries.push((
                    PropertyDescriptor::new("mode", PropertyKind::Choice(modes))
                        .label(t!("settings.patterns.field_mode"))
                        .description(t!("settings.patterns.desc_mode"))
                        .default_to(PropertyValue::Choice(Self::position(
                            &DetectionMode::ALL,
                            DetectionMode::Require,
                        ))),
                    PropertyValue::Choice(Self::position(&DetectionMode::ALL, detection.mode)),
                ));
                // Systems are always read whatever their case, so the switch has
                // no effect on that detection.
                if !matches!(detection.kind, DetectionRuleKind::SystemReport { .. }) {
                    entries.push((
                        PropertyDescriptor::new("case_insensitive", PropertyKind::Bool)
                            .label(t!("settings.patterns.field_case_insensitive"))
                            .description(t!("settings.patterns.desc_case_insensitive")),
                        PropertyValue::Bool(detection.case_insensitive),
                    ));
                }
            }
            NodeKind::Output(output) => {
                if output.kind == OutputKind::Tooltip {
                    entries.push((
                        PropertyDescriptor::new("emojis", PropertyKind::Bool)
                            .label(t!("settings.patterns.field_emojis"))
                            .default_to(PropertyValue::Bool(true)),
                        PropertyValue::Bool(output.tooltip.emojis),
                    ));
                }
                if output.kind == OutputKind::Log {
                    entries.push((
                        PropertyDescriptor::new("current_time", PropertyKind::Bool)
                            .label(t!("settings.patterns.field_current_time")),
                        PropertyValue::Bool(output.log.use_current_time),
                    ));
                }
            }
            NodeKind::Aggregator(aggregator) => {
                entries.push(Self::pins_entry(aggregator.inputs));
            }
            NodeKind::Formatter(formatter) => {
                entries.push((
                    PropertyDescriptor::new("template", PropertyKind::Text)
                        .label(t!("settings.patterns.field_template"))
                        .description(t!("settings.patterns.desc_template"))
                        .default_to(PropertyValue::Text("{all}".to_owned())),
                    PropertyValue::Text(formatter.template.clone()),
                ));
                entries.push(Self::pins_entry(formatter.inputs));
            }
        }
        entries
    }

    /// The parameters of the type of a detection.
    fn rule_entries(&self, kind: &DetectionRuleKind, entries: &mut Vec<Entry>) {
        match kind {
            DetectionRuleKind::PilotCount => {}
            DetectionRuleKind::SystemReport { ignore_words } => {
                entries.push((
                    PropertyDescriptor::new("ignore_words", PropertyKind::TextList)
                        .label(t!("settings.patterns.field_ignore_words")),
                    PropertyValue::TextList(ignore_words.clone()),
                ));
            }
            // Telescope defines these words: shown, not edited.
            DetectionRuleKind::ClearReport { keywords } | DetectionRuleKind::Query { keywords } => {
                entries.push(Self::read_only(
                    "keywords",
                    "settings.patterns.field_keywords",
                    keywords.join(", "),
                ));
            }
            DetectionRuleKind::Keyword { keywords } => {
                entries.push((
                    PropertyDescriptor::new("keywords", PropertyKind::TextList)
                        .label(t!("settings.patterns.field_keywords")),
                    PropertyValue::TextList(keywords.clone()),
                ));
            }
            DetectionRuleKind::ShipNames {
                dictionaries,
                sum_quantities,
            } => {
                let names: Vec<String> = dictionaries.iter().map(ToString::to_string).collect();
                entries.push(Self::read_only(
                    "dictionaries",
                    "settings.patterns.field_dictionaries",
                    names.join(", "),
                ));
                entries.push((
                    PropertyDescriptor::new("sum_quantities", PropertyKind::Bool)
                        .label(t!("settings.patterns.field_sum_quantities"))
                        .description(t!("settings.patterns.sum_quantities_hint")),
                    PropertyValue::Bool(*sum_quantities),
                ));
            }
            DetectionRuleKind::Custom {
                pattern,
                words,
                category,
                system_group,
            } => {
                entries.push((
                    PropertyDescriptor::new("custom_pattern", PropertyKind::Bool)
                        .label(t!("settings.patterns.field_custom_pattern")),
                    PropertyValue::Bool(pattern.is_some()),
                ));
                if let Some(pattern) = pattern {
                    entries.push((
                        PropertyDescriptor::new("pattern", PropertyKind::Text)
                            .label(t!("settings.patterns.field_pattern")),
                        PropertyValue::Text(pattern.clone()),
                    ));
                } else {
                    entries.push((
                        PropertyDescriptor::new("keywords", PropertyKind::TextList)
                            .label(t!("settings.patterns.field_keywords")),
                        PropertyValue::TextList(words.clone()),
                    ));
                }
                let labels: Vec<String> = Self::categories()
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect();
                let current = Self::categories()
                    .iter()
                    .position(|(value, _)| value == category)
                    .unwrap_or(0);
                entries.push((
                    PropertyDescriptor::new("category", PropertyKind::Choice(labels))
                        .label(t!("settings.patterns.field_category")),
                    PropertyValue::Choice(current),
                ));
                entries.push((
                    PropertyDescriptor::new("system_group", PropertyKind::Text)
                        .label(t!("settings.patterns.field_system_group")),
                    PropertyValue::Text(system_group.clone().unwrap_or_default()),
                ));
            }
        }
    }

    /// Writes the edited parameters of a detection type back.
    fn apply_rule(&self, model: &PropertyModel, kind: &mut DetectionRuleKind) {
        match kind {
            DetectionRuleKind::PilotCount
            | DetectionRuleKind::ClearReport { .. }
            | DetectionRuleKind::Query { .. } => {}
            DetectionRuleKind::SystemReport { ignore_words } => {
                if let Some(words) = Self::list(model, "ignore_words") {
                    *ignore_words = words;
                }
            }
            DetectionRuleKind::Keyword { keywords } => {
                if let Some(words) = Self::list(model, "keywords") {
                    *keywords = words;
                }
            }
            DetectionRuleKind::ShipNames { sum_quantities, .. } => {
                if let Some(flag) = Self::flag(model, "sum_quantities") {
                    *sum_quantities = flag;
                }
            }
            DetectionRuleKind::Custom {
                pattern,
                words,
                category,
                system_group,
            } => {
                if let Some(has_pattern) = Self::flag(model, "custom_pattern") {
                    if has_pattern {
                        let edited = Self::text(model, "pattern");
                        *pattern = Some(edited.or_else(|| pattern.take()).unwrap_or_default());
                    } else {
                        *pattern = None;
                    }
                }
                if pattern.is_none() {
                    if let Some(edited) = Self::list(model, "keywords") {
                        *words = edited;
                    }
                }
                if let Some(index) = Self::choice(model, "category") {
                    if let Some((value, _)) = Self::categories().into_iter().nth(index) {
                        *category = value;
                    }
                }
                if let Some(group) = Self::text(model, "system_group") {
                    *system_group = (!group.is_empty()).then_some(group);
                }
            }
        }
    }

    /// The categories a custom detection can give, none first.
    fn categories() -> Vec<(Option<IntelCategory>, String)> {
        vec![
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
        ]
    }

    /// A property shown and never edited. `label` is the key of its name, or
    /// the empty text for a row without one.
    fn read_only(id: &str, label: &str, text: String) -> Entry {
        let name = if label.is_empty() {
            String::new()
        } else {
            t!(label).into_owned()
        };
        (
            PropertyDescriptor::new(id, PropertyKind::ReadOnly)
                .label(name)
                .default_to(PropertyValue::Text(text.clone())),
            PropertyValue::Text(text),
        )
    }

    /// The number of input pins of an aggregator or a formatter. It has no
    /// default value to be marked against.
    fn pins_entry(inputs: u8) -> Entry {
        let current = i64::from(inputs);
        (
            PropertyDescriptor::new("inputs", PropertyKind::Int)
                .label(t!("settings.patterns.field_inputs"))
                .description(t!("settings.patterns.desc_inputs"))
                .range(1.0, 16.0)
                .default_to(PropertyValue::Int(current)),
            PropertyValue::Int(current),
        )
    }

    /// Where `wanted` is in `all`.
    fn position<T: PartialEq>(all: &[T], wanted: T) -> usize {
        all.iter().position(|item| *item == wanted).unwrap_or(0)
    }

    fn flag(model: &PropertyModel, id: &str) -> Option<bool> {
        model.get(&PropertyId::new(id))?.as_bool()
    }

    fn text(model: &PropertyModel, id: &str) -> Option<String> {
        model
            .get(&PropertyId::new(id))?
            .as_text()
            .map(str::to_owned)
    }

    fn list(model: &PropertyModel, id: &str) -> Option<Vec<String>> {
        model
            .get(&PropertyId::new(id))?
            .as_list()
            .map(<[String]>::to_vec)
    }

    fn choice(model: &PropertyModel, id: &str) -> Option<usize> {
        model.get(&PropertyId::new(id))?.as_choice()
    }

    fn pins(model: &PropertyModel) -> Option<u8> {
        let inputs = model.get(&PropertyId::new("inputs"))?.as_int()?;
        u8::try_from(inputs).ok()
    }
}
