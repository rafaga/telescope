use crate::app::file::IntelEventHandler;
use crate::app::messages::{CharacterSync, MapSync, Message, SettingsPage, Target, Type};
use crate::app::tiles::{TabPane, TileData, TreeBehavior, UniversePane};
use chrono::Utc;
use data::AppData;
use eframe::egui::{
    self, Button, Color32, FontFamily, FontId, Margin, RichText, TextFormat, Vec2,
    epaint::text::LayoutJob,
};
use eframe::egui::{IntoAtoms, TextEdit};
use egui_extras::{Column, TableBuilder};
//use egui_file_dialog::FileDialog;
use egui_map::map::objects::*;
use egui_tiles::{Tiles, Tree};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use patterns::{ActionConfig, PatternEngine, PatternMatch};
use sde::{SdeManager, objects::Universe};
use settings::Settings;
use std::thread;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};
use tokio::time::{Duration, sleep};
use webb::esi::EsiManager;

use self::messages::{AuthSpawner, MessageSpawner};
use self::tiles::RegionPane;
use native_tools::dialog::*;

mod data;
mod database_updater;
mod file;
mod messages;
pub mod patterns;
mod settings;
mod tiles;

pub struct TelescopeApp {
    initialized: bool,

    // 2d point to paint map
    points: Vec<MapPoint>,
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
                let _ = arc_msg_sender.try_send(Message::GenericNotification((
                    Type::Warning,
                    String::from("TelescopeApp"),
                    String::from("default"),
                    format!(
                        "SDE database not found or invalid ({error}); a fresh one will be built in the background."
                    ),
                )));
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
            points: Vec::new(),
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
            points: _points,
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
    }
}

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        while let Ok(message) = self.app_msg.1.try_recv() {
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
                    if self.settings.set_intel(directory_path.as_path()).is_ok() {
                        let _ = self.settings.scan_channels_logs();
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
                        if let Err(e) = self.settings.set_intel(tpath.as_path()) {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .unwrap();
                            let app_msg_tx = Arc::clone(&self.app_msg.0);
                            runtime.block_on(async {
                                let _span =
                                    tracing::info_span!("spawned intel message data").entered();
                                let _ = app_msg_tx
                                    .send(Message::GenericNotification((
                                        Type::Error,
                                        String::from("TelescopeApp"),
                                        String::from("DefaultIntelDirectory"),
                                        e.to_string(),
                                    )))
                                    .await;
                            });
                        } else {
                            let _ = self.settings.scan_channels_logs();
                        }
                    }
                }
            };
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn open_about_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("About Telescope")
            .fixed_size((400.0, 200.0))
            .open(&mut self.open[0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../../../assets/icon.png"))
                            .fit_to_exact_size(Vec2 { x: 200.0, y: 200.0 }),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading("Telescope");
                        ui.strong("v ".to_owned() + env!("CARGO_PKG_VERSION"));
                        ui.label("Author: Rafael Amador");
                        ui.label("Licensed under MIT");
                        if ui.link("https://github.com/rafaga/telescope").clicked() {
                            let _a = open::that("https://github.com/rafaga/telescope");
                        }
                        egui::warn_if_debug_build(ui);
                    });
                });
            });
    }

    #[tracing::instrument(skip(self, ctx))]
    fn open_settings_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Settings")
        .movable(true)
        .resizable(false)
        .fixed_size([700.0,510.0])
        .movable(true)
        .open(&mut self.open[2])
        .show(ctx, |ui| {
            ui.horizontal(|ui|{
                ui.vertical(|ui|{
                    let row_height = 25.0;
                    let labels = ["Intelligence","Data Sources","Characters"];
                    ui.push_id("settings_menu", |ui|{
                        TableBuilder::new(ui)
                        .column(Column::resizable(Column::exact(150.0),false))
                        .striped(false)
                        .vscroll(false)
                        .body(|body| {
                            body.rows(row_height, labels.len(), |mut row| {
                                let label = labels[row.index()];
                                let current_page = match row.index(){
                                    0 => SettingsPage::Intelligence,
                                    1 => SettingsPage::DataSources,
                                    2 => SettingsPage::Characters,
                                    _ => SettingsPage::Characters,
                                };
                                row.col(|ui: &mut egui::Ui|{
                                    let option_selected = || -> bool {
                                        self.selected_settings_page == current_page
                                    };
                                    if ui.selectable_label(option_selected(),label).clicked() {
                                        self.selected_settings_page = current_page;
                                    };
                                });
                            });
                        });
                    });
                    ui.add_space(480.0 - (labels.len() as f32 * row_height));
                });
                ui.separator();
                ui.push_id("settings_config", |ui|{
                    ui.vertical(|ui|{
                        egui::ScrollArea::vertical().show(ui,|ui|{
                            match self.selected_settings_page {
                                // Mapping
                                SettingsPage::Intelligence => {
                                    let mut keys:Vec<usize> = self
                                        .behavior
                                        .tile_data
                                        .keys()
                                        .copied().collect();
                                    keys.sort_unstable();
                                    let num_rows = keys.len().div_ceil(3);
                                    ui.label(RichText::new("Alerts").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        let mut data = self.settings.get_warning_area();
                                        ui.label("Warn me when an enemy is within");
                                        egui::ComboBox::from_label("systems close to me")
                                            .selected_text(data.to_string())
                                            .show_ui(ui, |ui| {
                                                for i in 1u8..8 {
                                                    if ui.selectable_value(&mut data, i, i.to_string()).changed() {
                                                        self.settings.set_warning_area(i);
                                                    }
                                                }
                                            });
                                        ui.end_row();
                                    });
                                    ui.horizontal(|ui|{
                                        let enabled = true;
                                        ui.label("EVE Channel logs:");
                                        let mut str_intel = self.settings.get_intel().to_string_lossy().to_string();
                                        ui.add_enabled(enabled, TextEdit::singleline(&mut str_intel));
                                        let atoms2= ("Select").into_atoms();
                                        if ui.add_enabled(enabled, Button::new(atoms2)).clicked(){
                                            let runtime = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .unwrap();
                                                let app_msg_tx = Arc::clone(&self.app_msg.0);
                                            self.dlg_intel_dir.open_file_dialog(move |result| {
                                                if let DialogResult::Ok(path) = result {
                                                    runtime.block_on(async {
                                                        let _span = tracing::info_span!("spawned intel message data").entered();
                                                        let _ = app_msg_tx
                                                            .send(Message::UpdateIntelDirectory(path))
                                                            .await;
                                                    });
                                                }
                                            });
                                        }
                                        let atoms = ("Default").into_atoms();
                                        if ui.add_enabled(enabled, Button::new(atoms)).clicked(){
                                            let runtime = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .unwrap();
                                            let app_msg_tx = Arc::clone(&self.app_msg.0);
                                            runtime.block_on(async {
                                                let _ = app_msg_tx
                                                    .send(Message::DefaultIntelDirectory)
                                                    .await;
                                            });
                                        }
                                    });
                                    let row_height = 18.0;
                                    let mut available_channels = self.settings.get_available_channels();
                                    // clone keys to avoid borrowing available_channels while we later mutably borrow it
                                    let mut channels: Vec<String> = available_channels.keys().cloned().collect();
                                    channels.sort_unstable();
                                    ui.label(RichText::new("Monitored channels").font(FontId::proportional(20.0)));
                                    ui.label("Select all the Intel Channels to monitor.");
                                    ui.push_id("chan_tbl",|ui|{
                                        TableBuilder::new(ui)
                                        .columns(Column::resizable(Column::exact(230.0), true), 2)
                                        .striped(true)
                                        .vscroll(true)
                                        .body(|mut body|{
                                            if !channels.is_empty() {
                                                body.rows(row_height, channels.len().div_ceil(2), |mut row| {
                                                    let index = row.index() * 2;
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let key = &channels[index];
                                                        let chan = available_channels.get_mut(key).unwrap();
                                                        ui.checkbox(chan, key);
                                                    });
                                                    if index < channels.len()-1 {
                                                        row.col(|ui: &mut egui::Ui| {
                                                            let key = &channels[index + 1];
                                                            let chan = available_channels.get_mut(key).unwrap();
                                                            ui.checkbox(chan, key);
                                                        });
                                                    }
                                                });
                                            } else {
                                                body.row(row_height,|mut row|{
                                                    row.col(|ui|{
                                                        ui.label("No intel channels detected");
                                                    });
                                                });
                                            }
                                        });
                                    });
                                    self.settings.set_available_channels(available_channels);
                                    ui.label(RichText::new("Start-up maps").font(FontId::proportional(20.0)));
                                    ui.label("By default the universe map its shown, and the regional maps where do you have linked characters, but you can override this setting marking the default regional maps to show on startup.").with_new_rect(ui.available_rect_before_wrap());
                                    ui.push_id("rgn_tbl",|ui|{
                                        TableBuilder::new(ui)
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .striped(true)
                                        .vscroll(false)
                                        .body(|body| {
                                            body.rows(row_height, num_rows, |mut row| {
                                                let key_index = row.index() * 3;
                                                row.col(|ui: &mut egui::Ui| {
                                                    let region = self.behavior.tile_data.get_mut(&keys[key_index]).unwrap();
                                                    let name = region.get_name();
                                                    //let checked = &mut self.behavior.tile_data.get_mut(&region.get_id()).unwrap().show_on_startup;
                                                    ui.checkbox(&mut region.show_on_startup, name);
                                                });
                                                let mut t_key_index = key_index + 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        ui.checkbox(&mut region.show_on_startup, name);
                                                    });
                                                }
                                                t_key_index += 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        ui.checkbox(&mut region.show_on_startup, name);
                                                    });
                                                }
                                            });
                                        });
                                    });
                                },
                                // Linked Characters
                                SettingsPage::DataSources => {
                                    ui.label(RichText::new("Data Paths").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        ui.label("SDE database:");
                                        let mut str_sde = self.settings.get_sde().to_string_lossy().to_string();
                                        if ui.text_edit_singleline(&mut str_sde).changed() {
                                            let _ = self.settings.set_sde(Path::new(&str_sde));
                                        }
                                    });
                                    ui.horizontal(|ui|{
                                        ui.label("private database:");
                                        let mut str_db = self.settings.get_db().to_string_lossy().to_string();
                                        if ui.text_edit_singleline(&mut str_db).changed() {
                                            let _ = self.settings.set_db(Path::new(&str_db));
                                        }
                                    });
                                    ui.horizontal(|ui|{
                                        if ui.button("🔄 Check for SDE updates").clicked() {
                                            let sde_cache_dir = Self::sde_build_cache_dir(&self.settings);
                                            database_updater::DatabaseUpdater::spawn(
                                                self.settings.get_sde().to_path_buf(),
                                                sde_cache_dir.join("data"),
                                                sde_cache_dir.join("sde"),
                                                Arc::clone(&self.app_msg.0),
                                                false,
                                            );
                                        }
                                    });
                                },
                                SettingsPage::Characters => {
                                    ui.label(RichText::new("Linked characters").font(FontId::proportional(20.0)));
                                    ui.label("These are used to emit notifications when something it is close to your location.");
                                    ui.horizontal(|ui|{
                                        if ui.button("➕ Add").clicked() {
                                            let auth_info = self.esi.get_authorize_url().unwrap();
                                            match open::that(auth_info.url) {
                                                Ok(()) => {
                                                    self.task_auth.spawn();
                                                }
                                                Err(err) => {
                                                    self.task_msg.spawn(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("get_authorize_url"),err.to_string())));
                                                }
                                            }
                                        }
                                        let button_state = !self.esi.characters.is_empty();
                                        if ui.add_enabled(button_state, egui::Button::new("✖ Remove")).clicked() {
                                            let mut index = 0;
                                            let mut vec_id = vec![];
                                            for char in &self.esi.characters {
                                                if let Some(active_char) = self.esi.active_character {
                                                    if active_char == char.id {
                                                        if let Some(sender) = &self.char_msg {
                                                            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                                                            let _result = runtime.block_on(async{sender.send(CharacterSync::Remove(char.id as usize)).await});
                                                        }
                                                        vec_id.push(char.id);
                                                        break;
                                                    }
                                                    index += 1;
                                                }
                                            }
                                            if self.esi.active_character.is_some() {
                                                self.esi.characters.remove(index);
                                                self.esi.active_character = None;
                                                if let Err(t_error) = self.esi.remove_characters(Some(vec_id)) {
                                                    self.task_msg.spawn(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("remove_characters"),t_error.to_string())));
                                                }
                                            }
                                        }
                                    });
                                    TableBuilder::new(ui)
                                    .column(Column::exact(470.00))
                                    .striped(true)
                                    .vscroll(false)
                                    .body(|mut body| {
                                        let characters = self.esi.characters.len();
                                        if characters > 0 {
                                            body.rows(100.0, characters, |mut row| {
                                                let index = row.index();
                                                row.col(|ui|{
                                                    ui.group(|ui|{
                                                        ui.push_id(self.esi.characters[index].id, |ui| {
                                                            let inner = ui.horizontal_centered(|ui| {
                                                                if let Some(idc) =
                                                                    self.esi.active_character
                                                                    && self.esi.characters[index].id == idc {
                                                                        ui.style_mut()
                                                                            .visuals
                                                                            .override_text_color =
                                                                            Some(eframe::egui::Color32::YELLOW);
                                                                        //ui.style_mut().visuals.selection.bg_fill = Color32::LIGHT_GRAY;
                                                                        //ui.style_mut().visuals.fade_out_to_color();
                                                                    }
                                                                if let Some(player_photo) = &self.esi.characters[index].photo
                                                                {
                                                                    ui.add(
                                                                        eframe::egui::Image::new(
                                                                            player_photo.as_str(),
                                                                        )
                                                                        .fit_to_exact_size(eframe::egui::Vec2::new(
                                                                            80.0, 80.0,
                                                                        )),
                                                                    );
                                                                }
                                                                ui.vertical(|ui| {
                                                                    ui.horizontal(|ui| {
                                                                        //ui.image(char_photo, Vec2::new(16.0,16.0));
                                                                        ui.label("Name:");
                                                                        ui.label(&self.esi.characters[index].name);
                                                                    });
                                                                    ui.horizontal(|ui| {
                                                                        ui.label("Alliance:");
                                                                        if let Some(alliance) =
                                                                        self.esi.characters[index].alliance.as_ref()
                                                                        {
                                                                            ui.label(&alliance.name);
                                                                        } else {
                                                                            ui.label("No alliance");
                                                                        }
                                                                    });
                                                                    ui.horizontal(|ui| {
                                                                        ui.label("Corporation:");
                                                                        if let Some(corp) =
                                                                        self.esi.characters[index].corp.as_ref()
                                                                        {
                                                                            ui.label(&corp.name);
                                                                        } else {
                                                                            ui.label("No corporation");
                                                                        }
                                                                    });
                                                                    ui.horizontal(|ui| {
                                                                        ui.label("Last Logon:");
                                                                        ui.label(
                                                                            self.esi.characters[index].last_logon.to_string(),
                                                                        );
                                                                    });
                                                                });
                                                            });
                                                            let response = inner
                                                                .response
                                                                .interact(egui::Sense::click());
                                                            if response.clicked() {
                                                                self.esi.active_character =
                                                                    Some(self.esi.characters[index].id);
                                                            }
                                                        });
                                                    });
                                                });
                                            });
                                        } else {
                                            body.row(200.00,|mut row|{
                                                row.col(|ui|{
                                                    ui.add_sized(ui.available_size(),egui::Label::new("⚠ There is no characters currently linked in Telescope."));
                                                });
                                            });
                                        }
                                    });
                                },
                            }
                        });
                    });
                });
            });
            ui.horizontal(|ui|{
                ui.add_space(650.00);
            });
            ui.horizontal(|ui|{
                if ui.add(Button::new("Save")).clicked() {
                    //self.settings.mapping.startup_regions.clear();
                    let mut startup_regions = vec![];
                    for region in self.behavior.tile_data.iter() {
                        if region.1.show_on_startup {
                            startup_regions.push(*region.0);
                        }
                    }
                    if !startup_regions.is_empty() {
                        self.settings.set_startup_regions(startup_regions);
                    }
                    if self.settings.get_intel().exists() {
                        let mut monitored_channels = Vec::new();
                        let channels = self.settings.get_available_channels();
                        for channel_data in channels.iter() {
                            if *channel_data.1 {
                                monitored_channels.push(channel_data.0.to_string());
                            }
                        }
                        monitored_channels.sort_unstable();
                        // Push the freshly-saved selection into the live
                        // handle the running watcher's event handler reads
                        // from, so newly checked/unchecked channels take
                        // effect immediately instead of only after a
                        // restart (the watcher's `IntelEventHandler` is
                        // constructed once and can't be swapped out).
                        if let Ok(mut guard) = self.intel_channels.write() {
                            *guard = monitored_channels.clone();
                        }
                        if monitored_channels.is_empty() && self.settings.get_intel().exists() {
                            let _ = self.watcher.unwatch(self.settings.get_intel());
                        } else {
                            let _ = self.watcher.watch(self.settings.get_intel(),RecursiveMode::NonRecursive);
                        }
                        self.settings.set_monitored_channels(monitored_channels);
                    }
                    if let Err(e) = self.settings.save() {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        let app_msg_tx = Arc::clone(&self.app_msg.0);
                        runtime.block_on(async {
                            let _span = tracing::info_span!("spawned intel message data").entered();
                            let _ = app_msg_tx
                                .send(Message::GenericNotification((Type::Error,String::from("TelescopeApp"),String::from("open_settings_window"),e.to_string())))
                                .await;
                        });
                    }
                }
                if !self.settings.its_saved() {
                    ui.colored_label(Color32::YELLOW, "⚠ unsaved changes");
                }
            });
        });
    }

    #[tracing::instrument(skip(self))]
    fn load_intel_file(&mut self, file_name: String) {
        let path = self.settings.get_intel();
        let path = &path.join(file_name.as_str());
        let mut log_files_map = self.settings.get_log_files_channels();

        //getting the first byte to read from the last recorded file lenght
        let mut start = 0;
        if let Some(log_entry) = log_files_map.get(&file_name) {
            start = log_entry.0;
        }

        //the channel name is the file name prefix before the first underscore
        let channel = file_name
            .split_once('_')
            .map(|(name, _)| name.to_string())
            .unwrap_or_default();

        if let Ok(mut intel_file) = File::open(path) {
            let file_length = intel_file.metadata().map(|meta| meta.len()).unwrap_or(0);
            //if the file shrank (log rotation), read it from the beginning
            if file_length < start {
                start = 0;
            }
            if file_length > start && intel_file.seek(SeekFrom::Start(start)).is_ok() {
                // EVE Online writes chat logs as UTF-16LE, never UTF-8, so
                // this cannot use read_to_string (it requires valid UTF-8
                // and fails on the very first byte of every real log file,
                // silently, since the caller only checks `is_ok()`). Read
                // the raw bytes and decode them ourselves.
                let mut chunk = intel_file.take(file_length - start);
                let mut raw = Vec::new();
                if let Ok(bytes_read) = chunk.read_to_end(&mut raw) {
                    let (new_data, consumed) = decode_utf16le_chunk(&raw[..bytes_read]);
                    self.parse_intel_data(&channel, &new_data);
                    log_files_map.entry(file_name).and_modify(|hash_entry| {
                        hash_entry.0 = start + consumed as u64;
                        hash_entry.1 = Utc::now();
                    });
                }
            }
        }
        self.settings.set_log_files_channels(log_files_map);
    }

    #[tracing::instrument(skip(self, data))]
    fn parse_intel_data(&self, channel: &str, data: &str) {
        for intel_match in self.pattern_engine.evaluate(channel, data) {
            match &intel_match.action {
                ActionConfig::Notify => {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Info,
                        String::from("PatternEngine"),
                        intel_match.rule_id.clone(),
                        intel_match.line.to_string(),
                    )));
                }
                ActionConfig::MapAlert { system_group } => {
                    self.dispatch_map_alert(&intel_match, system_group);
                }
            }
        }
    }

    #[tracing::instrument(skip(self))]
    fn dispatch_map_alert(&self, intel_match: &PatternMatch, system_group: &str) {
        let Some(system_name) = intel_match.named.get(system_group) else {
            return;
        };
        //validate the captured text before using it in any query; real
        //solar system names are at most 17 chars and may contain spaces
        //and dashes (e.g. "Old Man Star", "Tash-Murkon Prime")
        if system_name.is_empty()
            || system_name.len() > 20
            || !system_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ')
        {
            return;
        }
        let sde = SdeManager::new(self.settings.get_sde(), self.settings.get_factor());
        match sde.and_then(|s| s.get_system_id(system_name.to_lowercase())) {
            Ok(results) => {
                //prefer an exact name match over partial (LIKE) results
                let found = results
                    .iter()
                    .find(|entry| entry.1.eq_ignore_ascii_case(system_name))
                    .or(results.first());
                if let Some(entry) = found {
                    let system_id = usize::try_from(entry.0).unwrap_or(0);
                    if system_id > 0 {
                        let _ = self.map_msg.0.send(MapSync::SystemNotification((
                            system_id,
                            tokio::time::Instant::now(),
                        )));
                    }
                }
            }
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("PatternEngine"),
                    String::from("dispatch_map_alert"),
                    t_error.to_string(),
                )));
            }
        }
    }

    #[tracing::instrument(skip(self, response_data))]
    fn update_character_into_database(&mut self, response_data: (String, String)) {
        let auth_info = self.esi.get_authorize_url().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match rt
            .as_ref()
            .expect("Esi character authentication failure")
            .block_on(self.esi.auth_user(auth_info, response_data))
        {
            Ok(Some(player)) => {
                let id = player.id as usize;
                self.esi.characters.push(player);
                if self.esi.characters.len() == 1 {
                    self.start_watchdog(vec![id]);
                } else if let Some(sender) = &self.char_msg {
                    let _result = rt
                        .as_ref()
                        .expect("Esi character authentication failure")
                        .block_on(async { sender.send(CharacterSync::Add(id)).await });
                }
            }
            Ok(None) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Info,
                    String::from("EsiManager"),
                    String::from("auth_user"),
                    String::from(
                        "Apparently there was some kind of trouble authenticating the player.",
                    ),
                )));
            }
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("EsiManager"),
                    String::from("auth_user"),
                    t_error.to_string(),
                )));
            }
        };
    }

    /// Base scratch directory for `database_updater::DatabaseUpdater`'s
    /// background update/build (see `sde::builder::update::UpdatePaths`):
    /// callers join `"data"` onto it for the downloaded zip and the
    /// small `.build` file that avoids re-downloading unchanged data,
    /// and `"sde"` for the decompressed SDE tree. Kept next to `sde.db`
    /// itself (falling back to a relative `sde-build-cache` if
    /// `Settings::get_sde()` isn't configured yet, e.g. on a first run
    /// before the user has set a path in Settings -> Data Sources) so
    /// it's obvious, on disk, what it belongs to; it isn't meant to be
    /// user-facing.
    fn sde_build_cache_dir(settings: &Settings) -> PathBuf {
        match settings.get_sde().parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("sde-build-cache"),
            _ => PathBuf::from("sde-build-cache"),
        }
    }

    /// Reloads `self.universe` from `sde.db` after
    /// `database_updater::DatabaseUpdater` has (re)built it.
    ///
    /// Note this only refreshes the in-memory `universe` data (used e.g.
    /// to look up region/system names and coordinates); every map pane
    /// (`RegionPane`/`UniversePane`, see `tiles.rs`) and the search box
    /// already open their own `SdeManager` against `settings.get_sde()`
    /// on demand, so they pick up the new database automatically. What
    /// does *not* happen live is regenerating the region *tile* list
    /// built once at startup (`create_tree`/the `self.behavior.tile_data`
    /// entries) -- a region that didn't exist in `sde.db` before this
    /// update (i.e. this was the very first successful build) won't get
    /// a tile until Telescope is restarted.
    #[tracing::instrument(skip(self))]
    fn handle_database_updated(&mut self) {
        match SdeManager::new(self.settings.get_sde(), self.settings.get_factor()) {
            Ok(mut sde) => match sde.get_universe() {
                Ok(_) => {
                    self.universe = sde.universe;
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Info,
                        String::from("TelescopeApp"),
                        String::from("handle_database_updated"),
                        String::from(
                            "SDE data reloaded. Restart Telescope to see newly available regions on the map.",
                        ),
                    )));
                }
                Err(t_error) => {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Error,
                        String::from("TelescopeApp"),
                        String::from("handle_database_updated"),
                        t_error.to_string(),
                    )));
                }
            },
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("TelescopeApp"),
                    String::from("handle_database_updated"),
                    t_error.to_string(),
                )));
            }
        }
    }

    #[tracing::instrument(skip(self, message))]
    fn update_status_with_error(&mut self, message: (Type, String, String, String)) {
        let full_time = chrono::Local::now().time().to_string();
        let time = full_time.split_at(12);
        let mut job = LayoutJob::default();
        let normal_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::LIGHT_GRAY,
            ..Default::default()
        };
        let time_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DARK_GRAY,
            ..Default::default()
        };
        let warn = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::KHAKI,
            ..Default::default()
        };
        let info = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::BLUE,
            ..Default::default()
        };
        let debug = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DEBUG_COLOR,
            ..Default::default()
        };
        let error = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::RED,
            ..Default::default()
        };
        job.append("[", 0.0, normal_text.clone());
        job.append(time.0, 0.0, time_text.clone());
        job.append("] ", 0.0, normal_text.clone());
        match message.0 {
            Type::Error => {
                job.append("ERROR: ", 0.0, error.clone());
                job.append(
                    (message.1 + " - " + &message.2 + " - ").as_str(),
                    0.0,
                    normal_text.clone(),
                );
            }
            Type::Warning => {
                job.append("WARN: ", 0.0, warn.clone());
            }
            Type::Info => {
                job.append("INFO: ", 0.0, info.clone());
            }
            Type::Debug => {
                job.append("DEBUG: ", 0.0, debug.clone());
            }
        }
        job.append(&message.3, 0.0, normal_text.clone());
        self.app_messages.push(job);
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
            .insert(0, "Noto Sans TC".to_owned());

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
    pub fn start_watchdog(&mut self, character_id: Vec<usize>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (sender, mut receiver) = mpsc::channel::<CharacterSync>(10);
        let app_sender = Arc::clone(&self.app_msg.0);
        let map_sender = Arc::clone(&self.map_msg.0);
        let mut t_esi = self.esi.clone();
        thread::spawn(move || {
            runtime.block_on(async {
                let _span = tracing::info_span!("spawned watchdog").entered();

                let mut character_ids = vec![];
                if let Err(t_error) = t_esi.update_spec().await {
                    let _ = app_sender
                        .send(Message::GenericNotification((
                            Type::Error,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            t_error.to_string(),
                        )))
                        .await;
                } else {
                    let _ = app_sender
                        .send(Message::GenericNotification((
                            Type::Info,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            String::from("Starting watchdog"),
                        )))
                        .await;
                }
                for char_id in character_id {
                    character_ids.push((char_id, 0))
                }
                while !character_ids.is_empty() {
                    if !t_esi.valid_token().await {
                        if let Err(t_error) = t_esi.refresh_token().await {
                            let _ = app_sender
                                .send(Message::GenericNotification((
                                    Type::Error,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    t_error.to_string(),
                                )))
                                .await;
                            return;
                        } else {
                            let _ = app_sender
                                .send(Message::GenericNotification((
                                    Type::Debug,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    String::from("token refreshed successfully"),
                                )))
                                .await;
                        }
                    }
                    for item in &mut character_ids {
                        //PlayerDatabase
                        match t_esi.get_location(item.0.try_into().unwrap()).await {
                            Ok(new_location) => {
                                if item.1 != (new_location as usize) {
                                    item.1 = new_location as usize;
                                    let _ = app_sender
                                        .send(Message::PlayerNewLocation((
                                            item.0.try_into().unwrap(),
                                            new_location,
                                        )))
                                        .await;
                                    if let Err(t_error) =
                                        map_sender.send(MapSync::PlayerMoved((item.0, item.1)))
                                    {
                                        let _ = app_sender
                                            .send(Message::GenericNotification((
                                                Type::Error,
                                                String::from("Telescope App"),
                                                String::from("start_watchdog"),
                                                t_error.to_string(),
                                            )))
                                            .await;
                                    }
                                }
                            }
                            Err(t_error) => {
                                let _ = app_sender
                                    .send(Message::GenericNotification((
                                        Type::Error,
                                        String::from("Telescope App"),
                                        String::from("start_watchdog - get_location - ")
                                            + item.0.to_string().as_str(),
                                        t_error.to_string(),
                                    )))
                                    .await;
                                break;
                            }
                        }
                    }
                    sleep(Duration::new(5, 0)).await;
                    while let Ok(message) = receiver.try_recv() {
                        match message {
                            CharacterSync::Add(char_data) => character_ids.push((char_data, 0)),
                            CharacterSync::Remove(char_id) => {
                                for index in 0..character_ids.len() {
                                    if character_ids[index].0 == char_id {
                                        character_ids.remove(index);
                                        break;
                                    }
                                }
                                if character_ids.is_empty() {
                                    let _ = app_sender
                                        .send(Message::GenericNotification((
                                            Type::Info,
                                            String::from("Telescope App"),
                                            String::from("start_watchdog"),
                                            String::from("Watchdog ended"),
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }
                    }
                    if let Err(TryRecvError::Disconnected) = receiver.try_recv() {
                        character_ids.clear();
                        let _ = app_sender
                            .send(Message::GenericNotification((
                                Type::Info,
                                String::from("Telescope App"),
                                String::from("start_watchdog"),
                                String::from("Watchdog ended"),
                            )))
                            .await;
                        break;
                    }
                    sleep(Duration::new(25, 0)).await;
                }
            });
        });

        self.char_msg = Some(Arc::new(sender));
    }

    #[tracing::instrument(skip(self, ctx))]
    fn open_debug_menu(&mut self, ctx: &egui::Context) {
        egui::Window::new("Debug Menu")
            .fixed_size((400.0, 600.0))
            .open(&mut self.open[1])
            .show(ctx, |ui| {
                ui.heading("Search");

                ui.horizontal(|ui| {
                    ui.label("Name: ");
                    let response = ui.text_edit_singleline(&mut self.search_text);
                    if response.changed() {
                        if self.search_text.len() >= 3 {
                            let sde = SdeManager::new(
                                self.settings.get_sde(),
                                self.settings.get_factor(),
                            );
                            match sde.and_then(|s| {
                                s.get_system_id(self.search_text.clone().to_lowercase())
                            }) {
                                Ok(system_results) => self.search_results = system_results,
                                Err(t_error) => {
                                    self.task_msg.spawn(Message::GenericNotification((
                                        Type::Error,
                                        String::from("sde"),
                                        String::from("get_system_id"),
                                        t_error.to_string(),
                                    )));
                                }
                            }
                        }
                        if self.search_text.is_empty() {
                            self.search_results.clear();
                            self.search_selected_row = None;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.emit_notification, "Notify");
                    if ui.button("Clear").clicked() {
                        self.search_text.clear();
                        self.search_results.clear();
                        self.search_selected_row = None;
                    }
                    if ui.button("Advanced >>>").clicked() {}
                });
                ui.push_id("search_table", |ui| {
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto())
                        .column(Column::remainder())
                        .min_scrolled_height(0.0);

                    table = table.sense(egui::Sense::click());
                    table
                        .header(20.0, |mut header| {
                            header.col(|ui| {
                                ui.strong("System");
                            });
                            header.col(|ui| {
                                ui.strong("Region");
                            });
                        })
                        .body(|mut body| {
                            for row_index in 0..self.search_results.len() {
                                body.row(18.00, |mut row| {
                                    row.set_selected(false);
                                    if let Some(selected_row) = self.search_selected_row
                                        && row_index == selected_row
                                    {
                                        row.set_selected(true);
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].1).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let system_id = self.search_results[row_index].0;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                system_id.try_into().unwrap(),
                                                Target::System,
                                            )));
                                            if self.emit_notification {
                                                let _result =
                                                    tx_map.send(MapSync::SystemNotification((
                                                        system_id.try_into().unwrap(),
                                                        tokio::time::Instant::now(),
                                                    )));
                                            }
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let system_id = self.search_results[row_index].0;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            system_id.try_into().unwrap(),
                                            Target::System,
                                        )));
                                        if self.emit_notification {
                                            let _result =
                                                tx_map.send(MapSync::SystemNotification((
                                                    system_id.try_into().unwrap(),
                                                    tokio::time::Instant::now(),
                                                )));
                                        }
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].3).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let region_id = self.search_results[row_index].2;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                region_id.try_into().unwrap(),
                                                Target::Region,
                                            )));
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let region_id = self.search_results[row_index].2;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            region_id.try_into().unwrap(),
                                            Target::Region,
                                        )));
                                    }
                                    if row.response().clicked() {
                                        self.search_selected_row = Some(row_index);
                                    }
                                });
                                //self.toggle_row_selection(row_index, &row.response());
                            }
                            if self.search_results.is_empty() {
                                body.row(18.00, |mut row| {
                                    row.col(|ui| {
                                        ui.label("No result(s)");
                                    });
                                    row.col(|_ui| {});
                                });
                            }
                        });
                    //
                });
            });
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

/// Decodes a raw byte chunk read from an EVE Online chat log as UTF-16LE.
///
/// EVE writes chat logs as UTF-16LE and re-emits a byte-order mark (U+FEFF)
/// not only at the start of the file but at the start of every appended
/// line (each flush is encoded as its own fragment, BOM included) -- every
/// occurrence is stripped here, not just a single leading one, since
/// [`patterns::PatternEngine::parse_line`]'s line regex is anchored on a
/// literal `[` and would otherwise fail to match every line but the first.
///
/// Returns the decoded text and the number of bytes actually consumed from
/// `raw`. If `raw`'s length is odd, the trailing byte is half of a UTF-16
/// code unit split across two reads (the writer flushed mid-character); it
/// is left unconsumed (excluded from the returned count) so the caller
/// re-reads it, paired with its other half, on the next chunk instead of
/// corrupting decoding here. Malformed code units (unpaired surrogates)
/// are replaced with U+FFFD via [`String::from_utf16_lossy`] rather than
/// failing the whole read.
fn decode_utf16le_chunk(raw: &[u8]) -> (String, usize) {
    let usable_len = raw.len() - (raw.len() % 2);
    let code_units: Vec<u16> = raw[..usable_len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let text = String::from_utf16_lossy(&code_units).replace('\u{feff}', "");
    (text, usable_len)
}

#[cfg(test)]
mod decode_tests {
    use super::decode_utf16le_chunk;

    /// Encodes `s` as raw UTF-16LE bytes, the same wire format EVE writes,
    /// without needing an encoding crate as a test dependency.
    fn utf16le_bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn decodes_plain_ascii_line() {
        let raw = utf16le_bytes("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear\r\n");
        let (text, consumed) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear\r\n"
        );
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn strips_leading_file_bom() {
        let raw = utf16le_bytes("\u{feff}[ 2021.09.08 22:56:47 ] A > hi\r\n");
        let (text, _) = decode_utf16le_chunk(&raw);
        assert_eq!(text, "[ 2021.09.08 22:56:47 ] A > hi\r\n");
    }

    #[test]
    fn strips_every_per_line_bom_not_just_the_first() {
        // Real EVE logs re-emit U+FEFF at the start of every appended
        // line, not only once at the top of the file.
        let raw = utf16le_bytes(
            "\u{feff}[ 2021.09.08 22:56:47 ] A > line one\r\n\u{feff}[ 2021.09.08 22:56:48 ] B > line two\r\n",
        );
        let (text, _) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2021.09.08 22:56:47 ] A > line one\r\n[ 2021.09.08 22:56:48 ] B > line two\r\n"
        );
        assert!(!text.contains('\u{feff}'));
    }

    #[test]
    fn decodes_non_latin_script_correctly() {
        // The same corpus this fix was validated against has a large
        // Chinese-speaking population; a naive UTF-8 read does not just
        // mis-decode this text, it fails to decode the file at all (0xFF,
        // the first byte of the UTF-16LE BOM, is never a valid UTF-8
        // start byte).
        let raw = utf16le_bytes("[ 2023.03.27 02:19:05 ] Algae Roben > 有萨沙甲亢的配置吗\r\n");
        let (text, consumed) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2023.03.27 02:19:05 ] Algae Roben > 有萨沙甲亢的配置吗\r\n"
        );
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn holds_back_a_trailing_split_code_unit() {
        let full = utf16le_bytes("[ 2021.09.08 22:56:47 ] A > hi\r\n");
        // Simulate a read landing mid-character: drop the last byte,
        // leaving a dangling first byte of the final code unit ('\n').
        let raw = &full[..full.len() - 1];
        let (text, consumed) = decode_utf16le_chunk(raw);
        // The dangling byte must not be consumed nor corrupt the decoded
        // text.
        assert_eq!(consumed, raw.len() - 1);
        assert_eq!(text, "[ 2021.09.08 22:56:47 ] A > hi\r");
    }

    #[test]
    fn empty_input_decodes_to_empty_output() {
        let (text, consumed) = decode_utf16le_chunk(&[]);
        assert_eq!(text, "");
        assert_eq!(consumed, 0);
    }
}

#[cfg(test)]
mod sde_build_cache_dir_tests {
    use super::TelescopeApp;
    use super::settings::Settings;
    use std::path::Path;

    // `Settings::default()`'s `sde` path is always non-empty now (see
    // `settings::FilePaths::default`), so this is the realistic case:
    // the cache dir sits next to `sde.db`, not off on its own.
    #[test]
    fn sits_next_to_the_configured_sde_database() {
        let settings = Settings::default();
        let cache_dir = TelescopeApp::sde_build_cache_dir(&settings);
        assert_eq!(cache_dir.file_name(), Some("sde-build-cache".as_ref()));
        assert_eq!(cache_dir.parent(), settings.get_sde().parent());
    }

    // Regression test for the empty-path case `DatabaseUpdater::run`
    // also guards against directly: an unconfigured (or pre-default,
    // loaded-from-an-old-`telescope.toml`) `sde` path has no parent
    // directory to sit next to, so this must fall back to a relative
    // path instead of panicking or joining onto nothing.
    #[test]
    fn falls_back_to_a_relative_path_when_sde_is_unconfigured() {
        let mut settings = Settings::default();
        settings.set_sde_for_test(Path::new(""));
        let cache_dir = TelescopeApp::sde_build_cache_dir(&settings);
        assert_eq!(cache_dir, Path::new("sde-build-cache"));
    }
}
