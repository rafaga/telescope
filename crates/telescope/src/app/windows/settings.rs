//! The Settings screen, built with `egui-panels`: a navigation with the pages
//! grouped by what they configure, the selected page and the Cancel / Apply
//! / Accept bar.
//!
//! The tree has two roots: the intelligence rules (`patterns`), with the
//! `sources` they read from and the `alerts` they raise under them, and the
//! `application`, with the `characters` and the `maps`.
//!
//! Every change is a draft until Apply or Accept: the settings are edited in
//! place and `TelescopeApp::cancel_settings` puts back the snapshot taken
//! when the screen opened. A page whose values differ from that snapshot
//! shows a circle in the navigation. Linking and unlinking characters are the
//! exception: they go through EVE SSO and apply at once.

use crate::app::TelescopeApp;
use crate::app::intel::input::monitored_channel_names;
use crate::app::messages::{Message, SettingsPage};
use crate::app::settings::Settings;
use eframe::egui;
use egui_ltreeview::{Action as TreeAction, IndentHintStyle, NodeBuilder, TreeView, TreeViewState};
use egui_panels::{Action, ActionBar, SettingsLayout, StatusKind};
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
mod zkillboard;

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
    /// The same for the SDE database path (Application).
    sde_text: Option<String>,
    /// The window file dialogs belong to (see `Dialog::set_owner`).
    window_owner: Option<isize>,
    /// Which groups of the navigation tree are open, and what is selected.
    nav_tree: TreeViewState<NavNode>,
}

/// The groups of pages of the navigation tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NavGroupId {
    /// The rules, which are also the root of everything an intel line goes
    /// through: a group that is a page itself.
    Rules,
    /// Where a line is read from; inside `Rules`.
    Sources,
    /// The ways a reported system is alerted, and the page of how close it
    /// has to be to alert (a group that is a page itself); inside `Rules`.
    Alerts,
    /// The application itself, a page that is also the root of the characters
    /// and the maps.
    Application,
}

/// What a group (or the tree itself) holds: a page or another group.
#[derive(Clone, Copy)]
enum NavEntry {
    Page(SettingsPage),
    Group(NavGroupId),
}

impl NavGroupId {
    /// The roots of the tree, in order: the rules with everything an intel
    /// line goes through, and the application with the rest.
    const TOP: [NavEntry; 2] = [
        NavEntry::Group(NavGroupId::Rules),
        NavEntry::Group(NavGroupId::Application),
    ];

    /// Every group, nested or not.
    const ALL: [NavGroupId; 4] = [
        NavGroupId::Rules,
        NavGroupId::Sources,
        NavGroupId::Alerts,
        NavGroupId::Application,
    ];

    /// The page the group is itself, if it is one (clicking its row opens it).
    fn page(self) -> Option<SettingsPage> {
        match self {
            NavGroupId::Rules => Some(SettingsPage::Rules),
            NavGroupId::Application => Some(SettingsPage::Application),
            NavGroupId::Alerts => Some(SettingsPage::Alerts),
            NavGroupId::Sources => None,
        }
    }

    /// The node of the tree that is the group.
    fn node(self) -> NavNode {
        match self.page() {
            Some(page) => NavNode::Page(page),
            None => NavNode::Group(self),
        }
    }

    fn title(self) -> String {
        match self {
            NavGroupId::Rules => SettingsPage::Rules.title(),
            NavGroupId::Application => SettingsPage::Application.title(),
            NavGroupId::Sources => t!("settings.groups.sources").into_owned(),
            NavGroupId::Alerts => SettingsPage::Alerts.title(),
        }
    }

    /// What the group holds, in order.
    fn entries(self) -> &'static [NavEntry] {
        match self {
            NavGroupId::Rules => &[
                NavEntry::Group(NavGroupId::Sources),
                NavEntry::Group(NavGroupId::Alerts),
            ],
            NavGroupId::Sources => &[
                NavEntry::Page(SettingsPage::Sources),
                NavEntry::Page(SettingsPage::Zkillboard),
            ],
            NavGroupId::Alerts => &[
                NavEntry::Page(SettingsPage::AlertsVisual),
                NavEntry::Page(SettingsPage::AlertsSound),
                NavEntry::Page(SettingsPage::AlertsOverlay),
            ],
            NavGroupId::Application => &[
                NavEntry::Page(SettingsPage::Characters),
                NavEntry::Page(SettingsPage::Maps),
            ],
        }
    }
}

/// A node of the navigation tree: a group of pages, or a page.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NavNode {
    Group(NavGroupId),
    Page(SettingsPage),
}

impl SettingsUi {
    /// Forgets what was typed and not applied (Cancel).
    pub(crate) fn discard_drafts(&mut self) {
        self.db_text = None;
        self.sde_text = None;
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
        let pending = (!dirty.is_empty()).then(|| {
            let pages: Vec<String> = dirty.iter().map(|page| page.title()).collect();
            t!("settings.pending", pages = pages.join(", ")).into_owned()
        });
        let (cancel, apply, accept, saved) = (
            t!("common.cancel"),
            t!("settings.apply"),
            t!("common.accept"),
            t!("settings.saved"),
        );
        let bar = ActionBar::new(&cancel, &apply, &accept)
            .pending(pending.as_deref())
            .saved_message(&saved);

        let mut page = self.selected_settings_page;
        // The node editor fills the page itself.
        let scroll = !(page == SettingsPage::Rules && self.patterns_editor.is_open());
        let mut nav_tree = std::mem::take(&mut self.settings_ui.nav_tree);
        let action = SettingsLayout::new("settings")
            .scroll(scroll)
            .show_with_nav(
                ui,
                |ui, page| Self::show_settings_nav(ui, &mut nav_tree, page, &dirty),
                bar,
                &mut page,
                |ui, page| match page {
                    SettingsPage::Sources => self.show_sources_page(ui),
                    SettingsPage::Zkillboard => self.show_zkillboard_page(ui),
                    SettingsPage::Rules => self.show_patterns_page(ui),
                    SettingsPage::Alerts => self.show_alerts_page(ui),
                    SettingsPage::AlertsVisual => self.show_alerts_visual_page(ui),
                    SettingsPage::AlertsSound => self.show_alerts_sound_page(ui),
                    SettingsPage::AlertsOverlay => self.show_alerts_overlay_page(ui),
                    SettingsPage::Maps => self.show_maps_page(ui),
                    SettingsPage::Characters => self.show_characters_page(ui),
                    SettingsPage::Application => self.show_application_page(ui),
                },
            );
        self.settings_ui.nav_tree = nav_tree;
        self.selected_settings_page = page;
        match action {
            Some(Action::Cancel) => self.cancel_settings(),
            Some(Action::Apply) => {
                self.apply_settings();
            }
            Some(Action::Accept) => self.accept_settings(),
            None => {}
        }
    }

    /// The navigation of the Settings screen: the pages in a tree, grouped by
    /// what they configure. A page with changes not applied yet has a circle
    /// at the right of its row.
    /// `selected` follows the page picked (and the tree follows `selected`
    /// when something else changes it, as the steps of the intel flow do).
    fn show_settings_nav(
        ui: &mut egui::Ui,
        tree: &mut TreeViewState<NavNode>,
        selected: &mut SettingsPage,
        dirty: &[SettingsPage],
    ) {
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(t!("settings.title"))
                .size(theme.section_title_size + 3.0)
                .color(palette.strong_text),
        );
        ui.label(
            egui::RichText::new(format!("Telescope {}", env!("CARGO_PKG_VERSION")))
                .size(theme.small_size)
                .color(palette.muted_text),
        );
        ui.add_space(8.0);

        // The groups are always open: nothing in the tree collapses.
        let open_all = |tree: &mut TreeViewState<NavNode>| {
            for group in NavGroupId::ALL {
                tree.set_openness(group.node(), true);
            }
        };
        open_all(tree);
        tree.set_one_selected(NavNode::Page(*selected));
        let (_, actions) = TreeView::new(ui.scope_id().with("settings_nav"))
            .fill_available_width(true)
            // as tall as its rows: filling the panel would make it taller
            // than the space under the title and give the panel a scroll bar
            .fill_available_height(false)
            // one page at a time, and nothing to drag around
            .allow_multi_selection(false)
            .allow_drag_and_drop(false)
            // lines that connect every group and page to its parent
            .indent_hint_style(IndentHintStyle::Hook)
            .show_state(ui, tree, |builder| {
                for entry in NavGroupId::TOP {
                    Self::add_nav_entry(builder, entry, dirty, &palette);
                }
            });
        // a click on a group row must not close it either
        open_all(tree);
        // ... nor select it: only the pages are selected (a group without a
        // page is a disabled row)
        tree.set_one_selected(NavNode::Page(*selected));
        for action in actions {
            if let TreeAction::SetSelected(picked) = action
                && let Some(NavNode::Page(page)) = picked.first()
            {
                *selected = *page;
            }
        }
    }

    /// A page or a group of the navigation tree. A group holds its pages and
    /// the groups inside it, and has no button to collapse it. The rows are
    /// plain, the tree's lines connect them. A group
    /// that is a page itself (the rules, the application) opens it when
    /// clicked, and has the circle of changes not applied like any page; a group
    /// that is not a page (the sources, the alerts) is a disabled row.
    fn add_nav_entry(
        builder: &mut egui_ltreeview::TreeViewBuilder<'_, NavNode>,
        entry: NavEntry,
        dirty: &[SettingsPage],
        palette: &egui_panels::Palette,
    ) {
        let changed = |page: Option<SettingsPage>| page.is_some_and(|page| dirty.contains(&page));
        // The mark of a page with changes not applied yet: a filled circle at
        // the right end of its row, drawn as the tree's accessory.
        let dot = palette.dirty;
        let tip = t!("settings.page_changed").into_owned();
        let badge = move |ui: &mut egui::Ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 5.0, dot);
            response.on_hover_text(&tip);
        };
        let row = |title: String, page: Option<SettingsPage>| {
            // A row with no page behind it only groups other rows: it is
            // shown disabled, as there is nothing to open.
            let has_page = page.is_some();
            move |ui: &mut egui::Ui| {
                ui.add_enabled_ui(has_page, |ui| {
                    ui.add(egui::Label::new(title.clone()).selectable(false));
                });
            }
        };
        match entry {
            NavEntry::Page(page) => {
                let node =
                    NodeBuilder::leaf(NavNode::Page(page)).label_ui(row(page.title(), Some(page)));
                builder.node(if changed(Some(page)) {
                    node.accessory(badge)
                } else {
                    node
                });
            }
            NavEntry::Group(group) => {
                let node = NodeBuilder::dir(group.node())
                    .default_open(true)
                    .closer(|_ui, _state| {})
                    .label_ui(row(group.title(), group.page()));
                builder.node(if changed(group.page()) {
                    node.accessory(badge)
                } else {
                    node
                });
                for &inner in group.entries() {
                    Self::add_nav_entry(builder, inner, dirty, palette);
                }
                builder.close_dir();
            }
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
        if now.get_warning_area() != saved.get_warning_area() {
            pages.push(SettingsPage::Alerts);
        }
        if now.get_alert_duration_secs() != saved.get_alert_duration_secs()
            || now.get_center_on_alert() != saved.get_center_on_alert()
        {
            pages.push(SettingsPage::AlertsVisual);
        }
        if now.get_zkillboard() != saved.get_zkillboard() {
            pages.push(SettingsPage::Zkillboard);
        }
        if now.get_alert_sound() != saved.get_alert_sound() {
            pages.push(SettingsPage::AlertsSound);
        }
        if now.get_overlay_enabled() != saved.get_overlay_enabled()
            || now.get_overlay_all_characters() != saved.get_overlay_all_characters()
            || now.get_overlay_order() != saved.get_overlay_order()
            || now.get_overlay_max_rows() != saved.get_overlay_max_rows()
            || now.get_overlay_opacity() != saved.get_overlay_opacity()
            || now.get_overlay_hide_unfocused() != saved.get_overlay_hide_unfocused()
        {
            pages.push(SettingsPage::AlertsOverlay);
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
            || now.get_show_debug_log() != saved.get_show_debug_log()
            || now.get_sde() != saved.get_sde()
            || now.get_db() != saved.get_db()
        {
            pages.push(SettingsPage::Application);
        }
        // In the navigation's order.
        pages.sort_by_key(|page| *page as u8);
        pages
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::ZKillboardOptions;

    /// Every setting a page edits must mark that page as changed, or the
    /// circle in the navigation would not warn about changes to apply.
    #[test]
    fn the_settings_a_page_edits_mark_it_as_changed() {
        let dir = std::env::temp_dir().join(format!("telescope-dirty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = TelescopeApp::for_test(&dir);
        app.settings_snapshot = Some(app.settings.clone());
        assert!(app.dirty_settings_pages().is_empty());

        let debug_log = app.settings.get_show_debug_log();
        app.settings.set_show_debug_log(!debug_log);
        assert!(
            app.dirty_settings_pages()
                .contains(&SettingsPage::Application)
        );
        app.settings.set_show_debug_log(debug_log);
        assert!(app.dirty_settings_pages().is_empty());

        let hide = app.settings.get_overlay_hide_unfocused();
        app.settings.set_overlay_hide_unfocused(!hide);
        assert!(
            app.dirty_settings_pages()
                .contains(&SettingsPage::AlertsOverlay)
        );
        app.settings.set_overlay_hide_unfocused(hide);
        assert!(app.dirty_settings_pages().is_empty());

        let radius = app.settings.get_warning_area();
        app.settings.set_warning_area(radius % 7 + 1);
        assert!(app.dirty_settings_pages().contains(&SettingsPage::Alerts));
        app.settings.set_warning_area(radius);
        assert!(app.dirty_settings_pages().is_empty());

        let options = app.settings.get_zkillboard().clone();
        app.settings.set_zkillboard(ZKillboardOptions {
            ignore_npc_only: !options.ignore_npc_only,
            ..options.clone()
        });
        assert!(
            app.dirty_settings_pages()
                .contains(&SettingsPage::Zkillboard)
        );
        app.settings.set_zkillboard(options);
        assert!(app.dirty_settings_pages().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
