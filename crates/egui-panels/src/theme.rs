//! Spacing, sizes and colors shared by every component.

use egui::{Color32, Context, Id, Margin, Vec2, Visuals};

/// Sizes and spacing of the components, and optionally their colors.
///
/// Read it with [`Theme::get`] (the defaults unless a host stored its own with
/// [`Theme::set`]). Colors come from [`Theme::palette`]: derived from the
/// current [`Visuals`] unless [`Theme::colors`] fixes them.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Widest a [`crate::page`] grows. The default is unbounded: the page
    /// fills the width the window leaves free. Set a finite value to keep
    /// lines readable on very wide screens.
    pub page_max_width: f32,
    /// Vertical gap between the blocks of a page (header, sections).
    pub page_spacing: f32,
    /// Margin around the page inside [`crate::SettingsLayout`].
    pub page_margin: Margin,
    /// Padding inside framed elements: cards, dialogs and the rule cards of a
    /// host.
    pub section_margin: Margin,
    /// Corner radius of cards, tags and controls.
    pub radius: u8,
    /// Vertical gap between a section's heading and its contents, and between
    /// the contents.
    pub section_spacing: f32,
    /// Width of the label column of a [`crate::form`].
    pub label_width: f32,
    /// Gap between form columns (x) and rows (y).
    pub row_spacing: Vec2,
    /// Height of buttons, text fields and segmented controls.
    pub control_height: f32,
    /// Height of a [`crate::tile`] row.
    pub tile_min_height: f32,
    /// Font size of a page title.
    pub title_size: f32,
    /// Font size of a section title.
    pub section_title_size: f32,
    /// Font size of descriptions, hints and status lines.
    pub small_size: f32,
    /// Width of the [`crate::SideNav`] column in a [`crate::SettingsLayout`].
    pub nav_width: f32,
    /// Height of a [`crate::SideNav`] entry.
    pub nav_item_height: f32,
    /// Fixed colors, or `None` to derive them from the current visuals.
    pub colors: Option<Palette>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            page_max_width: f32::INFINITY,
            page_spacing: 20.0,
            page_margin: Margin::symmetric(36, 26),
            section_margin: Margin::symmetric(16, 12),
            radius: 4,
            section_spacing: 12.0,
            label_width: 168.0,
            row_spacing: Vec2::new(14.0, 10.0),
            control_height: 28.0,
            tile_min_height: 34.0,
            title_size: 20.0,
            section_title_size: 13.5,
            small_size: 12.5,
            nav_width: 224.0,
            nav_item_height: 30.0,
            colors: None,
        }
    }
}

impl Theme {
    fn id() -> Id {
        Id::new("egui_panels::theme")
    }

    /// The theme stored for `ctx`, or the default one.
    pub fn get(ctx: &Context) -> Self {
        ctx.data(|data| data.get_temp::<Self>(Self::id()))
            .unwrap_or_default()
    }

    /// Stores `theme` for every component drawn with `ctx`.
    pub fn set(ctx: &Context, theme: Self) {
        ctx.data_mut(|data| data.insert_temp(Self::id(), theme));
    }

    /// The colors to draw with: [`Self::colors`], or derived from `visuals`.
    pub fn palette(&self, visuals: &Visuals) -> Palette {
        self.colors
            .clone()
            .unwrap_or_else(|| Palette::from_visuals(visuals))
    }
}

/// The colors the components use.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// Fill of selected and primary elements.
    pub accent: Color32,
    /// Border of selected elements.
    pub accent_stroke: Color32,
    /// Text drawn on [`Self::accent`].
    pub on_accent: Color32,
    /// Fill of cards: transparent, so they take the page background like
    /// egui's own frames.
    pub card_fill: Color32,
    /// Fill of a selected card: the accent, faint.
    pub card_fill_selected: Color32,
    /// Border of cards, controls and tags.
    pub card_stroke: Color32,
    /// The hairline between sections and around the screen's panels: fainter
    /// than [`Self::card_stroke`].
    pub separator: Color32,
    /// Wash drawn behind a row under the pointer.
    pub hover_fill: Color32,
    /// Fill of the side navigation column.
    pub nav_fill: Color32,
    /// Regular text.
    pub text: Color32,
    /// Titles and emphasized text.
    pub strong_text: Color32,
    /// Descriptions, hints and secondary text.
    pub muted_text: Color32,
    /// Fill of badges and chips (transparent by default: a tag is a bordered
    /// label).
    pub badge_fill: Color32,
    /// Something is fine ([`crate::StatusKind::Ok`]).
    pub ok: Color32,
    /// Something needs attention ([`crate::StatusKind::Warning`]).
    pub warning: Color32,
    /// Something failed ([`crate::StatusKind::Error`]), destructive actions.
    pub error: Color32,
    /// The unsaved-change dot and message.
    pub dirty: Color32,
}

impl Palette {
    /// Colors that match `visuals` (egui's dark or light theme, or any custom
    /// one).
    pub fn from_visuals(visuals: &Visuals) -> Self {
        let panel = visuals.panel_fill;
        let strong = visuals.strong_text_color();
        let accent = visuals.selection.bg_fill;
        Self {
            accent,
            accent_stroke: mix(accent, strong, 0.25),
            on_accent: if visuals.dark_mode {
                Color32::WHITE
            } else {
                strong
            },
            card_fill: Color32::TRANSPARENT,
            card_fill_selected: mix(panel, accent, 0.22),
            card_stroke: visuals.widgets.noninteractive.bg_stroke.color,
            separator: mix(panel, visuals.widgets.noninteractive.bg_stroke.color, 0.5),
            hover_fill: Color32::from_rgba_unmultiplied(
                strong.r(),
                strong.g(),
                strong.b(),
                if visuals.dark_mode { 13 } else { 9 },
            ),
            nav_fill: panel,
            text: visuals.text_color(),
            strong_text: strong,
            muted_text: visuals.weak_text_color(),
            badge_fill: Color32::TRANSPARENT,
            ok: if visuals.dark_mode {
                Color32::from_rgb(127, 196, 127)
            } else {
                Color32::from_rgb(30, 125, 55)
            },
            warning: visuals.warn_fg_color,
            error: visuals.error_fg_color,
            dirty: visuals.warn_fg_color,
        }
    }
}

/// `a` moved towards `b` by `t` (0 = `a`, 1 = `b`), channel by channel.
pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
        channel(a.a(), b.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_theme_is_read_back() {
        let ctx = Context::default();
        assert_eq!(Theme::get(&ctx), Theme::default());
        let theme = Theme {
            label_width: 240.0,
            ..Theme::default()
        };
        Theme::set(&ctx, theme.clone());
        assert_eq!(Theme::get(&ctx), theme);
    }

    #[test]
    fn fixed_colors_win_over_the_visuals() {
        let palette = Palette::from_visuals(&Visuals::light());
        let theme = Theme {
            colors: Some(palette.clone()),
            ..Theme::default()
        };
        assert_eq!(theme.palette(&Visuals::dark()), palette);
        assert_ne!(Theme::default().palette(&Visuals::dark()), palette);
    }

    #[test]
    fn mixing_goes_from_one_color_to_the_other() {
        let black = Color32::BLACK;
        let white = Color32::WHITE;
        assert_eq!(mix(black, white, 0.0), black);
        assert_eq!(mix(black, white, 1.0), white);
        assert_eq!(mix(black, white, 0.5), Color32::from_gray(128));
    }
}
