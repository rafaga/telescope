# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

* `Canvas`, a surface of nodes joined by edges: `CanvasModel` (nodes with input and output ports, edges between ports with a state, a selection, rules that an edge has to follow such as `NoCycles`, and events for what changes), `CanvasView` (pan, zoom, fit and the conversion between screen and canvas) and `History` (undo and redo, where a drag is a single step). The user pans, zooms, selects (by click or with a rectangle), moves nodes, joins ports (the ports that take the edge are ringed and the reason is shown when they do not) and removes edges and nodes; a minimap (`Canvas::minimap`) that moves the view; an edge let go over nothing is told by `CanvasEvent::ConnectionDropped`, to offer a node that fits; the nodes and ports have accessible names.
* `Canvas::zoom_slider`, a vertical slider of the zoom over the canvas (top right, with the percentage under it), and `Canvas::pan_limit` (`CanvasView::limit_to`): the user can scroll past what is drawn by half of what is in view, no further.
* `EdgeProperties` (color, `LineStyle` solid, dashed or dotted, and width) on every edge, with `CanvasModel::set_edge_properties` and the event `CanvasEvent::EdgeChanged`, which `History` undoes. A right click on an edge opens a popup with the widgets of `egui` to set them, or to remove the edge.
* Controls in the body of a node, with the widgets of `egui`: `NodeControl` as a button, a check box, a switch (the one that `egui` lacks) or a group of radio buttons. Their value lives in the node (`CanvasModel::set_control`), `CanvasEvent::ControlChanged` tells a change and `History` undoes it, and `CanvasEvent::ButtonPressed` tells a press.
* Split out of `egui-ide` into its own crate: it depends on nothing but `egui`.
