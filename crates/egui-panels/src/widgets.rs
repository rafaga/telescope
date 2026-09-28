//! Controls and feedback elements.

use crate::theme::{Theme, mix};
use egui::{
    Align, Align2, Button, Color32, CornerRadius, FontId, Frame, ImageSource, InnerResponse, Label,
    Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, Vec2, WidgetInfo,
    WidgetText, WidgetType, pos2, vec2,
};

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

/// A small neutral pill: a tag, a count, an output kind.
pub fn badge(ui: &mut Ui, text: &str) -> Response {
    chip(ui, text, None)
}

/// A pill with a main text and, dimmed after it, a secondary one:
/// "Kara Voss · H-5GUI".
pub fn chip(ui: &mut Ui, text: &str, secondary: Option<&str>) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    Frame::new()
        .fill(palette.badge_fill)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
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
    ui.add(widget.min_size(vec2(0.0, theme.control_height)))
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
        ui.painter().text(
            segment.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if selected {
                palette.on_accent
            } else {
                palette.text
            },
        );
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

/// A clickable card showing whether an item of a list is picked: a check box,
/// a title and an optional dimmed subtitle. Returns a response marked changed
/// when it was flipped.
pub fn tile(ui: &mut Ui, on: &mut bool, title: &str, subtitle: Option<&str>) -> Response {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let width = ui.available_width();
    let height = if subtitle.is_some() {
        theme.tile_min_height
    } else {
        theme.tile_min_height.min(theme.control_height + 6.0)
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, title));
    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same(theme.radius.min(4));
        let (fill, stroke) = if *on {
            (palette.card_fill_selected, palette.accent_stroke)
        } else if response.hovered() {
            (
                mix(palette.card_fill, palette.strong_text, 0.04),
                palette.card_stroke,
            )
        } else {
            (palette.card_fill, palette.card_stroke)
        };
        ui.painter().rect(
            rect,
            radius,
            fill,
            Stroke::new(1.0, stroke),
            StrokeKind::Inside,
        );
        let box_size = 15.0;
        let box_rect = Rect::from_center_size(
            pos2(rect.left() + 12.0 + box_size / 2.0, rect.center().y),
            Vec2::splat(box_size),
        );
        if *on {
            ui.painter()
                .rect_filled(box_rect, CornerRadius::same(3), palette.accent);
            let stroke = Stroke::new(2.0, palette.on_accent);
            let at = |x: f32, y: f32| {
                pos2(
                    box_rect.left() + box_rect.width() * x,
                    box_rect.top() + box_rect.height() * y,
                )
            };
            ui.painter()
                .line_segment([at(0.22, 0.52), at(0.43, 0.74)], stroke);
            ui.painter()
                .line_segment([at(0.43, 0.74), at(0.8, 0.3)], stroke);
        } else {
            ui.painter().rect_stroke(
                box_rect,
                CornerRadius::same(3),
                Stroke::new(1.2, palette.muted_text),
                StrokeKind::Inside,
            );
        }
        let text_left = box_rect.right() + 10.0;
        let text_color = if *on {
            palette.strong_text
        } else {
            palette.text
        };
        let title_font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Body].size);
        match subtitle {
            Some(subtitle) => {
                ui.painter().text(
                    pos2(text_left, rect.center().y - 1.0),
                    Align2::LEFT_BOTTOM,
                    title,
                    title_font,
                    text_color,
                );
                ui.painter().text(
                    pos2(text_left, rect.center().y + 2.0),
                    Align2::LEFT_TOP,
                    subtitle,
                    FontId::proportional(theme.small_size - 0.5),
                    palette.muted_text,
                );
            }
            None => {
                ui.painter().text(
                    pos2(text_left, rect.center().y),
                    Align2::LEFT_CENTER,
                    title,
                    title_font,
                    text_color,
                );
            }
        }
    }
    response
}

/// Lays out `count` items in `columns` equal columns, row by row, calling
/// `add_item` with each index. Meant for [`tile`]s.
pub fn tile_grid(
    ui: &mut Ui,
    columns: usize,
    count: usize,
    mut add_item: impl FnMut(&mut Ui, usize),
) {
    let columns = columns.max(1);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
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

/// Numbered steps joined by arrows ("1 · Sources → 2 · Rules → 3 ·
/// Alerts"), the `current` one highlighted. Returns the index of a step that
/// was clicked.
pub fn stepper(ui: &mut Ui, steps: &[&str], current: usize) -> Option<usize> {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (index, step) in steps.iter().enumerate() {
            if index > 0 {
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 12.0), Sense::hover());
                let y = rect.center().y;
                let stroke = Stroke::new(1.4, palette.muted_text);
                ui.painter().line_segment(
                    [pos2(rect.left() + 2.0, y), pos2(rect.right() - 2.0, y)],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        pos2(rect.right() - 6.0, y - 4.0),
                        pos2(rect.right() - 2.0, y),
                    ],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        pos2(rect.right() - 6.0, y + 4.0),
                        pos2(rect.right() - 2.0, y),
                    ],
                    stroke,
                );
            }
            let selected = index == current;
            let text = RichText::new(format!("{} · {step}", index + 1))
                .size(theme.small_size)
                .color(if selected {
                    palette.on_accent
                } else {
                    palette.text
                });
            let pill = Button::new(text)
                .corner_radius(CornerRadius::same(12))
                .fill(if selected {
                    palette.accent
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(Stroke::new(1.0, palette.card_stroke))
                .selected(selected);
            if ui.add(pill).clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
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
    /// The response senses clicks on the whole card.
    pub fn show<R>(self, ui: &mut Ui, actions: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let stroke = if self.selected {
            palette.accent_stroke
        } else {
            palette.card_stroke
        };
        let fill = if self.selected {
            palette.card_fill_selected
        } else {
            palette.card_fill
        };
        let inner = Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(theme.radius))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let size = Vec2::splat(self.avatar_size);
                    match self.avatar {
                        Avatar::Image(source) => {
                            ui.add(egui::Image::new(source).fit_to_exact_size(size));
                        }
                        Avatar::Initials(initials) => {
                            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(4),
                                mix(palette.card_fill, palette.accent, 0.55),
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
                                palette.text
                            } else {
                                palette.muted_text
                            };
                            ui.label(line.color(color));
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), actions)
                        .inner
                })
                .inner
            });
        let response = inner.response.interact(Sense::click());
        InnerResponse::new(inner.inner, response)
    }
}

/// A horizontal slider drawn with the theme: a rail filled with the accent up
/// to a white knob. Drag or click to set it; with focus, the arrow keys move
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
            width: 240.0,
        }
    }

    /// Snaps the value to multiples of `step` (from the start of the range).
    pub fn step(mut self, step: f64) -> Self {
        self.step = (step > 0.0).then_some(step);
        self
    }

    /// Width of the rail, in points (default 240).
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
        let size = vec2(self.width, theme.control_height);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let knob_radius = (theme.control_height * 0.26).round();
        let rail = Rect::from_min_max(
            pos2(rect.left() + knob_radius, rect.center().y - 2.0),
            pos2(rect.right() - knob_radius, rect.center().y + 2.0),
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
            let off = ui.visuals().widgets.inactive.bg_fill;
            ui.painter().rect_filled(rail, CornerRadius::same(2), off);
            let filled = Rect::from_min_max(rail.min, pos2(knob_x, rail.max.y));
            ui.painter()
                .rect_filled(filled, CornerRadius::same(2), palette.accent);
            let border = if ui.visuals().dark_mode {
                Stroke::NONE
            } else {
                Stroke::new(1.0, palette.muted_text)
            };
            let radius = if response.hovered() || response.dragged() {
                knob_radius + 1.0
            } else {
                knob_radius
            };
            ui.painter().circle(
                pos2(knob_x, rect.center().y),
                radius,
                Color32::WHITE,
                border,
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
