# egui-appmenu

The application menu of an [egui](https://github.com/emilk/egui) app, described
once and shown the way each system expects:

* **macOS**: the native menu bar at the top of the screen (an `NSMenu`, built by
  [`native_tools::menu`](../native_tools)). Shortcuts are handled by the system.
* **Windows and Linux**: a menu bar drawn at the top of the window with
  `egui::MenuBar`. Shortcuts are read from egui's input.

The app builds a `native_tools::menu::MenuBar` and gets back, every frame, the
ids of the items that were chosen, whichever way they were chosen.

```rust,ignore
use egui_appmenu::AppMenu;
use native_tools::menu::{Key, MenuBar, MenuItem, Shortcut, Submenu};

// In the eframe creation closure (macOS needs the app to have launched):
let bar = MenuBar::new().menu(
    Submenu::new("File")
        .item(MenuItem::new("open", "Open…").shortcut(Shortcut::command(Key::Char('o'))))
        .separator()
        .item(MenuItem::new("quit", "Quit").shortcut(Shortcut::command(Key::Char('q')))),
);
let menu = AppMenu::new(&cc.egui_ctx, bar);

// In `App::ui`, before the other panels:
for id in self.menu.show(ui) {
    match id.as_str() {
        "open" => self.open(),
        "quit" => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
        _ => {}
    }
}

// To enable an item or move a check mark:
self.menu.bar_mut().set_enabled("open", false);
```

Do not add Copy / Paste / Undo items bound to `Cmd+C`, `Cmd+V`, `Cmd+Z`: with a
native menu the system takes those keys before egui sees them, and the text
fields stop copying.
