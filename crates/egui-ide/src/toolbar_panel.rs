use crate::toolbar::ToolbarUi;
use egui::{Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use std::hash::Hash;

/// The order of the groups and the one being dragged, kept in the memory of
/// the context.
#[derive(Debug, Clone, Default)]
struct PanelState {
    order: Vec<String>,
    dragging: Option<String>,
    /// Where inside the group the pointer took it.
    grab_offset: Vec2,
    /// The size every group had the last time it was laid out; it tells
    /// where a row must wrap and how big the placeholder of a drag is.
    sizes: HashMap<String, Vec2>,
    /// Where the groups that were not being dragged were laid out.
    rects: Vec<(String, Rect)>,
}

/// A group the host declared, waiting to be laid out.
struct GroupSpec<'a> {
    key: String,
    title: String,
    add: Box<dyn FnOnce(&mut ToolbarUi<'_>) + 'a>,
}

/// A panel of toolbars whose groups the user moves by dragging their grip.
/// The groups that do not fit in a row go on to the next one, and while a
/// group is dragged a placeholder of its size shows the place it would take
/// when dropped, while a faded copy of the group follows the pointer.
///
/// Each group has a `key` that names it, so the order survives the host
/// declaring its groups in another order or adding new ones (a new group goes
/// to the end). The order lives in the memory of the context; the host reads
/// it with [`order`](Self::order) to save it and puts it back with
/// [`set_order`](Self::set_order).
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui, grid: &mut bool) {
/// use egui_ide::{ToolbarIcon, ToolbarPanel};
///
/// ToolbarPanel::new("toolbars")
///     .group("file", "File", |bar| {
///         bar.icon_button(ToolbarIcon::Done, "Done");
///     })
///     .group("view", "View", |bar| {
///         bar.icon_toggle(grid, ToolbarIcon::Toolbox, "Toolbox");
///     })
///     .show(ui);
/// # }
/// ```
pub struct ToolbarPanel<'a> {
    id: Id,
    groups: Vec<GroupSpec<'a>>,
}

impl<'a> ToolbarPanel<'a> {
    /// A panel known by `id_salt`.
    pub fn new(id_salt: impl Hash + std::fmt::Debug) -> Self {
        Self {
            id: Id::unique(id_salt),
            groups: Vec::new(),
        }
    }

    /// Adds a group named `key` whose items `add` declares; `title` is shown
    /// when the pointer rests on it.
    #[must_use]
    pub fn group(
        mut self,
        key: impl Into<String>,
        title: impl Into<String>,
        add: impl FnOnce(&mut ToolbarUi<'_>) + 'a,
    ) -> Self {
        self.groups.push(GroupSpec {
            key: key.into(),
            title: title.into(),
            add: Box::new(add),
        });
        self
    }

    /// The order of the keys of the groups of the panel known by `id_salt`,
    /// to save it.
    #[must_use]
    pub fn order(ctx: &egui::Context, id_salt: impl Hash + std::fmt::Debug) -> Vec<String> {
        let id = Id::unique(id_salt);
        ctx.data(|data| data.get_temp::<PanelState>(id))
            .map(|state| state.order)
            .unwrap_or_default()
    }

    /// Puts back an order that [`order`](Self::order) gave.
    pub fn set_order(ctx: &egui::Context, id_salt: impl Hash + std::fmt::Debug, order: Vec<String>) {
        let id = Id::unique(id_salt);
        ctx.data_mut(|data| {
            let mut state = data.get_temp::<PanelState>(id).unwrap_or_default();
            state.order = order;
            data.insert_temp(id, state);
        });
    }

    /// Lays the groups out, in rows that wrap, in the order the user left
    /// them in.
    pub fn show(self, ui: &mut Ui) {
        let state_id = self.id;
        let mut state = ui
            .data(|data| data.get_temp::<PanelState>(state_id))
            .unwrap_or_default();

        // the saved order, without the groups that are gone, and the new
        // ones at the end
        let declared: Vec<String> = self.groups.iter().map(|group| group.key.clone()).collect();
        state.order.retain(|key| declared.contains(key));
        for key in &declared {
            if !state.order.contains(key) {
                state.order.push(key.clone());
            }
        }

        let pointer = ui.input(|input| input.pointer.interact_pos());
        let pressed = ui.input(|input| input.pointer.primary_down());

        // where the dragged group would land, by the layout of the last frame
        let mut target: Option<usize> = None;
        if let (Some(moving), Some(pointer)) = (state.dragging.clone(), pointer) {
            target = Some(
                state
                    .rects
                    .iter()
                    .filter(|(key, rect)| *key != moving && Self::is_before(*rect, pointer))
                    .count(),
            );
        }
        if state.dragging.is_some() && !pressed {
            // dropped
            if let (Some(moving), Some(target)) = (state.dragging.take(), target) {
                let mut order: Vec<String> = state
                    .order
                    .iter()
                    .filter(|key| **key != moving)
                    .cloned()
                    .collect();
                order.insert(target.min(order.len()), moving);
                state.order = order;
            }
            target = None;
        }
        let moving = state.dragging.clone();

        let mut specs = self.groups;
        let dragged = moving
            .as_ref()
            .and_then(|key| specs.iter().position(|spec| spec.key == *key))
            .map(|position| specs.remove(position));
        let ghost_size = moving
            .as_ref()
            .and_then(|key| state.sizes.get(key))
            .copied()
            .unwrap_or(Vec2::new(48.0, 28.0));
        let order: Vec<String> = state
            .order
            .iter()
            .filter(|key| moving.as_deref() != Some(key.as_str()))
            .cloned()
            .collect();
        let sizes = state.sizes.clone();

        let mut rects: Vec<(String, Rect)> = Vec::new();
        let mut grabbed: Option<(String, Vec2)> = None;
        ui.horizontal_wrapped(|ui| {
            for (index, key) in order.iter().enumerate() {
                if target == Some(index) {
                    Self::placeholder(ui, ghost_size);
                }
                let Some(position) = specs.iter().position(|spec| spec.key == *key) else {
                    continue;
                };
                let spec = specs.remove(position);
                if let Some(size) = sizes.get(key) {
                    Self::wrap_before(ui, size.x);
                }
                let shown = ToolbarUi { ui }.framed_group(&spec.title, spec.add);
                if shown.grip.drag_started() {
                    let offset = pointer.map_or(Vec2::ZERO, |pointer| pointer - shown.rect.min);
                    grabbed = Some((spec.key.clone(), offset));
                }
                rects.push((spec.key, shown.rect));
            }
            if target.is_some_and(|target| target >= order.len()) {
                Self::placeholder(ui, ghost_size);
            }
        });

        if let Some((key, offset)) = grabbed {
            state.dragging = Some(key);
            state.grab_offset = offset;
        }

        // the group follows the pointer, faded, without taking clicks
        if let (Some(spec), Some(pointer)) = (dragged, pointer) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            egui::Area::new(state_id.with("ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pointer - state.grab_offset)
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    ui.set_opacity(0.7);
                    ToolbarUi { ui }.framed_group(&spec.title, spec.add);
                });
        }

        // remember the sizes: a group whose size changed needs another frame
        // to wrap in the right place
        let mut stale = false;
        for (key, rect) in &rects {
            let changed = state
                .sizes
                .get(key)
                .is_none_or(|old| (*old - rect.size()).length() > 0.5);
            if changed {
                state.sizes.insert(key.clone(), rect.size());
                stale = true;
            }
        }
        if stale {
            ui.ctx().request_repaint();
        }
        state.rects = rects;
        ui.data_mut(|data| data.insert_temp(state_id, state));
    }

    /// Goes on to the next row when something `width` wide would not fit in
    /// what is left of this one.
    fn wrap_before(ui: &mut Ui, width: f32) {
        let next = ui.next_widget_position();
        let at_start = next.x <= ui.max_rect().left() + 0.5;
        if !at_start && next.x + width > ui.max_rect().right() {
            ui.end_row();
        }
    }

    /// The empty place a dragged group would take: a dashed-looking box of
    /// its size in the selection color.
    fn placeholder(ui: &mut Ui, size: Vec2) {
        Self::wrap_before(ui, size.x);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let color = ui.visuals().selection.bg_fill;
        let radius = ui.visuals().widgets.noninteractive.corner_radius;
        ui.painter().rect(
            rect,
            radius,
            color.gamma_multiply(0.25),
            Stroke::new(1.0, color),
            StrokeKind::Inside,
        );
    }

    /// Whether `rect` comes before the pointer in reading order: in a row
    /// above it or, in its row, at its left.
    fn is_before(rect: Rect, pointer: Pos2) -> bool {
        rect.max.y < pointer.y || (rect.min.y <= pointer.y && rect.center().x < pointer.x)
    }
}
