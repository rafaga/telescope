//! The controls a node carries in its body: a button, a check box, a switch
//! or a group of radio buttons.
//!
//! A control is data, like a port: its value lives in the node, so it is
//! told by a [`CanvasEvent`](crate::CanvasEvent) when the user changes it and
//! undone by [`History`](crate::History). The canvas shows them with the
//! widgets of `egui`.

use std::fmt;

/// The name a control goes by in its node, unique among the controls of that
/// node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ControlId(String);

impl ControlId {
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

impl fmt::Display for ControlId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ControlId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// What a control holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlValue {
    /// Checked or not, on or off.
    Flag(bool),
    /// The option of a group of radio buttons that is selected.
    Choice(usize),
}

/// What a control is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlKind {
    /// A button: it holds nothing, it is pressed.
    Button,
    /// A check box.
    Check,
    /// A switch, a check box that reads as on or off.
    Switch,
    /// Radio buttons, one per option, of which one is selected.
    Radio(Vec<String>),
}

/// A control in the body of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeControl {
    id: ControlId,
    label: String,
    kind: ControlKind,
    value: Option<ControlValue>,
    enabled: bool,
}

impl NodeControl {
    fn make(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: ControlKind,
        value: Option<ControlValue>,
    ) -> Self {
        Self {
            id: ControlId::new(id),
            label: label.into(),
            kind,
            value,
            enabled: true,
        }
    }

    /// A button named `id`, with the text `label`.
    pub fn button(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::make(id, label, ControlKind::Button, None)
    }

    /// A check box named `id`, with the text `label`.
    pub fn check(id: impl Into<String>, label: impl Into<String>, checked: bool) -> Self {
        Self::make(
            id,
            label,
            ControlKind::Check,
            Some(ControlValue::Flag(checked)),
        )
    }

    /// A switch named `id`, with the text `label`.
    pub fn switch(id: impl Into<String>, label: impl Into<String>, on: bool) -> Self {
        Self::make(id, label, ControlKind::Switch, Some(ControlValue::Flag(on)))
    }

    /// A group of radio buttons named `id`, one per option, with `selected`
    /// the index of the one that is on. `label` names the group for the ones
    /// that cannot see it (a screen reader); it is not drawn.
    pub fn radio(
        id: impl Into<String>,
        label: impl Into<String>,
        options: impl IntoIterator<Item = impl Into<String>>,
        selected: usize,
    ) -> Self {
        let options: Vec<String> = options.into_iter().map(Into::into).collect();
        let selected = selected.min(options.len().saturating_sub(1));
        Self::make(
            id,
            label,
            ControlKind::Radio(options),
            Some(ControlValue::Choice(selected)),
        )
    }

    /// Whether the user can use the control (it can by default).
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The id of the control.
    #[must_use]
    pub fn id(&self) -> &ControlId {
        &self.id
    }

    /// The text of the control.
    #[must_use]
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// What the control is.
    #[must_use]
    pub fn kind(&self) -> &ControlKind {
        &self.kind
    }

    /// What the control holds; a button holds nothing.
    #[must_use]
    pub fn value(&self) -> Option<ControlValue> {
        self.value
    }

    /// Whether the user can use the control.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether `value` is one the control can hold.
    pub(crate) fn accepts(&self, value: ControlValue) -> bool {
        match (&self.kind, value) {
            (ControlKind::Check | ControlKind::Switch, ControlValue::Flag(_)) => true,
            (ControlKind::Radio(options), ControlValue::Choice(index)) => index < options.len(),
            _ => false,
        }
    }

    /// Holds `value`, which the control has to accept.
    pub(crate) fn hold(&mut self, value: ControlValue) {
        self.value = Some(value);
    }
}
