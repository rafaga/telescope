//! # egui-panels
//!
//! Building blocks for settings-style screens in [`egui`]: the pieces that
//! make a configuration window feel like one coherent tool instead of a pile
//! of unrelated rows.
//!
//! * Layout: [`page`], [`page_header`], [`Section`] (a titled card) and
//!   [`form`] (rows with an aligned label column).
//! * Controls: [`switch`], [`Slider`], [`segmented`], [`tile`] / [`tile_grid`] (toggle
//!   cards for picking from a list), [`PathPicker`], [`stepper`], and
//!   [`button`] with a few [`Variant`]s.
//! * Feedback: [`status`] lines, [`badge`] / [`chip`] pills and the
//!   [`EntityCard`] used for accounts, devices or any listed item.
//! * Screen: [`SideNav`] (grouped pages with an unsaved-change dot),
//!   [`ActionBar`] (Cancel / Apply / Accept) and [`SettingsLayout`], which puts
//!   both around the selected page.
//! * State: [`Draft`], a saved value and the copy being edited, to know what
//!   changed and to commit or revert it.
//!
//! Every component reads its spacing, sizes and colors from [`Theme`]: colors
//! are derived from the current [`egui::Visuals`], so light and dark themes
//! both work, and a host can override any value with [`Theme::set`].
//!
//! The crate draws everything with egui primitives and depends on nothing
//! else; the host provides fonts, file dialogs and persistence.
//!
//! ```no_run
//! # fn demo(ui: &mut egui::Ui, enabled: &mut bool, level: &mut u8) {
//! egui_panels::page(ui, |ui| {
//!     egui_panels::page_header(ui, "Alerts", Some("What happens when a rule fires."));
//!     egui_panels::Section::new("Sound").show(ui, |ui| {
//!         egui_panels::form(ui, |form| {
//!             form.row("Enabled", |ui| egui_panels::switch(ui, enabled));
//!             form.row("Level", |ui| {
//!                 egui_panels::segmented(ui, level, &[(1, "Low"), (2, "Mid"), (3, "High")])
//!             });
//!         });
//!     });
//! });
//! # }
//! ```

mod action_bar;
mod draft;
mod layout;
mod nav;
mod shell;
mod theme;
mod widgets;

pub use action_bar::{Action, ActionBar};
pub use draft::Draft;
pub use layout::{Form, Section, form, page, page_header, page_header_with};
pub use nav::{NavGroup, NavItem, SideNav};
pub use shell::SettingsLayout;
pub use theme::{Palette, Theme};
pub use widgets::{
    Avatar, EntityCard, PathPicker, PathPickerResponse, Slider, StatusKind, Variant, badge, button,
    chip, segmented, status, stepper, switch, tile, tile_grid,
};
