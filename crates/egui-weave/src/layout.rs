//! How big a node is and where its ports are, in the coordinates of the
//! canvas. Nothing here draws.

use crate::controls::{ControlId, ControlKind};
use crate::model::{CanvasModel, NodeDescriptor, NodeId, PortId};
use egui::epaint::CubicBezierShape;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

/// The sizes and colors a canvas is drawn with, in the coordinates of the
/// canvas (the zoom scales them all).
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasStyle {
    /// The width of a node.
    pub node_width: f32,
    /// The height of the header with the title.
    pub header_height: f32,
    /// The height of the line with the summary and of each line of ports.
    pub row_height: f32,
    /// The space under the last line of a node.
    pub padding: f32,
    /// The radius of a port.
    pub port_radius: f32,
    /// The radius of the corners of a node.
    pub corner_radius: u8,
    /// The width of the line of an edge.
    pub edge_width: f32,
    /// The color of the edges that carried a true value.
    pub true_color: Color32,
    /// The color of the edges that carried a false value.
    pub false_color: Color32,
}

impl Default for CanvasStyle {
    fn default() -> Self {
        Self {
            node_width: 180.0,
            header_height: 26.0,
            row_height: 22.0,
            padding: 6.0,
            port_radius: 5.0,
            corner_radius: 6,
            edge_width: 2.0,
            true_color: Color32::from_rgb(76, 175, 80),
            false_color: Color32::from_rgb(229, 83, 75),
        }
    }
}

impl CanvasStyle {
    /// The four points of the curve of an edge from the output port at
    /// `from` to the input port at `to`: it leaves to the right and arrives
    /// from the left, however the two are placed.
    #[must_use]
    pub fn edge_control_points(&self, from: Pos2, to: Pos2) -> [Pos2; 4] {
        let reach = ((to.x - from.x).abs() * 0.5).max(40.0);
        [
            from,
            Pos2::new(from.x + reach, from.y),
            Pos2::new(to.x - reach, to.y),
            to,
        ]
    }

    /// How far `pos` is from the curve of an edge from the output port at
    /// `from` to the input port at `to`.
    #[must_use]
    pub fn edge_distance(&self, from: Pos2, to: Pos2, pos: Pos2) -> f32 {
        let curve = CubicBezierShape::from_points_stroke(
            self.edge_control_points(from, to),
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        );
        curve
            .flatten(Some(0.5))
            .windows(2)
            .map(|segment| Self::segment_distance(segment[0], segment[1], pos))
            .fold(f32::INFINITY, f32::min)
    }

    /// How far `pos` is from the segment between `a` and `b`.
    fn segment_distance(a: Pos2, b: Pos2, pos: Pos2) -> f32 {
        let line = b - a;
        let length_squared = line.length_sq();
        if length_squared <= f32::EPSILON {
            return a.distance(pos);
        }
        let along = ((pos - a).dot(line) / length_squared).clamp(0.0, 1.0);
        (a + line * along).distance(pos)
    }

    /// The nodes that `rect` (in the coordinates of the canvas) touches, from
    /// the one at the back to the one in front.
    #[must_use]
    pub fn nodes_in(&self, model: &CanvasModel, rect: Rect) -> Vec<NodeId> {
        model
            .nodes()
            .iter()
            .filter(|node| NodeLayout::of(node, self).rect().intersects(rect))
            .map(|node| node.id().clone())
            .collect()
    }

    /// The rectangle that holds all the nodes, or `None` without nodes.
    #[must_use]
    pub fn content_rect(&self, model: &CanvasModel) -> Option<Rect> {
        model
            .nodes()
            .iter()
            .map(|node| NodeLayout::of(node, self).rect())
            .reduce(Rect::union)
    }
}

/// How the canvas is drawn small inside a rectangle of the screen: the part
/// of the canvas that is shown (`world`) is scaled to fit, centered.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MinimapMap {
    world_center: Pos2,
    scale: f32,
    rect: Rect,
}

impl MinimapMap {
    /// The map of `world` (a rectangle of the canvas) into `rect`.
    pub(crate) fn new(world: Rect, rect: Rect) -> Self {
        let width = world.width().max(1.0);
        let height = world.height().max(1.0);
        Self {
            world_center: world.center(),
            scale: (rect.width() / width)
                .min(rect.height() / height)
                .max(f32::EPSILON),
            rect,
        }
    }

    /// Where a point of the canvas is in the minimap.
    pub(crate) fn to_map(&self, pos: Pos2) -> Pos2 {
        self.rect.center() + (pos - self.world_center) * self.scale
    }

    /// The point of the canvas that a point of the minimap shows.
    pub(crate) fn to_world(&self, pos: Pos2) -> Pos2 {
        self.world_center + (pos - self.rect.center()) / self.scale
    }
}

/// The place of a node and of its ports.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeLayout {
    rect: Rect,
    header: Rect,
    summary: Option<Rect>,
    inputs: Vec<(PortId, Pos2)>,
    outputs: Vec<(PortId, Pos2)>,
    controls: Vec<ControlRow>,
}

/// The line of a node that a control, or one option of a group of radio
/// buttons, is drawn in.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlRow {
    control: ControlId,
    option: Option<usize>,
    rect: Rect,
}

impl ControlRow {
    /// The control the line belongs to.
    #[must_use]
    pub fn control(&self) -> &ControlId {
        &self.control
    }

    /// The option of a group of radio buttons, or `None` for the other
    /// controls.
    #[must_use]
    pub fn option(&self) -> Option<usize> {
        self.option
    }

    /// Where the line is, kept off the ports on the sides of the node.
    #[must_use]
    pub fn rect(&self) -> Rect {
        self.rect
    }
}

impl NodeLayout {
    /// The layout of `node` with `style`. The input ports go down the left
    /// side and the output ports down the right side, one per line, under the
    /// header and the summary.
    #[must_use]
    pub fn of(node: &NodeDescriptor, style: &CanvasStyle) -> Self {
        let min = node.pos();
        let header = Rect::from_min_size(min, Vec2::new(style.node_width, style.header_height));
        let has_summary = !node.summary_text().is_empty();
        let summary_height = if has_summary { style.row_height } else { 0.0 };
        let summary = has_summary.then(|| {
            Rect::from_min_size(
                Pos2::new(min.x, header.max.y),
                Vec2::new(style.node_width, style.row_height),
            )
        });
        let rows_top = header.max.y + summary_height;
        let rows = node.inputs().len().max(node.outputs().len());
        let controls_top = rows_top + rows as f32 * style.row_height;
        let mut controls = Vec::new();
        for control in node.controls() {
            let options: Vec<Option<usize>> = match control.kind() {
                ControlKind::Radio(options) => (0..options.len()).map(Some).collect(),
                ControlKind::Button | ControlKind::Check | ControlKind::Switch => vec![None],
            };
            for option in options {
                let top = controls_top + controls.len() as f32 * style.row_height;
                controls.push(ControlRow {
                    control: control.id().clone(),
                    option,
                    rect: Rect::from_min_size(
                        Pos2::new(min.x + 12.0, top),
                        Vec2::new(style.node_width - 24.0, style.row_height),
                    ),
                });
            }
        }
        let height = style.header_height
            + summary_height
            + (rows + controls.len()) as f32 * style.row_height
            + style.padding;
        let row_center = |index: usize| rows_top + (index as f32 + 0.5) * style.row_height;
        let inputs = node
            .inputs()
            .iter()
            .enumerate()
            .map(|(index, port)| (port.id().clone(), Pos2::new(min.x, row_center(index))))
            .collect();
        let outputs = node
            .outputs()
            .iter()
            .enumerate()
            .map(|(index, port)| {
                (
                    port.id().clone(),
                    Pos2::new(min.x + style.node_width, row_center(index)),
                )
            })
            .collect();
        Self {
            rect: Rect::from_min_size(min, Vec2::new(style.node_width, height)),
            header,
            summary,
            inputs,
            outputs,
            controls,
        }
    }

    /// The whole node.
    #[must_use]
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The header, with the title.
    #[must_use]
    pub fn header(&self) -> Rect {
        self.header
    }

    /// The line with the summary, when the node has one.
    #[must_use]
    pub fn summary(&self) -> Option<Rect> {
        self.summary
    }

    /// The input ports and where they are, on the left edge.
    #[must_use]
    pub fn inputs(&self) -> &[(PortId, Pos2)] {
        &self.inputs
    }

    /// The output ports and where they are, on the right edge.
    #[must_use]
    pub fn outputs(&self) -> &[(PortId, Pos2)] {
        &self.outputs
    }

    /// The lines of the controls, under the ports, in order.
    #[must_use]
    pub fn controls(&self) -> &[ControlRow] {
        &self.controls
    }

    /// Where the port `id` is, on either side.
    #[must_use]
    pub fn port_pos(&self, id: &PortId) -> Option<Pos2> {
        self.inputs
            .iter()
            .chain(&self.outputs)
            .find(|(port, _)| port == id)
            .map(|(_, pos)| *pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::NodeControl;
    use crate::model::PortDescriptor;

    /// What the tests share.
    struct Fixture;

    impl Fixture {
        fn node() -> NodeDescriptor {
            NodeDescriptor::new("detection", "Detection")
                .at(Pos2::new(100.0, 50.0))
                .summary("Gate camp")
                .input(PortDescriptor::new("in", "Lines"))
                .output(PortDescriptor::new("true", "True"))
                .output(PortDescriptor::new("false", "False"))
        }
    }

    #[test]
    fn a_node_is_as_tall_as_its_header_summary_and_longest_side() {
        let style = CanvasStyle::default();
        let layout = NodeLayout::of(&Fixture::node(), &style);
        // header 26 + summary 22 + two lines of 22 + padding 6
        assert_eq!(layout.rect().min, Pos2::new(100.0, 50.0));
        assert_eq!(layout.rect().width(), 180.0);
        assert_eq!(layout.rect().height(), 26.0 + 22.0 + 44.0 + 6.0);
        assert_eq!(layout.header().height(), 26.0);
        assert_eq!(layout.summary().map(|rect| rect.min.y), Some(76.0));
    }

    #[test]
    fn the_controls_go_under_the_ports_one_line_each() {
        let style = CanvasStyle::default();
        let node = NodeDescriptor::new("a", "A")
            .input(PortDescriptor::new("in", "In"))
            .control(NodeControl::check("case", "Ignore case", false))
            .control(NodeControl::radio("mode", "Mode", ["One", "Two"], 0))
            .control(NodeControl::button("go", "Go"));
        let layout = NodeLayout::of(&node, &style);
        // header 26 + one line of ports + four lines of controls + padding 6
        assert_eq!(layout.rect().height(), 26.0 + 22.0 * 5.0 + 6.0);
        let rows = layout.controls();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].rect().min.y, 26.0 + 22.0);
        assert_eq!(rows[1].option(), Some(0));
        assert_eq!(rows[2].option(), Some(1));
        assert_eq!(rows[2].control(), rows[1].control());
        assert_eq!(rows[3].option(), None);
        // kept off the ports on the sides
        assert_eq!(rows[0].rect().min.x, 12.0);
        assert_eq!(rows[0].rect().width(), 180.0 - 24.0);
    }

    #[test]
    fn without_a_summary_the_ports_start_under_the_header() {
        let style = CanvasStyle::default();
        let node = NodeDescriptor::new("a", "A")
            .at(Pos2::ZERO)
            .input(PortDescriptor::new("in", "In"));
        let layout = NodeLayout::of(&node, &style);
        assert!(layout.summary().is_none());
        assert_eq!(layout.rect().height(), 26.0 + 22.0 + 6.0);
        assert_eq!(
            layout.port_pos(&PortId::new("in")),
            Some(Pos2::new(0.0, 26.0 + 11.0))
        );
    }

    #[test]
    fn inputs_are_on_the_left_and_outputs_on_the_right() {
        let style = CanvasStyle::default();
        let layout = NodeLayout::of(&Fixture::node(), &style);
        let input = layout.port_pos(&PortId::new("in")).unwrap();
        let yes = layout.port_pos(&PortId::new("true")).unwrap();
        let no = layout.port_pos(&PortId::new("false")).unwrap();
        assert_eq!(input.x, 100.0);
        assert_eq!(yes.x, 280.0);
        assert_eq!(no.x, 280.0);
        // one line per port, in the order they were added
        assert_eq!(input.y, yes.y);
        assert_eq!(no.y - yes.y, 22.0);
        assert!(layout.port_pos(&PortId::new("nope")).is_none());
    }

    #[test]
    fn a_node_without_ports_has_a_header_and_padding_only() {
        let style = CanvasStyle::default();
        let layout = NodeLayout::of(&NodeDescriptor::new("a", "A"), &style);
        assert_eq!(layout.rect().height(), 26.0 + 6.0);
        assert!(layout.inputs().is_empty());
        assert!(layout.outputs().is_empty());
    }

    #[test]
    fn an_edge_leaves_to_the_right_and_arrives_from_the_left() {
        let style = CanvasStyle::default();
        let from = Pos2::new(280.0, 100.0);
        let to = Pos2::new(400.0, 140.0);
        let [start, first, second, end] = style.edge_control_points(from, to);
        assert_eq!(start, from);
        assert_eq!(end, to);
        assert!(first.x > from.x);
        assert!(second.x < to.x);
        assert_eq!(first.y, from.y);
        assert_eq!(second.y, to.y);
        // going backwards still reaches out by at least 40
        let [_, back_first, back_second, _] = style.edge_control_points(to, from);
        assert!(back_first.x >= to.x + 40.0);
        assert!(back_second.x <= from.x - 40.0);
    }

    #[test]
    fn the_distance_to_an_edge_is_the_distance_to_its_curve() {
        let style = CanvasStyle::default();
        // the points are in a line, so the curve is that line
        let from = Pos2::new(0.0, 0.0);
        let to = Pos2::new(200.0, 0.0);
        let near = style.edge_distance(from, to, Pos2::new(100.0, 5.0));
        assert!((near - 5.0).abs() < 0.01);
        let beyond = style.edge_distance(from, to, Pos2::new(300.0, 0.0));
        assert!((beyond - 100.0).abs() < 0.01);
        assert!(style.edge_distance(from, to, Pos2::new(50.0, 0.0)) < 0.01);
    }

    #[test]
    fn a_rectangle_takes_the_nodes_it_touches() {
        let style = CanvasStyle::default();
        let mut model = CanvasModel::new();
        for (id, x) in [("a", 0.0), ("b", 300.0), ("c", 600.0)] {
            model
                .add_node(NodeDescriptor::new(id, id).at(Pos2::new(x, 0.0)))
                .unwrap();
        }
        // from the middle of the first node to the middle of the second
        let rect = Rect::from_two_pos(Pos2::new(90.0, 10.0), Pos2::new(390.0, 20.0));
        let ids: Vec<String> = style
            .nodes_in(&model, rect)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(ids, ["a", "b"]);
        // a rectangle in the gap between nodes takes none
        let gap = Rect::from_two_pos(Pos2::new(200.0, 0.0), Pos2::new(250.0, 20.0));
        assert!(style.nodes_in(&model, gap).is_empty());
    }

    #[test]
    fn the_minimap_fits_the_world_into_its_rectangle_centered() {
        let world = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 500.0));
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(210.0, 120.0));
        let map = MinimapMap::new(world, rect);
        let corner = map.to_map(world.min);
        let far = map.to_map(world.max);
        assert!((corner.x - 10.0).abs() < 1e-3 && (corner.y - 20.0).abs() < 1e-3);
        assert!((far.x - 210.0).abs() < 1e-3 && (far.y - 120.0).abs() < 1e-3);
        // a tall world leaves room at the sides
        let tall = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 400.0));
        let map = MinimapMap::new(tall, rect);
        assert!(map.to_map(tall.min).x > rect.min.x);
        assert!(map.to_map(tall.max).x < rect.max.x);
    }

    #[test]
    fn a_point_goes_to_the_minimap_and_back() {
        let world = Rect::from_min_max(Pos2::new(-300.0, -100.0), Pos2::new(700.0, 400.0));
        let rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(210.0, 120.0));
        let map = MinimapMap::new(world, rect);
        let pos = Pos2::new(123.0, 45.0);
        let back = map.to_world(map.to_map(pos));
        assert!((back.x - pos.x).abs() < 1e-3 && (back.y - pos.y).abs() < 1e-3);
    }

    #[test]
    fn the_content_rect_holds_every_node() {
        let style = CanvasStyle::default();
        let mut model = CanvasModel::new();
        assert!(style.content_rect(&model).is_none());
        model
            .add_node(NodeDescriptor::new("a", "A").at(Pos2::new(0.0, 0.0)))
            .unwrap();
        model
            .add_node(NodeDescriptor::new("b", "B").at(Pos2::new(500.0, 300.0)))
            .unwrap();
        let rect = style.content_rect(&model).unwrap();
        assert_eq!(rect.min, Pos2::new(0.0, 0.0));
        assert_eq!(rect.max, Pos2::new(680.0, 332.0));
    }
}
