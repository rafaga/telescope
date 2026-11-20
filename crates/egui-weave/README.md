# egui-weave

A canvas of nodes joined by edges for [egui](https://github.com/emilk/egui), in
egui's own look: it draws with the current `Visuals` and depends on nothing but
`egui`.

| Type | What it is |
|------|------------|
| `CanvasModel` | The nodes with their input and output ports, the edges between ports (true, false, absent or neutral), the selection, the rules an edge has to follow (such as `NoCycles`) and the events for what changes. |
| `Canvas` | The widget: pan, zoom, select (click or rectangle), move, join ports, remove edges and nodes, a minimap, a preview of a node dragged from a toolbox and a menu for an edge let go over nothing. |
| `CanvasView` | The pan and zoom, kept by the application so a toolbar can drive them. |
| `History` | Undo and redo, where a drag is a single step. |

## Try it

The `gallery` example of `egui-ide` shows the canvas next to the panels:

```text
cargo run -p egui-ide --example gallery
```
