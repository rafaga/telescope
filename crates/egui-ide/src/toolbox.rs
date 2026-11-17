use egui::{
    Align2, Area, Color32, CornerRadius, CursorIcon, DragAndDrop, Frame, Id, Order, Rect,
    Response, RichText, Role, Sense, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo, pos2, vec2,
};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

/// An entry of a [`Toolbox`]: the item it stands for (the value the host
/// gets when the entry is clicked or dropped), its label and, optionally, a
/// color mark.
#[derive(Debug, Clone)]
pub struct ToolboxEntry<T> {
    item: T,
    label: String,
    color: Option<Color32>,
    enabled: bool,
    disabled_hint: Option<String>,
}

impl<T> ToolboxEntry<T> {
    /// An entry that gives `item` back when it is used.
    pub fn new(item: T, label: impl Into<String>) -> Self {
        Self {
            item,
            label: label.into(),
            color: None,
            enabled: true,
            disabled_hint: None,
        }
    }

    /// A small square of `color` before the label.
    #[must_use]
    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }

    /// Whether the entry can be used; a disabled one can be neither clicked
    /// nor dragged.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// What a hover over the entry says while it is disabled.
    #[must_use]
    pub fn disabled_hint(mut self, hint: impl Into<String>) -> Self {
        self.disabled_hint = Some(hint.into());
        self
    }
}

/// A toolbox: an optional header and groups of entries under their titles. An
/// entry is a button as wide as the toolbox, used by a click, which
/// [`show`](Toolbox::show) reports, or by dragging it onto the canvas, where
/// the host takes it with [`Toolbox::dropped`].
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui, canvas: &egui::Response) {
/// use egui_ide::{Toolbox, ToolboxEntry};
///
/// if let Some(item) = Toolbox::new("tools")
///     .group("Shapes", [ToolboxEntry::new(1_u8, "Circle"), ToolboxEntry::new(2, "Square")])
///     .show(ui)
/// {
///     // a click: add `item` where it fits
/// # let _ = item;
/// }
/// if let Some(item) = Toolbox::<u8>::dropped(canvas) {
///     // dropped on the canvas at the pointer
/// # let _ = item;
/// }
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Toolbox<T> {
    id: Id,
    header: Option<(String, String)>,
    groups: Vec<(String, Vec<ToolboxEntry<T>>)>,
}

impl<T: Clone + Send + Sync + 'static> Toolbox<T> {
    /// A toolbox known by `id_salt`.
    pub fn new(id_salt: impl Hash + Debug) -> Self {
        Self {
            id: Id::unique(id_salt),
            header: None,
            groups: Vec::new(),
        }
    }

    /// A title in bold and, under it, a line that tells how to use the
    /// toolbox, before the groups.
    #[must_use]
    pub fn header(mut self, title: impl Into<String>, hint: impl Into<String>) -> Self {
        self.header = Some((title.into(), hint.into()));
        self
    }

    /// Adds a group with its entries.
    #[must_use]
    pub fn group(
        mut self,
        title: impl Into<String>,
        entries: impl IntoIterator<Item = ToolboxEntry<T>>,
    ) -> Self {
        self.groups
            .push((title.into(), entries.into_iter().collect()));
        self
    }

    /// The width the toolbox needs to show its widest entry or title without
    /// cutting it: the least a panel that holds it should be (without the
    /// panel's own margins).
    pub fn min_width(&self, ui: &Ui) -> f32 {
        let width = |text: &str, style: TextStyle| {
            ui.painter()
                .layout_no_wrap(
                    text.to_owned(),
                    style.resolve(ui.style()),
                    Color32::PLACEHOLDER,
                )
                .size()
                .x
        };
        let mut widest = 0.0_f32;
        if let Some((title, hint)) = &self.header {
            widest = widest
                .max(width(title, TextStyle::Body))
                .max(width(hint, TextStyle::Small));
        }
        for (title, entries) in &self.groups {
            widest = widest.max(width(title, TextStyle::Body));
            for entry in entries {
                let mark = if entry.color.is_some() {
                    Self::MARK_ROOM
                } else {
                    0.0
                };
                widest = widest.max(width(&entry.label, TextStyle::Button) + 2.0 * Self::PADDING + mark);
            }
        }
        widest.ceil()
    }

    /// Lays the header and the groups out. Returns the item of the entry that
    /// was clicked.
    pub fn show(self, ui: &mut Ui) -> Option<T> {
        let Self { id, header, groups } = self;
        // the id keeps the toolbox apart from another one in the same panel
        ui.push_id(id, |ui| {
            let mut picked = None;
            if let Some((title, hint)) = &header {
                ui.label(RichText::new(title).strong());
                ui.label(RichText::new(hint).small().weak());
            }
            for (title, entries) in groups {
                ui.add_space(4.0);
                ui.label(RichText::new(&title).strong());
                for entry in entries {
                    if let Some(item) = Self::entry_ui(ui, entry) {
                        picked = Some(item);
                    }
                }
            }
            picked
        })
        .inner
    }

    /// The item dropped on `target` in this frame, if an entry was released
    /// over it. The position is the pointer's (`ctx.pointer_latest_pos()`).
    pub fn dropped(target: &Response) -> Option<T> {
        target.dnd_release_payload::<T>().map(Arc::unwrap_or_clone)
    }

    /// Whether an entry is being dragged over `target` right now, to
    /// highlight it as a place where it can be dropped.
    pub fn hovered(target: &Response) -> bool {
        target.dnd_hover_payload::<T>().is_some()
    }

    /// The room the color mark takes before the label: the mark and the gap.
    const MARK_ROOM: f32 = 14.0;
    /// The room between the edge of an entry and what is in it.
    const PADDING: f32 = 8.0;

    /// An entry: a button as wide as the toolbox, with its color mark and
    /// label, that is clicked or dragged.
    fn entry_ui(ui: &mut Ui, entry: ToolboxEntry<T>) -> Option<T> {
        let ToolboxEntry {
            item,
            label,
            color,
            enabled,
            disabled_hint,
        } = entry;
        let sense = if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
        let (rect, response) = ui.allocate_exact_size(size, sense);
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));
        if ui.is_rect_visible(rect) {
            let visuals = if enabled {
                ui.style().interact_selectable(&response, false)
            } else {
                ui.visuals().widgets.inactive
            };
            let text_color = if enabled {
                visuals.fg_stroke.color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.painter().rect(
                rect,
                CornerRadius::same(2),
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                StrokeKind::Inside,
            );
            let mut x = rect.left() + Self::PADDING;
            if let Some(color) = color {
                let mark = Rect::from_center_size(pos2(x + 4.0, rect.center().y), Vec2::splat(8.0));
                ui.painter().rect_filled(mark, 1.0, color);
                x += Self::MARK_ROOM;
            }
            ui.painter().text(
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                &label,
                TextStyle::Button.resolve(ui.style()),
                text_color,
            );
        }
        let response = match disabled_hint {
            Some(hint) if !enabled => response.on_hover_text(hint),
            _ => response,
        };
        if response.drag_started() {
            DragAndDrop::set_payload(ui.ctx(), item.clone());
        }
        if response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            Self::drag_ghost(ui, response.id, &label);
        } else if response.hovered() && enabled {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        response.clicked().then_some(item)
    }

    /// The label of the entry being dragged, following the pointer.
    fn drag_ghost(ui: &Ui, id: Id, label: &str) {
        let Some(pointer) = ui.ctx().pointer_latest_pos() else {
            return;
        };
        Area::new(id.with("ghost"))
            .order(Order::Tooltip)
            .fixed_pos(pointer + vec2(12.0, 12.0))
            .pivot(Align2::LEFT_TOP)
            .interactable(false)
            .show(ui.ctx(), |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(label);
                });
            });
    }
}
