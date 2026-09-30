//! Controls and feedback elements.

use crate::theme::{Theme, mix};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Button, Color32, CornerRadius, FontId, Frame, ImageSource, InnerResponse, Label,
    Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2,
    WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};

/// Paints `text` on one line, cut with an ellipsis where it would pass
/// `max_width`. `anchor` is the point of the text box that sits at `pos`
/// (left or right end, vertically centered). Returns the rectangle painted.
pub(crate) fn paint_fitted(
    ui: &Ui,
    pos: egui::Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = ui.painter().layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// What a [`status`] line reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// Everything is fine (a check mark).
    Ok,
    /// Something needs attention (a warning triangle).
    Warning,
    /// Something failed (a cross).
    Error,
    /// Plain information (a ring).
    Info,
}

/// A short line of feedback with an icon in its color: "Folder found · 5
/// channels", "The database is missing". The icon is painted, not a font
/// glyph, so it shows with any fonts.
pub fn status(ui: &mut Ui, kind: StatusKind, text: &str) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let color = match kind {
        StatusKind::Ok => palette.ok,
        StatusKind::Warning => palette.warning,
        StatusKind::Error => palette.error,
        StatusKind::Info => palette.muted_text,
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(theme.small_size), Sense::hover());
        paint_status_icon(ui, rect, kind, color);
        ui.add(Label::new(RichText::new(text).size(theme.small_size).color(color)).wrap());
    })
    .response
}

/// A [`status`] line in a hairline box as wide as the space left: feedback
/// about a whole page ("The rule graph is valid") rather than about one
/// field.
pub fn notice(ui: &mut Ui, kind: StatusKind, text: &str) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    Frame::new()
        .stroke(Stroke::new(1.0, palette.card_stroke))
        .corner_radius(CornerRadius::same(theme.radius))
        .inner_margin(egui::Margin::symmetric(14, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            status(ui, kind, text);
        })
        .response
}

fn paint_status_icon(ui: &Ui, rect: Rect, kind: StatusKind, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.8, color);
    let at = |x: f32, y: f32| {
        pos2(
            rect.left() + rect.width() * x,
            rect.top() + rect.height() * y,
        )
    };
    match kind {
        StatusKind::Ok => {
            painter.line_segment([at(0.15, 0.55), at(0.42, 0.8)], stroke);
            painter.line_segment([at(0.42, 0.8), at(0.88, 0.25)], stroke);
        }
        StatusKind::Error => {
            painter.line_segment([at(0.2, 0.2), at(0.8, 0.8)], stroke);
            painter.line_segment([at(0.8, 0.2), at(0.2, 0.8)], stroke);
        }
        StatusKind::Warning => {
            let thin = Stroke::new(1.4, color);
            painter.line_segment([at(0.5, 0.08), at(0.05, 0.92)], thin);
            painter.line_segment([at(0.05, 0.92), at(0.95, 0.92)], thin);
            painter.line_segment([at(0.95, 0.92), at(0.5, 0.08)], thin);
            painter.line_segment([at(0.5, 0.38), at(0.5, 0.64)], thin);
            painter.circle_filled(at(0.5, 0.78), 0.9, color);
        }
        StatusKind::Info => {
            painter.circle_stroke(rect.center(), rect.width() * 0.4, Stroke::new(1.4, color));
        }
    }
}

/// A small bordered tag: a count, an output kind, a version.
pub fn badge(ui: &mut Ui, text: &str) -> Response {
    chip(ui, text, None)
}

/// Several [`badge`]s flowing over as many lines as the width needs: a badge
/// that does not fit in what is left of a line starts the next one, whole.
pub fn badges(ui: &mut Ui, labels: &[String]) {
    let theme = Theme::get(ui.ctx());
    let font = FontId::proportional(theme.small_size);
    let gap = 6.0;
    let available = ui.available_width();
    // Text plus the badge's margins (8 each side) and its border.
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| {
            let galley = ui
                .painter()
                .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE);
            galley.size().x + 18.0
        })
        .collect();
    let mut start = 0;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        while start < labels.len() {
            let mut end = start;
            let mut used = 0.0;
            while end < labels.len() {
                let next = used + widths[end] + if end > start { gap } else { 0.0 };
                if end > start && next > available {
                    break;
                }
                used = next;
                end += 1;
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for label in &labels[start..end] {
                    badge(ui, label);
                }
            });
            start = end;
        }
    });
}

/// A tag with a main text and, dimmed after it, a secondary one:
/// "Kara Voss · H-5GUI". A one pixel border and small corners, no fill.
pub fn chip(ui: &mut Ui, text: &str, secondary: Option<&str>) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    Frame::new()
        .fill(palette.badge_fill)
        .stroke(Stroke::new(1.0, palette.card_stroke))
        .corner_radius(CornerRadius::same(theme.radius.saturating_sub(1)))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            // A tag is never squeezed to a column of letters: it keeps its width.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(
                RichText::new(text)
                    .size(theme.small_size)
                    .color(palette.text),
            );
            if let Some(secondary) = secondary {
                ui.label(
                    RichText::new(format!("· {secondary}"))
                        .size(theme.small_size)
                        .color(palette.muted_text),
                );
            }
        })
        .response
}

/// How a [`button`] is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The main action of a page or dialog: filled with the accent.
    Primary,
    /// A regular action: egui's own button look.
    Secondary,
    /// A lesser action: only a border.
    Ghost,
    /// A destructive action: a border and text in the error color.
    Danger,
}

/// A button of [`Theme::control_height`] in the given [`Variant`].
pub fn button(ui: &mut Ui, text: impl Into<String>, variant: Variant) -> Response {
    ui.add(styled_button(ui, text, variant))
}

/// A [`button`] that opens a menu drawn by `add_contents` (close it with
/// [`Ui::close`] once an entry is picked). Returns the button's response.
pub fn menu_button(
    ui: &mut Ui,
    text: impl Into<String>,
    variant: Variant,
    add_contents: impl FnOnce(&mut Ui),
) -> Response {
    let text = format!("{} ⏷", text.into());
    egui::containers::menu::MenuButton::from_button(styled_button(ui, text, variant))
        .ui(ui, add_contents)
        .0
}

fn styled_button(ui: &Ui, text: impl Into<String>, variant: Variant) -> Button<'static> {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let text: String = text.into();
    let border = ui.visuals().widgets.inactive.bg_stroke.color;
    let widget = match variant {
        Variant::Primary => Button::new(RichText::new(text).color(palette.on_accent))
            .fill(palette.accent)
            .stroke(Stroke::new(1.0, palette.accent_stroke)),
        Variant::Secondary => Button::new(text),
        Variant::Ghost => Button::new(text)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, border.max(palette.card_stroke))),
        Variant::Danger => Button::new(RichText::new(text).color(palette.error))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(
                1.0,
                mix(palette.card_stroke, palette.error, 0.45),
            )),
    };
    widget
        .corner_radius(CornerRadius::same(theme.radius.saturating_sub(1)))
        .min_size(vec2(0.0, theme.control_height))
}

trait MaxColor {
    fn max(self, other: Color32) -> Color32;
}

impl MaxColor for Color32 {
    /// The more opaque of the two (a transparent theme stroke falls back to
    /// the card stroke).
    fn max(self, other: Color32) -> Color32 {
        if self.a() >= other.a() { self } else { other }
    }
}

/// An on/off switch. Returns a response marked changed when it was flipped.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let height = (theme.control_height * 0.62).round();
    let size = vec2(height * 1.8, height);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let off_fill = ui.visuals().widgets.inactive.bg_fill;
        let fill = mix(off_fill, palette.accent, how_on);
        let radius = rect.height() / 2.0;
        ui.painter()
            .rect(rect, radius, fill, visuals.bg_stroke, StrokeKind::Inside);
        let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        // A white knob reads on the accent and on the off track in both
        // themes; on a light theme it gets a thin border to stand out.
        let knob_border = if ui.visuals().dark_mode {
            Stroke::NONE
        } else {
            Stroke::new(1.0, palette.muted_text)
        };
        ui.painter().circle(
            pos2(knob_x, rect.center().y),
            radius * 0.72,
            Color32::WHITE,
            knob_border,
        );
    }
    response
}

/// A row of joined buttons picking one of `options` (value and label).
/// Returns a response marked changed when the value changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), palette.text);
            (galley.size().x + 20.0).max(theme.control_height)
        })
        .collect();
    // With less room than the labels need, every segment gets narrower by the
    // same factor and its label is cut short.
    let natural: f32 = widths.iter().sum();
    let scale = (ui.available_width() / natural).clamp(0.3, 1.0);
    let widths: Vec<f32> = widths.iter().map(|width| width * scale).collect();
    let total = vec2(widths.iter().sum(), theme.control_height);
    let (rect, mut response) = ui.allocate_exact_size(total, Sense::hover());
    let radius = CornerRadius::same(4);
    let mut x = rect.left();
    let mut changed = false;
    for (index, ((option, label), width)) in options.iter().zip(&widths).enumerate() {
        let segment = Rect::from_min_size(pos2(x, rect.top()), vec2(*width, rect.height()));
        x += width;
        let id = response.id.with(index);
        let segment_response = ui.interact(segment, id, Sense::click());
        let selected = *value == *option;
        segment_response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::SelectableLabel,
                ui.is_enabled(),
                selected,
                *label,
            )
        });
        if segment_response.clicked() && !selected {
            *value = *option;
            changed = true;
        }
        let selected = *value == *option;
        let fill = if selected {
            palette.accent
        } else if segment_response.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        let corner = match (index, options.len()) {
            (_, 1) => radius,
            (0, _) => CornerRadius {
                ne: 0,
                se: 0,
                ..radius
            },
            (last, count) if last + 1 == count => CornerRadius {
                nw: 0,
                sw: 0,
                ..radius
            },
            _ => CornerRadius::ZERO,
        };
        ui.painter().rect_filled(segment, corner, fill);
        let text_color = if selected {
            palette.on_accent
        } else {
            palette.text
        };
        let mut job = LayoutJob::single_section(
            label.to_string(),
            TextFormat::simple(font.clone(), text_color),
        );
        job.wrap = TextWrapping::truncate_at_width((segment.width() - 8.0).max(0.0));
        let galley = ui.painter().layout_job(job);
        let text_rect = Align2::CENTER_CENTER.anchor_size(segment.center(), galley.size());
        ui.painter().galley(text_rect.min, galley, text_color);
        if index > 0 {
            ui.painter().line_segment(
                [segment.left_top(), segment.left_bottom()],
                Stroke::new(1.0, palette.card_stroke),
            );
        }
    }
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.card_stroke),
        StrokeKind::Inside,
    );
    if changed {
        response.mark_changed();
    }
    response
}

/// A row of a list of things to pick from: a check box, the title and, dimmed
/// right after it, an optional subtitle. There is no card: the row only gets
/// a faint wash under the pointer. Returns a response marked changed when it
/// was flipped.
pub fn tile(ui: &mut Ui, on: &mut bool, title: &str, subtitle: Option<&str>) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme.tile_min_height),
        Sense::click(),
    );
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, title));
    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(theme.radius.saturating_sub(1)),
                palette.hover_fill,
            );
        }
        let box_size = 14.0;
        let box_rect = Rect::from_center_size(
            pos2(rect.left() + 8.0 + box_size / 2.0, rect.center().y),
            Vec2::splat(box_size),
        );
        paint_check_box(ui, box_rect, *on);
        let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Body].size);
        let color = if *on {
            palette.strong_text
        } else {
            palette.text
        };
        let left = box_rect.right() + 10.0;
        let right = rect.right() - 8.0;
        let title_rect = paint_fitted(
            ui,
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            font,
            color,
            right - left,
        );
        if let Some(subtitle) = subtitle {
            let start = title_rect.right() + 8.0;
            if right - start > 24.0 {
                paint_fitted(
                    ui,
                    pos2(start, rect.center().y),
                    Align2::LEFT_CENTER,
                    subtitle,
                    FontId::proportional(theme.small_size - 0.5),
                    palette.muted_text,
                    right - start,
                );
            }
        }
    }
    response
}

/// A check box like egui's: a small rounded square, filled with the accent
/// and holding a check mark when `on`.
fn paint_check_box(ui: &Ui, rect: Rect, on: bool) {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let radius = CornerRadius::same(3);
    if on {
        ui.painter().rect_filled(rect, radius, palette.accent);
        let stroke = Stroke::new(1.8, palette.on_accent);
        let at = |x: f32, y: f32| {
            pos2(
                rect.left() + rect.width() * x,
                rect.top() + rect.height() * y,
            )
        };
        ui.painter()
            .line_segment([at(0.22, 0.52), at(0.43, 0.74)], stroke);
        ui.painter()
            .line_segment([at(0.43, 0.74), at(0.8, 0.3)], stroke);
    } else {
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, palette.muted_text),
            StrokeKind::Inside,
        );
    }
}

/// A button that stays pressed: filled with the accent while `on`, a plain
/// bordered button otherwise, with an optional dimmed `tag` at its right end.
/// Meant for picking several items from a grid (see [`tile_grid`]). Returns a
/// response marked changed when it was flipped.
pub fn toggle_button(ui: &mut Ui, on: &mut bool, label: &str, tag: Option<&str>) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let (rect, mut response) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme.control_height + 4.0),
        Sense::click(),
    );
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(theme.radius.saturating_sub(1));
        let (fill, stroke, text) = if *on {
            (palette.accent, palette.accent_stroke, palette.on_accent)
        } else if response.hovered() {
            (
                ui.visuals().widgets.hovered.weak_bg_fill,
                palette.card_stroke,
                palette.text,
            )
        } else {
            (Color32::TRANSPARENT, palette.card_stroke, palette.text)
        };
        ui.painter().rect(
            rect,
            radius,
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        let font = FontId::proportional(theme.small_size);
        let right = rect.right() - 10.0;
        let mut label_right = right;
        if let Some(tag) = tag {
            let tag_rect = paint_fitted(
                ui,
                pos2(right, rect.center().y),
                Align2::RIGHT_CENTER,
                tag,
                FontId::proportional(theme.small_size - 1.5),
                if *on { text } else { palette.muted_text },
                (rect.width() * 0.4).max(0.0),
            );
            label_right = tag_rect.left() - 8.0;
        }
        paint_fitted(
            ui,
            pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            text,
            label_right - (rect.left() + 10.0),
        );
    }
    response
}

/// The narrowest a cell of a [`tile_grid`] gets before the grid drops a column.
const TILE_MIN_WIDTH: f32 = 170.0;

/// Lays out `count` items in up to `columns` equal columns, row by row, calling
/// `add_item` with each index. Meant for [`tile`]s and [`toggle_button`]s.
pub fn tile_grid(
    ui: &mut Ui,
    columns: usize,
    count: usize,
    mut add_item: impl FnMut(&mut Ui, usize),
) {
    // `columns` is the most the grid uses: with less room it has fewer, so
    // no cell is narrower than `TILE_MIN_WIDTH`.
    let gap = 20.0;
    let fitting = ((ui.available_width() + gap) / (TILE_MIN_WIDTH + gap)).floor() as usize;
    let columns = columns.min(fitting).max(1);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, 4.0);
        for row in 0..count.div_ceil(columns) {
            ui.columns(columns, |cells| {
                for (column, cell) in cells.iter_mut().enumerate() {
                    let index = row * columns + column;
                    if index < count {
                        add_item(cell, index);
                    }
                }
            });
        }
    });
}

/// A path text field with a Browse button and an optional Reset button.
///
/// The crate opens no dialog: when [`PathPickerResponse::browse`] is set,
/// the host opens its own file or folder dialog and writes the result back.
#[must_use = "call `show` to draw the picker"]
pub struct PathPicker<'a> {
    path: &'a mut String,
    browse_label: &'a str,
    reset_label: Option<&'a str>,
    hint: Option<&'a str>,
    editable: bool,
}

/// What happened to a [`PathPicker`] this frame.
pub struct PathPickerResponse {
    /// The text was edited by hand.
    pub edited: bool,
    /// The Browse button was clicked.
    pub browse: bool,
    /// The Reset button was clicked.
    pub reset: bool,
    /// The response of the text field.
    pub response: Response,
}

impl<'a> PathPicker<'a> {
    /// A picker editing `path`, with a Browse button labelled
    /// `browse_label`.
    pub fn new(path: &'a mut String, browse_label: &'a str) -> Self {
        Self {
            path,
            browse_label,
            reset_label: None,
            hint: None,
            editable: true,
        }
    }

    /// Adds a Reset button (back to a default path) labelled `label`.
    pub fn reset(mut self, label: &'a str) -> Self {
        self.reset_label = Some(label);
        self
    }

    /// Placeholder shown while the field is empty.
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }

    /// Whether the path can be typed (default `true`); when `false` only the
    /// buttons change it.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Draws the field and its buttons in the available width.
    pub fn show(self, ui: &mut Ui) -> PathPickerResponse {
        let theme = Theme::get(ui.ctx());
        let size = vec2(ui.available_width(), theme.control_height);
        let mut browse = false;
        let mut reset = false;
        // Right to left: the buttons take their size first, the field the
        // width that remains.
        let response = ui
            .allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(label) = self.reset_label {
                    reset = button(ui, label, Variant::Ghost).clicked();
                }
                browse = button(ui, self.browse_label, Variant::Secondary).clicked();
                let mut edit = TextEdit::singleline(self.path)
                    .desired_width(ui.available_width())
                    .min_size(vec2(0.0, theme.control_height))
                    .vertical_align(Align::Center)
                    .interactive(self.editable);
                if let Some(hint) = self.hint {
                    edit = edit.hint_text(hint);
                }
                ui.add(edit)
            })
            .inner;
        PathPickerResponse {
            edited: response.changed(),
            browse,
            reset,
            response,
        }
    }
}

/// Numbered steps in one joined group ("1 · Sources | 2 · Rules | 3 ·
/// Alerts"), the `current` one filled with the accent: the same control as
/// [`segmented`]. Returns the index of a step that was clicked.
pub fn stepper(ui: &mut Ui, steps: &[&str], current: usize) -> Option<usize> {
    let labels: Vec<String> = steps
        .iter()
        .enumerate()
        .map(|(index, step)| format!("{} · {step}", index + 1))
        .collect();
    let options: Vec<(usize, &str)> = labels.iter().map(String::as_str).enumerate().collect();
    let mut value = current;
    let response = segmented(ui, &mut value, &options);
    response.changed().then_some(value)
}

/// The phases of a running task, one per line: those before `current` are
/// done (a check mark), `current` is running (a spinner, emphasized text)
/// and the ones after it are pending (dimmed).
pub fn progress_steps(ui: &mut Ui, steps: &[&str], current: usize) -> Response {
    progress_steps_with_notes(ui, steps, &[], current)
}

/// [`progress_steps`] with a small dimmed note at the right end of each
/// line (a size, a duration). `notes[i]` belongs to `steps[i]`; a missing or
/// empty note draws nothing.
pub fn progress_steps_with_notes(
    ui: &mut Ui,
    steps: &[&str],
    notes: &[String],
    current: usize,
) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        for (index, step) in steps.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                let color = if index < current {
                    paint_status_icon(ui, rect.shrink(1.0), StatusKind::Ok, palette.ok);
                    palette.text
                } else if index == current {
                    egui::Spinner::new()
                        .color(palette.accent_stroke)
                        .paint_at(ui, rect);
                    palette.strong_text
                } else {
                    ui.painter().circle_stroke(
                        rect.center(),
                        rect.width() * 0.3,
                        Stroke::new(1.2, palette.muted_text),
                    );
                    palette.muted_text
                };
                ui.label(RichText::new(*step).color(color));
                if let Some(note) = notes.get(index).filter(|note| !note.is_empty()) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(note)
                                .size(theme.small_size)
                                .color(palette.muted_text),
                        );
                    });
                }
            });
        }
    })
    .response
}

/// The picture of an [`EntityCard`].
pub enum Avatar<'a> {
    /// An image (a portrait, a logo).
    Image(ImageSource<'a>),
    /// Initials on a tinted square, when there is no image.
    Initials(String),
    /// No picture.
    None,
}

/// A card for one item of a list (an account, a device, a character): a
/// picture, a title, a few lines and actions at the right end.
#[must_use = "call `show` to draw the card"]
pub struct EntityCard<'a> {
    title: &'a str,
    lines: Vec<WidgetText>,
    avatar: Avatar<'a>,
    avatar_size: f32,
    selected: bool,
}

impl<'a> EntityCard<'a> {
    /// A card titled `title`.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            lines: Vec::new(),
            avatar: Avatar::None,
            avatar_size: 64.0,
            selected: false,
        }
    }

    /// Adds a line under the title (the first is regular text, the next ones
    /// dimmed).
    pub fn line(mut self, line: impl Into<WidgetText>) -> Self {
        self.lines.push(line.into());
        self
    }

    /// The picture at the left.
    pub fn avatar(mut self, avatar: Avatar<'a>) -> Self {
        self.avatar = avatar;
        self
    }

    /// Side of the square picture, in points (default 64).
    pub fn avatar_size(mut self, size: f32) -> Self {
        self.avatar_size = size;
        self
    }

    /// Draws the card highlighted.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draws the card; `actions` are laid out right to left at its right end.
    /// The response senses clicks on the whole card, except on the widgets
    /// inside it (the actions), which get their own clicks.
    pub fn show<R>(self, ui: &mut Ui, actions: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let stroke = if self.selected {
            palette.accent_stroke
        } else {
            Color32::TRANSPARENT
        };
        let fill = if self.selected {
            palette.card_fill_selected
        } else {
            palette.card_fill
        };
        // The click sense is the scope's own, registered before its
        // contents: a sense added to the card's response afterwards would sit
        // on top of the action buttons and take their clicks.
        let scope = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(CornerRadius::same(theme.radius))
                .inner_margin(egui::Margin::symmetric(8, 14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 16.0;
                        let size = Vec2::splat(self.avatar_size);
                        match self.avatar {
                            Avatar::Image(source) => {
                                ui.add(egui::Image::new(source).fit_to_exact_size(size));
                            }
                            Avatar::Initials(initials) => {
                                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(theme.radius.saturating_sub(1)),
                                    ui.visuals().widgets.inactive.bg_fill,
                                );
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    initials,
                                    FontId::proportional(self.avatar_size * 0.32),
                                    palette.strong_text,
                                );
                            }
                            Avatar::None => {}
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.label(
                                RichText::new(self.title)
                                    .size(theme.section_title_size + 1.0)
                                    .color(palette.strong_text),
                            );
                            for (index, line) in self.lines.into_iter().enumerate() {
                                let color = if index == 0 {
                                    palette.muted_text
                                } else {
                                    palette.muted_text.gamma_multiply(0.8)
                                };
                                ui.label(line.color(color));
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), actions)
                            .inner
                    })
                    .inner
                })
                .inner
        });
        // A hairline under each card tells the rows apart.
        let rect = scope.response.rect;
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0, palette.separator),
        );
        InnerResponse::new(scope.inner, scope.response)
    }
}

/// A horizontal slider drawn with the theme: a thin rail filled with the
/// accent up to a small round knob. Drag or click to set it; with focus, the arrow keys move
/// it one step. Show the value next to it yourself (it has no label, so any
/// format fits: `4:00`, `60 %`).
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui, seconds: &mut u32) {
/// egui_panels::Slider::new(seconds, 10..=600).step(10.0).show(ui);
/// # }
/// ```
#[must_use = "call `show` to draw the slider"]
pub struct Slider<'a, N: egui::emath::Numeric> {
    value: &'a mut N,
    range: std::ops::RangeInclusive<N>,
    step: Option<f64>,
    width: f32,
}

impl<'a, N: egui::emath::Numeric> Slider<'a, N> {
    /// A slider editing `value` within `range`.
    pub fn new(value: &'a mut N, range: std::ops::RangeInclusive<N>) -> Self {
        Self {
            value,
            range,
            step: None,
            width: 320.0,
        }
    }

    /// Snaps the value to multiples of `step` (from the start of the range).
    pub fn step(mut self, step: f64) -> Self {
        self.step = (step > 0.0).then_some(step);
        self
    }

    /// Width of the rail, in points (default 320).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Draws the slider. Returns a response marked changed when the value
    /// changed.
    pub fn show(self, ui: &mut Ui) -> Response {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let (min, max) = (self.range.start().to_f64(), self.range.end().to_f64());
        // Never wider than the room there is, but never a stub either.
        let width = self.width.min(ui.available_width().max(80.0));
        let size = vec2(width, theme.control_height);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let knob_radius = 5.0;
        let rail = Rect::from_min_max(
            pos2(rect.left() + knob_radius + 3.0, rect.center().y - 1.0),
            pos2(rect.right() - knob_radius - 3.0, rect.center().y + 1.0),
        );
        let snap = |raw: f64| -> f64 {
            let stepped = match self.step {
                Some(step) => min + ((raw - min) / step).round() * step,
                None => raw,
            };
            stepped.clamp(min.min(max), max.max(min))
        };
        let old = self.value.to_f64();
        let mut new = old;
        if let Some(pointer) = response.interact_pointer_pos() {
            let t = ((pointer.x - rail.left()) / rail.width()).clamp(0.0, 1.0);
            new = snap(min + (max - min) * f64::from(t));
        }
        if response.has_focus() {
            let step = self.step.unwrap_or((max - min) / 100.0);
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::ArrowUp)
                {
                    new = snap(new + step);
                }
                if input.key_pressed(egui::Key::ArrowLeft)
                    || input.key_pressed(egui::Key::ArrowDown)
                {
                    new = snap(new - step);
                }
            });
        }
        if new != old {
            *self.value = N::from_f64(new);
            response.mark_changed();
        }
        response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), new, ""));
        if ui.is_rect_visible(rect) {
            let t = if max == min {
                0.0
            } else {
                ((self.value.to_f64() - min) / (max - min)).clamp(0.0, 1.0) as f32
            };
            let knob_x = egui::lerp(rail.left()..=rail.right(), t);
            ui.painter()
                .rect_filled(rail, CornerRadius::same(1), palette.card_stroke);
            let filled = Rect::from_min_max(rail.min, pos2(knob_x, rail.max.y));
            ui.painter()
                .rect_filled(filled, CornerRadius::same(1), palette.accent);
            let radius = if response.hovered() || response.dragged() {
                knob_radius + 1.0
            } else {
                knob_radius
            };
            // A small knob in the accent, ringed in the text color so it
            // reads on both themes.
            ui.painter().circle(
                pos2(knob_x, rect.center().y),
                radius - 1.0,
                palette.accent,
                Stroke::new(2.0, palette.strong_text),
            );
            if response.has_focus() {
                ui.painter().circle_stroke(
                    pos2(knob_x, rect.center().y),
                    radius + 3.0,
                    Stroke::new(1.0, palette.accent_stroke),
                );
            }
        }
        response
    }
}
