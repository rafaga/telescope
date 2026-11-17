use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{
    CursorIcon, Frame, Id, Label, Margin, Rect, RichText, Sense, Shape, StrokeKind, TextStyle, Ui,
    Vec2, pos2,
};
use std::fmt::Debug;
use std::hash::Hash;

/// A property window like the one of an IDE: [`group`](Properties::group)s that
/// fold away, each one a table of `name | value` rows with borders, under a band with its title.
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui, name: &mut String) {
/// let changed = egui_ide::PropertyGrid::new("properties").show(ui, |grid| {
///     grid.group("General", |rows| {
///         rows.row("Name", |ui| ui.text_edit_singleline(name).changed());
///     });
/// });
/// # let _ = changed;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PropertyGrid {
    id: Id,
    name_width: f32,
    min_value_width: f32,
    empty_hint: Option<String>,
}

impl PropertyGrid {
    /// A grid whose groups remember whether they are open under `id_salt`.
    pub fn new(id_salt: impl Hash + Debug) -> Self {
        Self {
            id: Id::unique(id_salt),
            name_width: 110.0,
            min_value_width: 100.0,
            empty_hint: None,
        }
    }

    /// The width of the name column, 110 points by default.
    #[must_use]
    pub fn name_width(mut self, width: f32) -> Self {
        self.name_width = width;
        self
    }

    /// The least width the value column gets, however narrow the panel is,
    /// 100 points by default.
    #[must_use]
    pub fn min_value_width(mut self, width: f32) -> Self {
        self.min_value_width = width;
        self
    }

    /// What the box under the groups says while no row is under the pointer or
    /// selected, dimmed.
    #[must_use]
    pub fn empty_hint(mut self, hint: impl Into<String>) -> Self {
        self.empty_hint = Some(hint.into());
        self
    }

    /// Lays the groups that `add` declares out in `ui`. Returns whether any
    /// value changed.
    ///
    /// The column of names can be resized by dragging the line that divides it
    /// from the values; a click on a row selects it. If any row has a
    /// description, a box under the groups shows the one of the row under the
    /// pointer, or else of the selected one.
    pub fn show(self, ui: &mut Ui, add: impl FnOnce(&mut Properties<'_>)) -> bool {
        let ctx = ui.ctx().clone();
        let width_id = self.id.with("name_width");
        let selected_id = self.id.with("selected");
        let info_id = self.id.with("selected_info");
        let table = Table {
            name_width: ctx
                .data(|d| d.get_temp::<f32>(width_id))
                .unwrap_or(self.name_width),
            min_value_width: self.min_value_width,
            selected: ctx.data(|d| d.get_temp::<Option<Id>>(selected_id).flatten()),
            selected_info: ctx.data(|d| d.get_temp::<Option<RowInfo>>(info_id).flatten()),
            hovered: None,
            has_descriptions: false,
            changed: false,
        };
        // Bands and tables touch each other: no gap between them.
        let gap = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut properties = Properties {
            ui,
            id: self.id,
            table,
        };
        add(&mut properties);
        let Properties { ui, table, .. } = properties;
        ui.spacing_mut().item_spacing.y = gap;
        if table.has_descriptions {
            ui.add_space(gap);
            let info = table
                .hovered
                .clone()
                .or_else(|| table.selected_info.clone());
            Frame::new()
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.set_min_height(ui.text_style_height(&TextStyle::Body) * 2.0);
                    if let Some(info) = info {
                        ui.label(RichText::new(info.name).strong());
                        ui.label(info.description);
                    } else if let Some(hint) = self.empty_hint {
                        ui.weak(hint);
                    }
                });
        }
        ctx.data_mut(|d| {
            d.insert_temp(width_id, table.name_width);
            d.insert_temp(selected_id, table.selected);
            d.insert_temp(info_id, table.selected_info);
        });
        table.changed
    }
}

/// What the description box shows for a row: its name and, if it has one,
/// its description.
#[derive(Clone)]
struct RowInfo {
    name: String,
    description: String,
}

/// What the groups of a [`PropertyGrid`] share while it is laid out.
struct Table {
    name_width: f32,
    min_value_width: f32,
    selected: Option<Id>,
    selected_info: Option<RowInfo>,
    hovered: Option<RowInfo>,
    has_descriptions: bool,
    changed: bool,
}

/// The groups of a [`PropertyGrid`], while it is being laid out.
pub struct Properties<'a> {
    ui: &'a mut Ui,
    id: Id,
    table: Table,
}

impl Properties<'_> {
    /// A group: a band across the panel with the title, that folds the group
    /// away (open at first) when it is clicked, and under it the rows that
    /// `rows` adds inside a bordered table. The group is remembered by its
    /// title.
    pub fn group(&mut self, title: &str, rows: impl FnOnce(&mut PropertyRows<'_>)) {
        let id = self.id.with(title);
        let mut state = CollapsingState::load_with_default_open(self.ui.ctx(), id, true);
        let band = self.ui.visuals().widgets.inactive.weak_bg_fill;
        let toggled = Frame::new()
            .fill(band)
            .inner_margin(Margin::symmetric(4, 2))
            .show(self.ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    state.show_toggle_button(ui, paint_default_icon);
                    ui.add(Label::new(RichText::new(title).strong()).sense(Sense::click()))
                        .clicked()
                })
                .inner
            })
            .inner;
        if toggled {
            state.toggle(self.ui);
        }
        let table = &mut self.table;
        let body = state.show_body_unindented(self.ui, |ui| {
            // The rows touch each other: their borders make the table.
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(&mut PropertyRows {
                ui,
                table: &mut *table,
                group: id,
            });
        });
        if let Some(body) = body {
            let stroke = self.ui.visuals().widgets.noninteractive.bg_stroke;
            let rect = body.response.rect;
            self.ui
                .painter()
                .rect_stroke(rect, 0.0, stroke, StrokeKind::Inside);
            // The line between names and values is a handle that resizes the
            // column of names.
            let line = rect.left() + self.table.name_width;
            let handle = Rect::from_x_y_ranges((line - 3.0)..=(line + 3.0), rect.y_range());
            let response = self.ui.interact(handle, id.with("split"), Sense::drag());
            if response.hovered() || response.dragged() {
                self.ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if response.dragged() {
                let widest = (rect.width() - self.table.min_value_width).max(60.0);
                self.table.name_width =
                    (self.table.name_width + response.drag_delta().x).clamp(60.0, widest);
            }
        }
    }
}

/// The rows of one group of a [`PropertyGrid`].
pub struct PropertyRows<'a> {
    ui: &'a mut Ui,
    table: &'a mut Table,
    group: Id,
}

impl<'a> PropertyRows<'a> {
    /// A row of the table: `name` in the first cell and, in the second, what
    /// `value` adds, stacked from the top so a list of widgets does not run
    /// along the row. `value` returns whether it changed the property. The
    /// cells are bordered; the name stays at the top of the row, level with
    /// the first line of the value.
    ///
    /// For a description or a mark of a modified value, see
    /// [`property`](Self::property).
    pub fn row(&mut self, name: &str, value: impl FnOnce(&mut Ui) -> bool) {
        self.property(name).show(value);
    }

    /// A row that shows `text` and cannot be edited.
    pub fn read_only(&mut self, name: &str, text: &str) {
        self.row(name, |ui| {
            ui.label(text);
            false
        });
    }

    /// A row with options: [`description`](Property::description) for the box
    /// under the groups and [`modified`](Property::modified) for a value that
    /// is not the default one. Finish it with [`show`](Property::show).
    pub fn property<'r>(&'r mut self, name: &'r str) -> Property<'r, 'a> {
        Property {
            rows: self,
            name,
            description: None,
            modified: false,
        }
    }

    /// The `Ui` of the group, for a row that is not `name | value`.
    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }
}

/// A row of a [`PropertyGrid`] with its options.
#[must_use = "call `show` to lay the row out"]
pub struct Property<'r, 'a> {
    rows: &'r mut PropertyRows<'a>,
    name: &'r str,
    description: Option<&'r str>,
    modified: bool,
}

impl<'r> Property<'r, '_> {
    /// What the row is for, shown in the box under the groups while the
    /// pointer is over the row or the row is selected.
    pub fn description(mut self, description: &'r str) -> Self {
        self.description = Some(description);
        self
    }

    /// The value differs from the default one: the name is drawn in bold.
    pub fn modified(mut self, modified: bool) -> Self {
        self.modified = modified;
        self
    }

    /// Lays the row out; see [`PropertyRows::row`].
    pub fn show(self, value: impl FnOnce(&mut Ui) -> bool) {
        let Self {
            rows,
            name,
            description,
            modified,
        } = self;
        let ui = &mut *rows.ui;
        let visuals = ui.visuals().clone();
        let stroke = visuals.widgets.noninteractive.bg_stroke;
        let margin = Margin::symmetric(6, 1);
        let padding = margin.sum().x;
        let width = ui.available_width();
        let name_width = rows.table.name_width.min(width);
        let value_width = (width - name_width).max(rows.table.min_value_width);
        let row_id = rows.group.with(name);
        let selected = rows.table.selected == Some(row_id);
        rows.table.has_descriptions |= description.is_some();
        let info = Some(RowInfo {
            name: name.to_owned(),
            description: description.unwrap_or_default().to_owned(),
        });
        // Painted last but drawn first: under the cells.
        let background = ui.painter().add(Shape::Noop);
        let mut changed = false;
        let row = ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.horizontal_top(|ui| {
                Frame::new().inner_margin(margin).show(ui, |ui| {
                    ui.set_width((name_width - padding).max(0.0));
                    let mut text = RichText::new(name);
                    if modified {
                        text = text.strong();
                    }
                    if selected {
                        text = text.color(visuals.selection.stroke.color);
                    }
                    // A vertical layout wraps a long name to the cell.
                    ui.vertical(|ui| ui.label(text));
                });
                Frame::new().inner_margin(margin).show(ui, |ui| {
                    ui.set_width((value_width - padding).max(0.0));
                    changed = ui.vertical(value).inner;
                });
            });
        });
        let rect = row.response.rect;
        let hovered = ui.rect_contains_pointer(rect);
        // The selected row fills its name cell, whole; the one under the
        // pointer, the whole row.
        if selected {
            let name_cell =
                Rect::from_min_max(rect.min, pos2(rect.left() + name_width, rect.bottom()));
            ui.painter().set(
                background,
                Shape::rect_filled(name_cell, 0.0, visuals.selection.bg_fill),
            );
        } else if hovered {
            ui.painter().set(
                background,
                Shape::rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill),
            );
        }
        // A press selects the row without taking it from the widgets in it.
        if hovered && ui.input(|input| input.pointer.primary_pressed()) {
            rows.table.selected = Some(row_id);
            rows.table.selected_info = info.clone();
        }
        if hovered {
            rows.table.hovered = info;
        }
        let painter = ui.painter();
        painter.hline(rect.x_range(), rect.bottom(), stroke);
        painter.vline(rect.left() + name_width, rect.y_range(), stroke);
        rows.table.changed |= changed;
    }
}
