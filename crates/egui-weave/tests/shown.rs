//! The canvas shown in a headless egui.

use egui_kittest::Harness;
use egui_weave::{
    Canvas, CanvasModel, CanvasView, EdgeState, NodeDescriptor, PortDescriptor, PortRef,
};

#[test]
fn a_canvas_with_nodes_and_edges_is_shown_and_keeps_its_view() {
    let mut model = CanvasModel::new();
    model
        .add_node(
            NodeDescriptor::new("input", "Input")
                .summary("Intel channel")
                .output(PortDescriptor::new("out", "Lines")),
        )
        .unwrap();
    model
        .add_node(
            NodeDescriptor::new("output", "Output")
                .at(egui::pos2(300.0, 0.0))
                .input(PortDescriptor::new("in", "Signal")),
        )
        .unwrap();
    let edge = model
        .connect(PortRef::new("input", "out"), PortRef::new("output", "in"))
        .unwrap();
    model.set_edge_state(edge, EdgeState::True);
    let mut harness = Harness::builder()
        .with_accessibility_check(false)
        .with_size(egui::vec2(800.0, 400.0))
        .build_ui_state(
            |ui, (model, view): &mut (CanvasModel, CanvasView)| {
                Canvas::new("canvas").show(ui, model, view);
            },
            (model, CanvasView::new()),
        );
    harness.run();
    harness.run();
    let (_, view) = harness.state();
    assert!(view.zoom().is_finite());
    assert!(view.zoom() >= view.min_zoom() && view.zoom() <= view.max_zoom());
}
