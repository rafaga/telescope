//! Page structure: the page column, its header, sections and forms.

use crate::theme::Theme;
use egui::{
    Align, CornerRadius, FontId, Frame, InnerResponse, Label, Layout, Response, RichText, Stroke,
    Ui, WidgetText, vec2,
};

/// Lays out a settings page: a column no wider than
/// [`Theme::page_max_width`], its blocks [`Theme::page_spacing`] apart.
pub fn page<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let theme = Theme::get(ui.ctx());
    let width = ui.available_width().min(theme.page_max_width);
    ui.allocate_ui_with_layout(
        vec2(width, 0.0),
        Layout::top_down_justified(Align::Min),
        |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = theme.page_spacing;
            add_contents(ui)
        },
    )
    .inner
}

/// The title of a page and, optionally, one line saying what it is for.
pub fn page_header(ui: &mut Ui, title: &str, description: Option<&str>) -> Response {
    page_header_with(ui, title, description, |_| {}).response
}

/// Like [`page_header`], with `actions` (usually a primary button) laid out
/// at the right end of the title row.
pub fn page_header_with<R>(
    ui: &mut Ui,
    title: &str,
    description: Option<&str>,
    actions: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let theme = Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let inner = ui
            .horizontal(|ui| {
                ui.add(Label::new(
                    RichText::new(title)
                        .font(FontId::proportional(theme.title_size))
                        .color(palette.strong_text),
                ));
                ui.with_layout(Layout::right_to_left(Align::Center), actions)
                    .inner
            })
            .inner;
        if let Some(description) = description {
            ui.label(RichText::new(description).color(palette.muted_text));
        }
        inner
    })
}

/// A titled card grouping related settings.
///
/// ```no_run
/// # fn demo(ui: &mut egui::Ui) {
/// egui_panels::Section::new("Chat log folder")
///     .description("Telescope reads only the new lines.")
///     .show(ui, |ui| {
///         ui.label("…");
///     });
/// # }
/// ```
#[must_use = "call `show` to draw the section"]
pub struct Section<'a> {
    title: &'a str,
    description: Option<&'a str>,
    highlighted: bool,
}

impl<'a> Section<'a> {
    /// A section titled `title`.
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            description: None,
            highlighted: false,
        }
    }

    /// One line under the title explaining the section.
    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }

    /// Draws the border in the accent color (the section is selected or is
    /// the one to look at).
    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    /// Draws the card with `add_contents` under its title.
    pub fn show<R>(self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let stroke = if self.highlighted {
            palette.accent_stroke
        } else {
            palette.card_stroke
        };
        Frame::new()
            .fill(palette.card_fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(CornerRadius::same(theme.radius))
            .inner_margin(theme.section_margin)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = theme.section_spacing;
                if !self.title.is_empty() || self.description.is_some() {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        if !self.title.is_empty() {
                            ui.label(
                                RichText::new(self.title)
                                    .font(FontId::proportional(theme.section_title_size))
                                    .color(palette.strong_text),
                            );
                        }
                        if let Some(description) = self.description {
                            ui.label(
                                RichText::new(description)
                                    .size(theme.small_size)
                                    .color(palette.muted_text),
                            );
                        }
                    });
                }
                add_contents(ui)
            })
    }
}

/// Rows of settings with their labels in one aligned column (see [`Form`]).
pub fn form<R>(ui: &mut Ui, add_rows: impl FnOnce(&mut Form<'_>) -> R) -> R {
    let theme = Theme::get(ui.ctx());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = theme.row_spacing.y;
        let mut form = Form { ui, theme };
        add_rows(&mut form)
    })
    .inner
}

/// The rows of a [`form`]: each has a label in a column of
/// [`Theme::label_width`] and its control in the remaining width, vertically
/// centered on a line of [`Theme::control_height`].
pub struct Form<'a> {
    ui: &'a mut Ui,
    theme: Theme,
}

impl Form<'_> {
    /// A row labelled `label` holding what `add_control` draws.
    pub fn row<R>(
        &mut self,
        label: impl Into<WidgetText>,
        add_control: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.row_with(Some(label.into()), None, add_control)
    }

    /// Like [`Self::row`], with `hint` shown when the label is hovered.
    pub fn row_hint<R>(
        &mut self,
        label: impl Into<WidgetText>,
        hint: &str,
        add_control: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.row_with(Some(label.into()), Some(hint), add_control)
    }

    /// A row with an empty label cell: a status line, a note or extra
    /// buttons under the control above.
    pub fn note<R>(&mut self, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
        self.row_with(None, None, add_contents)
    }

    fn row_with<R>(
        &mut self,
        label: Option<WidgetText>,
        hint: Option<&str>,
        add_control: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        let theme = &self.theme;
        self.ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = theme.row_spacing.x;
                ui.allocate_ui_with_layout(
                    vec2(theme.label_width, theme.control_height),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_width(theme.label_width);
                        if let Some(label) = label {
                            let response = ui.add(Label::new(label).wrap());
                            if let Some(hint) = hint {
                                response.on_hover_text(hint);
                            }
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), theme.control_height),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        add_control(ui)
                    },
                )
                .inner
            })
            .inner
    }

    /// The `Ui` the rows are added to, for anything that is not a row.
    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }
}
