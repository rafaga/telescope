//! A whole settings screen: navigation, page and action bar.

use crate::action_bar::{Action, ActionBar};
use crate::nav::SideNav;
use crate::theme::Theme;
use egui::{CentralPanel, Frame, Id, Margin, Panel, ScrollArea, Ui};

/// Puts a [`SideNav`] at the left, an [`ActionBar`] at the bottom and the
/// selected page, scrollable, in the rest of `ui`.
///
/// ```no_run
/// # use egui_panels::{ActionBar, NavGroup, NavItem, SettingsLayout, SideNav};
/// # fn demo(ui: &mut egui::Ui, page: &mut u8) {
/// let groups = [NavGroup::untitled(vec![NavItem::new(0, "General"), NavItem::new(1, "Sound")])];
/// let action = SettingsLayout::new("settings")
///     .show(
///         ui,
///         SideNav::new(&groups).title("Settings"),
///         ActionBar::new("Cancel", "Apply", "Accept"),
///         page,
///         |ui, page| {
///             ui.label(format!("page {page}"));
///         },
///     );
/// # }
/// ```
#[must_use = "call `show` to draw the screen"]
pub struct SettingsLayout {
    id: Id,
    scroll: bool,
}

impl SettingsLayout {
    /// A screen whose panels are identified by `id_salt`.
    pub fn new(id_salt: impl Into<Id>) -> Self {
        Self {
            id: id_salt.into(),
            scroll: true,
        }
    }

    /// Whether the page scrolls (the default). Turn it off for a page that
    /// fills the whole area itself, such as a node editor or a map.
    pub fn scroll(mut self, scroll: bool) -> Self {
        self.scroll = scroll;
        self
    }

    /// Draws the screen; `show_page` draws the page `selected` points at.
    /// Returns the action bar's button clicked, if any.
    pub fn show<T: PartialEq + Clone>(
        self,
        ui: &mut Ui,
        nav: SideNav<'_, T>,
        bar: ActionBar<'_>,
        selected: &mut T,
        show_page: impl FnOnce(&mut Ui, &T),
    ) -> Option<Action> {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let side_frame = Frame::new()
            .fill(palette.nav_fill)
            .inner_margin(Margin::symmetric(10, 16));
        let action = Panel::bottom(self.id.with("bar"))
            .frame(side_frame.inner_margin(Margin::symmetric(20, 6)))
            .show(ui, |ui| bar.show(ui))
            .inner;
        Panel::left(self.id.with("nav"))
            .resizable(false)
            .exact_size(theme.nav_width.min((ui.available_width() * 0.3).max(140.0)))
            .frame(side_frame)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt(self.id.with("nav_scroll"))
                    .show(ui, |ui| nav.show(ui, selected));
            });
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(theme.page_margin),
            )
            .show(ui, |ui| {
                if self.scroll {
                    ScrollArea::vertical()
                        .id_salt(self.id.with("page_scroll"))
                        .auto_shrink([false, false])
                        .show(ui, |ui| show_page(ui, selected));
                } else {
                    show_page(ui, selected);
                }
            });
        action
    }
}
