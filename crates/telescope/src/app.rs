//! The application root: [`TelescopeApp`] owns the UI state, the ESI manager, the
//! settings and the message channels that connect the file watcher, the
//! background tasks and the UI.
//!
//! This file holds the struct, its construction, the per-frame `ui` loop and the
//! map pane management. The rest of the behaviour lives in submodules:
//! `windows` (about, debug and settings windows), `intel` (chat log reading and
//! alerts), `watchdog` (character location polling), `database` (player database
//! and SDE updates), `notifications` (the status log), `persistence` (saving
//! settings), `tiles` (map panes), `settings`, `messages`, `file` and `patterns`.

use crate::app::file::IntelEventHandler;
use crate::app::messages::{
    CharacterSync, MapSync, Message, SettingsPage, Type, send_app_message, try_send_app_message,
};
use crate::app::tiles::{TabPane, TileData, TreeBehavior, UniversePane};
use data::AppData;
use eframe::egui::{self, epaint::text::LayoutJob};
use egui_tiles::{Tile, Tiles, Tree};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use sde::{SdeManager, objects::Universe};
use settings::Settings;
use sputnik::patterns::PatternEngine;
use std::{
    path::Path,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender};
use webb::esi::EsiManager;

use self::messages::{AuthSpawner, MessageSpawner};
use self::tiles::RegionPane;
use native_tools::dialog::*;

mod audio;
mod character_link;
mod data;
mod database;
mod database_updater;
mod file;
mod intel;
mod messages;
mod notifications;
mod persistence;
mod settings;
mod tiles;
mod watchdog;
mod windows;

/// Key of Noto Sans CJK in egui's font definitions (see
/// `TelescopeApp::font_definitions`).
const CJK_FONT: &str = "Noto Sans CJK";

/// Face of `NotoSansCJK-Medium.ttc` Telescope uses: 0 JP, 1 KR, 2 SC, 3 TC,
/// 4 HK. Every face has every glyph; only the shape of Han characters
/// changes.
const CJK_FONT_INDEX: u32 = 2;

pub struct TelescopeApp {
    initialized: bool,

    // generic messages
    app_msg: (Arc<Sender<Message>>, Receiver<Message>),
    // map synchronization Messages
    map_msg: (Arc<BCSender<MapSync>>, BCReceiver<MapSync>),
    char_msg: Option<Arc<Sender<CharacterSync>>>,

    // these are the flags to open the windows
    // 0 - About Window
    // 1 - Character Window
    // 2 - Preferences Window
    open: [bool; 3],

    // the ESI Manager
    esi: EsiManager,
    // Capped at `Settings::get_max_app_messages` by `update_status_with_error`.
    app_messages: Vec<LayoutJob>,
    search_text: String,
    emit_notification: bool,
    search_selected_row: Option<usize>,
    search_results: Vec<(isize, String, isize, String)>,
    // State of the Debug window's "Advanced" section.
    debug: windows::debug::DebugState,
    universe: Universe,
    selected_settings_page: SettingsPage,
    tree: Option<Tree<Box<dyn TabPane>>>,

    behavior: TreeBehavior,
    task_msg: Arc<MessageSpawner>,
    task_auth: AuthSpawner,
    settings: Settings,
    watcher: RecommendedWatcher,
    // Live-shared handle for the set of channels the file watcher's event
    // handler filters on. `IntelEventHandler` is moved into `watcher` at
    // construction time and can never be swapped out afterwards, so the
    // monitored-channel list it filters on has to be reachable through
    // shared, interior-mutable storage; otherwise every channel selected or
    // saved after startup is silently ignored until the app restarts.
    intel_channels: Arc<RwLock<Vec<String>>>,
    dlg_intel_dir: Dialog,
    pattern_engine: PatternEngine,
    // Alarm sound for `ActionConfig::MapAlert` matches -- see the
    // `audio` module docs for why this has to be a long-lived field
    // rather than something opened per alert.
    audio: audio::AlarmPlayer,
    // UI state for the "updating the SDE database" progress window --
    // see `database_updater`'s module docs.
    database_updater: database_updater::DatabaseUpdater,
    // Last `GenericNotification` accepted by `update_status_with_error`,
    // plus when it was accepted -- lets that function collapse an
    // immediate repeat (same type/source/context/text) arriving within
    // `Settings::get_notification_dedup_window`. Two independent upstream
    // sources can each emit back-to-back duplicates of the same
    // notification: the file watcher can report more than one event for a
    // single write, and the pattern engine can match more than one rule
    // against the same line. Both funnel through this one field, so a
    // single check here covers both cases without touching either source.
    last_notification: Option<(Type, String, String, String, std::time::Instant)>,
}

impl Default for TelescopeApp {
    #[tracing::instrument]
    fn default() -> Self {
        let mut settings = Settings::default();
        if settings.get_settings().exists() {
            settings = Settings::try_from(settings.get_settings().to_path_buf()).unwrap_or_default()
        }

        let _ = settings.save();

        // generic message handler
        let (gtx, grx) = mpsc::channel::<messages::Message>(40);
        // map synchronization handler
        let (mtx, mrx) = broadcast::channel::<messages::MapSync>(30);
        // Wrapped in an Arc immediately (rather than after the sde/esi
        // setup below, as before) so it can be handed to
        // `database_updater::DatabaseUpdater::spawn` here at startup.
        let arc_msg_sender = Arc::new(gtx);

        let app_data = AppData::new();
        let esi = webb::esi::EsiManager::new(
            app_data.user_agent.as_str(),
            app_data.client_id,
            app_data.secret_key,
            app_data.url.as_str(),
            app_data.scope,
            settings.get_db(),
        );

        // `sde.get_fingerprint()` is a local, synchronous check (no
        // network): `Ok(Some((_, true)))` means `sde.db` exists and its
        // `sdeFingerprint` row's hash still matches its content, so it's
        // safe to load right away. Anything else -- the file doesn't
        // exist yet, it predates the `sdeFingerprint` table, or the row
        // doesn't match (hand-edited, or a build that never finished
        // writing it) -- leaves `universe` at its empty default rather
        // than loading data that might not be trustworthy; `universe`
        // gets populated once `DatabaseUpdater` (spawned unconditionally
        // below) finishes building a fresh database, via
        // `Message::DatabaseUpdated`/`Self::handle_database_updated`.
        //
        // This is deliberately independent of the background check
        // below: a self-consistent database can still be for an old SDE
        // build, which `get_fingerprint` alone can't tell -- that's what
        // `DatabaseUpdater`'s `sde_index::update_as_needed` call (which
        // does need the network) is for.
        // `SdeManager::new` now returns a `Result`: it errs when `sde.db`
        // doesn't exist yet or isn't a valid SQLite database (e.g. on a
        // first run, before `DatabaseUpdater` below has built it). In
        // that case `universe` simply stays at its empty default, same
        // as before this became fallible.
        let mut universe = Universe::new(settings.get_factor());
        match SdeManager::new(settings.get_sde(), settings.get_factor()) {
            Ok(mut sde) => {
                if let Ok(Some((_fingerprint, true))) = sde.get_fingerprint() {
                    let _ = sde.get_universe();
                    universe = sde.universe;
                }
            }
            Err(error) => {
                tracing::warn!("SdeManager::new failed at startup: {error}");
                // Also surface it in the on-screen log (bottom panel) --
                // `tracing::warn!` alone only reaches the log
                // file/console, and this is exactly the kind of thing
                // (first run, missing/corrupted sde.db) a user watching
                // the app start up should see, not just find in a log
                // later. `DatabaseUpdater::spawn` below is what actually
                // handles it (builds a fresh database in the background).
                let _ = try_send_app_message(
                    &arc_msg_sender,
                    Message::GenericNotification((
                        Type::Warning,
                        String::from("TelescopeApp"),
                        String::from("default"),
                        format!(
                            "SDE database not found or invalid ({error}); a fresh one will be built in the background."
                        ),
                    )),
                );
            }
        }
        // Checks CCP's SDE index in the background and (re)builds
        // `sde.db` if it's missing or a newer build is available. Never
        // blocks startup -- see `database_updater`'s module docs for why
        // the old stub here (a synchronous call to a nonexistent
        // `eframe::run_ui_native`) was replaced.
        let sde_cache_dir = Self::sde_build_cache_dir(&settings);
        database_updater::DatabaseUpdater::spawn(
            settings.get_sde().to_path_buf(),
            sde_cache_dir.join("data"),
            sde_cache_dir.join("sde"),
            Arc::clone(&arc_msg_sender),
            true,
            settings.get_data_source_urls().clone(),
        );
        let arc_map_sender = Arc::new(mtx);
        let msgmon = Arc::new(MessageSpawner::new(Arc::clone(&arc_msg_sender)));
        let authmon = AuthSpawner::new(Arc::clone(&arc_msg_sender));
        // Built here, as its own `let`, rather than inline in the `Self`
        // literal below: `task_msg: msgmon` there moves `msgmon` out, so
        // anything else in that same literal that still needs a clone of
        // it (this, and `behavior`'s `TreeBehavior::new`) has to grab one
        // before that move happens.
        let audio = audio::AlarmPlayer::new(Arc::clone(&msgmon));

        // Compile the pattern matching engine once at startup. Rules that
        // fail validation are reported and skipped; a missing or corrupted
        // patterns.toml is regenerated from the embedded template.
        let pattern_report = PatternEngine::load_or_create(Path::new("patterns.toml"));
        for error in pattern_report.errors {
            msgmon.spawn(Message::GenericNotification((
                Type::Error,
                String::from("PatternEngine"),
                String::from("load"),
                error.to_string(),
            )));
        }
        if pattern_report.regenerated {
            let detail = match &pattern_report.backup {
                Some(backup_path) => format!(
                    "patterns.toml was corrupted and has been regenerated with default content; previous file backed up as {}",
                    backup_path.display()
                ),
                None => String::from(
                    "patterns.toml was missing and has been created with default content",
                ),
            };
            msgmon.spawn(Message::GenericNotification((
                Type::Info,
                String::from("PatternEngine"),
                String::from("load_or_create"),
                detail,
            )));
        }
        let pattern_engine = pattern_report.engine;

        let intel_channels: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(
            (*settings.get_cloned_monitored_channels()).clone(),
        ));
        let intel_event_handler =
            IntelEventHandler::new(Arc::clone(&intel_channels), Arc::clone(&arc_msg_sender));
        let mut watcher = RecommendedWatcher::new(intel_event_handler, Config::default()).unwrap();
        let mut dlg_intel_dir = Dialog::default();
        dlg_intel_dir.dialog_type = DialogType::Directory;

        if settings.get_intel().exists() {
            dlg_intel_dir.set_directory(settings.get_intel());
            if !settings.get_cloned_monitored_channels().is_empty() {
                watcher
                    .watch(settings.get_intel(), RecursiveMode::NonRecursive)
                    .expect("Error monitoring intel file path");
            }
        }

        Self {
            // Example stuff:
            initialized: false,
            app_msg: (arc_msg_sender, grx),
            map_msg: (arc_map_sender, mrx),
            char_msg: None,
            open: [false; 3],
            esi,
            app_messages: Vec::new(),
            search_text: String::new(),
            search_selected_row: None,
            emit_notification: false,
            behavior: TreeBehavior::new(
                Arc::clone(&msgmon),
                settings.get_factor(),
                settings.get_sde().to_path_buf(),
            ),
            search_results: Vec::new(),
            debug: windows::debug::DebugState::default(),
            tree: None,
            universe,
            selected_settings_page: SettingsPage::Intelligence,
            task_msg: msgmon,
            task_auth: authmon,
            settings,
            watcher,
            intel_channels,
            dlg_intel_dir,
            pattern_engine,
            audio,
            database_updater: database_updater::DatabaseUpdater::default(),
            last_notification: None,
        }
    }
}

impl eframe::App for TelescopeApp {
    /// Called by the frame work to save state before shutdown.
    /*fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }*/
    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    #[tracing::instrument(skip_all)]
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let Self {
            initialized: _,
            app_msg: _,
            map_msg: _,
            char_msg: _,
            open: _,
            esi: _,
            app_messages: _,
            search_text: _,
            emit_notification: _,
            search_selected_row: _,
            search_results: _,
            debug: _,
            tree: _,
            universe: _,
            selected_settings_page: _,
            behavior: _,
            task_msg: _,
            task_auth: _,
            settings: _,
            watcher: _,
            intel_channels: _,
            dlg_intel_dir: _,
            pattern_engine: _,
            audio: _,
            database_updater: _,
            last_notification: _,
        } = self;

        if !self.initialized {
            let _span = tracing::info_span!("telescope_init").entered();

            egui_extras::install_image_loaders(ui.ctx());
            // Wake the UI when a dependency logs a warning/error, so it
            // shows up in the log panel right away (see `log_bridge`).
            crate::log_bridge::set_repaint_context(ui.ctx());

            self.tree = Some(self.create_tree());
            let mut vec_chars = Vec::new();
            for pchar in self.esi.characters.iter() {
                vec_chars.push((pchar.id, pchar.photo.as_ref().unwrap().clone()));
            }

            let regions: Vec<u32> = self
                .universe
                .regions
                .keys()
                .copied()
                .filter(|val| val < &11000000)
                .collect();

            for key in &regions {
                let region = self.universe.regions.get(key).unwrap();
                self.behavior.tile_data.insert(
                    region.id as usize,
                    TileData::new(region.name.clone(), false),
                );
            }

            let startup_regions = self.settings.get_startup_regions().clone();
            for region in startup_regions {
                if regions.contains(&(region as u32)) {
                    self.behavior
                        .tile_data
                        .entry(region)
                        .and_modify(|z_region| {
                            z_region.show_on_startup = true;
                        });
                    self.create_new_regional_pane(region);
                }
            }

            self.report_player_database_status();
            if !self.esi.characters.is_empty() {
                let mut ids = vec![];
                for char in &self.esi.characters {
                    ids.push(char.id as usize);
                }
                self.start_watchdog(ids);
            }

            self.initialized = true;
            let app_msg_sender = Arc::clone(&self.app_msg.0);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let _ = send_app_message(&app_msg_sender, Message::ScanIntelFiles).await;
            });
        }

        self.event_manager();
        // Examples of how to create different panels and windows.
        // Pick whichever suits you.
        // Tip: a good default choice is to just keep the `CentralPanel`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        #[cfg(not(target_arch = "wasm32"))] // no top panel on web pages
        egui::Panel::top("top_panel").show(ui, |ui| {
            //egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(t!("menu.file"), |ui| {
                    if ui.button(t!("menu.preferences")).clicked() {
                        self.open[2] = true;
                    }
                    if ui.button(t!("menu.debug")).clicked() {
                        self.open[1] = true;
                    }
                    ui.separator();
                    if ui.button(t!("menu.quit")).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button(t!("menu.help"), |ui| {
                    if ui.button(t!("menu.about")).clicked() {
                        self.open[0] = true;
                    }
                });
            });
        });

        // Status log (collapsible), docked at the bottom.
        self.show_log_panel(ui);

        if self.open[0] {
            self.open_about_window(ui.ctx());
        }

        // Debug menu
        if self.open[1] {
            self.open_debug_menu(ui.ctx());
        }

        if self.open[2] {
            self.open_settings_window(ui.ctx());
        }

        self.database_updater.show(ui.ctx());

        egui::CentralPanel::default().show(ui, |ui| {
            let _span = tracing::info_span!("inserting map").entered();
            if let Some(tree) = &mut self.tree {
                // The log is its own bottom panel now: the maps get all the
                // remaining height.
                tree.set_height(ui.available_height());
                tree.ui(&mut self.behavior, ui);
            }
        });
        tracing::info!(tracy.frame_mark = true);
    }
}

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        // Warnings/errors that dependencies only reported through
        // `tracing`/`log` (see `log_bridge`), shown like any notification.
        for record in crate::log_bridge::drain() {
            let kind = if record.is_error {
                Type::Error
            } else {
                Type::Warning
            };
            let text = if record.is_error {
                record.text
            } else {
                // The log panel prints source/context only for errors.
                format!("{}: {}", record.source, record.text)
            };
            self.update_status_with_error((kind, record.source, record.context, text));
        }
        while let Ok(message) = self.app_msg.1.try_recv() {
            let _span =
                tracing::info_span!("dispatch app message", kind = message.kind()).entered();
            match message {
                Message::CharacterAuthenticated(linked) => {
                    self.handle_character_authenticated(*linked)
                }
                Message::GenericNotification(message) => self.update_status_with_error(message),
                Message::MapHidden(region_id) => self.hide_abstract_map(region_id),
                Message::NewRegionalPane(region_id) => self.create_new_regional_pane(region_id),
                Message::MapShown(region_id) => self.show_abstract_map(region_id),
                Message::PlayerNewLocation((player_id, solar_system_id)) => {
                    self.update_player_location(player_id, solar_system_id)
                }
                Message::IntelFileChanged(file_name) => {
                    self.load_intel_file(file_name);
                }
                Message::UpdateIntelDirectory(directory_path) => {
                    match self.settings.set_intel(directory_path.as_path()) {
                        Ok(()) => {
                            if let Err(e) = self.settings.scan_channels_logs() {
                                self.notify_intel_error("UpdateIntelDirectory", e);
                            }
                        }
                        Err(e) => self.notify_intel_error("UpdateIntelDirectory", e),
                    }
                }
                Message::DatabaseUpdateProgress(status) => {
                    self.database_updater.set_status(status);
                }
                Message::DatabaseUpdated(rebuilt) => {
                    self.database_updater.hide();
                    if rebuilt {
                        self.handle_database_updated();
                    }
                }
                Message::DefaultIntelDirectory => {
                    if let Some(os_dirs) = directories::BaseDirs::new() {
                        let tpath = os_dirs
                            .home_dir()
                            .join("Documents")
                            .join("EVE")
                            .join("logs")
                            .join("ChatLogs");
                        match self.settings.set_intel(tpath.as_path()) {
                            Ok(()) => {
                                if let Err(e) = self.settings.scan_channels_logs() {
                                    self.notify_intel_error("DefaultIntelDirectory", e);
                                }
                            }
                            Err(e) => self.notify_intel_error("DefaultIntelDirectory", e),
                        }
                    }
                }
                Message::ScanIntelFiles => {
                    let _ = self.settings.scan_channels_logs();
                }
            };
        }
    }

    /// Removes an unlinked character's marker from every pane.
    pub(crate) fn remove_player_marker(&mut self, player_id: i32) {
        if let Some(tree) = self.tree.as_mut() {
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Pane(pane) = tile {
                    pane.remove_marker(player_id as usize);
                }
            }
        }
    }

    /// Puts every linked character's last known location on a new pane, so
    /// it doesn't wait for the character to move to show its marker.
    fn seed_player_markers(&self, pane: &mut dyn TabPane) {
        for character in &self.esi.characters {
            if character.location > 0 {
                pane.update_marker(
                    character.id as usize,
                    character.location as usize,
                    &character.name,
                );
            }
        }
    }

    #[tracing::instrument(skip(self))]
    fn create_new_regional_pane(&mut self, region_id: usize) {
        let mut pane = Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.get_sde().to_path_buf(),
            self.settings.get_region_factor(),
            Some(region_id),
            Arc::clone(&self.task_msg),
            self.settings.get_node_style(),
        );
        self.seed_player_markers(pane.as_mut());
        let tile_id = self.tree.as_mut().unwrap().tiles.insert_pane(pane);
        let root = self.tree.as_ref().unwrap().root.unwrap();
        let counter = self.tree.as_ref().unwrap().tiles.len();
        self.tree
            .as_mut()
            .unwrap()
            .move_tile_to_container(tile_id, root, counter, false);
        self.behavior.tile_data.entry(region_id).and_modify(|data| {
            data.set_visible(true);
            data.set_tile_id(Some(tile_id));
        });
    }

    #[tracing::instrument(skip(self))]
    fn show_abstract_map(&mut self, region_id: usize) {
        self.behavior
            .tile_data
            .entry(region_id)
            .and_modify(|region| {
                self.tree
                    .as_mut()
                    .unwrap()
                    .set_visible(region.get_tile_id().unwrap(), true);
                region.set_visible(true);
            });
    }

    /// Called once before the first frame.
    #[tracing::instrument(skip(cc))]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        cc.egui_ctx.set_fonts(Self::font_definitions());
        let app: TelescopeApp = Default::default();
        crate::i18n::apply_language(&app.settings.get_ui_state().language);
        app
    }

    /// The fonts Telescope draws with: egui's defaults, then Noto Sans CJK as
    /// the fallback of the proportional and monospace families, and Fira Sans
    /// Bold as the `Custom` family (map labels).
    ///
    /// Noto Sans CJK is always included, whatever the interface language:
    /// intel channels carry lines in Chinese, Japanese, Korean and Russian
    /// even when the interface is in English, and without the fallback those
    /// lines are drawn as empty boxes. It covers Latin, Cyrillic, Greek,
    /// kana, Hangul and Han; egui's own fonts go first so Latin text keeps
    /// its look.
    ///
    /// `NotoSansCJK-Medium.ttc` holds the same glyphs five times, with the
    /// Han characters shaped for each region (JP, KR, SC, TC, HK);
    /// [`CJK_FONT_INDEX`] picks the Simplified Chinese one.
    pub(crate) fn font_definitions() -> eframe::egui::FontDefinitions {
        use eframe::egui::{FontData, FontDefinitions, FontFamily};

        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            CJK_FONT.to_owned(),
            Arc::new(FontData {
                index: CJK_FONT_INDEX,
                ..FontData::from_static(include_bytes!("../../../assets/NotoSansCJK-Medium.ttc"))
            }),
        );
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push(CJK_FONT.to_owned());
        }

        fonts.font_data.insert(
            "Fira Sans Bold".to_owned(),
            Arc::new(FontData::from_static(include_bytes!(
                "../../../assets/FiraSans-Bold.ttf"
            ))),
        );
        fonts.families.insert(
            FontFamily::Name("Custom".into()),
            vec!["Fira Sans Bold".to_owned()],
        );
        fonts
    }

    #[tracing::instrument(skip(receiver, task_msg))]
    fn generate_pane(
        receiver: BCReceiver<MapSync>,
        path: PathBuf,
        factor: f64,
        region_id: Option<usize>,
        task_msg: Arc<MessageSpawner>,
        node_style: crate::app::settings::NodeStyle,
    ) -> Box<dyn TabPane> {
        let pane: Box<dyn TabPane> = if let Some(region) = region_id {
            Box::new(RegionPane::new(
                receiver, path, factor, region, task_msg, node_style,
            ))
        } else {
            Box::new(UniversePane::new(receiver, path, factor, task_msg))
        };
        pane
    }

    #[tracing::instrument(skip(self))]
    fn hide_abstract_map(&mut self, region_id: usize) {
        if let Some(tile_id) = self
            .behavior
            .tile_data
            .get(&region_id)
            .unwrap()
            .get_tile_id()
        {
            self.tree.as_mut().unwrap().tiles.toggle_visibility(tile_id);
            self.behavior
                .tile_data
                .entry(region_id)
                .and_modify(|entry| {
                    entry.set_visible(false);
                });
        }
    }

    #[tracing::instrument(skip(self))]
    fn create_tree(&self) -> Tree<Box<dyn TabPane>> {
        let mut tiles = Tiles::default();
        let mut pane = Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.get_sde().to_path_buf(),
            self.settings.get_factor(),
            None,
            Arc::clone(&self.task_msg),
            self.settings.get_node_style(),
        );
        self.seed_player_markers(pane.as_mut());
        let id = tiles.insert_pane(pane);
        let tile_ids = vec![id];
        let root = tiles.insert_tab_tile(tile_ids);
        egui_tiles::Tree::new("maps", root, tiles)
    }

    #[tracing::instrument(skip(self))]
    fn update_player_location(&mut self, player_id: i32, solar_system_id: i32) {
        let name = self
            .esi
            .characters
            .iter()
            .find(|character| character.id == player_id)
            .map(|character| character.name.clone())
            .unwrap_or_default();
        // Straight to every pane (visible or not) instead of through the
        // `MapSync` broadcast: panes only drain that channel while they are
        // drawn, so a hidden tab fell behind, lost the one-off location
        // message once the channel lagged, and the marker only showed up
        // after a restart.
        if let Some(tree) = self.tree.as_mut() {
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Pane(pane) = tile {
                    pane.update_marker(player_id as usize, solar_system_id as usize, &name);
                }
            }
        }
        for index in 0..self.esi.characters.len() {
            if self.esi.characters[index].id == player_id {
                self.esi.characters[index].location = solar_system_id;
                let char = &mut self.esi.characters[index].clone();
                if let Ok(_a) = self.esi.write_character(char) {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Debug,
                        String::from("Telescope App"),
                        String::from("update_player_location"),
                        String::from("Player location updated"),
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod font_tests {
    use super::*;
    use eframe::egui::{Color32, FontId};

    /// One line of each script intel channels carry besides Latin.
    const SAMPLES: [&str; 5] = [
        "有萨沙甲亢的配置吗", // Simplified Chinese
        "有薩沙甲亢的配置嗎", // Traditional Chinese
        "ジタ クリア です",   // Japanese
        "적 함대 발견",       // Korean
        "Нейтрал в системе",  // Russian
    ];

    /// Whether every glyph of `text` is in the fonts and gets drawn (a glyph
    /// egui can't read from the font file is laid out with nothing in the
    /// atlas).
    fn draws(ctx: &egui::Context, text: &str) -> bool {
        let font_id = FontId::proportional(14.0);
        ctx.fonts_mut(|fonts| {
            if !fonts.has_glyphs(&font_id, text) {
                return false;
            }
            let galley = fonts.layout_no_wrap(text.to_owned(), font_id, Color32::WHITE);
            let drawn = galley
                .rows
                .iter()
                .flat_map(|row| row.glyphs.iter())
                .filter(|glyph| !glyph.chr.is_whitespace() && !glyph.uv_rect.is_nothing())
                .count();
            drawn == text.chars().filter(|c| !c.is_whitespace()).count()
        })
    }

    fn context_with(fonts: egui::FontDefinitions) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        // Fonts set with `set_fonts` are loaded at the start of the next pass.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        ctx
    }

    /// The atlas region (top left and bottom right corners) of the one glyph
    /// `text` is laid out with.
    fn glyph_uv(ctx: &egui::Context, text: &str) -> ([u16; 2], [u16; 2]) {
        ctx.fonts_mut(|fonts| {
            let galley =
                fonts.layout_no_wrap(text.to_owned(), FontId::proportional(14.0), Color32::WHITE);
            let uv = galley.rows[0].glyphs[0].uv_rect;
            (uv.min, uv.max)
        })
    }

    // `draws` can't be used for the icon: egui reports every glyph of the font
    // holding its replacement glyph (NotoEmoji, `◻`) as missing, emoji
    // included. Instead, check the icon isn't painted as that `◻`.
    #[test]
    fn tooltip_icons_are_drawn() {
        let ctx = context_with(TelescopeApp::font_definitions());
        for text in [
            tiles::CHARACTER_ICON,
            sputnik::map_alerts::ALERT_ICON,
            sputnik::map_alerts::CLEAR_ICON,
        ] {
            let icon = glyph_uv(&ctx, text);
            assert_ne!(icon.0, icon.1, "{text}");
            assert_ne!(icon, glyph_uv(&ctx, "◻"), "{text}");
        }
        // A code point no font has, to show the check tells them apart.
        assert_eq!(glyph_uv(&ctx, "\u{10FFFD}"), glyph_uv(&ctx, "◻"));
    }

    #[test]
    fn intel_lines_in_every_script_are_drawn() {
        let ctx = context_with(TelescopeApp::font_definitions());
        for sample in SAMPLES {
            assert!(draws(&ctx, sample), "{sample}");
        }
    }

    // Guards the test above: egui's own fonts can't draw these lines, so it
    // really is Noto Sans CJK drawing them.
    #[test]
    fn egui_default_fonts_do_not_draw_cjk() {
        let ctx = context_with(egui::FontDefinitions::default());
        assert!(!draws(&ctx, SAMPLES[0]));
    }
}
