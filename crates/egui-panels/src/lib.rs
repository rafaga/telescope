//! # egui-panels
//!
//! Building blocks for settings-style screens in [`egui`]: the pieces that
//! make a configuration window feel like one coherent tool instead of a pile
//! of unrelated rows.
//!
//! * Layout: [`page`], [`page_header`], [`Section`] (a heading with a hairline
//!   between sections, no card), [`divider`] and [`form`] (rows with an
//!   aligned label column).
//! * Controls: [`switch`], [`Slider`], [`segmented`], [`tile`] / [`tile_grid`]
//!   (check box rows for picking from a list), [`toggle_button`] (a button
//!   that stays pressed), [`PathPicker`], [`stepper`], and
//!   [`button`] / [`menu_button`] with a few [`Variant`]s.
//! * Feedback: [`status`] lines, [`progress_steps`], [`badge`] / [`chip`] tags
//!   and the [`EntityCard`] used for accounts, devices or any listed item.
//! * Screen: [`SideNav`] (grouped pages with an unsaved-change dot),
//!   [`ActionBar`] (Cancel / Apply / Accept) and [`SettingsLayout`], which puts
//!   both around the selected page; [`dialog_frame`] for floating windows
//!   drawn the same way.
//! * State: [`Draft`], a saved value and the copy being edited, to know what
//!   changed and to commit or revert it.
//!
//! Every component reads its spacing, sizes and colors from [`Theme`]: colors
//! are derived from the current [`egui::Visuals`], so light and dark themes
//! both work, and a host can override any value with [`Theme::set`].
//!
//! The look is egui's own, not a foreign design language: flat panels, one
//! pixel strokes, small corner radii, the accent only where egui uses its
//! selection color, and a shadow only on floating windows.
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
pub use layout::{Form, Section, dialog_frame, divider, form, page, page_header, page_header_with};
pub use nav::{NavGroup, NavItem, SideNav};
pub use shell::SettingsLayout;
pub use theme::{Palette, Theme};
pub use widgets::{
    Avatar, EntityCard, PathPicker, PathPickerResponse, Slider, StatusKind, Variant, badge, badges,
    button, chip, menu_button, notice, progress_steps, progress_steps_with_notes, segmented,
    status, stepper, switch, tile, tile_grid, toggle_button,
};
