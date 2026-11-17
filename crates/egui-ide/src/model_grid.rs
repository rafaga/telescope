//! The editors that [`PropertyGrid::show_model`] gives each kind of property.

use crate::model::{
    PropertyDescriptor, PropertyId, PropertyKind, PropertyLabels, PropertyModel, PropertyValue,
    Severity,
};
use crate::property_grid::PropertyGrid;
#[cfg(feature = "native-dialogs")]
use native_tools::dialog::{Dialog, DialogResult, DialogType};
#[cfg(feature = "native-dialogs")]
use std::path::Path;
#[cfg(feature = "native-dialogs")]
use std::sync::{Arc, Mutex};
use egui::{
    Align, Align2, Button, ComboBox, DragValue, Frame, Grid, Id, Label, Layout, Margin, Popup, Rect,
    RichText, Sense, StrokeKind, TextEdit, TextStyle, Ui, Vec2, Window, vec2,
};

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
            #[cfg(feature = "native-dialogs")]
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
            #[cfg(not(feature = "native-dialogs"))]
            (PropertyKind::File | PropertyKind::Folder, PropertyValue::Text(text)) => ui
                .add(TextEdit::singleline(text).desired_width(f32::INFINITY))
                .changed(),
            (PropertyKind::Date, PropertyValue::Text(text)) => {
                let view_id = ui.scope_id().with(("date_view", self.id().as_str()));
                let mut changed = false;
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // the button is placed first, so the text takes the rest
                    let button = ui.button("\u{1F4C6}");
                    if button.clicked() {
                        let (year, month, _) = Calendar::parse(text).unwrap_or_else(Calendar::today);
                        ui.data_mut(|data| data.insert_temp(view_id, (year, month)));
                    }
                    Popup::menu(&button).show(|ui| {
                        let (mut year, mut month) = ui
                            .data(|data| data.get_temp::<(i32, u32)>(view_id))
                            .unwrap_or_else(|| {
                                let (year, month, _) = Calendar::today();
                                (year, month)
                            });
                        ui.horizontal(|ui| {
                            if ui.button("\u{25C0}").clicked() {
                                (year, month) = if month == 1 { (year - 1, 12) } else { (year, month - 1) };
                            }
                            let name = labels.months.get(month as usize - 1).map_or("", String::as_str);
                            ui.label(RichText::new(format!("{name} {year}")).strong());
                            if ui.button("\u{25B6}").clicked() {
                                (year, month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
                            }
                        });
                        let first = Calendar::weekday(year, month, 1);
                        let count = Calendar::days_in_month(year, month);
                        let chosen = Calendar::parse(text);
                        Grid::new(view_id.with("days")).show(ui, |ui| {
                            for name in labels.weekdays.iter().take(7) {
                                ui.label(RichText::new(name).weak());
                            }
                            ui.end_row();
                            for index in 0..(first + count).div_ceil(7) * 7 {
                                if index >= first && index - first < count {
                                    let day = index - first + 1;
                                    let selected = chosen == Some((year, month, day));
                                    if ui.add(Button::selectable(selected, day.to_string())).clicked() {
                                        *text = Calendar::format(year, month, day);
                                        changed = true;
                                        ui.close();
                                    }
                                } else {
                                    ui.label("");
                                }
                                if index % 7 == 6 {
                                    ui.end_row();
                                }
                            }
                        });
                        ui.data_mut(|data| data.insert_temp(view_id, (year, month)));
                    });
                    let valid = text.is_empty() || Calendar::parse(text).is_some();
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
                ComboBox::from_id_salt((self.id().as_str(), "bool"))
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
                changed
            }
            (PropertyKind::Flags(names), PropertyValue::Flags(flags)) => {
                if flags.len() < names.len() {
                    flags.resize(names.len(), false);
                }
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    for (name, flag) in names.iter().zip(flags.iter_mut()) {
                        changed |= ui.checkbox(flag, name).changed();
                    }
                });
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
                ComboBox::from_id_salt(self.id().as_str())
                    .selected_text(shown)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (position, option) in options.iter().enumerate() {
                            if ui.selectable_value(index, position, option).changed() {
                                changed = true;
                            }
                        }
                    });
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
        let extra = ui.data(|data| data.get_temp::<f32>(extra_id)).unwrap_or(0.0);
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
                            let response = ui.add(
                                TextEdit::singleline(key).desired_width(f32::INFINITY),
                            );
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
        for x in [outer.left() + marker_width, outer.left() + marker_width + key_width] {
            painter.vline(x, outer.y_range(), stroke);
        }
        let measured = (outer.width() - width).max(0.0);
        ui.data_mut(|data| data.insert_temp(extra_id, measured));
    }
}

/// The dates of the calendar of a date property, by the days since 1970-01-01
/// of the Gregorian calendar.
struct Calendar;

impl Calendar {
    /// The days since 1970-01-01 of a date.
    fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
        let year = i64::from(year) - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted = i64::from(if month > 2 { month - 3 } else { month + 9 });
        let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// The date of a number of days since 1970-01-01.
    fn civil_from_days(days: i64) -> (i32, u32, u32) {
        let days = days + 719_468;
        let era = days.div_euclid(146_097);
        let day_of_era = days - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * shifted + 2) / 5 + 1) as u32;
        let month = if shifted < 10 { shifted + 3 } else { shifted - 9 } as u32;
        let year = (year_of_era + era * 400 + i64::from(month <= 2)) as i32;
        (year, month, day)
    }

    /// Today, by the clock of the system.
    fn today() -> (i32, u32, u32) {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Self::civil_from_days((seconds / 86_400) as i64)
    }

    /// The days of a month.
    fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
            _ => 28,
        }
    }

    /// The day of the week of a date, counted from Monday as 0.
    fn weekday(year: i32, month: u32, day: u32) -> u32 {
        (Self::days_from_civil(year, month, day) + 3).rem_euclid(7) as u32
    }

    /// The date of a `YYYY-MM-DD` text, if it is one.
    fn parse(text: &str) -> Option<(i32, u32, u32)> {
        let mut parts = text.trim().split('-');
        let year = parts.next()?.parse::<i32>().ok()?;
        let month = parts.next()?.parse::<u32>().ok()?;
        let day = parts.next()?.parse::<u32>().ok()?;
        let valid = parts.next().is_none()
            && (1..=12).contains(&month)
            && (1..=Self::days_in_month(year, month)).contains(&day);
        valid.then_some((year, month, day))
    }

    /// A date as `YYYY-MM-DD`.
    fn format(year: i32, month: u32, day: u32) -> String {
        format!("{year:04}-{month:02}-{day:02}")
    }
}
