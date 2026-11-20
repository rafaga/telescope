//! The canvas in a headless egui: what the pointer does to the model.

use egui::{Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_weave::{
    Canvas, CanvasModel, CanvasStyle, CanvasView, NodeDescriptor, NodeLayout, PortDescriptor,
    PortId, PortRef,
};

/// The graphs and gestures the tests use.
struct Fixture;

impl Fixture {
    /// An input and an output, apart from each other.
    fn model() -> CanvasModel {
        let mut model = CanvasModel::new();
        model
            .add_node(
                NodeDescriptor::new("input", "Input")
                    .at(Pos2::new(40.0, 40.0))
                    .output(PortDescriptor::new("out", "Lines").kind("text")),
            )
            .unwrap();
        model
            .add_node(
                NodeDescriptor::new("output", "Output")
                    .at(Pos2::new(400.0, 40.0))
                    .input(PortDescriptor::new("in", "Signal").kind("text")),
            )
            .unwrap();
        model.take_events();
        model
    }

    fn harness(model: CanvasModel) -> Harness<'static, (CanvasModel, CanvasView)> {
        let mut harness = Harness::builder()
            .with_accessibility_check(false)
            .with_size(Vec2::new(800.0, 400.0))
            .build_ui_state(
                |ui, (model, view): &mut (CanvasModel, CanvasView)| {
                    Canvas::new("canvas").show(ui, model, view);
                },
                (model, CanvasView::new()),
            );
        harness.run();
        harness
    }

    /// Where a port is on the screen: with the view at 100 % and over the
    /// origin the canvas and the screen are the same.
    fn port(model: &CanvasModel, node: &str, port: &str) -> Pos2 {
        let descriptor = model.node(&node.into()).unwrap();
        NodeLayout::of(descriptor, &CanvasStyle::default())
            .port_pos(&PortId::new(port))
            .unwrap()
    }

    /// Presses at `from`, drags to `to` in two steps and releases.
    fn drag(harness: &mut Harness<'_, (CanvasModel, CanvasView)>, from: Pos2, to: Pos2) {
        harness.hover_at(from);
        harness.run();
        harness.drag_at(from);
        harness.run();
        harness.hover_at(from + (to - from) * 0.5);
        harness.run();
        harness.hover_at(to);
        harness.run();
        harness.drop_at(to);
        harness.run();
    }
}

#[test]
fn a_click_on_a_node_selects_it() {
    let mut harness = Fixture::harness(Fixture::model());
    harness.get_by_label("Output").click();
    harness.run();
    assert_eq!(harness.state().0.selection().len(), 1);
    assert_eq!(harness.state().0.selection()[0].as_str(), "output");
}

#[test]
fn dragging_a_port_onto_another_joins_them() {
    let mut harness = Fixture::harness(Fixture::model());
    let from = Fixture::port(&harness.state().0, "input", "out");
    let to = Fixture::port(&harness.state().0, "output", "in");
    Fixture::drag(&mut harness, from, to);
    let model = &harness.state().0;
    assert_eq!(model.edges().len(), 1);
    assert_eq!(model.edges()[0].from(), &PortRef::new("input", "out"));
    assert_eq!(model.edges()[0].to(), &PortRef::new("output", "in"));
}

#[test]
fn dragging_a_port_to_nowhere_joins_nothing() {
    let mut harness = Fixture::harness(Fixture::model());
    let from = Fixture::port(&harness.state().0, "input", "out");
    Fixture::drag(&mut harness, from, Pos2::new(300.0, 300.0));
    assert!(harness.state().0.edges().is_empty());
}

#[test]
fn dragging_a_node_moves_it() {
    let mut harness = Fixture::harness(Fixture::model());
    let id = "input".into();
    let before = harness.state().0.node(&id).unwrap().pos();
    // the header of the node
    let from = before + Vec2::new(90.0, 13.0);
    Fixture::drag(&mut harness, from, from + Vec2::new(60.0, 40.0));
    let after = harness.state().0.node(&id).unwrap().pos();
    assert!(after.x > before.x && after.y > before.y);
}

#[test]
fn dragging_the_background_pans_the_view() {
    let mut harness = Fixture::harness(Fixture::model());
    let from = Pos2::new(300.0, 300.0);
    Fixture::drag(&mut harness, from, from + Vec2::new(50.0, 30.0));
    assert!(harness.state().1.offset().x > 0.0);
    assert!(harness.state().1.offset().y > 0.0);
}
