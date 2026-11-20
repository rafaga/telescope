//! # egui-appmenu
//!
//! The application menu of an egui app: described once as a
//! [`native_tools::menu::MenuBar`], shown as the native menu bar on macOS and
//! as a bar at the top of the window on Windows and Linux.
//!
//! [`AppMenu::show`] is called once per frame, before the other panels, and
//! returns the ids of the items that were chosen, by mouse or by shortcut.
//!
//! ```no_run
//! use egui_appmenu::AppMenu;
//! use native_tools::menu::{Key, MenuBar, MenuItem, Shortcut, Submenu};
//!
//! # fn demo(ctx: &egui::Context, ui: &mut egui::Ui) {
//! let bar = MenuBar::new().menu(
//!     Submenu::new("File")
//!         .item(MenuItem::new("open", "Open…").shortcut(Shortcut::command(Key::Char('o')))),
//! );
//! let mut menu = AppMenu::new(ctx, bar);
//! for id in menu.show(ui) {
//!     if id == "open" { /* … */ }
//! }
//! # }
//! ```

use native_tools::menu::{Key, MenuBar, MenuEntry, NativeMenu, Shortcut, Submenu};

/// The application menu: a [`MenuBar`] and whoever shows it.
pub struct AppMenu {
    bar: MenuBar,
    /// The installed macOS menu bar; `None` when the menu is drawn in the window.
    native: Option<NativeMenu>,
    /// The bar changed since it was last copied to the native menu.
    dirty: bool,
}

impl AppMenu {
    /// Shows `bar` as the native menu bar when the system has one, and in the
    /// window otherwise. If the native menu cannot be installed the menu falls
    /// back to the window and the reason is logged.
    ///
    /// On macOS call it once the app has launched: from the creation closure
    /// of eframe, not before `run_native`.
    pub fn new(ctx: &egui::Context, bar: MenuBar) -> Self {
        if !NativeMenu::SUPPORTED {
            return Self::in_window(bar);
        }
        let repaint = ctx.clone();
        match NativeMenu::install(&bar, move || repaint.request_repaint()) {
            Ok(native) => Self {
                bar,
                native: Some(native),
                dirty: false,
            },
            Err(error) => {
                tracing::warn!(%error, "native menu bar not installed, drawing the menu in the window");
                Self::in_window(bar)
            }
        }
    }

    /// Draws `bar` inside the window on every system.
    pub fn in_window(bar: MenuBar) -> Self {
        Self {
            bar,
            native: None,
            dirty: false,
        }
    }

    /// Whether the menu is the system's menu bar and not part of the window.
    pub fn is_native(&self) -> bool {
        self.native.is_some()
    }

    pub fn bar(&self) -> &MenuBar {
        &self.bar
    }

    /// The bar, to enable items or move check marks. The change reaches the
    /// screen on the next [`Self::show`].
    pub fn bar_mut(&mut self) -> &mut MenuBar {
        self.dirty = true;
        &mut self.bar
    }

    /// Shows the menu and returns the ids of the items chosen since the last
    /// call. Call it at the start of the frame, before other panels (the
    /// in-window bar takes the top of the screen) and before widgets that
    /// would read the same shortcuts.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Vec<String> {
        if let Some(native) = &self.native {
            if std::mem::take(&mut self.dirty) {
                native.sync(&self.bar);
            }
            return native.drain();
        }

        let mut chosen = self.consume_shortcuts(ui);
        egui::Panel::top("egui_appmenu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for menu in &self.bar.menus {
                    ui.menu_button(&menu.label, |ui| {
                        Self::entries_ui(ui, menu, &mut chosen);
                    });
                }
            });
        });
        self.dirty = false;
        chosen
    }

    /// The ids of the enabled items whose shortcut was pressed this frame.
    fn consume_shortcuts(&self, ui: &egui::Ui) -> Vec<String> {
        let mut chosen = Vec::new();
        for item in self.bar.items() {
            let Some(shortcut) = item.shortcut.and_then(Self::keyboard_shortcut) else {
                continue;
            };
            if item.enabled && ui.input_mut(|input| input.consume_shortcut(&shortcut)) {
                chosen.push(item.id.clone());
            }
        }
        chosen
    }

    fn entries_ui(ui: &mut egui::Ui, menu: &Submenu, chosen: &mut Vec<String>) {
        for entry in &menu.entries {
            match entry {
                MenuEntry::Item(item) => {
                    let clicked = match item.checked {
                        Some(checked) => {
                            let mut value = checked;
                            ui.add_enabled(item.enabled, egui::Checkbox::new(&mut value, &item.label))
                                .changed()
                        }
                        None => {
                            let mut button = egui::Button::new(&item.label);
                            if let Some(shortcut) = item.shortcut.and_then(Self::keyboard_shortcut) {
                                button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
                            }
                            ui.add_enabled(item.enabled, button).clicked()
                        }
                    };
                    if clicked {
                        chosen.push(item.id.clone());
                        ui.close();
                    }
                }
                MenuEntry::Submenu(nested) => {
                    ui.menu_button(&nested.label, |ui| Self::entries_ui(ui, nested, chosen));
                }
                // Items the system implements exist on the macOS menu bar only.
                MenuEntry::Standard { .. } => {}
                MenuEntry::Separator => {
                    ui.separator();
                }
            }
        }
    }

    /// The egui shortcut of a menu shortcut; `None` for a key egui has no name for.
    fn keyboard_shortcut(shortcut: Shortcut) -> Option<egui::KeyboardShortcut> {
        let key = match shortcut.key {
            Key::Char(c) => egui::Key::from_name(&c.to_uppercase().to_string())?,
            Key::F(n) => egui::Key::from_name(&format!("F{n}"))?,
            Key::Enter => egui::Key::Enter,
            Key::Escape => egui::Key::Escape,
            Key::Tab => egui::Key::Tab,
            Key::Space => egui::Key::Space,
            Key::Backspace => egui::Key::Backspace,
            Key::Delete => egui::Key::Delete,
            Key::Left => egui::Key::ArrowLeft,
            Key::Right => egui::Key::ArrowRight,
            Key::Up => egui::Key::ArrowUp,
            Key::Down => egui::Key::ArrowDown,
        };
        let mut modifiers = egui::Modifiers::NONE;
        if shortcut.command {
            modifiers |= egui::Modifiers::COMMAND;
        }
        if shortcut.ctrl {
            modifiers |= egui::Modifiers::CTRL;
        }
        if shortcut.alt {
            modifiers |= egui::Modifiers::ALT;
        }
        if shortcut.shift {
            modifiers |= egui::Modifiers::SHIFT;
        }
        Some(egui::KeyboardShortcut::new(modifiers, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_tools::menu::MenuItem;

    fn bar() -> MenuBar {
        MenuBar::new().menu(
            Submenu::new("File")
                .item(MenuItem::new("open", "Open").shortcut(Shortcut::command(Key::Char('o'))))
                .item(
                    MenuItem::new("save", "Save")
                        .shortcut(Shortcut::command(Key::Char('s')))
                        .enabled(false),
                ),
        )
    }

    /// Runs one frame with `key` pressed together with the command key.
    fn frame_with(menu: &mut AppMenu, key: egui::Key) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.modifiers = egui::Modifiers::COMMAND;
        raw.events.push(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        let mut chosen = Vec::new();
        let _ = ctx.run_ui(raw, |ui| chosen = menu.show(ui));
        chosen
    }

    #[test]
    fn a_shortcut_chooses_its_item() {
        let mut menu = AppMenu::in_window(bar());
        assert_eq!(frame_with(&mut menu, egui::Key::O), ["open"]);
    }

    #[test]
    fn a_disabled_item_ignores_its_shortcut() {
        let mut menu = AppMenu::in_window(bar());
        assert!(frame_with(&mut menu, egui::Key::S).is_empty());
    }

    #[test]
    fn letters_and_function_keys_have_an_egui_key() {
        let letter = AppMenu::keyboard_shortcut(Shortcut::command(Key::Char('o'))).unwrap();
        assert_eq!(letter.logical_key, egui::Key::O);
        let function = AppMenu::keyboard_shortcut(Shortcut::plain(Key::F(5))).unwrap();
        assert_eq!(function.logical_key, egui::Key::F5);
    }

    #[test]
    fn the_bar_is_in_the_window_when_not_native() {
        let menu = AppMenu::in_window(bar());
        assert!(!menu.is_native());
    }
}
