//! A settings window built only with `egui-panels`.
//!
//! ```sh
//! cargo run -p egui-panels --example settings_demo
//! ```

use eframe::egui;
use egui_panels::{
    Action, ActionBar, Avatar, Draft, EntityCard, NavGroup, NavItem, PathPicker, Section,
    SettingsLayout, SideNav, StatusKind, Variant,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Sources,
    Alerts,
    Accounts,
    Appearance,
}

#[derive(Clone, PartialEq)]
struct Settings {
    folder: String,
    channels: Vec<(String, bool)>,
    radius: u8,
    sound: bool,
    duration_secs: u32,
    dark: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: String::from("~/Documents/logs"),
            channels: ["Intel", "Local", "Alliance", "Corp", "Help"]
                .iter()
                .enumerate()
                .map(|(index, name)| (name.to_string(), index == 0))
                .collect(),
            radius: 3,
            sound: true,
            duration_secs: 240,
            dark: true,
        }
    }
}

struct Demo {
    page: Page,
    settings: Draft<Settings>,
    last_action: Option<Action>,
}

impl Demo {
    fn nav(&self) -> Vec<NavGroup<Page>> {
        let draft = &self.settings;
        vec![
            NavGroup::new(
                "Input",
                vec![
                    NavItem::new(Page::Sources, "Sources")
                        .dirty(draft.differs(|s| (s.folder.clone(), s.channels.clone()))),
                    NavItem::new(Page::Alerts, "Alerts")
                        .dirty(draft.differs(|s| (s.radius, s.sound, s.duration_secs))),
                ],
            ),
            NavGroup::new("Account", vec![NavItem::new(Page::Accounts, "Accounts")]),
            NavGroup::new(
                "System",
                vec![NavItem::new(Page::Appearance, "Appearance").dirty(draft.differs(|s| s.dark))],
            ),
        ]
    }
}

fn sources(ui: &mut egui::Ui, settings: &mut Settings) {
    egui_panels::page(ui, |ui| {
        egui_panels::page_header(ui, "Sources", Some("Where the data comes from."));
        if let Some(step) = egui_panels::stepper(ui, &["Sources", "Rules", "Alerts"], 0) {
            ui.label(format!("step {step} clicked"));
        }
        Section::new("Folder")
            .description("Only new lines are read.")
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    let picker = form.row("Folder", |ui| {
                        PathPicker::new(&mut settings.folder, "Browse…")
                            .reset("Default")
                            .show(ui)
                    });
                    if picker.reset {
                        settings.folder = Settings::default().folder;
                    }
                    form.note(|ui| {
                        egui_panels::status(ui, StatusKind::Ok, "Folder found · 5 channels")
                    });
                });
            });
        Section::new("Channels")
            .description("Only lines from the picked channels are used.")
            .show(ui, |ui| {
                let count = settings.channels.len();
                egui_panels::tile_grid(ui, 2, count, |ui, index| {
                    let (name, on) = &mut settings.channels[index];
                    egui_panels::tile(ui, on, name, Some("last line 2 min ago"));
                });
            });
    });
}

fn alerts(ui: &mut egui::Ui, settings: &mut Settings) {
    egui_panels::page(ui, |ui| {
        egui_panels::page_header(ui, "Alerts", Some("What happens when a rule fires."));
        Section::new("Distance").show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row("Sound within", |ui| {
                    let options: Vec<(u8, String)> = (1..=7).map(|n| (n, n.to_string())).collect();
                    let options: Vec<(u8, &str)> = options
                        .iter()
                        .map(|(n, label)| (*n, label.as_str()))
                        .collect();
                    egui_panels::segmented(ui, &mut settings.radius, &options);
                    ui.label("jumps");
                });
                form.row("Measured from", |ui| {
                    egui_panels::chip(ui, "Kara Voss", Some("H-5GUI"));
                    egui_panels::chip(ui, "Orrin Tal", Some("1DQ1-A"));
                });
            });
        });
        Section::new("Map and sound").show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row("Duration", |ui| {
                    egui_panels::Slider::new(&mut settings.duration_secs, 10..=600)
                        .step(10.0)
                        .show(ui);
                    ui.label(format!("{} s", settings.duration_secs));
                });
                form.row_hint("Play a sound", "Only near your characters", |ui| {
                    egui_panels::switch(ui, &mut settings.sound);
                });
                form.note(|ui| {
                    egui_panels::button(ui, "Test the alert", Variant::Ghost);
                    egui_panels::status(ui, StatusKind::Warning, "No sound device found");
                });
            });
        });
    });
}

fn accounts(ui: &mut egui::Ui) {
    egui_panels::page(ui, |ui| {
        egui_panels::page_header_with(ui, "Accounts", Some("Linked accounts."), |ui| {
            egui_panels::button(ui, "＋ Link account", Variant::Primary);
        });
        for (name, initials) in [("Kara Voss", "KV"), ("Orrin Tal", "OT")] {
            EntityCard::new(name)
                .avatar(Avatar::Initials(initials.to_string()))
                .line("[CORPORATION] · [ALLIANCE]")
                .line("Last login 2026-09-27 21:04 UTC")
                .show(ui, |ui| egui_panels::button(ui, "Unlink", Variant::Danger));
        }
        egui_panels::status(ui, StatusKind::Info, "Linking applies at once.");
    });
}

fn appearance(ui: &mut egui::Ui, settings: &mut Settings) {
    egui_panels::page(ui, |ui| {
        egui_panels::page_header(ui, "Appearance", None);
        Section::new("Theme").show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row("Dark theme", |ui| {
                    egui_panels::switch(ui, &mut settings.dark)
                });
                form.row("Badges", |ui| {
                    egui_panels::badge(ui, "Visual");
                    egui_panels::badge(ui, "Sound");
                    egui_panels::badge(ui, "Log");
                });
            });
        });
    });
}

impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().set_visuals(if self.settings.current().dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let groups = self.nav();
        let nav = SideNav::new(&groups)
            .title("Settings")
            .subtitle("egui-panels demo");
        let dirty: Vec<&str> = groups
            .iter()
            .flat_map(|group| &group.items)
            .filter(|item| item.dirty)
            .map(|item| item.label.as_str())
            .collect();
        let pending =
            (!dirty.is_empty()).then(|| format!("Unsaved changes in {}", dirty.join(", ")));
        let bar = ActionBar::new("Cancel", "Apply", "Accept")
            .pending(pending.as_deref())
            .saved_message("All saved");
        let settings = &mut self.settings;
        let action = SettingsLayout::new("demo").show(ui, nav, bar, &mut self.page, |ui, page| {
            let current = settings.current_mut();
            match page {
                Page::Sources => sources(ui, current),
                Page::Alerts => alerts(ui, current),
                Page::Accounts => accounts(ui),
                Page::Appearance => appearance(ui, current),
            }
        });
        match action {
            Some(Action::Cancel) => self.settings.revert(),
            Some(Action::Apply | Action::Accept) => self.settings.commit(),
            None => {}
        }
        if action.is_some() {
            self.last_action = action;
        }
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "egui-panels demo",
        eframe::NativeOptions::default(),
        Box::new(|_| {
            Ok(Box::new(Demo {
                page: Page::Sources,
                settings: Draft::new(Settings::default()),
                last_action: None,
            }))
        }),
    )
}
