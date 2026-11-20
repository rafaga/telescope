//! The widget that shows a [`CanvasModel`] and lets the user edit it: the
//! nodes with their ports and the edges between them, inside a surface that
//! is panned and zoomed.

use crate::controls::{ControlId, ControlKind, ControlValue};
use crate::edge_style::LineStyle;
use crate::layout::{CanvasStyle, MinimapMap, NodeLayout};
use crate::model::{
    CanvasModel, EdgeId, EdgeState, NodeDescriptor, NodeId, PortDirection, PortRef,
};
use crate::view::CanvasView;
use egui::epaint::CubicBezierShape;
use egui::{
    Align2, Color32, CornerRadius, Id, Key, Pos2, Rect, Response, Role, Scene, Sense, Shape,
    Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo,
};
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

/// Shows a [`CanvasModel`] and edits it with the pointer:
///
/// * dragging the background pans the canvas, the wheel or a pinch zooms it,
///   and a click on the background clears the selection; dragging it with
///   shift held draws a rectangle that selects the nodes it touches;
/// * a click on a node selects it (with shift or the command key it is added
///   to or taken from the selection) and dragging moves the selected nodes;
/// * dragging from a port to another one joins them, as long as
///   [`CanvasModel::check_connection`] allows it: the ports that take the
///   edge are ringed, and the reason is shown when the one under the pointer
///   does not;
/// * an edge let go over nothing is told with
///   [`CanvasEvent::ConnectionDropped`](crate::CanvasEvent::ConnectionDropped)
///   and, when the application gave it a [`drop_menu`](Self::drop_menu), a
///   popup with what that draws opens where it was let go;
/// * a double click on an edge removes it, and the delete key removes the
///   selected nodes.
///
/// The pan and the zoom are kept in a [`CanvasView`] that the application
/// owns, so that it can be driven from a toolbar. What the user changes is
/// told by [`CanvasModel::take_events`].
pub struct Canvas<'a> {
    id: Id,
    style: CanvasStyle,
    drop_menu: Option<DropMenu<'a>>,
    preview: Option<NodeDescriptor>,
    minimap: Option<Vec2>,
    pan_limit: Option<f32>,
    zoom_slider: bool,
}

/// What draws the menu of an edge that was let go over nothing: it gets the
/// model, the port the edge came from and where it was let go, and tells
/// whether the menu is done (something was chosen) and has to close.
type DropMenu<'a> = Box<dyn FnMut(&mut Ui, &mut CanvasModel, &PortRef, Pos2) -> bool + 'a>;

impl std::fmt::Debug for Canvas<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canvas")
            .field("id", &self.id)
            .field("style", &self.style)
            .field("drop_menu", &self.drop_menu.is_some())
            .finish()
    }
}

impl<'a> Canvas<'a> {
    /// A canvas that is told apart from the others in the same place by
    /// `id_salt`.
    pub fn new(id_salt: impl Hash + Debug) -> Self {
        Self {
            id: Id::unique(id_salt),
            style: CanvasStyle::default(),
            drop_menu: None,
            preview: None,
            minimap: None,
            pan_limit: Some(0.5),
            zoom_slider: false,
        }
    }

    /// Shows a minimap of `size` points in the bottom right corner: the nodes
    /// and edges small, and a frame over the part that is in view. A click or
    /// a drag on it moves the view there. It is left out when the canvas is
    /// too small for it.
    #[must_use]
    pub fn minimap(mut self, size: Vec2) -> Self {
        self.minimap = Some(size);
        self
    }

    /// Shows a vertical slider of the zoom, with the percentage under it, in
    /// the top right corner (the minimap goes in the bottom right one). Its
    /// panel is see-through until the pointer is over it. It is left out when
    /// the canvas is too small for it.
    #[must_use]
    pub fn zoom_slider(mut self, shown: bool) -> Self {
        self.zoom_slider = shown;
        self
    }

    /// How far the user can scroll past what is drawn, as a fraction of the
    /// size of what is in view (`0.5` by default); `None` leaves the view
    /// free. With no nodes there is nothing to limit to.
    #[must_use]
    pub fn pan_limit(mut self, margin: Option<f32>) -> Self {
        self.pan_limit = margin;
        self
    }

    /// A node to preview while the pointer is over the canvas: a faint box
    /// with its header, title and ports, centered on the pointer, at the zoom
    /// of the canvas. It is what to show while an entry of a toolbox is
    /// dragged over the canvas (`None` shows nothing).
    #[must_use]
    pub fn drop_preview(mut self, node: Option<NodeDescriptor>) -> Self {
        self.preview = node;
        self
    }

    /// The menu that opens where an edge dragged from a port is let go over
    /// nothing. `menu` draws its content in a popup and gets the model, to
    /// add the node that is chosen and join it, the port the edge came from
    /// and the place in the canvas. It returns `true` when something was
    /// chosen, to close the popup; the popup also closes with escape or a
    /// click outside.
    #[must_use]
    pub fn drop_menu(
        mut self,
        menu: impl FnMut(&mut Ui, &mut CanvasModel, &PortRef, Pos2) -> bool + 'a,
    ) -> Self {
        self.drop_menu = Some(Box::new(menu));
        self
    }

    /// The sizes and colors to draw with.
    #[must_use]
    pub fn style(mut self, style: CanvasStyle) -> Self {
        self.style = style;
        self
    }

    /// Shows the model in the space that is left in `ui`. The pan and the
    /// zoom the user makes are written into `view`, and the edits into
    /// `model`.
    pub fn show(mut self, ui: &mut Ui, model: &mut CanvasModel, view: &mut CanvasView) -> Response {
        ui.push_id(self.id, |ui| {
            let viewport = ui.available_rect_before_wrap();
            let usable = viewport.width() > 1.0 && viewport.height() > 1.0;
            // the surface keeps the part of the canvas that is in view
            let mut scene_rect = view.visible_rect(viewport);
            let visible = scene_rect;
            // the surface only zooms: the panning by dragging is done here,
            // so that the nodes can be dragged over the background
            let shown = Scene::new()
                .sense(Sense::hover())
                .zoom_range(view.min_zoom()..=view.max_zoom())
                .show(ui, &mut scene_rect, |ui| self.edit(ui, model, visible));
            scene_rect = scene_rect.translate(-shown.inner);
            if usable {
                view.set_from_scene_rect(scene_rect, viewport);
            }
            if usable
                && let Some(margin) = self.pan_limit
                && let Some(content) = self.style.content_rect(model)
            {
                view.limit_to(content, viewport, margin);
            }
            if usable {
                self.show_minimap(ui, model, view, viewport);
            }
            if usable && self.zoom_slider {
                self.show_zoom_slider(ui, view, viewport);
            }
            self.show_drop_menu(ui, model, view);
            self.show_edge_menu(ui, model, view);
            shown.response
        })
        .inner
    }

    /// The zoom slider, in a layer over the canvas so that a press on it does
    /// not pan the background. A change keeps the center of the canvas where
    /// it is.
    fn show_zoom_slider(&self, ui: &mut Ui, view: &mut CanvasView, viewport: Rect) {
        let margin = 8.0;
        let padding = 6.0;
        let gap = 4.0;
        let rail_length = 100.0;
        let thickness = ui.spacing().interact_size.y;
        let label_height = ui.text_style_height(&egui::TextStyle::Small);
        let size = egui::vec2(44.0, padding + rail_length + gap + label_height + padding);
        let minimap_height = self.minimap.map_or(0.0, |minimap| minimap.y + margin);
        if viewport.width() < size.x + 2.0 * margin
            || viewport.height() < size.y + minimap_height + 2.0 * margin
        {
            return;
        }
        let top_left = egui::pos2(viewport.right() - margin - size.x, viewport.top() + margin);
        let dragging_id = self.id.with("zoom_dragging");
        let dragging = ui
            .data(|data| data.get_temp::<bool>(dragging_id))
            .unwrap_or(false);
        let min = view.min_zoom() * 100.0;
        let max = view.max_zoom() * 100.0;
        let mut percent = view.zoom() * 100.0;
        egui::Area::new(self.id.with("zoom_slider"))
            .order(egui::Order::Middle)
            .fixed_pos(top_left)
            .show(ui.ctx(), |ui| {
                // under the slider, so that a press on the panel is not the
                // background's
                let (rect, _) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let active = dragging || ui.rect_contains_pointer(rect);
                let opacity = if active { 0.6 } else { 0.0 };
                ui.painter().rect_filled(
                    rect,
                    4.0,
                    ui.visuals().window_fill.gamma_multiply(opacity),
                );
                let rail = Rect::from_min_size(
                    egui::pos2(rect.center().x - thickness / 2.0, rect.top() + padding),
                    egui::vec2(thickness, rail_length),
                );
                let response = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(rail), |ui| {
                        ui.spacing_mut().slider_width = rail_length;
                        ui.add(
                            egui::Slider::new(&mut percent, min..=max)
                                .logarithmic(true)
                                .show_value(false)
                                .orientation(egui::SliderOrientation::Vertical),
                        )
                    })
                    .inner;
                response.widget_info(|| WidgetInfo::slider(true, f64::from(percent), "Zoom"));
                ui.data_mut(|data| data.insert_temp(dragging_id, response.dragged()));
                if response.changed() {
                    view.zoom_to(percent / 100.0, viewport.center());
                }
                ui.painter().text(
                    egui::pos2(rect.center().x, rail.bottom() + gap),
                    egui::Align2::CENTER_TOP,
                    format!("{:.0}%", view.zoom() * 100.0),
                    egui::TextStyle::Small.resolve(ui.style()),
                    ui.visuals().text_color(),
                );
            });
    }

    /// The minimap, in a layer over the canvas so that it takes the pointer
    /// before the background does.
    fn show_minimap(
        &self,
        ui: &mut Ui,
        model: &CanvasModel,
        view: &mut CanvasView,
        viewport: Rect,
    ) {
        let Some(size) = self.minimap else {
            return;
        };
        let margin = 8.0;
        if viewport.width() < size.x + 2.0 * margin || viewport.height() < size.y + 2.0 * margin {
            return;
        }
        let top_left = viewport.right_bottom() - size - Vec2::splat(margin);
        let visible = view.visible_rect(viewport);
        let world = self
            .style
            .content_rect(model)
            .map_or(visible, |content| content.union(visible))
            .expand(24.0);
        let layouts = self.layouts(model);
        let by_id: HashMap<&NodeId, &NodeLayout> =
            layouts.iter().map(|(id, layout)| (id, layout)).collect();
        egui::Area::new(self.id.with("minimap"))
            .order(egui::Order::Middle)
            .fixed_pos(top_left)
            .show(ui.ctx(), |ui| {
                let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Minimap"));
                let map = MinimapMap::new(world, rect);
                if (response.clicked() || response.dragged())
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    view.center_on(map.to_world(pos), viewport);
                }
                let visuals = ui.visuals();
                let painter = ui.painter().with_clip_rect(rect);
                painter.rect_filled(rect, 4.0, visuals.window_fill.gamma_multiply(0.92));
                for edge in model.edges() {
                    let (Some(from), Some(to)) = (
                        by_id
                            .get(&edge.from().node)
                            .and_then(|layout| layout.port_pos(&edge.from().port)),
                        by_id
                            .get(&edge.to().node)
                            .and_then(|layout| layout.port_pos(&edge.to().port)),
                    ) else {
                        continue;
                    };
                    painter.line_segment(
                        [map.to_map(from), map.to_map(to)],
                        Stroke::new(1.0, self.edge_color(ui, edge.state())),
                    );
                }
                for (node, (_, layout)) in model.nodes().iter().zip(&layouts) {
                    let small = Rect::from_min_max(
                        map.to_map(layout.rect().min),
                        map.to_map(layout.rect().max),
                    );
                    let color = node
                        .accent_color()
                        .unwrap_or(visuals.widgets.active.bg_fill);
                    painter.rect_filled(small, 1.0, color);
                    if model.selection().contains(node.id()) {
                        painter.rect_stroke(
                            small,
                            1.0,
                            visuals.selection.stroke,
                            StrokeKind::Outside,
                        );
                    }
                }
                let shown = Rect::from_min_max(map.to_map(visible.min), map.to_map(visible.max));
                painter.rect_filled(shown, 0.0, visuals.selection.bg_fill.gamma_multiply(0.15));
                painter.rect_stroke(
                    shown,
                    0.0,
                    Stroke::new(1.5, visuals.selection.stroke.color),
                    StrokeKind::Inside,
                );
                painter.rect_stroke(
                    rect,
                    4.0,
                    visuals.widgets.noninteractive.bg_stroke,
                    StrokeKind::Inside,
                );
            });
    }

    /// The popup with the properties of an edge (a right click on it), with
    /// the widgets of `egui`, while it is open.
    fn show_edge_menu(&self, ui: &mut Ui, model: &mut CanvasModel, view: &CanvasView) {
        let memory = self.id.with("edge_menu");
        let Some((id, pos)) = ui
            .data(|data| data.get_temp::<Option<(EdgeId, Pos2)>>(memory))
            .flatten()
        else {
            return;
        };
        let Some(properties) = model.edge(id).map(|edge| *edge.properties()) else {
            ui.data_mut(|data| data.remove_temp::<Option<(EdgeId, Pos2)>>(memory));
            return;
        };
        let default_color = self.edge_color(ui, EdgeState::Neutral);
        let mut custom_color = properties.custom_color().is_some();
        let mut color = properties.custom_color().unwrap_or(default_color);
        let mut line = properties.line_style();
        let mut custom_width = properties.custom_width().is_some();
        let mut width = properties.custom_width().unwrap_or(self.style.edge_width);
        let mut remove = false;
        let area = egui::Area::new(self.id.with("edge_menu_area"))
            .order(egui::Order::Foreground)
            .fixed_pos(view.canvas_to_screen(pos))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut custom_color, "Color");
                        ui.add_enabled_ui(custom_color, |ui| {
                            ui.color_edit_button_srgba(&mut color);
                        });
                    });
                    egui::ComboBox::from_label("Line")
                        .selected_text(line.name())
                        .show_ui(ui, |ui| {
                            for style in LineStyle::all() {
                                ui.selectable_value(&mut line, style, style.name());
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut custom_width, "Width");
                        ui.add_enabled(
                            custom_width,
                            egui::DragValue::new(&mut width).range(0.5..=8.0).speed(0.1),
                        );
                    });
                    ui.separator();
                    remove = ui.button("Remove").clicked();
                });
            });
        let edited = properties
            .color(custom_color.then_some(color))
            .line(line)
            .width(custom_width.then_some(width));
        model.set_edge_properties(id, edited);
        if remove {
            model.disconnect(id);
        }
        // a popup of the widgets (the colors, the list) is outside the area
        let outside = ui.input(|input| input.pointer.any_pressed())
            && !area.response.contains_pointer()
            && !egui::Popup::is_any_open(ui.ctx());
        if remove || outside || ui.input(|input| input.key_pressed(Key::Escape)) {
            ui.data_mut(|data| data.remove_temp::<Option<(EdgeId, Pos2)>>(memory));
        }
    }

    /// The popup of an edge that was let go over nothing, while it is open.
    fn show_drop_menu(&mut self, ui: &mut Ui, model: &mut CanvasModel, view: &CanvasView) {
        let memory = self.id.with("drop_menu");
        let area_id = self.id.with("drop_menu_area");
        let Some(menu) = self.drop_menu.as_mut() else {
            return;
        };
        let Some((from, pos)) = ui
            .data(|data| data.get_temp::<Option<(PortRef, Pos2)>>(memory))
            .flatten()
        else {
            return;
        };
        let area = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(view.canvas_to_screen(pos))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| menu(ui, model, &from, pos))
                    .inner
            });
        let outside =
            ui.input(|input| input.pointer.any_pressed()) && !area.response.contains_pointer();
        if area.inner || outside || ui.input(|input| input.key_pressed(Key::Escape)) {
            ui.data_mut(|data| data.remove_temp::<Option<(PortRef, Pos2)>>(memory));
        }
    }

    /// The layout of every node, with its id.
    fn layouts(&self, model: &CanvasModel) -> Vec<(NodeId, NodeLayout)> {
        model
            .nodes()
            .iter()
            .map(|node| (node.id().clone(), NodeLayout::of(node, &self.style)))
            .collect()
    }

    /// How far from a port the pointer still takes it.
    fn port_reach(&self) -> f32 {
        self.style.port_radius + 4.0
    }

    /// The port that is under `pos`, on any node.
    fn port_at(&self, layouts: &[(NodeId, NodeLayout)], pos: Pos2) -> Option<PortRef> {
        let reach = self.port_reach();
        layouts.iter().find_map(|(node, layout)| {
            layout
                .inputs()
                .iter()
                .chain(layout.outputs())
                .find(|(_, port_pos)| port_pos.distance(pos) <= reach)
                .map(|(port, _)| PortRef {
                    node: node.clone(),
                    port: port.clone(),
                })
        })
    }

    /// The edge that is the closest to `pos`, when one is near enough.
    fn edge_at(&self, model: &CanvasModel, pos: Pos2) -> Option<EdgeId> {
        let layouts = self.layouts(model);
        let by_id: HashMap<&NodeId, &NodeLayout> =
            layouts.iter().map(|(id, layout)| (id, layout)).collect();
        model
            .edges()
            .iter()
            .filter_map(|edge| {
                let from = by_id.get(&edge.from().node)?.port_pos(&edge.from().port)?;
                let to = by_id.get(&edge.to().node)?.port_pos(&edge.to().port)?;
                Some((edge.id(), self.style.edge_distance(from, to, pos)))
            })
            .filter(|(_, distance)| *distance <= 6.0)
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(id, _)| id)
    }

    /// The two ports in the order an edge needs: the output first.
    fn ordered(model: &CanvasModel, start: &PortRef, other: &PortRef) -> (PortRef, PortRef) {
        let starts_at_output = model
            .node(&start.node)
            .and_then(|node| node.port(&start.port))
            .is_some_and(|(side, _)| side == PortDirection::Output);
        if starts_at_output {
            (start.clone(), other.clone())
        } else {
            (other.clone(), start.clone())
        }
    }

    /// Reads what the pointer does, changes the model by it, and draws the
    /// result. Returns how far the background was dragged, in the
    /// coordinates of the canvas.
    fn edit(&self, ui: &mut Ui, model: &mut CanvasModel, visible: Rect) -> Vec2 {
        // the pointer arrives in the coordinates of the screen
        let to_canvas = ui.ctx().layer_transform_from_global(ui.layer_id());
        let scale = to_canvas.map_or(1.0, |transform| transform.scaling);
        let pointer = ui
            .input(|input| input.pointer.latest_pos())
            .map(|pos| to_canvas.map_or(pos, |transform| transform * pos));
        let delta = ui.input(|input| input.pointer.delta()) * scale;
        let modifiers = ui.input(|input| input.modifiers);
        let mut pan = Vec2::ZERO;

        // the background goes first, so that everything else is over it
        let background = ui.interact(visible, self.id.with("background"), Sense::click_and_drag());
        // a rectangle that starts on the background with shift held
        let marquee_memory = self.id.with("marquee");
        let mut marquee: Option<Pos2> = ui
            .data(|data| data.get_temp::<Option<Pos2>>(marquee_memory))
            .flatten();
        if background.drag_started() && modifiers.shift {
            marquee = pointer;
        }
        let mut marquee_rect = None;
        if background.dragged() {
            if let (Some(start), Some(now)) = (marquee, pointer) {
                let rect = Rect::from_two_pos(start, now);
                model.set_selection(self.style.nodes_in(model, rect));
                marquee_rect = Some(rect);
            } else {
                pan = delta;
            }
        }
        if background.drag_stopped() {
            marquee = None;
        }
        if marquee.is_some() {
            ui.data_mut(|data| data.insert_temp(marquee_memory, marquee));
        } else {
            ui.data_mut(|data| data.remove_temp::<Option<Pos2>>(marquee_memory));
        }
        if background.clicked() {
            model.clear_selection();
        }
        if background.double_clicked()
            && let Some(pos) = pointer
            && let Some(edge) = self.edge_at(model, pos)
        {
            model.disconnect(edge);
        }
        if background.secondary_clicked()
            && let Some(pos) = pointer
            && let Some(edge) = self.edge_at(model, pos)
        {
            let menu = self.id.with("edge_menu");
            ui.data_mut(|data| data.insert_temp(menu, Some((edge, pos))));
        }
        if background.contains_pointer() && ui.input(|input| input.key_pressed(Key::Delete)) {
            for id in model.selection().to_vec() {
                model.remove_node(&id);
            }
        }

        // the node that would be dropped, centered on the pointer
        let ghost = self
            .preview
            .as_ref()
            .zip(pointer)
            .filter(|(_, pos)| visible.contains(*pos))
            .map(|(node, pos)| {
                let half = Vec2::new(self.style.node_width, self.style.header_height) * 0.5;
                let node = node.clone().at(pos - half);
                let layout = NodeLayout::of(&node, &self.style);
                (node, layout)
            });

        let layouts = self.layouts(model);
        for (id, layout) in &layouts {
            let response = ui.interact(
                layout.rect(),
                self.id.with(("node", id.as_str())),
                Sense::click_and_drag(),
            );
            let title = model
                .node(id)
                .map_or_else(String::new, |node| node.title_text().to_owned());
            response.widget_info(|| WidgetInfo::labeled(Role::Button, true, title.clone()));
            if response.drag_started() && !model.selection().contains(id) {
                model.set_selection([id.clone()]);
            }
            if response.dragged() {
                for selected in model.selection().to_vec() {
                    if let Some(pos) = model.node(&selected).map(|node| node.pos()) {
                        model.move_node(&selected, pos + delta);
                    }
                }
            }
            if response.clicked() {
                let mut selection = model.selection().to_vec();
                if modifiers.shift || modifiers.command {
                    match selection.iter().position(|selected| selected == id) {
                        Some(index) => {
                            selection.remove(index);
                        }
                        None => selection.push(id.clone()),
                    }
                } else {
                    selection = vec![id.clone()];
                }
                model.set_selection(selection);
            }
        }

        // the ports are over the nodes
        let memory = self.id.with("connecting");
        let mut connecting: Option<PortRef> = ui
            .data(|data| data.get_temp::<Option<PortRef>>(memory))
            .flatten();
        let reach = self.port_reach();
        for (id, layout) in &layouts {
            for (port, pos) in layout.inputs().iter().chain(layout.outputs()) {
                let response = ui.interact(
                    Rect::from_center_size(*pos, Vec2::splat(reach * 2.0)),
                    self.id.with(("port", id.as_str(), port.as_str())),
                    Sense::drag(),
                );
                let name = model.node(id).and_then(|node| {
                    let (_, descriptor) = node.port(port)?;
                    Some(format!("{} {}", node.title_text(), descriptor.label_text()))
                });
                response.widget_info(|| {
                    WidgetInfo::labeled(Role::Button, true, name.clone().unwrap_or_default())
                });
                if response.drag_started() {
                    connecting = Some(PortRef {
                        node: id.clone(),
                        port: port.clone(),
                    });
                }
            }
        }
        if let Some(start) = connecting.clone() {
            if ui.input(|input| !input.pointer.any_down()) {
                if let Some(target) = pointer.and_then(|pos| self.port_at(&layouts, pos)) {
                    let (from, to) = Self::ordered(model, &start, &target);
                    // when it is refused the reason was shown while dragging
                    let _ = model.connect(from, to);
                } else if let Some(pos) = pointer {
                    if self.drop_menu.is_some() {
                        let menu = self.id.with("drop_menu");
                        ui.data_mut(|data| {
                            data.insert_temp(menu, Some((start.clone(), pos)));
                        });
                    }
                    model.report_dropped_connection(start, pos);
                }
                connecting = None;
                ui.data_mut(|data| data.remove_temp::<Option<PortRef>>(memory));
            } else {
                ui.data_mut(|data| data.insert_temp(memory, Some(start)));
            }
        }

        self.paint(
            ui,
            model,
            connecting.as_ref(),
            pointer,
            marquee_rect,
            ghost.as_ref(),
        );
        // the widgets go over what was painted
        self.show_controls(ui, model);
        pan
    }

    /// The controls of the nodes, with the widgets of `egui` in the line of
    /// each, over the body of the node. What the user changes goes to the
    /// model.
    fn show_controls(&self, ui: &mut Ui, model: &mut CanvasModel) {
        let mut changes: Vec<(NodeId, ControlId, ControlValue)> = Vec::new();
        let mut presses: Vec<(NodeId, ControlId)> = Vec::new();
        for (id, layout) in self.layouts(model) {
            for row in layout.controls() {
                let Some(control) = model
                    .node(&id)
                    .and_then(|node| node.control_named(row.control()))
                else {
                    continue;
                };
                let (kind, value, label, enabled) = (
                    control.kind().clone(),
                    control.value(),
                    control.label_text().to_owned(),
                    control.is_enabled(),
                );
                let builder = egui::UiBuilder::new()
                    .id_salt((id.as_str(), row.control().as_str(), row.option()))
                    .max_rect(row.rect())
                    .layout(egui::Layout::left_to_right(egui::Align::Center));
                ui.scope_builder(builder, |ui| {
                    ui.add_enabled_ui(enabled, |ui| match (&kind, value) {
                        (ControlKind::Button, _) => {
                            let button = egui::Button::new(label.as_str())
                                .min_size(egui::vec2(row.rect().width(), 0.0));
                            if ui.add(button).clicked() {
                                presses.push((id.clone(), row.control().clone()));
                            }
                        }
                        (ControlKind::Check, Some(ControlValue::Flag(mut on))) => {
                            if ui.checkbox(&mut on, label.as_str()).changed() {
                                changes.push((
                                    id.clone(),
                                    row.control().clone(),
                                    ControlValue::Flag(on),
                                ));
                            }
                        }
                        (ControlKind::Switch, Some(ControlValue::Flag(mut on))) => {
                            if Self::switch(ui, &mut on, &label).changed() {
                                changes.push((
                                    id.clone(),
                                    row.control().clone(),
                                    ControlValue::Flag(on),
                                ));
                            }
                        }
                        (ControlKind::Radio(options), Some(ControlValue::Choice(selected))) => {
                            let option = row.option().unwrap_or(0);
                            if let Some(text) = options.get(option)
                                && ui.radio(selected == option, text.as_str()).clicked()
                            {
                                changes.push((
                                    id.clone(),
                                    row.control().clone(),
                                    ControlValue::Choice(option),
                                ));
                            }
                        }
                        _ => {}
                    });
                });
            }
        }
        for (node, control, value) in changes {
            model.set_control(&node, &control, value);
        }
        for (node, control) in presses {
            model.press(&node, &control);
        }
    }

    /// A switch: `egui` has no widget for it, so it is a check box that
    /// slides, made as the examples of `egui` make theirs.
    fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
        let height = ui.spacing().interact_size.y * 0.7;
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(height * 1.8, height), Sense::click());
        let text = ui.add(egui::Label::new(label).sense(Sense::click()));
        if response.clicked() || text.clicked() {
            *on = !*on;
            response.mark_changed();
        }
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(Role::Switch, enabled, label));
        if ui.is_rect_visible(rect) {
            let progress = ui.ctx().animate_bool_responsive(response.id, *on);
            let visuals = ui.style().interact_selectable(&response, *on);
            let radius = rect.height() / 2.0;
            ui.painter().rect(
                rect,
                radius,
                visuals.bg_fill,
                visuals.bg_stroke,
                StrokeKind::Inside,
            );
            let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), progress);
            ui.painter().circle_filled(
                egui::pos2(x, rect.center().y),
                0.75 * radius,
                visuals.fg_stroke.color,
            );
        }
        response
    }

    /// The color of an edge by what it says.
    fn edge_color(&self, ui: &Ui, state: EdgeState) -> Color32 {
        match state {
            EdgeState::True => self.style.true_color,
            EdgeState::False => self.style.false_color,
            EdgeState::Absent | EdgeState::Neutral => ui.visuals().weak_text_color(),
        }
    }

    /// Draws the edges, then the nodes over them, and the edge that is being
    /// dragged over all of it.
    fn paint(
        &self,
        ui: &Ui,
        model: &CanvasModel,
        connecting: Option<&PortRef>,
        pointer: Option<Pos2>,
        marquee: Option<Rect>,
        ghost: Option<&(NodeDescriptor, NodeLayout)>,
    ) {
        let layouts = self.layouts(model);
        let by_id: HashMap<&NodeId, &NodeLayout> =
            layouts.iter().map(|(id, layout)| (id, layout)).collect();
        let painter = ui.painter();

        for edge in model.edges() {
            let (Some(from), Some(to)) = (
                by_id
                    .get(&edge.from().node)
                    .and_then(|layout| layout.port_pos(&edge.from().port)),
                by_id
                    .get(&edge.to().node)
                    .and_then(|layout| layout.port_pos(&edge.to().port)),
            ) else {
                continue;
            };
            let points = self.style.edge_control_points(from, to);
            let properties = edge.properties();
            // what the edge says when the graph runs goes over what it is
            let color = if edge.state() == EdgeState::Neutral {
                properties
                    .custom_color()
                    .unwrap_or_else(|| self.edge_color(ui, EdgeState::Neutral))
            } else {
                self.edge_color(ui, edge.state())
            };
            let width = properties.custom_width().unwrap_or(self.style.edge_width);
            let stroke = Stroke::new(width, color);
            let line = if edge.state() == EdgeState::Absent {
                LineStyle::Dashed
            } else {
                properties.line_style()
            };
            let curve =
                CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, stroke);
            match line {
                LineStyle::Solid => {
                    painter.add(curve);
                }
                LineStyle::Dashed => {
                    painter.extend(Shape::dashed_line(
                        &curve.flatten(Some(0.5)),
                        stroke,
                        6.0,
                        4.0,
                    ));
                }
                LineStyle::Dotted => {
                    painter.extend(Shape::dashed_line(
                        &curve.flatten(Some(0.5)),
                        stroke,
                        width.max(1.0),
                        width.max(1.0) + 3.0,
                    ));
                }
            }
        }

        let visuals = ui.visuals();
        let radius = self.style.corner_radius;
        let body_font = TextStyle::Body.resolve(ui.style());
        let small_font = TextStyle::Small.resolve(ui.style());
        for (node, (_, layout)) in model.nodes().iter().zip(&layouts) {
            let rect = layout.rect();
            let selected = model.selection().contains(node.id());
            painter.rect_filled(rect, radius, visuals.window_fill);
            let header_color = node
                .accent_color()
                .unwrap_or(visuals.widgets.active.bg_fill);
            let title_color = if node.accent_color().is_some() {
                Color32::WHITE
            } else {
                visuals.strong_text_color()
            };
            painter.rect_filled(
                layout.header(),
                CornerRadius {
                    nw: radius,
                    ne: radius,
                    sw: 0,
                    se: 0,
                },
                header_color,
            );
            let outline = if selected {
                visuals.selection.stroke
            } else {
                visuals.widgets.noninteractive.bg_stroke
            };
            painter.rect_stroke(rect, radius, outline, StrokeKind::Inside);

            let inner = rect.shrink2(egui::vec2(8.0, 0.0));
            let clipped = painter.with_clip_rect(rect);
            clipped.text(
                egui::pos2(inner.min.x, layout.header().center().y),
                Align2::LEFT_CENTER,
                node.title_text(),
                body_font.clone(),
                title_color,
            );
            if let Some(summary) = layout.summary() {
                clipped.text(
                    egui::pos2(inner.min.x, summary.center().y),
                    Align2::LEFT_CENTER,
                    node.summary_text(),
                    small_font.clone(),
                    visuals.weak_text_color(),
                );
            }

            let gap = self.style.port_radius + 4.0;
            for (descriptor, (port, pos)) in node.inputs().iter().zip(layout.inputs()) {
                let color = descriptor.port_color().unwrap_or(visuals.text_color());
                painter.circle_filled(*pos, self.style.port_radius, color);
                self.ring(
                    ui,
                    model,
                    connecting,
                    &PortRef {
                        node: node.id().clone(),
                        port: port.clone(),
                    },
                    *pos,
                );
                clipped.text(
                    egui::pos2(pos.x + gap, pos.y),
                    Align2::LEFT_CENTER,
                    descriptor.label_text(),
                    small_font.clone(),
                    visuals.text_color(),
                );
            }
            for (descriptor, (port, pos)) in node.outputs().iter().zip(layout.outputs()) {
                let color = descriptor.port_color().unwrap_or(visuals.text_color());
                painter.circle_filled(*pos, self.style.port_radius, color);
                self.ring(
                    ui,
                    model,
                    connecting,
                    &PortRef {
                        node: node.id().clone(),
                        port: port.clone(),
                    },
                    *pos,
                );
                clipped.text(
                    egui::pos2(pos.x - gap, pos.y),
                    Align2::RIGHT_CENTER,
                    descriptor.label_text(),
                    small_font.clone(),
                    visuals.text_color(),
                );
            }
        }

        if let Some((node, layout)) = ghost {
            let accent = node
                .accent_color()
                .unwrap_or(visuals.widgets.active.bg_fill);
            painter.rect_filled(layout.rect(), radius, accent.gamma_multiply(0.25));
            painter.rect_filled(
                layout.header(),
                CornerRadius {
                    nw: radius,
                    ne: radius,
                    sw: 0,
                    se: 0,
                },
                accent.gamma_multiply(0.7),
            );
            painter.rect_stroke(
                layout.rect(),
                radius,
                Stroke::new(1.0, accent),
                StrokeKind::Inside,
            );
            painter.text(
                egui::pos2(layout.rect().min.x + 8.0, layout.header().center().y),
                Align2::LEFT_CENTER,
                node.title_text(),
                body_font.clone(),
                Color32::WHITE.gamma_multiply(0.9),
            );
            for (_, pos) in layout.inputs().iter().chain(layout.outputs()) {
                painter.circle_filled(*pos, self.style.port_radius, accent.gamma_multiply(0.7));
            }
        }

        if let Some(rect) = marquee {
            painter.rect_filled(rect, 0.0, visuals.selection.bg_fill.gamma_multiply(0.4));
            painter.rect_stroke(rect, 0.0, visuals.selection.stroke, StrokeKind::Inside);
        }

        let (Some(start), Some(pointer)) = (connecting, pointer) else {
            return;
        };
        let Some(start_pos) = by_id
            .get(&start.node)
            .and_then(|layout| layout.port_pos(&start.port))
        else {
            return;
        };
        let (from, to) = Self::ordered(model, start, start);
        let from_start = from == *start;
        let points = if from_start {
            self.style.edge_control_points(start_pos, pointer)
        } else {
            self.style.edge_control_points(pointer, start_pos)
        };
        painter.add(CubicBezierShape::from_points_stroke(
            points,
            false,
            Color32::TRANSPARENT,
            Stroke::new(self.style.edge_width, visuals.selection.stroke.color),
        ));
        let layouts_ref: &[(NodeId, NodeLayout)] = &layouts;
        if let Some(target) = self.port_at(layouts_ref, pointer) {
            let (from, to) = Self::ordered(model, start, &target);
            if let Err(error) = model.check_connection(&from, &to) {
                painter.text(
                    pointer + Vec2::new(12.0, 12.0),
                    Align2::LEFT_TOP,
                    error.to_string(),
                    small_font,
                    visuals.error_fg_color,
                );
            }
        }
        let _ = to;
    }

    /// While an edge is dragged, rings the ports that take it.
    fn ring(
        &self,
        ui: &Ui,
        model: &CanvasModel,
        connecting: Option<&PortRef>,
        port: &PortRef,
        pos: Pos2,
    ) {
        let Some(start) = connecting else {
            return;
        };
        if start == port {
            return;
        }
        let (from, to) = Self::ordered(model, start, port);
        if model.check_connection(&from, &to).is_ok() {
            ui.painter().circle_stroke(
                pos,
                self.style.port_radius + 3.0,
                Stroke::new(1.5, self.style.true_color),
            );
        }
    }
}
