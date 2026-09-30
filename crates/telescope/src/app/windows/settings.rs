//! The Settings screen, built with `egui-panels`: a navigation with the pages
//! grouped by what they configure, the selected page and the Cancel / Apply
//! / Accept bar.
//!
//! The intel pages follow the path of a chat line -- `sources` (where it is
//! read), `patterns` (the rules that turn it into an alert) and `alerts`
//! (what the alert does) -- then come `maps`, `characters` and
//! `application`.
//!
//! Every change is a draft until Apply or Accept: the settings are edited in
//! place and `TelescopeApp::cancel_settings` puts back the snapshot taken
//! when the screen opened. A page whose values differ from that snapshot
//! shows a dot in the navigation. Linking and unlinking characters are the
//! exception: they go through EVE SSO and apply at once.

use crate::app::TelescopeApp;
use crate::app::intel::input::monitored_channel_names;
use crate::app::messages::{Message, SettingsPage};
use crate::app::settings::Settings;
use eframe::egui;
use egui_panels::{Action, ActionBar, NavGroup, NavItem, SettingsLayout, SideNav, StatusKind};
use native_tools::dialog::{Dialog, DialogResult, DialogType, FileFilter};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use webb::graph::RuleGraph;

mod alerts;
mod application;
mod characters;
mod maps;
pub(crate) mod patterns;
mod sources;

/// View state of the Settings pages that is not a setting.
#[derive(Default)]
pub(crate) struct SettingsUi {
    /// Text typed to filter the region tiles (Maps).
    region_filter: String,
    /// Outcome of the last "Test the full alert" (Alerts).
    test_result: Option<(StatusKind, String)>,
    /// The graph last validated for the Rules page and its errors, so the
    /// graph is validated again only when it changes.
    validation: Option<(RuleGraph, Vec<String>)>,
    /// The private database path being typed while it isn't a valid one
    /// (Application); `None` when the field shows the path in the settings.
    db_text: Option<String>,
    /// The window file dialogs belong to (see `Dialog::set_owner`).
    window_owner: Option<isize>,
    /// A step of the intel flow was clicked this frame (it changes the page
    /// like the navigation does).
    step_clicked: bool,
}

impl SettingsUi {
    /// Forgets what was typed and not applied (Cancel).
    pub(crate) fn discard_drafts(&mut self) {
        self.db_text = None;
    }

    /// Records the app window's native id, for the file dialogs.
    pub(crate) fn set_window_owner(&mut self, owner: Option<isize>) {
        self.window_owner = owner;
    }

    /// The errors of `graph` (see [`RuleGraph::validate`]).
    fn validation_errors(&mut self, graph: &RuleGraph) -> &[String] {
        let stale = self
            .validation
            .as_ref()
            .is_none_or(|(validated, _)| validated != graph);
        if stale {
            let errors = graph
                .validate()
                .iter()
                .map(|error| error.to_string())
                .collect();
            self.validation = Some((graph.clone(), errors));
        }
        self.validation
            .as_ref()
            .map_or(&[], |(_, errors)| errors.as_slice())
    }
}

impl TelescopeApp {
    /// Draws the Settings screen over the whole window.
    #[tracing::instrument(skip(self, ui))]
    pub(crate) fn show_settings_screen(&mut self, ui: &mut egui::Ui) {
        let dirty = self.dirty_settings_pages();
        let item = |page: SettingsPage| {
            NavItem::new(page, page.title())
                .icon(page.icon())
                .dirty(dirty.contains(&page))
        };
        let groups = [
            NavGroup::new(
                t!("settings.groups.intel"),
                SettingsPage::INTEL_FLOW.into_iter().map(item).collect(),
            ),
            NavGroup::new(t!("settings.groups.view"), vec![item(SettingsPage::Maps)]),
            NavGroup::new(
                t!("settings.groups.account"),
                vec![item(SettingsPage::Characters)],
            ),
            NavGroup::new(
                t!("settings.groups.system"),
                vec![item(SettingsPage::Application)],
            ),
        ];
        let title = t!("settings.title");
        let subtitle = format!("Telescope {}", env!("CARGO_PKG_VERSION"));
        let nav = SideNav::new(&groups).title(&title).subtitle(&subtitle);

        let pending = (!dirty.is_empty()).then(|| {
            let pages: Vec<String> = dirty.iter().map(|page| page.title()).collect();
            t!("settings.pending", pages = pages.join(", ")).into_owned()
        });
        let (cancel, apply, accept, saved) = (
            t!("settings.cancel"),
            t!("settings.apply"),
            t!("settings.accept"),
            t!("settings.saved"),
        );
        let bar = ActionBar::new(&cancel, &apply, &accept)
            .pending(pending.as_deref())
            .saved_message(&saved);

        let mut page = self.selected_settings_page;
        // The node editor fills the page itself.
        let scroll = !(page == SettingsPage::Rules && self.patterns_editor.is_open());
        let action = SettingsLayout::new("settings").scroll(scroll).show(
            ui,
            nav,
            bar,
            &mut page,
            |ui, page| match page {
                SettingsPage::Sources => self.show_sources_page(ui),
                SettingsPage::Rules => self.show_patterns_page(ui),
                SettingsPage::Alerts => self.show_alerts_page(ui),
                SettingsPage::Maps => self.show_maps_page(ui),
                SettingsPage::Characters => self.show_characters_page(ui),
                SettingsPage::Application => self.show_application_page(ui),
            },
        );
        // A step of the intel flow clicked inside the page wins over the
        // navigation, which didn't change then.
        if page != self.selected_settings_page && !self.settings_ui.step_clicked {
            self.selected_settings_page = page;
        }
        self.settings_ui.step_clicked = false;
        match action {
            Some(Action::Cancel) => self.cancel_settings(),
            Some(Action::Apply) => {
                self.apply_settings();
            }
            Some(Action::Accept) => self.accept_settings(),
            None => {}
        }
    }

    /// The pages whose values differ from those the screen opened with (or
    /// last applied).
    fn dirty_settings_pages(&self) -> Vec<SettingsPage> {
        let mut pages = Vec::new();
        if self.patterns_editor.is_modified(&self.intel_graph) {
            pages.push(SettingsPage::Rules);
        }
        let Some(saved) = &self.settings_snapshot else {
            return pages;
        };
        let now = &self.settings;
        if now.get_intel() != saved.get_intel() || watched_channels(now) != watched_channels(saved)
        {
            pages.push(SettingsPage::Sources);
        }
        if now.get_warning_area() != saved.get_warning_area()
            || now.get_alert_sound() != saved.get_alert_sound()
            || now.get_alert_duration_secs() != saved.get_alert_duration_secs()
            || now.get_center_on_alert() != saved.get_center_on_alert()
        {
            pages.push(SettingsPage::Alerts);
        }
        let startup: HashSet<usize> = saved.get_startup_regions().iter().copied().collect();
        let startup_changed = self
            .behavior
            .tile_data
            .iter()
            .any(|(region, data)| data.show_on_startup != startup.contains(region));
        if startup_changed || now.get_glow_intensity() != saved.get_glow_intensity() {
            pages.push(SettingsPage::Maps);
        }
        if now.get_ui_state().language != saved.get_ui_state().language
            || now.get_sde() != saved.get_sde()
            || now.get_db() != saved.get_db()
        {
            pages.push(SettingsPage::Application);
        }
        // In the navigation's order.
        pages.sort_by_key(|page| *page as u8);
        pages
    }

    /// The "1 · Sources → 2 · Rules → 3 · Alerts" steps at the top of the
    /// intel pages; clicking one opens that page.
    fn intel_flow_stepper(&mut self, ui: &mut egui::Ui, current: SettingsPage) {
        let titles: Vec<String> = SettingsPage::INTEL_FLOW
            .iter()
            .map(|page| page.title())
            .collect();
        let steps: Vec<&str> = titles.iter().map(String::as_str).collect();
        let index = SettingsPage::INTEL_FLOW
            .iter()
            .position(|page| *page == current)
            .unwrap_or_default();
        let clicked = ui
            .horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let clicked = egui_panels::stepper(ui, &steps, index);
                ui.weak(t!("settings.intel_flow_hint"));
                clicked
            })
            .inner;
        if let Some(step) = clicked {
            self.selected_settings_page = SettingsPage::INTEL_FLOW[step];
            self.settings_ui.step_clicked = true;
        }
    }

    /// Opens `dialog` at `start` (its folder, for a file); the path picked is
    /// sent to the app as `message(path)`.
    fn pick_path(
        &self,
        mut dialog: Dialog,
        start: &Path,
        message: impl Fn(PathBuf) -> Message + Send + Sync + 'static,
    ) {
        let directory = if start.is_dir() {
            Some(start)
        } else {
            start.parent().filter(|parent| parent.is_dir())
        };
        if let Some(directory) = directory {
            dialog.set_directory(directory);
        }
        if let Some(owner) = self.settings_ui.window_owner {
            dialog.set_owner(owner);
        }
        let task_msg = Arc::clone(&self.task_msg);
        dialog.open_file_dialog(move |result| {
            if let DialogResult::Ok(path) = result {
                task_msg.spawn(message(path));
            }
        });
    }
}

/// A dialog of `dialog_type` titled `title`.
fn dialog(dialog_type: DialogType, title: &str) -> Dialog {
    let mut dialog = Dialog::new(dialog_type);
    dialog.set_title(title);
    dialog
}

/// A dialog picking a SQLite database file, titled `title`.
fn database_dialog(title: &str) -> Dialog {
    let mut dialog = dialog(DialogType::File, title);
    dialog.add_filter(FileFilter::new(
        t!("settings.dialogs.database_files"),
        &["db", "sqlite", "sqlite3"],
    ));
    dialog
}

/// The channels checked in Sources, sorted.
fn watched_channels(settings: &Settings) -> Vec<String> {
    let mut channels = monitored_channel_names(&settings.get_available_channels());
    channels.sort_unstable();
    channels
}

/// "just now", "5 min ago", "3 h ago" or "2 d ago".
fn time_ago(when: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(when)
        .map_or(0, |elapsed| elapsed.as_secs());
    match secs {
        0..60 => t!("settings.time.just_now"),
        60..3_600 => t!("settings.time.minutes", count = secs / 60),
        3_600..86_400 => t!("settings.time.hours", count = secs / 3_600),
        _ => t!("settings.time.days", count = secs / 86_400),
    }
    .into_owned()
}
