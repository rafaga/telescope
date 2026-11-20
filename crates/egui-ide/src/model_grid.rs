//! The editors that [`PropertyGrid::show_model`] gives each kind of property.

use crate::model::{
    PropertyDescriptor, PropertyId, PropertyKind, PropertyLabels, PropertyModel, PropertyValue,
    Severity,
};
use crate::property_grid::PropertyGrid;
use egui::{
    Align, Align2, Button, ComboBox, DragValue, Frame, Id, Label, Layout, Margin, Rect, RichText,
    Role, Sense, StrokeKind, TextEdit, TextStyle, Ui, Vec2, WidgetInfo, Window, vec2,
};
use egui_extras::DatePickerButton;
use native_tools::date_picker::Date;
use native_tools::dialog::{Dialog, DialogResult, DialogType};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What is being edited in the window of a list, apart from the value until
/// the window is accepted.
#[derive(Debug, Clone, Default)]
struct Draft {
    text: String,
    pairs: Vec<(String, String)>,
    selected: usize,
    /// The row whose key is a text field; the others are labels.
    editing: Option<usize>,
    /// The key that was just made editable takes the focus.
    focus_pending: bool,
}

/// What the window of a list was asked to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftAction {
    Keep,
    Apply,
    Cancel,
}

impl PropertyGrid {
    /// Lays out the groups of `model`, an editor for each property that fits
    /// its kind. Returns the ids of the properties changed this frame, by
    /// the user or by a reset to the default value.
    ///
    /// A modified value is marked, a rule that finds an issue shows it under
    /// the value, and a list opens a window with Accept and Cancel.
    pub fn show_model(self, ui: &mut Ui, model: &mut PropertyModel) -> Vec<PropertyId> {
        let mut changed = Vec::new();
        let PropertyModel {
            groups,
            values,
            labels,
            activated,
        } = model;
        let labels: &PropertyLabels = labels;
        self.show(ui, |grid| {
            for group in groups.iter() {
                grid.group(group.title(), |rows| {
                    for descriptor in group.descriptors() {
                        let Some(value) = values.get_mut(descriptor.id()) else {
                            continue;
                        };
                        let modified = descriptor.kind().holds_value()
                            && *value != *descriptor.default_value();
                        let issue = descriptor.check(value);
                        let mut edited = false;
                        let mut restored = false;
                        let mut pressed = false;
                        let mut property =
                            rows.property(descriptor.label_text()).modified(modified);
                        if let Some(description) = descriptor.description_text() {
                            property = property.description(description);
                        }
                        property.show(|ui| {
                            let show_reset = modified && descriptor.is_enabled();
                            // The editor takes what the reset button leaves, which is
                            // at the right end of the row.
                            ui.horizontal(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if show_reset {
                                        let reset = ui
                                            .small_button("\u{21BA}")
                                            .on_hover_text(&labels.reset);
                                        if reset.clicked() {
                                            *value = descriptor.default_value().clone();
                                            restored = true;
                                        }
                                    }
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.add_enabled_ui(descriptor.is_enabled(), |ui| {
                                            edited =
                                                descriptor.edit(ui, value, labels, &mut pressed);
                                        });
                                    });
                                });
                            });
                            if let Some(issue) = &issue {
                                let color = match issue.severity() {
                                    Severity::Error => ui.visuals().error_fg_color,
                                    Severity::Warning => ui.visuals().warn_fg_color,
                                };
                                ui.label(RichText::new(issue.message()).small().color(color));
                            }
                            edited || restored
                        });
                        if edited || restored {
                            changed.push(descriptor.id().clone());
                        }
                        if pressed {
                            activated.push(descriptor.id().clone());
                        }
                    }
                });
            }
        });
        changed
    }
}

impl PropertyDescriptor {
    /// The editor of the property in `ui`. Returns whether `value` changed.
    pub(crate) fn edit(
        &self,
        ui: &mut Ui,
        value: &mut PropertyValue,
        labels: &PropertyLabels,
        pressed: &mut bool,
    ) -> bool {
        match (self.kind(), value) {
            (PropertyKind::Text, PropertyValue::Text(text)) => ui
                .add(TextEdit::singleline(text).desired_width(f32::INFINITY))
                .changed(),
            (kind @ (PropertyKind::File | PropertyKind::Folder), PropertyValue::Text(text)) => {
                // the dialog answers from its own thread, into a slot that
                // lives as long as the property's row
                let slot_id = ui.scope_id().with(("chosen_path", self.id().as_str()));
                let slot: Arc<Mutex<Option<String>>> = ui.data_mut(|data| {
                    data.get_temp_mut_or_insert_with(slot_id, Arc::<Mutex<Option<String>>>::default)
                        .clone()
                });
                let chosen = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                let mut changed = false;
                if let Some(path) = chosen {
                    *text = path;
                    changed = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // the button is placed first, so the text takes the rest
                    if ui.button("\u{2026}").clicked() {
                        let dialog_type = if *kind == PropertyKind::Folder {
                            DialogType::Directory
                        } else {
                            DialogType::File
                        };
                        let mut dialog = Dialog::new(dialog_type);
                        let current = Path::new(text.as_str());
                        let start = if current.is_dir() {
                            Some(current)
                        } else {
                            current.parent().filter(|parent| parent.is_dir())
                        };
                        if let Some(start) = start {
                            dialog.set_directory(start);
                        }
                        let ctx = ui.ctx().clone();
                        let answer = Arc::clone(&slot);
                        dialog.open_file_dialog(move |result| {
                            if let DialogResult::Ok(path) = result {
                                *answer.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Some(path.to_string_lossy().into_owned());
                                ctx.request_repaint();
                            }
                        });
                    }
                    changed |= ui
                        .add(TextEdit::singleline(text).desired_width(f32::INFINITY))
                        .changed();
                });
                changed
            }
            (PropertyKind::Date, PropertyValue::Text(text)) => {
                let mut changed = false;
                let current = Date::parse_iso(text);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // the button is placed first, so the text takes the rest
                    let mut day = current
                        .and_then(|date| {
                            jiff::civil::Date::new(
                                date.year as i16,
                                date.month as i8,
                                date.day as i8,
                            )
                            .ok()
                        })
                        .unwrap_or_else(|| {
                            jiff::Timestamp::now()
                                .to_zoned(jiff::tz::TimeZone::UTC)
                                .date()
                        });
                    // only the icon: the text is the field beside it
                    let response = ui.add(
                        DatePickerButton::new(&mut day)
                            .id_salt(self.id().as_str())
                            .format(""),
                    );
                    if response.changed()
                        && let Some(date) =
                            Date::new(i32::from(day.year()), day.month() as u8, day.day() as u8)
                    {
                        *text = date.to_iso();
                        changed = true;
                    }
                    let valid = text.is_empty() || Date::parse_iso(text).is_some();
                    let mut edit = TextEdit::singleline(text)
                        .hint_text(labels.date_hint.as_str())
                        .desired_width(f32::INFINITY);
                    if !valid {
                        edit = edit.text_color(ui.visuals().error_fg_color);
                    }
                    changed |= ui.add(edit).changed();
                });
                changed
            }
            (PropertyKind::ReadOnly, value) => {
                // A long text breaks into lines instead of widening the grid.
                ui.add(Label::new(RichText::new(value.to_string()).weak()).wrap());
                false
            }
            (PropertyKind::Int, PropertyValue::Int(number)) => {
                let mut drag = DragValue::new(number);
                if let Some((least, most)) = self.limits() {
                    drag = drag.range((least as i64)..=(most as i64));
                }
                if let Some(unit) = self.unit_text() {
                    drag = drag.suffix(format!(" {unit}"));
                }
                ui.add_sized(Self::number_size(ui), drag).changed()
            }
            (PropertyKind::Float, PropertyValue::Float(number)) => {
                let mut drag = DragValue::new(number);
                if let Some((least, most)) = self.limits() {
                    drag = drag.range(least..=most);
                }
                if let Some(unit) = self.unit_text() {
                    drag = drag.suffix(format!(" {unit}"));
                }
                ui.add_sized(Self::number_size(ui), drag).changed()
            }
            (PropertyKind::Multiline, PropertyValue::Text(text)) => ui
                .add(
                    TextEdit::multiline(text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                )
                .changed(),
            (PropertyKind::Bool, PropertyValue::Bool(flag)) => {
                let mut changed = false;
                let shown = if *flag { &labels.yes } else { &labels.no };
                let enabled = ui.is_enabled();
                let name = self.label_text();
                let combo = ComboBox::from_id_salt((self.id().as_str(), "bool"))
                    .selected_text(shown)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (option, text) in [(true, &labels.yes), (false, &labels.no)] {
                            if ui.selectable_label(*flag == option, text).clicked()
                                && *flag != option
                            {
                                *flag = option;
                                changed = true;
                            }
                        }
                    });
                combo
                    .response
                    .widget_info(|| WidgetInfo::labeled(Role::ComboBox, enabled, name));
                changed
            }
            (PropertyKind::Action(text), _) => {
                *pressed = ui.button(text).clicked();
                false
            }
            (PropertyKind::Link(text), _) => {
                *pressed = ui.link(text).clicked();
                false
            }
            (PropertyKind::Reference, PropertyValue::Text(target)) => {
                *pressed = ui.link(format!("{target} \u{2197}")).clicked();
                false
            }
            (PropertyKind::Choice(options), PropertyValue::Choice(index)) => {
                let mut changed = false;
                let shown = options.get(*index).map_or("", String::as_str);
                let enabled = ui.is_enabled();
                let name = self.label_text();
                let combo = ComboBox::from_id_salt(self.id().as_str())
                    .selected_text(shown)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (position, option) in options.iter().enumerate() {
                            if ui.selectable_value(index, position, option).changed() {
                                changed = true;
                            }
                        }
                    });
                combo
                    .response
                    .widget_info(|| WidgetInfo::labeled(Role::ComboBox, enabled, name));
                changed
            }
            (PropertyKind::Color, PropertyValue::Color(rgb)) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let changed = ui.color_edit_button_srgb(rgb).changed();
                    // the text is kept while it is being typed, and follows the
                    // color when it is not
                    let edit_id = ui.scope_id().with("hex");
                    let mut text: String = if ui.memory(|memory| memory.has_focus(edit_id)) {
                        ui.data(|data| data.get_temp(edit_id)).unwrap_or_default()
                    } else {
                        format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
                    };
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .id(edit_id)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(84.0),
                    );
                    let mut edited = false;
                    if response.changed() {
                        let digits = text.trim().trim_start_matches('#');
                        if digits.len() == 6 {
                            if let Ok(value) = u32::from_str_radix(digits, 16) {
                                *rgb = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
                                edited = true;
                            }
                        }
                    }
                    if response.has_focus() {
                        ui.data_mut(|data| data.insert_temp(edit_id, text));
                    }
                    changed || edited
                })
                .inner
            }
            (PropertyKind::TextList | PropertyKind::KeyValue, value) => {
                self.edit_collection(ui, value, labels)
            }
            _ => {
                ui.label(RichText::new(self.default_value().type_name()).weak());
                false
            }
        }
    }

    /// The room of a number: wide enough to show its digits whatever layout
    /// the cell is in.
    fn number_size(ui: &Ui) -> egui::Vec2 {
        egui::vec2(
            ui.available_width().clamp(64.0, 120.0),
            ui.spacing().interact_size.y,
        )
    }

    /// A list: how many entries it has and a button that opens its window.
    fn edit_collection(
        &self,
        ui: &mut Ui,
        value: &mut PropertyValue,
        labels: &PropertyLabels,
    ) -> bool {
        let state: Id = ui.make_persistent_id(("collection", self.id().as_str()));
        let count = match &*value {
            PropertyValue::TextList(items) => items.len(),
            PropertyValue::KeyValue(pairs) => pairs.len(),
            _ => return false,
        };
        let mut open = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            open = ui.button(&labels.edit).clicked();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(format!("{count} {}", labels.items)).weak());
            });
        });
        if open {
            let draft = Draft::from_value(value);
            ui.data_mut(|data| data.insert_temp(state, Some(draft)));
        }
        let ctx = ui.ctx().clone();
        let Some(Some(mut draft)) = ctx.data(|data| data.get_temp::<Option<Draft>>(state)) else {
            return false;
        };
        let mut action = DraftAction::Keep;
        Window::new(self.label_text())
            .id(state)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(&ctx, |ui| {
                match &*value {
                    PropertyValue::TextList(_) => {
                        ui.add(
                            TextEdit::multiline(&mut draft.text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(6),
                        );
                    }
                    _ => draft.show_pairs(ui, state, labels),
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(&labels.ok).clicked() {
                        action = DraftAction::Apply;
                    }
                    if ui.button(&labels.cancel).clicked() {
                        action = DraftAction::Cancel;
                    }
                });
            });
        match action {
            DraftAction::Keep => {
                ctx.data_mut(|data| data.insert_temp(state, Some(draft)));
                false
            }
            DraftAction::Cancel => {
                ctx.data_mut(|data| data.insert_temp(state, None::<Draft>));
                false
            }
            DraftAction::Apply => {
                ctx.data_mut(|data| data.insert_temp(state, None::<Draft>));
                let edited = draft.into_value(value);
                let changed = edited != *value;
                *value = edited;
                changed
            }
        }
    }
}

impl Draft {
    /// The draft of the window of `value`.
    fn from_value(value: &PropertyValue) -> Self {
        match value {
            PropertyValue::TextList(items) => Self {
                text: items.join("\n"),
                ..Self::default()
            },
            PropertyValue::KeyValue(pairs) => Self {
                pairs: pairs.clone(),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }

    /// What the draft makes of `value`: a line per entry, without the empty
    /// ones, or the pairs that have a key.
    fn into_value(self, value: &PropertyValue) -> PropertyValue {
        match value {
            PropertyValue::TextList(_) => PropertyValue::TextList(
                self.text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            PropertyValue::KeyValue(_) => PropertyValue::KeyValue(
                self.pairs
                    .into_iter()
                    .filter(|(key, _)| !key.trim().is_empty())
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// The table of keys and values, in the look of the property grid: a
    /// header over each column and, in the first one, a triangle on the row
    /// being edited. A key is a label until it is double-clicked; then it is
    /// a text field until it loses the focus.
    fn show_pairs(&mut self, ui: &mut Ui, id: Id, labels: &PropertyLabels) {
        ui.horizontal(|ui| {
            if ui.button(&labels.add).clicked() {
                self.pairs.push((String::new(), String::new()));
                self.selected = self.pairs.len() - 1;
                self.editing = Some(self.selected);
                self.focus_pending = true;
            }
            let has_rows = !self.pairs.is_empty();
            if ui
                .add_enabled(has_rows, Button::new(&labels.remove))
                .clicked()
            {
                let at = self.selected.min(self.pairs.len() - 1);
                self.pairs.remove(at);
                self.selected = at.saturating_sub(1);
                self.editing = None;
            }
        });
        let visuals = ui.visuals().clone();
        let stroke = visuals.widgets.noninteractive.bg_stroke;
        let margin = Margin::symmetric(6, 1);
        let padding = margin.sum().x;
        let marker_width = 22.0;
        let key_width = 140.0;
        // The table fills the window. What it measures beyond the width it
        // was given (the borders of its cells) is taken off the next time,
        // or the window would grow a little every frame.
        let extra_id = id.with("table_extra");
        let extra = ui
            .data(|data| data.get_temp::<f32>(extra_id))
            .unwrap_or(0.0);
        let width = (ui.available_width() - extra).max(320.0);
        let value_width = (width - marker_width - key_width).max(100.0);
        let widths = [marker_width, key_width, value_width];
        let height = ui.spacing().interact_size.y;
        let Self {
            pairs,
            selected,
            editing,
            focus_pending,
            ..
        } = self;
        let mut lines: Vec<Rect> = Vec::new();
        let table = ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let header = Frame::new()
                .fill(visuals.widgets.inactive.weak_bg_fill)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let titles = ["", labels.key.as_str(), labels.value.as_str()];
                        for (width, title) in widths.iter().zip(titles) {
                            Frame::new().inner_margin(margin).show(ui, |ui| {
                                ui.set_width((width - padding).max(0.0));
                                ui.label(RichText::new(title).strong());
                            });
                        }
                    });
                });
            lines.push(header.response.rect);
            for (position, (key, value)) in pairs.iter_mut().enumerate() {
                let row = ui.horizontal_top(|ui| {
                    Frame::new().inner_margin(margin).show(ui, |ui| {
                        ui.set_width((marker_width - padding).max(0.0));
                        ui.set_min_height(height);
                        if *selected == position {
                            ui.label(
                                RichText::new("\u{25B6}").color(visuals.selection.stroke.color),
                            );
                        }
                    });
                    Frame::new().inner_margin(margin).show(ui, |ui| {
                        ui.set_width((key_width - padding).max(0.0));
                        if *editing == Some(position) {
                            let response =
                                ui.add(TextEdit::singleline(key).desired_width(f32::INFINITY));
                            if *focus_pending {
                                response.request_focus();
                                *focus_pending = false;
                            } else if response.lost_focus() {
                                *editing = None;
                            }
                        } else {
                            let (rect, response) = ui.allocate_exact_size(
                                vec2((key_width - padding).max(0.0), height),
                                Sense::click(),
                            );
                            ui.painter().text(
                                rect.left_center(),
                                Align2::LEFT_CENTER,
                                key.as_str(),
                                TextStyle::Body.resolve(ui.style()),
                                visuals.text_color(),
                            );
                            if response.double_clicked() {
                                *editing = Some(position);
                                *focus_pending = true;
                            }
                        }
                    });
                    Frame::new().inner_margin(margin).show(ui, |ui| {
                        ui.set_width((value_width - padding).max(0.0));
                        let response =
                            ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
                        if response.gained_focus() {
                            *selected = position;
                        }
                    });
                });
                let rect = row.response.rect;
                if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.primary_pressed()) {
                    *selected = position;
                }
                lines.push(rect);
            }
        });
        let outer = table.response.rect;
        let painter = ui.painter();
        painter.rect_stroke(outer, 0.0, stroke, StrokeKind::Inside);
        for rect in &lines {
            painter.hline(outer.x_range(), rect.bottom(), stroke);
        }
        for x in [
            outer.left() + marker_width,
            outer.left() + marker_width + key_width,
        ] {
            painter.vline(x, outer.y_range(), stroke);
        }
        let measured = (outer.width() - width).max(0.0);
        ui.data_mut(|data| data.insert_temp(extra_id, measured));
    }
}
