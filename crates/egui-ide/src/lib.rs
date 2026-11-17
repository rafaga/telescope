//! # egui-ide
//!
//! Panels for editors built with [`egui`], the kind an IDE has around its
//! canvas: a property grid, a toolbox and a toolbar.
//!
//! * [`PropertyGrid`]: groups that fold away, each one a grid of
//!   `name | value` rows, for the properties of what is selected.
//! * [`PropertyModel`]: the properties a grid shows, set up and read through
//!   an API, with [`PropertyGrid::show_model`] as its editor.
//! * [`Toolbox`]: groups of entries that are added to the canvas with a click
//!   or by dragging them onto it.
//! * [`Toolbar`]: a row of buttons, toggles and separators, or a panel of
//!   groups with compact icon buttons ([`ToolbarIcon`]), dropdowns and split
//!   buttons. [`ToolbarPanel`] lets the user drag the groups around by their
//!   grip.
//!
//! The look is egui's own: the crate only composes [`egui::CollapsingHeader`],
//! [`egui::Grid`], [`egui::Button`] and the style of the current
//! [`egui::Visuals`], with no theme, palette or colors of its own, so it
//! follows whatever the host sets. It depends on nothing but `egui`.
//!
//! ```no_run
//! # fn demo(ui: &mut egui::Ui, enabled: &mut bool) {
//! use egui_ide::{PropertyGrid, Toolbox, ToolboxEntry};
//!
//! let picked = Toolbox::new("toolbox")
//!     .group("Shapes", [ToolboxEntry::new(1_u8, "Circle"), ToolboxEntry::new(2, "Square")])
//!     .show(ui);
//! let changed = PropertyGrid::new("properties").show(ui, |grid| {
//!     grid.group("General", |rows| {
//!         rows.row("Enabled", |ui| ui.checkbox(enabled, "").changed());
//!     });
//! });
//! # let _ = (picked, changed);
//! # }
//! ```

mod editable;
mod model;
mod model_grid;
mod property_grid;
mod toolbar;
mod toolbar_icon;
mod toolbar_panel;
mod toolbox;

pub use editable::{Editable, PropertyChoice};
pub use model::{
    PropertyDescriptor, PropertyError, PropertyGroup, PropertyId, PropertyIssue, PropertyKind,
    PropertyLabels, PropertyModel, PropertyValue, Severity,
};
pub use property_grid::{Properties, PropertyGrid, PropertyRows};
pub use toolbar::{Toolbar, ToolbarUi};
pub use toolbar_icon::ToolbarIcon;
pub use toolbar_panel::ToolbarPanel;
pub use toolbox::{Toolbox, ToolboxEntry};
