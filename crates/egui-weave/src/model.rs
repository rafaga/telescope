//! The data of a canvas: nodes with ports, and the edges that join the ports.
//!
//! Nothing here draws: the model is what the application owns and what the
//! canvas shows. Every change that the user could undo is told through a
//! [`CanvasEvent`], taken with [`CanvasModel::take_events`].

use crate::controls::{ControlId, ControlKind, ControlValue, NodeControl};
use crate::edge_style::EdgeProperties;
use egui::{Color32, Pos2};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// The name a node goes by in a [`CanvasModel`], unique in the model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
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

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodeId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// The name a port goes by in its node, unique among the inputs and outputs
/// of that node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PortId(String);

impl PortId {
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

impl fmt::Display for PortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for PortId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

/// The number of an edge, given when it is made and never used again by the
/// same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EdgeId(u64);

impl EdgeId {
    /// The number of the id.
    #[must_use]
    pub fn number(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// A port of a node, by the node and the name of the port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortRef {
    /// The node the port belongs to.
    pub node: NodeId,
    /// The port in that node.
    pub port: PortId,
}

impl PortRef {
    /// The port `port` of the node `node`.
    pub fn new(node: impl Into<NodeId>, port: impl Into<PortId>) -> Self {
        Self {
            node: node.into(),
            port: port.into(),
        }
    }
}

impl fmt::Display for PortRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.node, self.port)
    }
}

/// Whether a port takes edges in or sends them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortDirection {
    /// The edges arrive here (the left side of the node).
    Input,
    /// The edges leave from here (the right side of the node).
    Output,
}

/// A port of a node: where an edge starts or ends.
#[derive(Debug, Clone, PartialEq)]
pub struct PortDescriptor {
    id: PortId,
    label: String,
    kind: Option<String>,
    color: Option<Color32>,
    max_edges: Option<usize>,
}

impl PortDescriptor {
    /// A port named `id`, shown with `label`.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: PortId::new(id),
            label: label.into(),
            kind: None,
            color: None,
            max_edges: None,
        }
    }

    /// The kind of data the port carries. Two ports can only be joined when
    /// their kinds are the same, or when one of them has none.
    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// The color of the port and of the edges that leave from it, instead of
    /// the one of the style.
    #[must_use]
    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }

    /// The most edges the port takes (no limit when it is not set).
    #[must_use]
    pub fn max_edges(mut self, max: usize) -> Self {
        self.max_edges = Some(max);
        self
    }

    /// The id of the port.
    #[must_use]
    pub fn id(&self) -> &PortId {
        &self.id
    }

    /// The text shown next to the port.
    #[must_use]
    pub fn label_text(&self) -> &str {
        &self.label
    }

    /// The kind of data the port carries, if it has one.
    #[must_use]
    pub fn kind_name(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    /// The color of the port, if it has one of its own.
    #[must_use]
    pub fn port_color(&self) -> Option<Color32> {
        self.color
    }

    /// The most edges the port takes, if there is a limit.
    #[must_use]
    pub fn edge_limit(&self) -> Option<usize> {
        self.max_edges
    }
}

/// A node: a box with a title and a summary, and ports on its sides.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDescriptor {
    id: NodeId,
    title: String,
    summary: String,
    accent: Option<Color32>,
    pos: Pos2,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
    controls: Vec<NodeControl>,
}

impl NodeDescriptor {
    /// A node named `id`, shown with `title`, at the origin and without ports.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: NodeId::new(id),
            title: title.into(),
            summary: String::new(),
            accent: None,
            pos: Pos2::ZERO,
            inputs: Vec::new(),
            outputs: Vec::new(),
            controls: Vec::new(),
        }
    }

    /// The line under the title.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// The color of the header of the node, instead of the one of the style.
    #[must_use]
    pub fn accent(mut self, color: Color32) -> Self {
        self.accent = Some(color);
        self
    }

    /// Where the top left corner of the node is, in the coordinates of the
    /// canvas.
    #[must_use]
    pub fn at(mut self, pos: Pos2) -> Self {
        self.pos = pos;
        self
    }

    /// Adds a port the edges arrive at.
    #[must_use]
    pub fn input(mut self, port: PortDescriptor) -> Self {
        self.inputs.push(port);
        self
    }

    /// Adds a port the edges leave from.
    #[must_use]
    pub fn output(mut self, port: PortDescriptor) -> Self {
        self.outputs.push(port);
        self
    }

    /// Adds a control under the ports, one line each (a group of radio
    /// buttons takes one line per option).
    #[must_use]
    pub fn control(mut self, control: NodeControl) -> Self {
        self.controls.push(control);
        self
    }

    /// The id of the node.
    #[must_use]
    pub fn id(&self) -> &NodeId {
        &self.id
    }

    /// The title of the node.
    #[must_use]
    pub fn title_text(&self) -> &str {
        &self.title
    }

    /// The line under the title.
    #[must_use]
    pub fn summary_text(&self) -> &str {
        &self.summary
    }

    /// The color of the header, if the node has one of its own.
    #[must_use]
    pub fn accent_color(&self) -> Option<Color32> {
        self.accent
    }

    /// Where the top left corner of the node is.
    #[must_use]
    pub fn pos(&self) -> Pos2 {
        self.pos
    }

    /// The ports the edges arrive at, in order.
    #[must_use]
    pub fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    /// The ports the edges leave from, in order.
    #[must_use]
    pub fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    /// The controls of the node, in order.
    #[must_use]
    pub fn controls(&self) -> &[NodeControl] {
        &self.controls
    }

    /// The control named `id`.
    #[must_use]
    pub fn control_named(&self, id: &ControlId) -> Option<&NodeControl> {
        self.controls.iter().find(|control| control.id() == id)
    }

    /// The port named `id`, and which side it is on.
    #[must_use]
    pub fn port(&self, id: &PortId) -> Option<(PortDirection, &PortDescriptor)> {
        self.inputs
            .iter()
            .find(|port| port.id() == id)
            .map(|port| (PortDirection::Input, port))
            .or_else(|| {
                self.outputs
                    .iter()
                    .find(|port| port.id() == id)
                    .map(|port| (PortDirection::Output, port))
            })
    }
}

/// What an edge says when the graph runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EdgeState {
    /// Nothing to tell: the edge is as it was drawn.
    #[default]
    Neutral,
    /// The edge carried a true value.
    True,
    /// The edge carried a false value.
    False,
    /// The edge was not evaluated.
    Absent,
}

/// An edge: from an output port to an input port.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    id: EdgeId,
    from: PortRef,
    to: PortRef,
    state: EdgeState,
    properties: EdgeProperties,
}

impl Edge {
    /// The id of the edge.
    #[must_use]
    pub fn id(&self) -> EdgeId {
        self.id
    }

    /// The output port the edge leaves from.
    #[must_use]
    pub fn from(&self) -> &PortRef {
        &self.from
    }

    /// The input port the edge arrives at.
    #[must_use]
    pub fn to(&self) -> &PortRef {
        &self.to
    }

    /// What the edge says when the graph runs.
    #[must_use]
    pub fn state(&self) -> EdgeState {
        self.state
    }

    /// How the edge is drawn: its color, line and width.
    #[must_use]
    pub fn properties(&self) -> &EdgeProperties {
        &self.properties
    }
}

/// Why a change to a [`CanvasModel`] was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasError {
    /// There is a node with that id already.
    DuplicateNode(NodeId),
    /// A node has two ports with the same id.
    DuplicatePort(PortRef),
    /// A node has two controls with the same id.
    DuplicateControl(NodeId, ControlId),
    /// There is no node with that id.
    UnknownNode(NodeId),
    /// The node has no port with that id.
    UnknownPort(PortRef),
    /// The port is on the wrong side: an edge leaves from an output and
    /// arrives at an input.
    WrongDirection(PortRef),
    /// An edge cannot join a node to itself.
    SameNode(NodeId),
    /// The kinds of data of the two ports differ.
    KindMismatch {
        /// The port the edge would leave from.
        from: PortRef,
        /// The port the edge would arrive at.
        to: PortRef,
    },
    /// Those two ports are joined already.
    AlreadyConnected(EdgeId),
    /// The port takes no more edges.
    PortFull(PortRef),
    /// A [`ConnectionRule`] refused the edge, with its reason.
    Rejected(String),
}

impl fmt::Display for CanvasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateNode(id) => write!(f, "there is a node `{id}` already"),
            Self::DuplicatePort(port) => write!(f, "the port `{port}` is there twice"),
            Self::DuplicateControl(node, control) => {
                write!(f, "the control `{control}` is in the node `{node}` twice")
            }
            Self::UnknownNode(id) => write!(f, "there is no node `{id}`"),
            Self::UnknownPort(port) => write!(f, "there is no port `{port}`"),
            Self::WrongDirection(port) => write!(f, "the port `{port}` is on the wrong side"),
            Self::SameNode(id) => write!(f, "the node `{id}` cannot be joined to itself"),
            Self::KindMismatch { from, to } => {
                write!(f, "`{from}` and `{to}` carry different kinds of data")
            }
            Self::AlreadyConnected(id) => write!(f, "those ports are joined by the edge {id}"),
            Self::PortFull(port) => write!(f, "the port `{port}` takes no more edges"),
            Self::Rejected(reason) => f.write_str(reason),
        }
    }
}

impl Error for CanvasError {}

/// Something that changed in a [`CanvasModel`], with what is needed to undo
/// it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CanvasEvent {
    /// A node was added, as it is.
    NodeAdded(NodeDescriptor),
    /// A node was removed, with the edges that went to or from it.
    NodeRemoved {
        /// The node as it was.
        node: NodeDescriptor,
        /// The edges that were removed with it.
        edges: Vec<Edge>,
    },
    /// A node was moved.
    NodeMoved {
        /// The node.
        node: NodeId,
        /// Where it was.
        from: Pos2,
        /// Where it is.
        to: Pos2,
    },
    /// Two ports were joined.
    Connected(Edge),
    /// An edge was removed.
    Disconnected(Edge),
    /// The properties of an edge changed.
    EdgeChanged {
        /// The edge.
        edge: EdgeId,
        /// What they were.
        from: EdgeProperties,
        /// What they are.
        to: EdgeProperties,
    },
    /// The user changed a control of a node: a check box, a switch or a
    /// group of radio buttons.
    ControlChanged {
        /// The node.
        node: NodeId,
        /// The control.
        control: ControlId,
        /// What it held.
        from: ControlValue,
        /// What it holds.
        to: ControlValue,
    },
    /// The user pressed a button of a node. Nothing changed in the model, so
    /// there is nothing to undo.
    ButtonPressed {
        /// The node.
        node: NodeId,
        /// The button.
        control: ControlId,
    },
    /// The selected nodes changed; this is the selection now.
    SelectionChanged(Vec<NodeId>),
    /// The user dragged an edge from a port and let go over nothing, at
    /// `pos` (in the coordinates of the canvas): a place to offer to create
    /// a node that fits. Nothing changed in the model.
    ConnectionDropped {
        /// The port the edge was dragged from.
        from: PortRef,
        /// Where it was let go.
        pos: Pos2,
    },
}

/// A rule that an edge has to follow to be made, in addition to the ones the
/// model always checks (the sides of the ports, their kinds, their limits).
pub trait ConnectionRule: Send + Sync {
    /// `Ok` when an edge from `from` to `to` may be made, or the reason it
    /// may not.
    ///
    /// # Errors
    ///
    /// The reason the edge is refused.
    fn check(&self, model: &CanvasModel, from: &PortRef, to: &PortRef) -> Result<(), String>;
}

/// The rule that forbids an edge that would close a loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCycles;

impl ConnectionRule for NoCycles {
    fn check(&self, model: &CanvasModel, from: &PortRef, to: &PortRef) -> Result<(), String> {
        if model.reaches(&to.node, &from.node) {
            Err(format!(
                "`{}` leads back to `{}`: the edge would close a loop",
                to.node, from.node
            ))
        } else {
            Ok(())
        }
    }
}

/// The nodes of a canvas, their ports, and the edges between the ports.
#[derive(Clone, Default)]
pub struct CanvasModel {
    nodes: Vec<NodeDescriptor>,
    edges: Vec<Edge>,
    selection: Vec<NodeId>,
    rules: Vec<Arc<dyn ConnectionRule>>,
    events: Vec<CanvasEvent>,
    next_edge: u64,
}

impl fmt::Debug for CanvasModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CanvasModel")
            .field("nodes", &self.nodes)
            .field("edges", &self.edges)
            .field("selection", &self.selection)
            .field("rules", &self.rules.len())
            .finish_non_exhaustive()
    }
}

impl CanvasModel {
    /// An empty model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a rule that every edge has to follow.
    pub fn rule(&mut self, rule: impl ConnectionRule + 'static) -> &mut Self {
        self.rules.push(Arc::new(rule));
        self
    }

    /// Adds a node, drawn over the ones that were there.
    ///
    /// # Errors
    ///
    /// [`CanvasError::DuplicateNode`] when there is a node with that id, and
    /// [`CanvasError::DuplicatePort`] when two of its ports share an id.
    pub fn add_node(&mut self, node: NodeDescriptor) -> Result<(), CanvasError> {
        if self.node(node.id()).is_some() {
            return Err(CanvasError::DuplicateNode(node.id().clone()));
        }
        for (index, control) in node.controls().iter().enumerate() {
            if node.controls()[..index]
                .iter()
                .any(|other| other.id() == control.id())
            {
                return Err(CanvasError::DuplicateControl(
                    node.id().clone(),
                    control.id().clone(),
                ));
            }
        }
        let ports: Vec<&PortDescriptor> = node.inputs().iter().chain(node.outputs()).collect();
        for (index, port) in ports.iter().enumerate() {
            if ports[..index].iter().any(|other| other.id() == port.id()) {
                return Err(CanvasError::DuplicatePort(PortRef {
                    node: node.id().clone(),
                    port: port.id().clone(),
                }));
            }
        }
        self.events.push(CanvasEvent::NodeAdded(node.clone()));
        self.nodes.push(node);
        Ok(())
    }

    /// Removes a node, the edges that go to or from it, and its place in the
    /// selection.
    pub fn remove_node(&mut self, id: &NodeId) -> Option<NodeDescriptor> {
        let index = self.nodes.iter().position(|node| node.id() == id)?;
        let node = self.nodes.remove(index);
        let (gone, kept): (Vec<Edge>, Vec<Edge>) = std::mem::take(&mut self.edges)
            .into_iter()
            .partition(|edge| edge.from.node == *id || edge.to.node == *id);
        self.edges = kept;
        self.events.push(CanvasEvent::NodeRemoved {
            node: node.clone(),
            edges: gone,
        });
        if self.selection.contains(id) {
            self.selection.retain(|selected| selected != id);
            self.events
                .push(CanvasEvent::SelectionChanged(self.selection.clone()));
        }
        Some(node)
    }

    /// The node with that id.
    #[must_use]
    pub fn node(&self, id: &NodeId) -> Option<&NodeDescriptor> {
        self.nodes.iter().find(|node| node.id() == id)
    }

    /// The nodes, from the one at the back to the one in front.
    #[must_use]
    pub fn nodes(&self) -> &[NodeDescriptor] {
        &self.nodes
    }

    /// Moves a node. Returns whether it moved: it does not when there is no
    /// such node or it is there already.
    pub fn move_node(&mut self, id: &NodeId, pos: Pos2) -> bool {
        let Some(node) = self.nodes.iter_mut().find(|node| node.id() == id) else {
            return false;
        };
        if node.pos == pos {
            return false;
        }
        let from = node.pos;
        node.pos = pos;
        self.events.push(CanvasEvent::NodeMoved {
            node: id.clone(),
            from,
            to: pos,
        });
        true
    }

    /// Sets what a control holds. Returns whether it changed: it does not
    /// when there is no such control, the value is not one it can hold or it
    /// holds it already.
    pub fn set_control(&mut self, node: &NodeId, control: &ControlId, value: ControlValue) -> bool {
        let Some(target) = self
            .nodes
            .iter_mut()
            .find(|candidate| candidate.id() == node)
            .and_then(|found| found.controls.iter_mut().find(|item| item.id() == control))
        else {
            return false;
        };
        let Some(from) = target.value() else {
            return false;
        };
        if !target.accepts(value) || from == value {
            return false;
        }
        target.hold(value);
        self.events.push(CanvasEvent::ControlChanged {
            node: node.clone(),
            control: control.clone(),
            from,
            to: value,
        });
        true
    }

    /// Tells that a button was pressed. Returns whether it was: the control
    /// has to be an enabled button.
    pub fn press(&mut self, node: &NodeId, control: &ControlId) -> bool {
        let pressed = self
            .node(node)
            .and_then(|found| found.control_named(control))
            .is_some_and(|item| item.is_enabled() && *item.kind() == ControlKind::Button);
        if pressed {
            self.events.push(CanvasEvent::ButtonPressed {
                node: node.clone(),
                control: control.clone(),
            });
        }
        pressed
    }

    /// The side and the description of a port.
    fn port(&self, port: &PortRef) -> Result<(PortDirection, &PortDescriptor), CanvasError> {
        self.node(&port.node)
            .ok_or_else(|| CanvasError::UnknownNode(port.node.clone()))?
            .port(&port.port)
            .ok_or_else(|| CanvasError::UnknownPort(port.clone()))
    }

    /// Whether an edge from `from` to `to` may be made, without making it:
    /// what a canvas asks while the user drags an edge, to show which ports
    /// take it.
    ///
    /// # Errors
    ///
    /// The reason the edge may not be made.
    pub fn check_connection(&self, from: &PortRef, to: &PortRef) -> Result<(), CanvasError> {
        let (from_side, from_port) = self.port(from)?;
        let (to_side, to_port) = self.port(to)?;
        if from_side != PortDirection::Output {
            return Err(CanvasError::WrongDirection(from.clone()));
        }
        if to_side != PortDirection::Input {
            return Err(CanvasError::WrongDirection(to.clone()));
        }
        if from.node == to.node {
            return Err(CanvasError::SameNode(from.node.clone()));
        }
        if let (Some(a), Some(b)) = (from_port.kind_name(), to_port.kind_name())
            && a != b
        {
            return Err(CanvasError::KindMismatch {
                from: from.clone(),
                to: to.clone(),
            });
        }
        if let Some(edge) = self.edges.iter().find(|e| e.from == *from && e.to == *to) {
            return Err(CanvasError::AlreadyConnected(edge.id));
        }
        for (port, description) in [(from, from_port), (to, to_port)] {
            if let Some(limit) = description.edge_limit()
                && self.edges_at(port).count() >= limit
            {
                return Err(CanvasError::PortFull(port.clone()));
            }
        }
        for rule in &self.rules {
            rule.check(self, from, to).map_err(CanvasError::Rejected)?;
        }
        Ok(())
    }

    /// Joins the output port `from` to the input port `to`. Two nodes can be
    /// joined by several edges, as long as they go through different ports.
    ///
    /// # Errors
    ///
    /// The reason the edge may not be made; see
    /// [`check_connection`](Self::check_connection).
    pub fn connect(&mut self, from: PortRef, to: PortRef) -> Result<EdgeId, CanvasError> {
        self.check_connection(&from, &to)?;
        let id = EdgeId(self.next_edge);
        self.next_edge += 1;
        let edge = Edge {
            id,
            from,
            to,
            state: EdgeState::Neutral,
            properties: EdgeProperties::default(),
        };
        self.events.push(CanvasEvent::Connected(edge.clone()));
        self.edges.push(edge);
        Ok(id)
    }

    /// Puts back an edge that was removed, with its id and state. The ports
    /// are not checked again: the edge was valid when it was made. What it
    /// tells is not recorded as an event, since it is for undoing.
    pub(crate) fn restore_edge(&mut self, edge: Edge) {
        if self.edges.iter().any(|other| other.id == edge.id) {
            return;
        }
        self.next_edge = self.next_edge.max(edge.id.0 + 1);
        self.edges.push(edge);
    }

    /// Tells that an edge dragged from `from` was let go over nothing at
    /// `pos`. The model does not change.
    pub(crate) fn report_dropped_connection(&mut self, from: PortRef, pos: Pos2) {
        self.events
            .push(CanvasEvent::ConnectionDropped { from, pos });
    }

    /// Removes an edge.
    pub fn disconnect(&mut self, id: EdgeId) -> Option<Edge> {
        let index = self.edges.iter().position(|edge| edge.id == id)?;
        let edge = self.edges.remove(index);
        self.events.push(CanvasEvent::Disconnected(edge.clone()));
        Some(edge)
    }

    /// Sets how an edge is drawn. Returns whether it changed: it does not
    /// when there is no such edge or it has those properties already.
    pub fn set_edge_properties(&mut self, id: EdgeId, properties: EdgeProperties) -> bool {
        let Some(edge) = self.edges.iter_mut().find(|edge| edge.id == id) else {
            return false;
        };
        if edge.properties == properties {
            return false;
        }
        let from = edge.properties;
        edge.properties = properties;
        self.events.push(CanvasEvent::EdgeChanged {
            edge: id,
            from,
            to: properties,
        });
        true
    }

    /// Sets what an edge says when the graph runs. Returns whether there is
    /// such an edge. It is not an edit of the graph, so no event is told.
    pub fn set_edge_state(&mut self, id: EdgeId, state: EdgeState) -> bool {
        match self.edges.iter_mut().find(|edge| edge.id == id) {
            Some(edge) => {
                edge.state = state;
                true
            }
            None => false,
        }
    }

    /// The edge with that id.
    #[must_use]
    pub fn edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.iter().find(|edge| edge.id == id)
    }

    /// The edges, in the order they were made.
    #[must_use]
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// The edges that start or end at a port.
    pub fn edges_at<'a>(&'a self, port: &'a PortRef) -> impl Iterator<Item = &'a Edge> + 'a {
        self.edges
            .iter()
            .filter(move |edge| edge.from == *port || edge.to == *port)
    }

    /// Whether following the edges from the node `from` leads to the node
    /// `to` (a node reaches itself).
    #[must_use]
    pub fn reaches(&self, from: &NodeId, to: &NodeId) -> bool {
        let mut seen: Vec<&NodeId> = Vec::new();
        let mut pending: Vec<&NodeId> = vec![from];
        while let Some(node) = pending.pop() {
            if node == to {
                return true;
            }
            if seen.contains(&node) {
                continue;
            }
            seen.push(node);
            pending.extend(
                self.edges
                    .iter()
                    .filter(|edge| edge.from.node == *node)
                    .map(|edge| &edge.to.node),
            );
        }
        false
    }

    /// The selected nodes, in the order they were selected.
    #[must_use]
    pub fn selection(&self) -> &[NodeId] {
        &self.selection
    }

    /// Selects those nodes and no others. The ones that are not in the model
    /// and the repeated ones are left out.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let mut selection: Vec<NodeId> = Vec::new();
        for id in ids {
            if self.node(&id).is_some() && !selection.contains(&id) {
                selection.push(id);
            }
        }
        if selection != self.selection {
            self.selection = selection;
            self.events
                .push(CanvasEvent::SelectionChanged(self.selection.clone()));
        }
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self) {
        self.set_selection(std::iter::empty());
    }

    /// What changed since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<CanvasEvent> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graphs the tests work on.
    struct Fixture;

    impl Fixture {
        /// `input` -> `detection` (true / false) -> `output`.
        fn model() -> CanvasModel {
            let mut model = CanvasModel::new();
            model
                .add_node(
                    NodeDescriptor::new("input", "Input")
                        .at(Pos2::new(0.0, 0.0))
                        .output(PortDescriptor::new("out", "Lines").kind("text")),
                )
                .unwrap();
            model
                .add_node(
                    NodeDescriptor::new("detection", "Detection")
                        .at(Pos2::new(200.0, 0.0))
                        .input(PortDescriptor::new("in", "Lines").kind("text").max_edges(1))
                        .output(PortDescriptor::new("true", "T").kind("signal"))
                        .output(PortDescriptor::new("false", "F").kind("signal")),
                )
                .unwrap();
            model
                .add_node(
                    NodeDescriptor::new("output", "Output")
                        .at(Pos2::new(400.0, 0.0))
                        .input(PortDescriptor::new("in", "Signal").kind("signal")),
                )
                .unwrap();
            model.take_events();
            model
        }

        fn port(node: &str, port: &str) -> PortRef {
            PortRef::new(node, port)
        }
    }

    #[test]
    fn add_node_refuses_a_repeated_node_or_port() {
        let mut model = Fixture::model();
        assert_eq!(
            model.add_node(NodeDescriptor::new("input", "Again")),
            Err(CanvasError::DuplicateNode(NodeId::new("input")))
        );
        let twice = NodeDescriptor::new("both", "Both")
            .input(PortDescriptor::new("a", "A"))
            .output(PortDescriptor::new("a", "A again"));
        assert_eq!(
            model.add_node(twice),
            Err(CanvasError::DuplicatePort(Fixture::port("both", "a")))
        );
        assert!(model.node(&NodeId::new("both")).is_none());
        assert_eq!(model.nodes().len(), 3);
    }

    #[test]
    fn add_node_tells_it_and_keeps_the_order() {
        let mut model = CanvasModel::new();
        model.add_node(NodeDescriptor::new("a", "A")).unwrap();
        model.add_node(NodeDescriptor::new("b", "B")).unwrap();
        let ids: Vec<&str> = model.nodes().iter().map(|n| n.id().as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        let added: Vec<String> = model
            .take_events()
            .iter()
            .filter_map(|event| match event {
                CanvasEvent::NodeAdded(node) => Some(node.id().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(added, ["a", "b"]);
        assert!(model.take_events().is_empty());
    }

    #[test]
    fn connect_joins_an_output_to_an_input_and_tells_it() {
        let mut model = Fixture::model();
        let id = model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        let edge = model.edge(id).unwrap().clone();
        assert_eq!(edge.from(), &Fixture::port("input", "out"));
        assert_eq!(edge.to(), &Fixture::port("detection", "in"));
        assert_eq!(edge.state(), EdgeState::Neutral);
        assert_eq!(model.take_events(), [CanvasEvent::Connected(edge)]);
    }

    #[test]
    fn connect_refuses_what_does_not_exist() {
        let mut model = Fixture::model();
        assert_eq!(
            model.connect(
                Fixture::port("nope", "out"),
                Fixture::port("detection", "in")
            ),
            Err(CanvasError::UnknownNode(NodeId::new("nope")))
        );
        assert_eq!(
            model.connect(
                Fixture::port("input", "nope"),
                Fixture::port("detection", "in")
            ),
            Err(CanvasError::UnknownPort(Fixture::port("input", "nope")))
        );
        assert!(model.edges().is_empty());
        assert!(model.take_events().is_empty());
    }

    #[test]
    fn connect_refuses_the_wrong_side_and_the_same_node() {
        let mut model = Fixture::model();
        // an input as the start
        assert_eq!(
            model.connect(
                Fixture::port("detection", "in"),
                Fixture::port("output", "in")
            ),
            Err(CanvasError::WrongDirection(Fixture::port(
                "detection",
                "in"
            )))
        );
        // an output as the end
        assert_eq!(
            model.connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "true")
            ),
            Err(CanvasError::WrongDirection(Fixture::port(
                "detection",
                "true"
            )))
        );
        let mut model = Fixture::model();
        model
            .add_node(
                NodeDescriptor::new("pass", "Pass")
                    .input(PortDescriptor::new("in", "In"))
                    .output(PortDescriptor::new("out", "Out")),
            )
            .unwrap();
        assert_eq!(
            model.connect(Fixture::port("pass", "out"), Fixture::port("pass", "in")),
            Err(CanvasError::SameNode(NodeId::new("pass")))
        );
    }

    #[test]
    fn connect_refuses_different_kinds_but_takes_a_port_without_kind() {
        let mut model = Fixture::model();
        assert_eq!(
            model.connect(Fixture::port("input", "out"), Fixture::port("output", "in")),
            Err(CanvasError::KindMismatch {
                from: Fixture::port("input", "out"),
                to: Fixture::port("output", "in"),
            })
        );
        model
            .add_node(
                NodeDescriptor::new("log", "Log").input(PortDescriptor::new("in", "Anything")),
            )
            .unwrap();
        assert!(
            model
                .connect(Fixture::port("input", "out"), Fixture::port("log", "in"))
                .is_ok()
        );
    }

    #[test]
    fn two_nodes_can_share_edges_through_different_ports() {
        let mut model = Fixture::model();
        model
            .add_node(
                NodeDescriptor::new("both", "Both")
                    .input(PortDescriptor::new("yes", "Yes").kind("signal"))
                    .input(PortDescriptor::new("no", "No").kind("signal")),
            )
            .unwrap();
        let yes = model
            .connect(
                Fixture::port("detection", "true"),
                Fixture::port("both", "yes"),
            )
            .unwrap();
        let no = model
            .connect(
                Fixture::port("detection", "false"),
                Fixture::port("both", "no"),
            )
            .unwrap();
        assert_ne!(yes, no);
        assert_eq!(model.edges().len(), 2);
        // the same two ports twice is not
        assert_eq!(
            model.connect(
                Fixture::port("detection", "true"),
                Fixture::port("both", "yes")
            ),
            Err(CanvasError::AlreadyConnected(yes))
        );
    }

    #[test]
    fn a_port_takes_no_more_edges_than_its_limit() {
        let mut model = Fixture::model();
        model
            .add_node(
                NodeDescriptor::new("other", "Other")
                    .output(PortDescriptor::new("out", "Lines").kind("text")),
            )
            .unwrap();
        model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        assert_eq!(
            model.connect(
                Fixture::port("other", "out"),
                Fixture::port("detection", "in")
            ),
            Err(CanvasError::PortFull(Fixture::port("detection", "in")))
        );
        // an output has no limit unless it is set: it can feed several inputs
        model
            .add_node(
                NodeDescriptor::new("second", "Second")
                    .input(PortDescriptor::new("in", "Lines").kind("text")),
            )
            .unwrap();
        assert!(
            model
                .connect(Fixture::port("input", "out"), Fixture::port("second", "in"))
                .is_ok()
        );
    }

    #[test]
    fn no_cycles_refuses_an_edge_that_closes_a_loop() {
        let mut model = CanvasModel::new();
        model.rule(NoCycles);
        for name in ["a", "b", "c"] {
            model
                .add_node(
                    NodeDescriptor::new(name, name)
                        .input(PortDescriptor::new("in", "In"))
                        .output(PortDescriptor::new("out", "Out")),
                )
                .unwrap();
        }
        model
            .connect(Fixture::port("a", "out"), Fixture::port("b", "in"))
            .unwrap();
        model
            .connect(Fixture::port("b", "out"), Fixture::port("c", "in"))
            .unwrap();
        assert!(matches!(
            model.connect(Fixture::port("c", "out"), Fixture::port("a", "in")),
            Err(CanvasError::Rejected(_))
        ));
        assert!(model.reaches(&NodeId::new("a"), &NodeId::new("c")));
        assert!(!model.reaches(&NodeId::new("c"), &NodeId::new("a")));
        assert_eq!(model.edges().len(), 2);
    }

    #[test]
    fn check_connection_does_not_change_the_model() {
        let mut model = Fixture::model();
        assert!(
            model
                .check_connection(
                    &Fixture::port("input", "out"),
                    &Fixture::port("detection", "in")
                )
                .is_ok()
        );
        assert!(model.edges().is_empty());
        assert!(model.take_events().is_empty());
    }

    #[test]
    fn a_dropped_connection_is_told_and_changes_nothing() {
        let mut model = Fixture::model();
        model.report_dropped_connection(Fixture::port("input", "out"), Pos2::new(5.0, 6.0));
        assert!(model.edges().is_empty());
        assert_eq!(
            model.take_events(),
            [CanvasEvent::ConnectionDropped {
                from: Fixture::port("input", "out"),
                pos: Pos2::new(5.0, 6.0),
            }]
        );
    }

    #[test]
    fn disconnect_removes_the_edge_and_tells_it() {
        let mut model = Fixture::model();
        let id = model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        model.take_events();
        let edge = model.disconnect(id).unwrap();
        assert_eq!(edge.id(), id);
        assert!(model.edge(id).is_none());
        assert_eq!(model.take_events(), [CanvasEvent::Disconnected(edge)]);
        assert!(model.disconnect(id).is_none());
    }

    #[test]
    fn edge_ids_are_not_used_again() {
        let mut model = Fixture::model();
        let first = model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        model.disconnect(first);
        let second = model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn edge_state_is_set_without_an_event() {
        let mut model = Fixture::model();
        let id = model
            .connect(
                Fixture::port("detection", "true"),
                Fixture::port("output", "in"),
            )
            .unwrap();
        model.take_events();
        assert!(model.set_edge_state(id, EdgeState::True));
        assert_eq!(model.edge(id).unwrap().state(), EdgeState::True);
        assert!(model.take_events().is_empty());
        assert!(!model.set_edge_state(EdgeId(999), EdgeState::False));
    }

    #[test]
    fn remove_node_takes_its_edges_and_its_selection_with_it() {
        let mut model = Fixture::model();
        let into = model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        let out = model
            .connect(
                Fixture::port("detection", "true"),
                Fixture::port("output", "in"),
            )
            .unwrap();
        model.set_selection([NodeId::new("detection"), NodeId::new("output")]);
        model.take_events();

        let node = model.remove_node(&NodeId::new("detection")).unwrap();
        assert_eq!(node.id().as_str(), "detection");
        assert!(model.edges().is_empty());
        assert_eq!(model.selection(), [NodeId::new("output")]);
        let events = model.take_events();
        assert_eq!(events.len(), 2);
        match &events[0] {
            CanvasEvent::NodeRemoved { node, edges } => {
                assert_eq!(node.id().as_str(), "detection");
                let ids: Vec<EdgeId> = edges.iter().map(Edge::id).collect();
                assert_eq!(ids, [into, out]);
            }
            other => panic!("unexpected event {other:?}"),
        }
        assert_eq!(
            events[1],
            CanvasEvent::SelectionChanged(vec![NodeId::new("output")])
        );
        assert!(model.remove_node(&NodeId::new("detection")).is_none());
    }

    #[test]
    fn move_node_tells_where_it_was_and_where_it_is() {
        let mut model = Fixture::model();
        let id = NodeId::new("detection");
        assert!(model.move_node(&id, Pos2::new(10.0, 20.0)));
        assert_eq!(model.node(&id).unwrap().pos(), Pos2::new(10.0, 20.0));
        assert_eq!(
            model.take_events(),
            [CanvasEvent::NodeMoved {
                node: id.clone(),
                from: Pos2::new(200.0, 0.0),
                to: Pos2::new(10.0, 20.0),
            }]
        );
        // there already, or not there at all: nothing to tell
        assert!(!model.move_node(&id, Pos2::new(10.0, 20.0)));
        assert!(!model.move_node(&NodeId::new("nope"), Pos2::ZERO));
        assert!(model.take_events().is_empty());
    }

    #[test]
    fn selection_leaves_out_what_is_not_there_and_tells_only_changes() {
        let mut model = Fixture::model();
        model.set_selection([
            NodeId::new("input"),
            NodeId::new("nope"),
            NodeId::new("input"),
            NodeId::new("output"),
        ]);
        assert_eq!(
            model.selection(),
            [NodeId::new("input"), NodeId::new("output")]
        );
        assert_eq!(
            model.take_events(),
            [CanvasEvent::SelectionChanged(vec![
                NodeId::new("input"),
                NodeId::new("output"),
            ])]
        );
        // the same selection again tells nothing
        model.set_selection([NodeId::new("input"), NodeId::new("output")]);
        assert!(model.take_events().is_empty());
        model.clear_selection();
        assert!(model.selection().is_empty());
        assert_eq!(
            model.take_events(),
            [CanvasEvent::SelectionChanged(Vec::new())]
        );
    }

    #[test]
    fn edges_at_a_port_are_the_ones_that_start_or_end_there() {
        let mut model = Fixture::model();
        model
            .connect(
                Fixture::port("input", "out"),
                Fixture::port("detection", "in"),
            )
            .unwrap();
        model
            .connect(
                Fixture::port("detection", "true"),
                Fixture::port("output", "in"),
            )
            .unwrap();
        let port = Fixture::port("detection", "in");
        assert_eq!(model.edges_at(&port).count(), 1);
        let port = Fixture::port("detection", "false");
        assert_eq!(model.edges_at(&port).count(), 0);
    }
}
