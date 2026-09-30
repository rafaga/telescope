//! The side navigation of a settings screen.

use crate::theme::Theme;
use egui::{
    Align2, Color32, CornerRadius, FontId, Response, RichText, Sense, Ui, WidgetInfo, WidgetType,
    pos2, vec2,
};

/// One page in a [`SideNav`].
#[derive(Clone, Debug, PartialEq)]
pub struct NavItem<T> {
    /// The value [`SideNav::show`] selects.
    pub id: T,
    /// The label shown.
    pub label: String,
    /// An optional icon drawn before the label (any text the host's fonts
    /// can draw, such as an emoji).
    pub icon: Option<String>,
    /// Whether the page has changes not applied yet: a dot at the right end.
    pub dirty: bool,
}

impl<T> NavItem<T> {
    /// An entry selecting `id`, labelled `label`.
    pub fn new(id: T, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            icon: None,
            dirty: false,
        }
    }

    /// Draws `icon` before the label.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Marks the page as having changes not applied yet.
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }
}

/// A group of [`NavItem`]s under an optional caption.
#[derive(Clone, Debug, PartialEq)]
pub struct NavGroup<T> {
    /// The caption above the group (shown in small capitals), if any.
    pub title: Option<String>,
    /// The pages of the group.
    pub items: Vec<NavItem<T>>,
}

impl<T> NavGroup<T> {
    /// A group captioned `title`.
    pub fn new(title: impl Into<String>, items: Vec<NavItem<T>>) -> Self {
        Self {
            title: Some(title.into()),
            items,
        }
    }

    /// A group with no caption.
    pub fn untitled(items: Vec<NavItem<T>>) -> Self {
        Self { title: None, items }
    }
}

/// A vertical list of pages in captioned groups, the selected one filled with
/// the accent and a dot on those with unsaved changes.
#[must_use = "call `show` to draw the navigation"]
pub struct SideNav<'a, T> {
    title: Option<&'a str>,
    subtitle: Option<&'a str>,
    groups: &'a [NavGroup<T>],
}

impl<'a, T: PartialEq + Clone> SideNav<'a, T> {
    /// A navigation listing `groups`.
    pub fn new(groups: &'a [NavGroup<T>]) -> Self {
        Self {
            title: None,
            subtitle: None,
            groups,
        }
    }

    /// A heading above the groups (the screen's name).
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// A dimmed line under the heading (a version, an account).
    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    /// Whether any page is marked dirty.
    pub fn any_dirty(&self) -> bool {
        self.groups
            .iter()
            .flat_map(|group| &group.items)
            .any(|item| item.dirty)
    }

    /// Draws the navigation and updates `selected` when an entry is
    /// clicked. The returned response is marked changed when it did.
    pub fn show(self, ui: &mut Ui, selected: &mut T) -> Response {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let mut changed = false;
        let mut response = ui
            .vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if let Some(title) = self.title {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(title)
                            .size(theme.section_title_size + 3.0)
                            .color(palette.strong_text),
                    );
                    if let Some(subtitle) = self.subtitle {
                        ui.label(
                            RichText::new(subtitle)
                                .size(theme.small_size)
                                .color(palette.muted_text),
                        );
                    }
                    ui.add_space(8.0);
                }
                for group in self.groups {
                    if let Some(caption) = &group.title {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(caption.to_uppercase())
                                .size(theme.small_size - 1.5)
                                .color(palette.muted_text),
                        );
                        ui.add_space(2.0);
                    }
                    for item in &group.items {
                        let is_selected = *selected == item.id;
                        if nav_entry(ui, &theme, item, is_selected).clicked() && !is_selected {
                            *selected = item.id.clone();
                            changed = true;
                        }
                    }
                }
            })
            .response;
        if changed {
            response.mark_changed();
        }
        response
    }
}

fn nav_entry<T>(ui: &mut Ui, theme: &Theme, item: &NavItem<T>, selected: bool) -> Response {
    let palette = theme.palette(ui.visuals());
    let size = vec2(ui.available_width(), theme.nav_item_height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &item.label,
        )
    });
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            palette.accent
        } else if response.hovered() {
            palette.hover_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        let color = if selected {
            palette.on_accent
        } else {
            palette.text
        };
        let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Body].size);
        let mut x = rect.left() + 10.0;
        if let Some(icon) = &item.icon {
            ui.painter().text(
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                icon,
                font.clone(),
                color,
            );
            x += 24.0;
        }
        let right = rect.right() - if item.dirty { 22.0 } else { 8.0 };
        crate::widgets::paint_fitted(
            ui,
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            &item.label,
            font,
            color,
            right - x,
        );
        if item.dirty {
            ui.painter().circle_filled(
                pos2(rect.right() - 12.0, rect.center().y),
                3.0,
                palette.dirty,
            );
        }
    }
    response
}
