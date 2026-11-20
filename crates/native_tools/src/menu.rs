//! The application menu, described once and built natively where the system
//! has one.
//!
//! This module is the description of a menu bar ([`MenuBar`]: menus, items,
//! shortcuts, check marks) plus [`NativeMenu`], which turns it into the real
//! thing on macOS: an `NSMenu` installed as the application's main menu (the
//! bar at the top of the screen), built straight on AppKit with `objc2`.
//!
//! On Windows and Linux the menu lives inside the window and is drawn by the
//! GUI toolkit, so [`NativeMenu::install`] fails there with a message and
//! [`NativeMenu::SUPPORTED`] is `false`. A GUI crate (see `egui-appmenu`) draws
//! the same [`MenuBar`] itself, so the application describes its menu once and
//! reacts to one kind of event: the `id` of the item that was chosen.
//!
//! ```
//! use native_tools::menu::{Key, MenuBar, MenuItem, Shortcut, Submenu};
//!
//! let bar = MenuBar::new()
//!     .menu(Submenu::new("File")
//!         .item(MenuItem::new("open", "Open…").shortcut(Shortcut::command(Key::Char('o'))))
//!         .separator()
//!         .item(MenuItem::new("quit", "Quit").shortcut(Shortcut::command(Key::Char('q')))));
//! assert!(bar.duplicate_ids().is_empty());
//! ```

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeMenu;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub use unsupported::NativeMenu;

/// A key of a [`Shortcut`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A letter, a digit or a punctuation mark, as printed on the key
    /// (case does not matter: `Shift` is a modifier of the shortcut).
    Char(char),
    /// A function key, `F(1)` to `F(12)`.
    F(u8),
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
}

/// A key combination that chooses a menu item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: Key,
    /// The platform's command key: `Cmd` on macOS, `Ctrl` elsewhere.
    pub command: bool,
    /// The `Control` key itself (a different key from `Cmd` on macOS).
    pub ctrl: bool,
    /// `Option` on macOS, `Alt` elsewhere.
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    /// The key alone, with no modifier.
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            command: false,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// The platform's command key plus `key` (`Cmd+O` on macOS, `Ctrl+O` elsewhere).
    pub fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::plain(key)
        }
    }

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }

    pub fn with_alt(mut self) -> Self {
        self.alt = true;
        self
    }

    pub fn with_ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }
}

/// An item that does something when it is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// What the application receives when the item is chosen. Unique in a
    /// [`MenuBar`].
    pub id: String,
    pub label: String,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    /// `Some` makes the item a check item, and says whether it is checked.
    pub checked: Option<bool>,
}

impl MenuItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }

    pub fn shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Makes this a check item with its current state.
    pub fn checkable(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
}

/// The items the system itself knows how to do. They exist on macOS only; the
/// in-window menus of the other systems skip them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    /// The system's About panel (name and icon of the app bundle).
    About,
    /// The "Services" submenu the system fills in.
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
}

/// A menu the system treats in a special way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The system lists the open windows in it.
    Windows,
    /// The system adds its search field to it.
    Help,
}

/// One line of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Submenu(Submenu),
    /// An item the system implements (macOS only), with the label to show.
    Standard { kind: Standard, label: String },
    Separator,
}

/// A menu: a top level one of the bar, or one nested in another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submenu {
    pub label: String,
    pub role: Option<Role>,
    pub entries: Vec<MenuEntry>,
}

impl Submenu {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            role: None,
            entries: Vec::new(),
        }
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(MenuEntry::Item(item));
        self
    }

    pub fn submenu(mut self, submenu: Submenu) -> Self {
        self.entries.push(MenuEntry::Submenu(submenu));
        self
    }

    pub fn standard(mut self, kind: Standard, label: impl Into<String>) -> Self {
        self.entries.push(MenuEntry::Standard {
            kind,
            label: label.into(),
        });
        self
    }

    pub fn separator(mut self) -> Self {
        self.entries.push(MenuEntry::Separator);
        self
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a MenuItem>) {
        for entry in &self.entries {
            match entry {
                MenuEntry::Item(item) => out.push(item),
                MenuEntry::Submenu(submenu) => submenu.collect(out),
                MenuEntry::Standard { .. } | MenuEntry::Separator => {}
            }
        }
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut MenuItem> {
        for entry in &mut self.entries {
            match entry {
                MenuEntry::Item(item) if item.id == id => return Some(item),
                MenuEntry::Submenu(submenu) => {
                    if let Some(item) = submenu.find_mut(id) {
                        return Some(item);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// The whole menu bar. On macOS the first menu is the application menu: the
/// system shows the app's name over it, so its own label does not matter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBar {
    pub menus: Vec<Submenu>,
}

impl MenuBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn menu(mut self, menu: Submenu) -> Self {
        self.menus.push(menu);
        self
    }

    /// Every item of the bar, nested ones included, in the order they are shown.
    pub fn items(&self) -> Vec<&MenuItem> {
        let mut out = Vec::new();
        for menu in &self.menus {
            menu.collect(&mut out);
        }
        out
    }

    /// The item with this id.
    pub fn item_mut(&mut self, id: &str) -> Option<&mut MenuItem> {
        self.menus.iter_mut().find_map(|menu| menu.find_mut(id))
    }

    /// Enables or disables an item; `false` when there is no such item.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> bool {
        self.item_mut(id).map(|item| item.enabled = enabled).is_some()
    }

    /// Sets the check mark of a check item; `false` when there is no such
    /// check item.
    pub fn set_checked(&mut self, id: &str, checked: bool) -> bool {
        match self.item_mut(id) {
            Some(item) if item.checked.is_some() => {
                item.checked = Some(checked);
                true
            }
            _ => false,
        }
    }

    /// The ids used by more than one item (each one once). A bar with
    /// duplicates cannot be installed: the application could not tell the
    /// items apart.
    pub fn duplicate_ids(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut repeated = Vec::new();
        for item in self.items() {
            if !seen.insert(item.id.as_str()) && !repeated.contains(&item.id) {
                repeated.push(item.id.clone());
            }
        }
        repeated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MenuBar {
        MenuBar::new()
            .menu(
                Submenu::new("App")
                    .standard(Standard::About, "About")
                    .separator()
                    .item(MenuItem::new("quit", "Quit")),
            )
            .menu(
                Submenu::new("View")
                    .item(MenuItem::new("overlay", "Overlay").checkable(false))
                    .submenu(Submenu::new("Zoom").item(MenuItem::new("zoom_in", "In"))),
            )
    }

    #[test]
    fn items_come_in_display_order_and_include_nested_ones() {
        let bar = sample();
        let ids: Vec<&str> = bar.items().iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["quit", "overlay", "zoom_in"]);
    }

    #[test]
    fn duplicate_ids_are_reported_once() {
        let bar = sample()
            .menu(Submenu::new("X").item(MenuItem::new("quit", "Again")).item(MenuItem::new("quit", "And again")));
        assert_eq!(bar.duplicate_ids(), ["quit"]);
        assert!(sample().duplicate_ids().is_empty());
    }

    #[test]
    fn enabling_and_checking_reach_nested_items() {
        let mut bar = sample();
        assert!(bar.set_enabled("zoom_in", false));
        assert!(!bar.item_mut("zoom_in").unwrap().enabled);
        assert!(bar.set_checked("overlay", true));
        assert_eq!(bar.item_mut("overlay").unwrap().checked, Some(true));
        // Only check items have a check mark, and only existing ones exist.
        assert!(!bar.set_checked("quit", true));
        assert!(!bar.set_enabled("nothing", true));
    }

    #[test]
    fn shortcut_builders_add_modifiers() {
        let shortcut = Shortcut::command(Key::Char('s')).with_shift().with_alt();
        assert!(shortcut.command && shortcut.shift && shortcut.alt && !shortcut.ctrl);
        assert_eq!(Shortcut::plain(Key::F(5)).command, false);
    }
}
