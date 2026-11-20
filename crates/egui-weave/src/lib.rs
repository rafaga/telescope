//! # egui-weave
//!
//! A canvas of nodes joined by edges for [`egui`], in egui's own look.
//!
//! The data ([`CanvasModel`]), the pan and zoom ([`CanvasView`]) and the
//! widget that shows them ([`Canvas`]), with [`History`] to undo and redo.
//! It depends on nothing but `egui`.

mod controls;
mod edge_style;
mod history;
mod layout;
mod model;
mod view;
mod widget;

pub use controls::{ControlId, ControlKind, ControlValue, NodeControl};
pub use edge_style::{EdgeProperties, LineStyle};
pub use history::History;
pub use layout::{CanvasStyle, ControlRow, NodeLayout};
pub use model::{
    CanvasError, CanvasEvent, CanvasModel, ConnectionRule, Edge, EdgeId, EdgeState, NoCycles,
    NodeDescriptor, NodeId, PortDescriptor, PortDirection, PortId, PortRef,
};
pub use view::CanvasView;
pub use widget::Canvas;
