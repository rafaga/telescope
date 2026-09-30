# egui-panels

Building blocks for settings-style screens in [egui](https://github.com/emilk/egui):
the pieces that make a configuration window feel like one coherent tool
instead of a pile of unrelated rows.

It depends only on `egui` (no fonts, renderer or platform code), so it fits any
egui host: eframe, bevy_egui or a custom integration.

## Components

| Component | What it is |
|-----------|------------|
| `page`, `page_header`, `page_header_with` | A page column with a readable maximum width, its title, a one-line description and optional header actions. |
| `Section` | A titled card grouping related settings, optionally highlighted. |
| `form` / `Form` | Rows with an aligned label column; `row`, `row_hint` (tooltip on the label) and `note` (a line under the control). |
| `switch` | An animated on/off switch. |
| `Slider` | A horizontal slider in the theme's colors (accent fill, white knob), with steps and arrow keys. |
| `segmented` | Joined buttons picking one value. |
| `tile`, `tile_grid` | Toggle cards (check box, title, subtitle) in equal columns, for picking from a list. |
| `PathPicker` | A path field with Browse and optional Reset buttons; the host opens its own dialog. |
| `stepper` | Numbered steps joined by arrows, for screens that follow a flow. |
| `button` + `Variant` | Primary, Secondary, Ghost and Danger buttons of one height. |
| `status` + `StatusKind` | A feedback line with a painted icon: ok, warning, error, info. |
| `badge`, `chip` | Small pills: a tag, or a main text with a dimmed secondary one. |
| `EntityCard` + `Avatar` | A card for an account, device or any listed item: picture or initials, title, lines, actions. |
| `SideNav`, `NavGroup`, `NavItem` | Grouped page navigation with a dot on pages that have unsaved changes. |
| `ActionBar` + `Action` | Cancel / Apply / Accept, with the pending-changes message. |
| `SettingsLayout` | Navigation at the left, action bar at the bottom, the selected page scrollable in the rest (`scroll(false)` for a page that fills the area itself, such as a node editor). |
| `Draft` | The saved value and the copy being edited: `is_dirty`, `differs` (per page), `commit`, `revert`. |

## Theme

Every component reads its spacing, sizes and colors from `Theme`. Colors are
derived from the current `egui::Visuals` (`Palette::from_visuals`), so egui's
light and dark themes, or a custom one, work without changes. Override any
value for a context:

```rust
let theme = egui_panels::Theme {
    label_width: 220.0,
    ..egui_panels::Theme::default()
};
egui_panels::Theme::set(ctx, theme);
```

## A settings screen

```rust
use egui_panels::{Action, ActionBar, Draft, NavGroup, NavItem, Section, SettingsLayout, SideNav};

let groups = [NavGroup::new(
    "General",
    vec![NavItem::new(Page::Sound, "Sound").dirty(draft.differs(|s| s.volume))],
)];
let bar = ActionBar::new("Cancel", "Apply", "Accept")
    .pending(draft.is_dirty().then_some("Unsaved changes"))
    .saved_message("All saved");
let action = SettingsLayout::new("settings").show(
    ui,
    SideNav::new(&groups).title("Settings"),
    bar,
    &mut page,
    |ui, page| {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(ui, "Sound", Some("How alerts sound."));
            Section::new("Volume").show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    form.row("Enabled", |ui| egui_panels::switch(ui, &mut draft.current_mut().sound));
                });
            });
        });
    },
);
match action {
    Some(Action::Cancel) => draft.revert(),
    Some(Action::Apply | Action::Accept) => draft.commit(),
    None => {}
}
```

A complete window is in `examples/settings_demo.rs`:

```sh
cargo run -p egui-panels --example settings_demo
```

## Tests

`cargo test -p egui-panels` runs the components on a headless egui context and
clicks them with simulated pointer input.

## License

MIT
