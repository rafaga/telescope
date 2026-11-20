//! The controls of a node in a headless egui: what a click does to the model.

use egui::{Pos2, Vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use egui_weave::{
    Canvas, CanvasEvent, CanvasModel, CanvasView, ControlId, ControlValue, NodeControl,
    NodeDescriptor,
};

/// The graph and the clicks the tests use.
struct Fixture;

impl Fixture {
    /// A node with one control of each kind.
    fn harness() -> Harness<'static, (CanvasModel, CanvasView)> {
        let mut model = CanvasModel::new();
        model
            .add_node(
                NodeDescriptor::new("rule", "Rule")
                    .at(Pos2::new(40.0, 40.0))
                    .control(NodeControl::check("case", "Ignore case", false))
                    .control(NodeControl::switch("notify", "Notify", false))
                    .control(NodeControl::radio("mode", "Mode", ["First", "Second"], 0))
                    .control(NodeControl::button("go", "Go")),
            )
            .unwrap();
        model.take_events();
        let mut harness = Harness::builder()
            .with_accessibility_check(false)
            .with_size(Vec2::new(500.0, 400.0))
            .build_ui_state(
                |ui, (model, view): &mut (CanvasModel, CanvasView)| {
                    Canvas::new("canvas").show(ui, model, view);
                },
                (model, CanvasView::new()),
            );
        harness.run();
        harness
    }

    /// What the control `control` of the node changed to, once `label` was
    /// clicked.
    fn clicked(label: &str) -> Vec<CanvasEvent> {
        let mut harness = Self::harness();
        harness.get_by_label(label).click();
        harness.run();
        harness.state_mut().0.take_events()
    }

    fn changed(control: &str, from: ControlValue, to: ControlValue) -> CanvasEvent {
        CanvasEvent::ControlChanged {
            node: "rule".into(),
            control: ControlId::new(control),
            from,
            to,
        }
    }
}

#[test]
fn a_check_box_changes_its_value_and_tells_it() {
    let events = Fixture::clicked("Ignore case");
    assert!(events.contains(&Fixture::changed(
        "case",
        ControlValue::Flag(false),
        ControlValue::Flag(true)
    )));
}

#[test]
fn a_switch_changes_its_value_and_tells_it() {
    let events = Fixture::clicked("Notify");
    assert!(events.contains(&Fixture::changed(
        "notify",
        ControlValue::Flag(false),
        ControlValue::Flag(true)
    )));
}

#[test]
fn a_radio_button_selects_its_option() {
    let events = Fixture::clicked("Second");
    assert!(events.contains(&Fixture::changed(
        "mode",
        ControlValue::Choice(0),
        ControlValue::Choice(1)
    )));
}

#[test]
fn a_button_tells_it_was_pressed_and_changes_nothing() {
    let events = Fixture::clicked("Go");
    assert!(events.contains(&CanvasEvent::ButtonPressed {
        node: "rule".into(),
        control: ControlId::new("go"),
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CanvasEvent::ControlChanged { .. }))
    );
}

#[test]
fn clicking_a_control_does_not_move_the_node() {
    let events = Fixture::clicked("Ignore case");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CanvasEvent::NodeMoved { .. }))
    );
}
