//! The macOS menu bar: an `NSMenu` built with AppKit and installed as the
//! application's main menu.
//!
//! Items that belong to the app (`Item`) all point at one small Objective-C
//! object, [`MenuTarget`], whose single action looks the item's `tag` up in a
//! list of ids and queues the id. The application empties the queue with
//! [`NativeMenu::poll`] / [`NativeMenu::drain`]. Items the system implements
//! (`Standard`) have no target: they go down the responder chain to
//! `NSApplication` or the key window, as in any Mac app.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};

use super::{Key, MenuBar, MenuEntry, MenuItem, Role, Shortcut, Standard, Submenu};

/// What the target keeps: the ids the item tags point into, the queue the
/// chosen ids go to and what wakes the application up.
struct TargetState {
    ids: RefCell<Vec<String>>,
    queue: Rc<RefCell<VecDeque<String>>>,
    wakeup: Box<dyn Fn()>,
}

define_class!(
    /// Receives the action of every item of the application.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NativeToolsMenuTarget"]
    #[ivars = TargetState]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(menuItemChosen:))]
        fn menu_item_chosen(&self, sender: &NSMenuItem) {
            let state = self.ivars();
            let Ok(index) = usize::try_from(sender.tag()) else {
                return;
            };
            let Some(id) = state.ids.borrow().get(index).cloned() else {
                return;
            };
            state.queue.borrow_mut().push_back(id);
            (state.wakeup)();
        }
    }
);

impl MenuTarget {
    #[allow(unsafe_code)]
    fn new(mtm: MainThreadMarker, state: TargetState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

/// The menu bar of the application, installed in the system.
///
/// Not `Send`: it lives on the main thread, where AppKit needs it. Keep it
/// alive as long as the menu should work (the items do not own their target).
pub struct NativeMenu {
    _target: Retained<MenuTarget>,
    queue: Rc<RefCell<VecDeque<String>>>,
    items: HashMap<String, Retained<NSMenuItem>>,
}

impl NativeMenu {
    /// Whether this system has a native menu bar to install into.
    pub const SUPPORTED: bool = true;

    /// Builds `bar` and installs it as the application's main menu.
    ///
    /// Must be called on the main thread, once the application has finished
    /// launching: the event loop creates a default menu of its own at that
    /// point and would replace this one. With eframe, call it from the
    /// creation closure of the app. `wakeup` runs when an item is chosen, to
    /// get the GUI to run a frame (`ctx.request_repaint()`).
    pub fn install(bar: &MenuBar, wakeup: impl Fn() + 'static) -> Result<Self, String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| String::from("the menu bar must be installed from the main thread"))?;
        let duplicates = bar.duplicate_ids();
        if !duplicates.is_empty() {
            return Err(format!("menu ids used more than once: {}", duplicates.join(", ")));
        }

        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let target = MenuTarget::new(
            mtm,
            TargetState {
                ids: RefCell::new(Vec::new()),
                queue: Rc::clone(&queue),
                wakeup: Box::new(wakeup),
            },
        );
        let application = NSApplication::sharedApplication(mtm);

        let mut builder = Builder {
            mtm,
            application: &application,
            target: &target,
            items: HashMap::new(),
        };
        let main_menu = builder.menu("");
        for submenu in &bar.menus {
            let entry = NSMenuItem::new(mtm);
            entry.setTitle(&NSString::from_str(&submenu.label));
            entry.setSubmenu(Some(&builder.submenu(submenu)));
            main_menu.addItem(&entry);
        }
        let items = builder.items;
        application.setMainMenu(Some(&main_menu));

        Ok(Self {
            _target: target,
            queue,
            items,
        })
    }

    /// Brings the enabled state and check marks of the installed items in
    /// line with `bar` (the same bar that was installed, changed since).
    pub fn sync(&self, bar: &MenuBar) {
        for item in bar.items() {
            let Some(native) = self.items.get(&item.id) else {
                continue;
            };
            native.setEnabled(item.enabled);
            if let Some(checked) = item.checked {
                native.setState(Builder::state(checked));
            }
        }
    }

    /// The id of the oldest item chosen and not yet taken.
    pub fn poll(&self) -> Option<String> {
        self.queue.borrow_mut().pop_front()
    }

    /// The ids of every item chosen and not yet taken, oldest first.
    pub fn drain(&self) -> Vec<String> {
        self.queue.borrow_mut().drain(..).collect()
    }
}

/// Builds the AppKit objects of a [`MenuBar`].
struct Builder<'a> {
    mtm: MainThreadMarker,
    application: &'a NSApplication,
    target: &'a Retained<MenuTarget>,
    items: HashMap<String, Retained<NSMenuItem>>,
}

impl Builder<'_> {
    fn state(checked: bool) -> isize {
        if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        }
    }

    /// An empty menu. Items are enabled by what the app says, not by the
    /// system's validation (which would enable any item with a target).
    fn menu(&self, title: &str) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(self.mtm), &NSString::from_str(title));
        menu.setAutoenablesItems(false);
        menu
    }

    fn submenu(&mut self, submenu: &Submenu) -> Retained<NSMenu> {
        let menu = self.menu(&submenu.label);
        for entry in &submenu.entries {
            let native = match entry {
                MenuEntry::Item(item) => self.item(item),
                MenuEntry::Submenu(nested) => {
                    let native = NSMenuItem::new(self.mtm);
                    native.setTitle(&NSString::from_str(&nested.label));
                    native.setSubmenu(Some(&self.submenu(nested)));
                    native
                }
                MenuEntry::Standard { kind, label } => self.standard(*kind, label),
                MenuEntry::Separator => NSMenuItem::separatorItem(self.mtm),
            };
            menu.addItem(&native);
        }
        match submenu.role {
            Some(Role::Windows) => self.application.setWindowsMenu(Some(&menu)),
            Some(Role::Help) => self.application.setHelpMenu(Some(&menu)),
            None => {}
        }
        menu
    }

    /// An item of the application: its tag is the position of its id in the
    /// target's list.
    #[allow(unsafe_code)]
    fn item(&mut self, item: &MenuItem) -> Retained<NSMenuItem> {
        let (key, modifiers) = match item.shortcut {
            Some(shortcut) => (shortcut.key_equivalent(), shortcut.modifier_flags()),
            None => (String::new(), NSEventModifierFlags::empty()),
        };
        let native = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(&item.label),
                Some(sel!(menuItemChosen:)),
                &NSString::from_str(&key),
            )
        };
        native.setKeyEquivalentModifierMask(modifiers);
        native.setEnabled(item.enabled);
        if let Some(checked) = item.checked {
            native.setState(Self::state(checked));
        }
        let mut ids = self.target.ivars().ids.borrow_mut();
        native.setTag(ids.len() as isize);
        ids.push(item.id.clone());
        unsafe { native.setTarget(Some(self.target)) };
        self.items.insert(item.id.clone(), native.clone());
        native
    }

    /// An item the system implements: no target, so the action travels the
    /// responder chain.
    #[allow(unsafe_code)]
    fn standard(&self, kind: Standard, label: &str) -> Retained<NSMenuItem> {
        let command = NSEventModifierFlags::Command;
        let (action, key, modifiers): (Sel, &str, NSEventModifierFlags) = match kind {
            Standard::About => (sel!(orderFrontStandardAboutPanel:), "", command),
            Standard::Services => (sel!(submenuAction:), "", command),
            Standard::Hide => (sel!(hide:), "h", command),
            Standard::HideOthers => (
                sel!(hideOtherApplications:),
                "h",
                command | NSEventModifierFlags::Option,
            ),
            Standard::ShowAll => (sel!(unhideAllApplications:), "", command),
            Standard::Minimize => (sel!(performMiniaturize:), "m", command),
            Standard::Zoom => (sel!(performZoom:), "", command),
        };
        let native = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(label),
                Some(action),
                &NSString::from_str(key),
            )
        };
        native.setKeyEquivalentModifierMask(modifiers);
        if kind == Standard::Services {
            // The system fills this submenu with the services of other apps.
            let services = self.menu(label);
            native.setSubmenu(Some(&services));
            self.application.setServicesMenu(Some(&services));
        }
        native
    }
}

impl Key {
    /// The key as AppKit names it: the lower case character, or the control
    /// character or private-use code point it uses for special keys.
    fn key_equivalent(self) -> String {
        match self {
            Key::Char(c) => c.to_lowercase().collect(),
            // NSF1FunctionKey is U+F704, and the others follow.
            Key::F(n) => char::from_u32(0xF704 + u32::from(n.clamp(1, 35)) - 1)
                .map(String::from)
                .unwrap_or_default(),
            Key::Enter => String::from("\r"),
            Key::Escape => String::from("\u{1b}"),
            Key::Tab => String::from("\t"),
            Key::Space => String::from(" "),
            Key::Backspace => String::from("\u{8}"),
            Key::Delete => String::from("\u{f728}"),
            Key::Up => String::from("\u{f700}"),
            Key::Down => String::from("\u{f701}"),
            Key::Left => String::from("\u{f702}"),
            Key::Right => String::from("\u{f703}"),
        }
    }
}

impl Shortcut {
    fn key_equivalent(&self) -> String {
        self.key.key_equivalent()
    }

    /// `command` is `Cmd` here; AppKit's default mask is `Cmd` alone, so the
    /// mask is always set explicitly.
    fn modifier_flags(&self) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        if self.command {
            flags |= NSEventModifierFlags::Command;
        }
        if self.ctrl {
            flags |= NSEventModifierFlags::Control;
        }
        if self.alt {
            flags |= NSEventModifierFlags::Option;
        }
        if self.shift {
            flags |= NSEventModifierFlags::Shift;
        }
        flags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_are_lower_case_and_shift_is_a_modifier() {
        let shortcut = Shortcut::command(Key::Char('S')).with_shift();
        assert_eq!(shortcut.key_equivalent(), "s");
        let flags = shortcut.modifier_flags();
        assert!(flags.contains(NSEventModifierFlags::Command));
        assert!(flags.contains(NSEventModifierFlags::Shift));
        assert!(!flags.contains(NSEventModifierFlags::Option));
    }

    #[test]
    fn special_keys_use_appkit_codes() {
        assert_eq!(Key::F(1).key_equivalent(), "\u{f704}");
        assert_eq!(Key::F(12).key_equivalent(), "\u{f70f}");
        assert_eq!(Key::Enter.key_equivalent(), "\r");
        assert_eq!(Key::Left.key_equivalent(), "\u{f702}");
    }

    #[test]
    fn a_plain_shortcut_has_no_modifier() {
        assert!(Shortcut::plain(Key::F(5)).modifier_flags().is_empty());
    }
}
