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
use eframe::egui::{self, Margin, epaint::text::LayoutJob};
use egui_tiles::{Tiles, Tree};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use patterns::PatternEngine;
use sde::{SdeManager, objects::Universe};
use settings::Settings;
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

mod data;
mod database;
mod database_updater;
mod file;
mod intel;
mod messages;
mod notifications;
pub mod patterns;
mod persistence;
mod settings;
mod tiles;
mod watchdog;
mod windows;

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
    // Capped at `MAX_APP_MESSAGES` by `update_status_with_error` -- see that
    // constant's doc comment.
    app_messages: Vec<LayoutJob>,
    search_text: String,
    emit_notification: bool,
    search_selected_row: Option<usize>,
    search_results: Vec<(isize, String, isize, String)>,
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
    // UI state for the "updating the SDE database" progress window --
    // see `database_updater`'s module docs.
    database_updater: database_updater::DatabaseUpdater,
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
        );
        let arc_map_sender = Arc::new(mtx);
        let msgmon = Arc::new(MessageSpawner::new(Arc::clone(&arc_msg_sender)));
        let authmon = AuthSpawner::new(Arc::clone(&arc_msg_sender));

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
            database_updater: database_updater::DatabaseUpdater::default(),
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
            database_updater: _,
        } = self;

        if !self.initialized {
            let _span = tracing::info_span!("telescope_init").entered();

            egui_extras::install_image_loaders(ui.ctx());

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
                ui.menu_button("File", |ui| {
                    if ui.button("Preferences").clicked() {
                        self.open[2] = true;
                    }
                    if ui.button("Debug").clicked() {
                        self.open[1] = true;
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Telescope").clicked() {
                        self.open[0] = true;
                    }
                });
            });
        });

        // Bottom menu
        egui::Panel::bottom("bottom_panel").show(ui, |ui| {
            //egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.separator();
            });
        });

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
                let mut rect = ui.available_size_before_wrap();
                rect.y -= 100.0;
                tree.set_height(rect.y);
                tree.ui(&mut self.behavior, ui);
            }
            let _ = egui::Frame::canvas(ui.style())
                .inner_margin(Margin::symmetric(2, 5))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(100.0)
                        .max_width(f32::INFINITY)
                        .auto_shrink(false)
                        .show_rows(
                            ui,
                            ui.text_style_height(&egui::TextStyle::Body),
                            self.app_messages.len(),
                            |ui, row_range| {
                                ui.vertical(|ui| {
                                    for index in row_range {
                                        ui.label(self.app_messages[index].clone());
                                    }
                                });
                            },
                        );
                });
        });

        //ui.add(&mut self.map);
        /*if let Some(points) = self.universe.points {

        }*/
        //ui.label("鑑於對人類家庭所有成員的固有尊嚴及其平等的和不移的權利的承認，乃是世界自由、正義與和平的基礎");
        tracing::info!(tracy.frame_mark = true);
    }
}

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        while let Ok(message) = self.app_msg.1.try_recv() {
            let _span =
                tracing::info_span!("dispatch app message", kind = message.kind()).entered();
            match message {
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character)
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

    #[tracing::instrument(skip(self))]
    fn create_new_regional_pane(&mut self, region_id: usize) {
        let pane = Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.get_sde().to_path_buf(),
            self.settings.get_region_factor(),
            Some(region_id),
            Arc::clone(&self.task_msg),
        );
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
        let mut fonts = eframe::egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Noto Sans TC".to_owned(),
            Arc::new(eframe::egui::FontData::from_static(include_bytes!(
                "../../../assets/NotoSansTC-VariableFont_wght.ttf"
            ))),
        );
        fonts
            .families
            .get_mut(&eframe::egui::FontFamily::Proportional)
            .unwrap()
            .push("Noto Sans TC".to_owned());

        let custom_family = eframe::egui::FontFamily::Name("Custom".into());
        fonts.families.insert(custom_family.clone(), Vec::new());

        fonts.font_data.insert(
            "Fira Sans Bold".to_owned(),
            Arc::new(eframe::egui::FontData::from_static(include_bytes!(
                "../../../assets/FiraSans-Bold.ttf"
            ))),
        );
        fonts
            .families
            .get_mut(&eframe::egui::FontFamily::Name("Custom".into()))
            .unwrap()
            .push("Fira Sans Bold".to_owned());

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        /*if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }*/
        cc.egui_ctx.set_fonts(fonts);
        let app: TelescopeApp = Default::default();
        app
    }

    #[tracing::instrument(skip(receiver, task_msg))]
    fn generate_pane(
        receiver: BCReceiver<MapSync>,
        path: PathBuf,
        factor: f64,
        region_id: Option<usize>,
        task_msg: Arc<MessageSpawner>,
    ) -> Box<dyn TabPane> {
        let pane: Box<dyn TabPane> = if let Some(region) = region_id {
            Box::new(RegionPane::new(receiver, path, factor, region, task_msg))
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
        let id = tiles.insert_pane(Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.get_sde().to_path_buf(),
            self.settings.get_factor(),
            None,
            Arc::clone(&self.task_msg),
        ));
        let tile_ids = vec![id];
        let root = tiles.insert_tab_tile(tile_ids);
        egui_tiles::Tree::new("maps", root, tiles)
    }

    #[tracing::instrument(skip(self))]
    fn update_player_location(&mut self, player_id: i32, solar_system_id: i32) {
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
