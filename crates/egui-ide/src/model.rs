//! The data a [`PropertyGrid`](crate::PropertyGrid) shows, set up through an
//! API: what each property is, which value it has and whether it is valid.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// The name a property goes by in a [`PropertyModel`], unique in the model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertyId(String);

impl PropertyId {
    /// An id from its name.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The name of the id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PropertyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for PropertyId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// The value of a property.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// A line of text, also for a path.
    Text(String),
    /// A whole number.
    Int(i64),
    /// A number with decimals.
    Float(f64),
    /// A yes or a no.
    Bool(bool),
    /// The index of the chosen option of a [`PropertyKind::Choice`].
    Choice(usize),
    /// A list of texts, one value per entry.
    TextList(Vec<String>),
    /// A list of `key | value` pairs.
    KeyValue(Vec<(String, String)>),
    /// A color as red, green and blue.
    Color([u8; 3]),
}

impl PropertyValue {
    /// The name of the kind of value, for messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Bool(_) => "bool",
            Self::Choice(_) => "choice",
            Self::TextList(_) => "text list",
            Self::KeyValue(_) => "key/value list",
            Self::Color(_) => "color",
        }
    }

    /// The text, if this is [`Text`](Self::Text).
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The number, if this is [`Int`](Self::Int).
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(number) => Some(*number),
            _ => None,
        }
    }

    /// The number, if this is [`Float`](Self::Float).
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(number) => Some(*number),
            _ => None,
        }
    }

    /// The flag, if this is [`Bool`](Self::Bool).
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    /// The index, if this is [`Choice`](Self::Choice).
    #[must_use]
    pub fn as_choice(&self) -> Option<usize> {
        match self {
            Self::Choice(index) => Some(*index),
            _ => None,
        }
    }

    /// The entries, if this is [`TextList`](Self::TextList).
    #[must_use]
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Self::TextList(items) => Some(items),
            _ => None,
        }
    }

    /// The pairs, if this is [`KeyValue`](Self::KeyValue).
    #[must_use]
    pub fn as_pairs(&self) -> Option<&[(String, String)]> {
        match self {
            Self::KeyValue(pairs) => Some(pairs),
            _ => None,
        }
    }
}

impl fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Int(number) => write!(f, "{number}"),
            Self::Float(number) => write!(f, "{number}"),
            Self::Bool(flag) => write!(f, "{flag}"),
            Self::Choice(index) => write!(f, "{index}"),
            Self::TextList(items) => f.write_str(&items.join(", ")),
            Self::KeyValue(pairs) => {
                let pairs: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
                f.write_str(&pairs.join(", "))
            }
            Self::Color([r, g, b]) => write!(f, "#{r:02X}{g:02X}{b:02X}"),
        }
    }
}

/// What a property is, which decides the editor the grid gives it and the
/// [`PropertyValue`] it holds.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyKind {
    /// A line of text, holding [`PropertyValue::Text`].
    Text,
    /// The path of a file, holding [`PropertyValue::Text`]. The grid adds a
    /// button that opens the native file dialog.
    File,
    /// The path of a folder, holding [`PropertyValue::Text`]. The grid adds a
    /// button that opens the native folder dialog.
    Folder,
    /// A value that is shown and cannot be edited, of any type.
    ReadOnly,
    /// A whole number, holding [`PropertyValue::Int`].
    Int,
    /// A number with decimals, holding [`PropertyValue::Float`].
    Float,
    /// A yes or a no, holding [`PropertyValue::Bool`].
    Bool,
    /// One of these options, holding [`PropertyValue::Choice`].
    Choice(Vec<String>),
    /// A list of texts, holding [`PropertyValue::TextList`].
    TextList,
    /// A list of `key | value` pairs, holding [`PropertyValue::KeyValue`].
    KeyValue,
    /// A color, holding [`PropertyValue::Color`].
    Color,
    /// A date, holding [`PropertyValue::Text`] as `YYYY-MM-DD` (or empty).
    /// The grid shows it in a field that can be edited and a button that
    /// opens a calendar.
    Date,
    /// Several lines of text, holding [`PropertyValue::Text`].
    Multiline,
    /// A button with this text. It holds no value: a press is reported by
    /// [`PropertyModel::take_activated`].
    Action(String),
    /// A link with this text. It holds no value: a click is reported by
    /// [`PropertyModel::take_activated`].
    Link(String),
    /// A link to something else, such as a node, whose name is the text it
    /// holds as [`PropertyValue::Text`]. A click is reported by
    /// [`PropertyModel::take_activated`].
    Reference,
}

impl PropertyKind {
    /// The value a property of this kind has until it is given another.
    #[must_use]
    pub fn default_value(&self) -> PropertyValue {
        match self {
            Self::Text
            | Self::File
            | Self::Folder
            | Self::Date
            | Self::ReadOnly
            | Self::Multiline
            | Self::Action(_)
            | Self::Link(_)
            | Self::Reference => PropertyValue::Text(String::new()),
            Self::Int => PropertyValue::Int(0),
            Self::Float => PropertyValue::Float(0.0),
            Self::Bool => PropertyValue::Bool(false),
            Self::Choice(_) => PropertyValue::Choice(0),
            Self::TextList => PropertyValue::TextList(Vec::new()),
            Self::KeyValue => PropertyValue::KeyValue(Vec::new()),
            Self::Color => PropertyValue::Color([0, 0, 0]),
        }
    }

    /// Whether the value of a property of this kind is something the user
    /// edits, which a grid marks when it differs from the default and puts
    /// back with a reset button.
    #[must_use]
    pub fn holds_value(&self) -> bool {
        !matches!(
            self,
            Self::ReadOnly | Self::Action(_) | Self::Link(_) | Self::Reference
        )
    }

    /// Whether a property of this kind can hold `value`.
    #[must_use]
    pub fn accepts(&self, value: &PropertyValue) -> bool {
        matches!(
            (self, value),
            (Self::ReadOnly | Self::Action(_) | Self::Link(_), _)
                | (
                    Self::Text
                        | Self::File
                        | Self::Folder
                        | Self::Date
                        | Self::Multiline
                        | Self::Reference,
                    PropertyValue::Text(_),
                )
                | (Self::Int, PropertyValue::Int(_))
                | (Self::Float, PropertyValue::Float(_))
                | (Self::Bool, PropertyValue::Bool(_))
                | (Self::Choice(_), PropertyValue::Choice(_))
                | (Self::TextList, PropertyValue::TextList(_))
                | (Self::KeyValue, PropertyValue::KeyValue(_))
                | (Self::Color, PropertyValue::Color(_))
        )
    }
}

/// How serious a [`PropertyIssue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The value works but is probably not what was meant.
    Warning,
    /// The value is not valid.
    Error,
}

/// What a rule found wrong in a value, shown under it in the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyIssue {
    severity: Severity,
    message: String,
}

impl PropertyIssue {
    /// A value that is not valid.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    /// A value that is valid but suspicious.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    /// How serious the issue is.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// What is wrong, for the user.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Why a [`PropertyModel`] refused a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyError {
    /// The model has no property with this id.
    Unknown(PropertyId),
    /// The value is not of the type the property holds.
    WrongType {
        /// The property.
        id: PropertyId,
        /// The type the property holds.
        expected: &'static str,
        /// The type of the value given.
        found: &'static str,
    },
}

impl fmt::Display for PropertyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "there is no property `{id}`"),
            Self::WrongType {
                id,
                expected,
                found,
            } => write!(f, "property `{id}` holds a {expected}, not a {found}"),
        }
    }
}

impl Error for PropertyError {}

type Rule = Arc<dyn Fn(&PropertyValue) -> Option<PropertyIssue> + Send + Sync>;

/// One property: what it is called, what it holds and how it is checked.
///
/// ```
/// use egui_ide::{PropertyDescriptor, PropertyIssue, PropertyKind, PropertyValue};
///
/// let age = PropertyDescriptor::new("age", PropertyKind::Int)
///     .label("Age")
///     .description("How many hours back a line is still read.")
///     .unit("h")
///     .range(0.0, 72.0)
///     .default_to(PropertyValue::Int(12))
///     .rule(|value| match value.as_int() {
///         Some(hours) if hours < 0 => Some(PropertyIssue::error("It cannot be negative.")),
///         _ => None,
///     });
/// assert_eq!(age.label_text(), "Age");
/// ```
#[derive(Clone)]
pub struct PropertyDescriptor {
    id: PropertyId,
    label: String,
    description: Option<String>,
    kind: PropertyKind,
    default: PropertyValue,
    unit: Option<String>,
    range: Option<(f64, f64)>,
    rule: Option<Rule>,
    enabled: bool,
}

impl fmt::Debug for PropertyDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyDescriptor")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("kind", &self.kind)
            .field("default", &self.default)
            .finish_non_exhaustive()
    }
}

impl PropertyDescriptor {
    /// A property called `id`, labelled with its id until
    /// [`label`](Self::label) says otherwise, holding the default value of
    /// its `kind`.
    pub fn new(id: impl Into<String>, kind: PropertyKind) -> Self {
        let id = PropertyId::new(id);
        Self {
            label: id.as_str().to_owned(),
            id,
            description: None,
            default: kind.default_value(),
            kind,
            unit: None,
            range: None,
            rule: None,
            enabled: true,
        }
    }

    /// The name shown in the first column.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// What the property is for, shown in the box under the groups.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The value the property has until it is changed. A property whose value
    /// differs from it is drawn as modified, and can be reset to it.
    ///
    /// A value that the kind does not accept is ignored.
    #[must_use]
    pub fn default_to(mut self, value: PropertyValue) -> Self {
        if self.kind.accepts(&value) {
            self.default = value;
        }
        self
    }

    /// The unit written after a number, such as `ms`.
    #[must_use]
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// The least and the most a number can be dragged to.
    #[must_use]
    pub fn range(mut self, least: f64, most: f64) -> Self {
        self.range = Some((least, most));
        self
    }

    /// Whether the property can be edited, `true` by default.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A rule that checks the value after each change: it gives the issue
    /// that the grid shows under the value, or `None` if the value is fine.
    #[must_use]
    pub fn rule(
        mut self,
        rule: impl Fn(&PropertyValue) -> Option<PropertyIssue> + Send + Sync + 'static,
    ) -> Self {
        self.rule = Some(Arc::new(rule));
        self
    }

    /// The id of the property.
    #[must_use]
    pub fn id(&self) -> &PropertyId {
        &self.id
    }

    /// The name in the first column.
    #[must_use]
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// What the property is for, if it says.
    #[must_use]
    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// What the property is.
    #[must_use]
    pub fn kind(&self) -> &PropertyKind {
        &self.kind
    }

    /// The value the property starts with.
    #[must_use]
    pub fn default_value(&self) -> &PropertyValue {
        &self.default
    }

    /// The unit written after a number.
    #[must_use]
    pub fn unit_text(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    /// The limits of a number.
    #[must_use]
    pub fn limits(&self) -> Option<(f64, f64)> {
        self.range
    }

    /// Whether the property can be edited.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// What the rule of the property finds wrong in `value`, if it has one.
    #[must_use]
    pub fn check(&self, value: &PropertyValue) -> Option<PropertyIssue> {
        self.rule.as_ref().and_then(|rule| rule(value))
    }
}

/// A group of properties, under one band of the grid.
#[derive(Debug, Clone)]
pub struct PropertyGroup {
    title: String,
    descriptors: Vec<PropertyDescriptor>,
}

impl PropertyGroup {
    /// Adds a property to the group, below the ones it has.
    pub fn property(&mut self, descriptor: PropertyDescriptor) -> &mut Self {
        self.descriptors.push(descriptor);
        self
    }

    /// The title of the band.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The properties of the group, in order.
    #[must_use]
    pub fn descriptors(&self) -> &[PropertyDescriptor] {
        &self.descriptors
    }
}

/// The words the editors of the grid use, in English until they are set.
#[derive(Debug, Clone)]
pub struct PropertyLabels {
    /// The button of a window that keeps what was edited.
    pub ok: String,
    /// The button of a window that drops what was edited.
    pub cancel: String,
    /// The button that adds an entry.
    pub add: String,
    /// The button that removes the selected entry.
    pub remove: String,
    /// The button that opens the window of a list.
    pub edit: String,
    /// What follows the count of entries of a list, such as `items`.
    pub items: String,
    /// The head of the column of keys.
    pub key: String,
    /// The head of the column of values.
    pub value: String,
    /// The tip of the button that puts the default value back.
    pub reset: String,
    /// The `true` option of a flag.
    pub yes: String,
    /// The `false` option of a flag.
    pub no: String,
    /// The hint of an empty date.
    pub date_hint: String,
}

impl Default for PropertyLabels {
    fn default() -> Self {
        Self {
            ok: "OK".to_owned(),
            cancel: "Cancel".to_owned(),
            add: "+".to_owned(),
            remove: "\u{2212}".to_owned(),
            edit: "\u{2026}".to_owned(),
            items: "items".to_owned(),
            key: "Key".to_owned(),
            value: "Value".to_owned(),
            reset: "Restore the default value".to_owned(),
            yes: "True".to_owned(),
            no: "False".to_owned(),
            date_hint: "YYYY-MM-DD".to_owned(),
        }
    }
}

/// What a [`PropertyGrid`](crate::PropertyGrid) shows and edits: groups of
/// properties and the value of each, set up and read through this API.
///
/// ```
/// use egui_ide::{PropertyDescriptor, PropertyKind, PropertyModel, PropertyValue};
///
/// let mut model = PropertyModel::new();
/// model.group("General", |group| {
///     group.property(PropertyDescriptor::new("enabled", PropertyKind::Bool));
/// });
/// let enabled = "enabled".into();
/// model.set(&enabled, PropertyValue::Bool(true)).unwrap();
/// assert_eq!(model.get(&enabled).and_then(PropertyValue::as_bool), Some(true));
/// assert!(model.is_modified(&enabled));
/// ```
#[derive(Debug, Clone, Default)]
pub struct PropertyModel {
    pub(crate) groups: Vec<PropertyGroup>,
    pub(crate) values: HashMap<PropertyId, PropertyValue>,
    pub(crate) labels: PropertyLabels,
    pub(crate) activated: Vec<PropertyId>,
}

impl PropertyModel {
    /// A model with no properties.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The words of the editors.
    #[must_use]
    pub fn labels(mut self, labels: PropertyLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Adds a group that `add` fills with properties. Each property starts
    /// with its default value, unless the model already holds a value for its
    /// id, which is kept.
    pub fn group(
        &mut self,
        title: impl Into<String>,
        add: impl FnOnce(&mut PropertyGroup),
    ) -> &mut Self {
        let mut group = PropertyGroup {
            title: title.into(),
            descriptors: Vec::new(),
        };
        add(&mut group);
        for descriptor in &group.descriptors {
            self.values
                .entry(descriptor.id().clone())
                .or_insert_with(|| descriptor.default_value().clone());
        }
        self.groups.push(group);
        self
    }

    /// The properties whose button or link was used since the last call, in
    /// the order they were used: the answer to a [`PropertyKind::Action`],
    /// [`PropertyKind::Link`] or [`PropertyKind::Reference`].
    pub fn take_activated(&mut self) -> Vec<PropertyId> {
        std::mem::take(&mut self.activated)
    }

    /// The groups, in order.
    #[must_use]
    pub fn groups(&self) -> &[PropertyGroup] {
        &self.groups
    }

    /// The property called `id`.
    #[must_use]
    pub fn descriptor(&self, id: &PropertyId) -> Option<&PropertyDescriptor> {
        self.groups
            .iter()
            .flat_map(|group| group.descriptors.iter())
            .find(|descriptor| descriptor.id() == id)
    }

    /// The value of the property called `id`.
    #[must_use]
    pub fn get(&self, id: &PropertyId) -> Option<&PropertyValue> {
        self.values.get(id)
    }

    /// Gives the property called `id` a value. The value is kept even if the
    /// rule of the property finds it wrong: see [`issue`](Self::issue).
    ///
    /// # Errors
    ///
    /// Fails if the model has no such property, or if the value is not of the
    /// type that the property holds.
    pub fn set(&mut self, id: &PropertyId, value: PropertyValue) -> Result<(), PropertyError> {
        let descriptor = self
            .descriptor(id)
            .ok_or_else(|| PropertyError::Unknown(id.clone()))?;
        if !descriptor.kind().accepts(&value) {
            return Err(PropertyError::WrongType {
                id: id.clone(),
                expected: descriptor.default_value().type_name(),
                found: value.type_name(),
            });
        }
        self.values.insert(id.clone(), value);
        Ok(())
    }

    /// Whether the value of the property differs from its default one.
    #[must_use]
    pub fn is_modified(&self, id: &PropertyId) -> bool {
        match (self.descriptor(id), self.values.get(id)) {
            (Some(descriptor), Some(value)) => value != descriptor.default_value(),
            _ => false,
        }
    }

    /// Puts the default value back. Returns whether the property exists.
    pub fn reset(&mut self, id: &PropertyId) -> bool {
        let Some(default) = self.descriptor(id).map(|d| d.default_value().clone()) else {
            return false;
        };
        self.values.insert(id.clone(), default);
        true
    }

    /// What the rule of the property finds wrong in its current value.
    #[must_use]
    pub fn issue(&self, id: &PropertyId) -> Option<PropertyIssue> {
        let descriptor = self.descriptor(id)?;
        descriptor.check(self.values.get(id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> PropertyModel {
        let mut model = PropertyModel::new();
        model.group("General", |group| {
            group
                .property(PropertyDescriptor::new("enabled", PropertyKind::Bool))
                .property(
                    PropertyDescriptor::new("age", PropertyKind::Int)
                        .default_to(PropertyValue::Int(12))
                        .rule(|value| match value.as_int() {
                            Some(age) if age < 0 => Some(PropertyIssue::error("negative")),
                            _ => None,
                        }),
                );
        });
        model
    }

    #[test]
    fn a_property_starts_with_its_default_and_is_not_modified() {
        let model = model();
        let age = PropertyId::new("age");
        assert_eq!(model.get(&age), Some(&PropertyValue::Int(12)));
        assert!(!model.is_modified(&age));
    }

    #[test]
    fn a_change_marks_the_property_as_modified_until_it_is_reset() {
        let mut model = model();
        let age = PropertyId::new("age");
        model.set(&age, PropertyValue::Int(3)).unwrap();
        assert!(model.is_modified(&age));
        assert!(model.reset(&age));
        assert!(!model.is_modified(&age));
    }

    #[test]
    fn a_value_of_another_type_or_for_an_unknown_id_is_refused() {
        let mut model = model();
        assert!(matches!(
            model.set(&PropertyId::new("age"), PropertyValue::Bool(true)),
            Err(PropertyError::WrongType { .. })
        ));
        assert!(matches!(
            model.set(&PropertyId::new("nope"), PropertyValue::Int(1)),
            Err(PropertyError::Unknown(_))
        ));
    }

    #[test]
    fn the_rule_reports_an_issue_but_the_value_is_kept() {
        let mut model = model();
        let age = PropertyId::new("age");
        model.set(&age, PropertyValue::Int(-1)).unwrap();
        assert_eq!(model.get(&age), Some(&PropertyValue::Int(-1)));
        let issue = model.issue(&age).unwrap();
        assert_eq!(issue.severity(), Severity::Error);
    }
}
