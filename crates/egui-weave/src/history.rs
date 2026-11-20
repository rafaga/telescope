//! Undo and redo of what the user does on a [`CanvasModel`].
//!
//! The history is fed with the events the model tells
//! ([`CanvasModel::take_events`]); each batch of events is a step that is
//! undone or redone at once. The selection and the connections that were
//! let go over nothing are not part of it.

use crate::model::{CanvasEvent, CanvasModel, NodeId};

/// The steps that can be undone and the ones that can be redone.
#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<Vec<CanvasEvent>>,
    redo: Vec<Vec<CanvasEvent>>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// An empty history that keeps the last 100 steps.
    #[must_use]
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: 100,
        }
    }

    /// The most steps that are kept: the oldest ones are forgotten.
    #[must_use]
    pub fn limit(mut self, steps: usize) -> Self {
        self.limit = steps.max(1);
        self
    }

    /// Adds what the model told (`CanvasModel::take_events`) as one step.
    ///
    /// While `gesture` is true (the pointer is down, for example) a step
    /// that only moves the same nodes as the one before is joined to it, so
    /// that a drag is undone at once and not frame by frame. Anything that is
    /// recorded drops the steps that could be redone.
    pub fn record(&mut self, events: Vec<CanvasEvent>, gesture: bool) {
        let step: Vec<CanvasEvent> = events
            .into_iter()
            .filter(|event| {
                !matches!(
                    event,
                    CanvasEvent::SelectionChanged(_)
                        | CanvasEvent::ConnectionDropped { .. }
                        | CanvasEvent::ButtonPressed { .. }
                )
            })
            .collect();
        if step.is_empty() {
            return;
        }
        self.redo.clear();
        if gesture
            && let Some(last) = self.undo.last_mut()
            && Self::merge(last, &step)
        {
            return;
        }
        self.undo.push(step);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Whether there is a step to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forgets every step.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Undoes the last step in `model`. Returns whether there was one. What
    /// the model told before has to be recorded first.
    pub fn undo(&mut self, model: &mut CanvasModel) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        for event in step.iter().rev() {
            Self::revert(model, event);
        }
        // what the undoing told is not a new step
        model.take_events();
        self.redo.push(step);
        true
    }

    /// Redoes the last step that was undone. Returns whether there was one.
    pub fn redo(&mut self, model: &mut CanvasModel) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        for event in &step {
            Self::apply(model, event);
        }
        model.take_events();
        self.undo.push(step);
        true
    }

    /// The nodes a step moves, or `None` when it does anything else.
    fn moved_nodes(step: &[CanvasEvent]) -> Option<Vec<NodeId>> {
        let mut nodes = Vec::new();
        for event in step {
            match event {
                CanvasEvent::NodeMoved { node, .. } => nodes.push(node.clone()),
                CanvasEvent::NodeAdded(_)
                | CanvasEvent::NodeRemoved { .. }
                | CanvasEvent::Connected(_)
                | CanvasEvent::Disconnected(_)
                | CanvasEvent::EdgeChanged { .. }
                | CanvasEvent::ControlChanged { .. }
                | CanvasEvent::ButtonPressed { .. }
                | CanvasEvent::SelectionChanged(_)
                | CanvasEvent::ConnectionDropped { .. } => return None,
            }
        }
        nodes.sort();
        nodes.dedup();
        Some(nodes)
    }

    /// Joins `new` to `last` when both only move the same nodes. Returns
    /// whether it did.
    fn merge(last: &mut [CanvasEvent], new: &[CanvasEvent]) -> bool {
        let (Some(before), Some(now)) = (Self::moved_nodes(last), Self::moved_nodes(new)) else {
            return false;
        };
        if before != now {
            return false;
        }
        for event in new {
            let CanvasEvent::NodeMoved { node, to, .. } = event else {
                continue;
            };
            for prior in last.iter_mut() {
                if let CanvasEvent::NodeMoved {
                    node: prior_node,
                    to: prior_to,
                    ..
                } = prior
                    && prior_node == node
                {
                    *prior_to = *to;
                }
            }
        }
        true
    }

    /// Does the opposite of what `event` told.
    fn revert(model: &mut CanvasModel, event: &CanvasEvent) {
        match event {
            CanvasEvent::NodeAdded(node) => {
                model.remove_node(node.id());
            }
            CanvasEvent::NodeRemoved { node, edges } => {
                // the node is as it was, so there is no reason to refuse it
                let _ = model.add_node(node.clone());
                for edge in edges {
                    model.restore_edge(edge.clone());
                }
            }
            CanvasEvent::NodeMoved { node, from, .. } => {
                model.move_node(node, *from);
            }
            CanvasEvent::Connected(edge) => {
                model.disconnect(edge.id());
            }
            CanvasEvent::Disconnected(edge) => model.restore_edge(edge.clone()),
            CanvasEvent::EdgeChanged { edge, from, .. } => {
                model.set_edge_properties(*edge, *from);
            }
            CanvasEvent::ControlChanged {
                node,
                control,
                from,
                ..
            } => {
                model.set_control(node, control, *from);
            }
            CanvasEvent::SelectionChanged(_)
            | CanvasEvent::ConnectionDropped { .. }
            | CanvasEvent::ButtonPressed { .. } => {}
        }
    }

    /// Does again what `event` told.
    fn apply(model: &mut CanvasModel, event: &CanvasEvent) {
        match event {
            CanvasEvent::NodeAdded(node) => {
                let _ = model.add_node(node.clone());
            }
            CanvasEvent::NodeRemoved { node, .. } => {
                model.remove_node(node.id());
            }
            CanvasEvent::NodeMoved { node, to, .. } => {
                model.move_node(node, *to);
            }
            CanvasEvent::Connected(edge) => model.restore_edge(edge.clone()),
            CanvasEvent::Disconnected(edge) => {
                model.disconnect(edge.id());
            }
            CanvasEvent::EdgeChanged { edge, to, .. } => {
                model.set_edge_properties(*edge, *to);
            }
            CanvasEvent::ControlChanged {
                node, control, to, ..
            } => {
                model.set_control(node, control, *to);
            }
            CanvasEvent::SelectionChanged(_)
            | CanvasEvent::ConnectionDropped { .. }
            | CanvasEvent::ButtonPressed { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{ControlId, ControlValue, NodeControl};
    use crate::edge_style::{EdgeProperties, LineStyle};
    use crate::model::{NodeDescriptor, NodeId, PortDescriptor, PortRef};
    use egui::{Color32, Pos2};

    /// What the tests share.
    struct Fixture;

    impl Fixture {
        /// Two nodes that can be joined, with the events already taken.
        fn model() -> CanvasModel {
            let mut model = CanvasModel::new();
            model
                .add_node(
                    NodeDescriptor::new("a", "A")
                        .at(Pos2::new(0.0, 0.0))
                        .output(PortDescriptor::new("out", "Out")),
                )
                .unwrap();
            model
                .add_node(
                    NodeDescriptor::new("b", "B")
                        .at(Pos2::new(200.0, 0.0))
                        .input(PortDescriptor::new("in", "In")),
                )
                .unwrap();
            model.take_events();
            model
        }

        fn names(model: &CanvasModel) -> Vec<String> {
            model.nodes().iter().map(|n| n.id().to_string()).collect()
        }
    }

    #[test]
    fn selection_alone_is_not_a_step() {
        let mut model = Fixture::model();
        let mut history = History::new();
        model.set_selection([NodeId::new("a")]);
        history.record(model.take_events(), false);
        assert!(!history.can_undo());
    }

    #[test]
    fn an_added_node_is_removed_and_added_again() {
        let mut model = Fixture::model();
        let mut history = History::new();
        model.add_node(NodeDescriptor::new("c", "C")).unwrap();
        history.record(model.take_events(), false);
        assert!(history.undo(&mut model));
        assert_eq!(Fixture::names(&model), ["a", "b"]);
        assert!(history.can_redo());
        assert!(history.redo(&mut model));
        assert_eq!(Fixture::names(&model), ["a", "b", "c"]);
        // undoing and redoing is not recorded as new steps
        assert!(model.take_events().is_empty());
    }

    #[test]
    fn an_edge_comes_back_with_its_id() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let id = model
            .connect(PortRef::new("a", "out"), PortRef::new("b", "in"))
            .unwrap();
        history.record(model.take_events(), false);
        assert!(history.undo(&mut model));
        assert!(model.edge(id).is_none());
        assert!(history.redo(&mut model));
        assert!(model.edge(id).is_some());
        // the number is not used again
        model.disconnect(id);
        let next = model
            .connect(PortRef::new("a", "out"), PortRef::new("b", "in"))
            .unwrap();
        assert_ne!(next, id);
    }

    #[test]
    fn a_removed_node_comes_back_with_its_edges() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let id = model
            .connect(PortRef::new("a", "out"), PortRef::new("b", "in"))
            .unwrap();
        model.take_events();
        model.remove_node(&NodeId::new("b"));
        history.record(model.take_events(), false);
        assert!(model.edges().is_empty());
        assert!(history.undo(&mut model));
        assert_eq!(Fixture::names(&model).len(), 2);
        assert_eq!(model.edge(id).map(|e| e.id()), Some(id));
        assert!(history.redo(&mut model));
        assert_eq!(Fixture::names(&model), ["a"]);
        assert!(model.edges().is_empty());
    }

    #[test]
    fn a_drag_is_one_step_while_the_gesture_lasts() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let id = NodeId::new("a");
        for x in [10.0, 20.0, 30.0] {
            model.move_node(&id, Pos2::new(x, 0.0));
            history.record(model.take_events(), true);
        }
        assert!(history.undo(&mut model));
        assert_eq!(model.node(&id).unwrap().pos(), Pos2::new(0.0, 0.0));
        assert!(!history.can_undo());
        assert!(history.redo(&mut model));
        assert_eq!(model.node(&id).unwrap().pos(), Pos2::new(30.0, 0.0));
    }

    #[test]
    fn two_drags_of_the_same_node_are_two_steps() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let id = NodeId::new("a");
        model.move_node(&id, Pos2::new(10.0, 0.0));
        history.record(model.take_events(), true);
        // the pointer was released: the next move is another gesture
        model.move_node(&id, Pos2::new(50.0, 0.0));
        history.record(model.take_events(), false);
        assert!(history.undo(&mut model));
        assert_eq!(model.node(&id).unwrap().pos(), Pos2::new(10.0, 0.0));
        assert!(history.undo(&mut model));
        assert_eq!(model.node(&id).unwrap().pos(), Pos2::new(0.0, 0.0));
    }

    #[test]
    fn moving_other_nodes_starts_another_step() {
        let mut model = Fixture::model();
        let mut history = History::new();
        model.move_node(&NodeId::new("a"), Pos2::new(10.0, 0.0));
        history.record(model.take_events(), true);
        model.move_node(&NodeId::new("b"), Pos2::new(210.0, 0.0));
        history.record(model.take_events(), true);
        assert!(history.undo(&mut model));
        assert_eq!(
            model.node(&NodeId::new("a")).unwrap().pos(),
            Pos2::new(10.0, 0.0)
        );
        assert!(history.can_undo());
    }

    #[test]
    fn recording_drops_what_could_be_redone() {
        let mut model = Fixture::model();
        let mut history = History::new();
        model.add_node(NodeDescriptor::new("c", "C")).unwrap();
        history.record(model.take_events(), false);
        history.undo(&mut model);
        assert!(history.can_redo());
        model.add_node(NodeDescriptor::new("d", "D")).unwrap();
        history.record(model.take_events(), false);
        assert!(!history.can_redo());
    }

    #[test]
    fn the_oldest_steps_are_forgotten_past_the_limit() {
        let mut model = Fixture::model();
        let mut history = History::new().limit(2);
        for name in ["c", "d", "e"] {
            model.add_node(NodeDescriptor::new(name, name)).unwrap();
            history.record(model.take_events(), false);
        }
        assert!(history.undo(&mut model));
        assert!(history.undo(&mut model));
        assert!(!history.undo(&mut model));
        assert_eq!(Fixture::names(&model), ["a", "b", "c"]);
    }

    #[test]
    fn without_steps_there_is_nothing_to_undo_or_redo() {
        let mut model = Fixture::model();
        let mut history = History::new();
        assert!(!history.undo(&mut model));
        assert!(!history.redo(&mut model));
        history.clear();
        assert!(!history.can_undo() && !history.can_redo());
    }

    #[test]
    fn the_properties_of_an_edge_are_undone_and_redone() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let edge = model
            .connect(PortRef::new("a", "out"), PortRef::new("b", "in"))
            .unwrap();
        history.record(model.take_events(), false);
        let dashed_red = EdgeProperties::new()
            .color(Some(Color32::RED))
            .line(LineStyle::Dashed)
            .width(Some(4.0));
        assert!(model.set_edge_properties(edge, dashed_red));
        assert!(!model.set_edge_properties(edge, dashed_red));
        history.record(model.take_events(), false);
        assert_eq!(model.edge(edge).unwrap().properties(), &dashed_red);
        assert!(history.undo(&mut model));
        assert_eq!(
            model.edge(edge).unwrap().properties(),
            &EdgeProperties::new()
        );
        assert!(history.redo(&mut model));
        assert_eq!(model.edge(edge).unwrap().properties(), &dashed_red);
    }

    #[test]
    fn an_edge_that_comes_back_keeps_its_properties() {
        let mut model = Fixture::model();
        let mut history = History::new();
        let edge = model
            .connect(PortRef::new("a", "out"), PortRef::new("b", "in"))
            .unwrap();
        let dotted = EdgeProperties::new().line(LineStyle::Dotted);
        model.set_edge_properties(edge, dotted);
        model.take_events();
        model.disconnect(edge);
        history.record(model.take_events(), false);
        assert!(history.undo(&mut model));
        assert_eq!(model.edge(edge).unwrap().properties(), &dotted);
    }

    #[test]
    fn a_control_is_undone_and_redone_and_a_press_is_not_a_step() {
        let mut model = CanvasModel::new();
        model
            .add_node(
                NodeDescriptor::new("a", "A")
                    .control(NodeControl::check("case", "Ignore case", false))
                    .control(NodeControl::button("go", "Go")),
            )
            .unwrap();
        model.take_events();
        let mut history = History::new();
        let node = NodeId::new("a");
        let case = ControlId::new("case");
        assert!(model.set_control(&node, &case, ControlValue::Flag(true)));
        assert!(model.press(&node, &ControlId::new("go")));
        history.record(model.take_events(), false);
        assert!(history.undo(&mut model));
        assert_eq!(
            model
                .node(&node)
                .unwrap()
                .control_named(&case)
                .unwrap()
                .value(),
            Some(ControlValue::Flag(false))
        );
        assert!(!history.can_undo());
        assert!(history.redo(&mut model));
        assert_eq!(
            model
                .node(&node)
                .unwrap()
                .control_named(&case)
                .unwrap()
                .value(),
            Some(ControlValue::Flag(true))
        );
    }
}
