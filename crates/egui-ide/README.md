# egui-ide

Panels for editors built with [egui](https://github.com/emilk/egui), in egui's
own look: only `CollapsingHeader`, `Grid`, `Button` and the current `Visuals`,
with no theme or colors of its own. It depends on nothing but `egui`, so it can
leave this workspace at any time.

| Component | What it is |
|-----------|------------|
| `PropertyGrid` | Groups under a band that folds them away, each a bordered table of name and value rows; reports whether a value changed. |
| `PropertyModel` | The properties a grid shows, set up and read through an API (`group`, `property`, `get`, `set`), each with a kind, default value, description and optional rule; `PropertyGrid::show_model` edits it and returns what changed. |
| `Toolbox` | Titled groups of full-width button entries (and an optional header) that are added with a click or dragged onto the canvas (`Toolbox::dropped`, `Toolbox::hovered`). |
| `Toolbar` | Buttons, toggles and separators in a row, or a panel of groups with icon buttons, icon and text buttons, dropdowns, split buttons and separators. |
| `ToolbarPanel` | Groups of toolbars that the user moves by dragging their grip (a placeholder shows the room it takes; groups that do not fit wrap to another row); the order can be saved with `ToolbarPanel::order` and put back with `set_order`. |

Property kinds: text, multiline, file and folder (a button opens the native dialog, feature `native-dialogs`), date (editable, with a calendar), read-only, integer, decimal, boolean
(a `True`/`False` dropdown), choice, flags, color (editable hex), list of texts, key/value
list, reference, link and action (the last three are reported by
`PropertyModel::take_activated`).

## Try it

```text
cargo run -p egui-ide --example gallery
```

A window with every component: drag a toolbar group by its grip, click or drag
the entries of the toolbox onto the canvas and edit a property of each kind;
what the components report goes to the log at the bottom.

