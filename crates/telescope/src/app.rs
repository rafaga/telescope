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
use crate::app::tiles::{TabPane, TreeBehavior, UniversePane};
use data::AppData;
use eframe::egui::{self, epaint::text::LayoutJob};
use egui_tiles::{Tile, Tiles, Tree};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use sde::{SdeManager, objects::Universe};
use settings::Settings;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender};
use webb::esi::EsiManager;
use webb::graph::{Executor, InputNode, Node, NodeKind, RuleGraph};
use webb::rules::InputKind;

use self::messages::{AuthSpawner, MessageSpawner};
use self::tiles::RegionPane;
use self::windows::settings::patterns::PatternsEditor;

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

/// The native id of the app's window where the file dialogs need one (the
/// `HWND` on Windows), if the platform has one.
fn native_window_id(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Capacity of the app message channel. Senders never wait on it (a full
/// channel drops the message), so it has room for bursts: every write to a
/// watched chat log is one `IntelFileChanged`.
pub(crate) const APP_MESSAGE_CAPACITY: usize = 256;

/// Capacity of the `MapSync` broadcast channel. Every pane drains it each
/// frame (see `drain_map_messages`); the room is for bursts, since each intel
/// line can send an alert and a tooltip per reported system.
const MAP_SYNC_CAPACITY: usize = 512;

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
    // Debug window only (not in release builds, see `windows.rs`).
    #[cfg(debug_assertions)]
    search_text: String,
    #[cfg(debug_assertions)]
    emit_notification: bool,
    #[cfg(debug_assertions)]
    search_selected_row: Option<usize>,
    #[cfg(debug_assertions)]
    search_results: Vec<(isize, String, isize, String)>,
    // State of the Debug window's "Advanced" section.
    #[cfg(debug_assertions)]
    debug: windows::debug::DebugState,
    universe: Universe,
    selected_settings_page: SettingsPage,
    tree: Option<Tree<Box<dyn TabPane>>>,

    behavior: TreeBehavior,
    task_msg: Arc<MessageSpawner>,
    task_auth: AuthSpawner,
    settings: Settings,
    /// Snapshot of `settings` taken when the Settings screen opens, so
    /// Cancel can revert every change made in the session.
    settings_snapshot: Option<Settings>,
    watcher: RecommendedWatcher,
    // Live-shared handle for the set of channels the file watcher's event
    // handler filters on. `IntelEventHandler` is moved into `watcher` at
    // construction time and can never be swapped out afterwards, so the
    // monitored-channel list it filters on has to be reachable through
    // shared, interior-mutable storage; otherwise every channel selected or
    // saved after startup is silently ignored until the app restarts.
    intel_channels: Arc<RwLock<Vec<String>>>,
    /// The chat log folder the watcher watches, if any. It changes only when
    /// the settings are applied, while the folder in `settings` can be a
    /// draft of the Settings screen: logs are read from this one.
    intel_watched: intel::reader::WatchedDir,
    /// Live graph executor, shared with the detection thread.
    intel_executor: intel::detection::ExecutorHandle,
    /// System resolver injected into the executor (backed by the SDE).
    intel_resolver: intel::detection::ResolverHandle,
    /// The graph the executor was built from (source of truth for the editor).
    intel_graph: RuleGraph,
    /// Where the reading of each monitored chat log stopped; shared with the
    /// reader thread (`intel::reader`).
    intel_offsets: intel::reader::SharedOffsets,
    /// What the dispatch thread reads (alarm settings, character locations,
    /// stargate graph); refreshed by `sync_alarm_shared`.
    alarm_shared: Arc<intel::dispatch::AlarmShared>,
    /// In-memory state of the Settings -> Rules page (rules being edited).
    patterns_editor: PatternsEditor,
    /// View state of the other Settings pages (filters, dialogs).
    settings_ui: windows::settings::SettingsUi,
    /// The "Third-party licenses" window, while it is open.
    licenses: Option<windows::licenses::LicensesWindow>,
    // Alarm sound for the intel rules' Sound output -- see the
    // `audio` module docs for why this has to be a long-lived field
    // rather than something opened per alert.
    audio: Arc<audio::AudioHandle>,
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
        let (gtx, grx) = mpsc::channel::<messages::Message>(APP_MESSAGE_CAPACITY);
        // map synchronization handler
        let (mtx, mrx) = broadcast::channel::<messages::MapSync>(MAP_SYNC_CAPACITY);
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
        // `Message::DatabaseUpdated`/`Self::reload_sde`.
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
        let audio = Arc::new(audio::AudioHandle::spawn(Arc::clone(&msgmon)));

        // Load the intel node graph from the player database (seeded with the
        // built-in default graph, `rules.toml`, by the schema migration),
        // compile the executor once, and start the detection thread that
        // evaluates the lines the watcher reports.
        let mut intel_graph = match esi.load_graph() {
            Ok(graph) if !graph.nodes.is_empty() => graph,
            // No rules yet (fresh or emptied database): fall back to the
            // built-in default graph.
            Ok(_) => RuleGraph::default_graph(),
            Err(error) => {
                msgmon.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("Intel"),
                    String::from("load_graph"),
                    error.to_string(),
                )));
                RuleGraph::default_graph()
            }
        };
        // The Patterns page lists input nodes, so there must always be at
        // least one. Create the chat-log input if none exists (and fill its
        // path, left empty by the migration, from the settings).
        let intel_dir = settings.get_intel().display().to_string();
        let has_input = intel_graph
            .nodes
            .iter()
            .any(|node| matches!(node.kind, NodeKind::Input(_)));
        if !has_input {
            intel_graph.nodes.push(Node {
                id: String::from("chat_logs"),
                enabled: true,
                x: 0.0,
                y: 0.0,
                kind: NodeKind::Input(InputNode {
                    description: String::from("chat logs"),
                    kind: InputKind::ChatLog,
                    path: intel_dir.clone(),
                    channels: (*settings.get_cloned_monitored_channels()).clone(),
                    exclude_motd: true,
                }),
            });
        } else if let Some(Node {
            kind: NodeKind::Input(input),
            ..
        }) = intel_graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "chat_logs")
            && input.path.is_empty()
        {
            input.path = intel_dir;
        }
        let intel_resolver: intel::detection::ResolverHandle =
            Arc::new(intel::resolve::SharedResolver::new(&universe));
        let (executor, errors) = Executor::new(intel_graph.clone());
        for error in errors {
            msgmon.spawn(Message::GenericNotification((
                Type::Error,
                String::from("Intel"),
                String::from("load_graph"),
                error.to_string(),
            )));
        }
        let intel_executor: intel::detection::ExecutorHandle = Arc::new(RwLock::new(executor));
        let (intel_input, intel_input_rx) =
            mpsc::channel::<intel::input::InputEvent>(intel::detection::INPUT_CAPACITY);
        let (intel_output_tx, intel_output_rx) =
            mpsc::channel::<intel::detection::DetectedLine>(intel::detection::OUTPUT_CAPACITY);
        intel::detection::spawn(
            Arc::clone(&intel_executor),
            Arc::clone(&intel_resolver),
            intel_input_rx,
            intel_output_tx,
        );
        // Detection -> dispatch (map messages, alarm sound, status log): its
        // own thread, so alarms don't wait for the UI loop.
        let alarm_shared = Arc::new(intel::dispatch::AlarmShared::default());
        *alarm_shared
            .jumps
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            intel::resolve::jump_graph(&universe);
        intel::dispatch::spawn(
            intel::dispatch::Dispatcher {
                shared: Arc::clone(&alarm_shared),
                audio: Arc::clone(&audio),
                map_msg: Arc::clone(&arc_map_sender),
                task_msg: Arc::clone(&msgmon),
            },
            intel_output_rx,
        );

        // Everything already in the monitored logs is history: start at
        // their current end.
        let mut offsets = intel::input::IntelOffsets::default();
        offsets.sync(
            settings.get_intel(),
            &settings.get_cloned_monitored_channels(),
        );
        let intel_offsets: intel::reader::SharedOffsets = Arc::new(Mutex::new(offsets));
        let intel_watched: intel::reader::WatchedDir = Arc::new(RwLock::new(None));
        // Watcher -> reader thread -> detection.
        let (intel_files_tx, intel_files_rx) = std::sync::mpsc::channel::<String>();
        intel::reader::spawn(
            intel::reader::ReaderShared {
                watched: Arc::clone(&intel_watched),
                offsets: Arc::clone(&intel_offsets),
                input: intel_input,
                app_msg: Arc::clone(&arc_msg_sender),
            },
            intel_files_rx,
        );

        // Sorted: the watcher's event handler binary-searches it, and a
        // hand-edited `telescope.toml` may list the channels in any order.
        let mut monitored = (*settings.get_cloned_monitored_channels()).clone();
        monitored.sort_unstable();
        let intel_channels: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(monitored));
        let intel_event_handler = IntelEventHandler::new(
            Arc::clone(&intel_channels),
            Arc::clone(&arc_msg_sender),
            intel_files_tx,
        );
        let mut watcher = RecommendedWatcher::new(intel_event_handler, Config::default()).unwrap();
        if settings.get_intel().exists() && !settings.get_cloned_monitored_channels().is_empty() {
            match watcher.watch(settings.get_intel(), RecursiveMode::NonRecursive) {
                Ok(()) => {
                    *intel_watched
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(settings.get_intel().to_path_buf())
                }
                // Reported, not fatal: the maps work without intel, and
                // applying the Sources page tries again.
                Err(error) => msgmon.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("TelescopeApp"),
                    String::from("watch intel folder"),
                    error.to_string(),
                ))),
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
            #[cfg(debug_assertions)]
            search_text: String::new(),
            #[cfg(debug_assertions)]
            search_selected_row: None,
            #[cfg(debug_assertions)]
            emit_notification: false,
            behavior: TreeBehavior::new(
                Arc::clone(&msgmon),
                settings.get_factor(),
                settings.get_sde().to_path_buf(),
            ),
            #[cfg(debug_assertions)]
            search_results: Vec::new(),
            #[cfg(debug_assertions)]
            debug: windows::debug::DebugState::default(),
            tree: None,
            universe,
            selected_settings_page: SettingsPage::Sources,
            task_msg: msgmon,
            task_auth: authmon,
            settings,
            settings_snapshot: None,
            watcher,
            intel_channels,
            intel_watched,
            intel_executor,
            intel_resolver,
            intel_graph,
            intel_offsets,
            alarm_shared,
            patterns_editor: PatternsEditor::default(),
            settings_ui: windows::settings::SettingsUi::default(),
            licenses: None,
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
    /// The work that must go on while the window is minimized or hidden.
    ///
    /// eframe runs no egui pass then, so `ui()` is never called -- and
    /// everything the intel pipeline does on this thread lives in
    /// [`Self::event_manager`]: reading the chat log that changed, handing its
    /// lines to the detection thread, dispatching what the rules matched
    /// (alarm sound, map pulses, tooltip entries, the status log), and the
    /// characters' locations the alarm radius depends on. Without this hook
    /// the alerts of a minimized app sat in the queues until the window came
    /// back (and the message channel dropped whatever overflowed meanwhile).
    /// eframe calls this about every 100 ms while hidden, and before every
    /// `ui()` while shown; nothing here draws.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The first frame builds the maps and starts the watchers (see `ui`).
        if !self.initialized {
            return;
        }
        self.event_manager();
        self.drain_map_messages();
        // What the UI thread queued for itself while handling the above (a
        // notification, a message) gets its own pass, as at the end of `ui`.
        if crate::repaint::take_pending() {
            ctx.request_repaint();
        }
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    #[tracing::instrument(skip_all)]
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(not(target_arch = "wasm32"))]
        crate::gpu_diagnostics::frame_tick();
        let Self {
            initialized: _,
            app_msg: _,
            map_msg: _,
            char_msg: _,
            open: _,
            esi: _,
            app_messages: _,
            #[cfg(debug_assertions)]
                search_text: _,
            #[cfg(debug_assertions)]
                emit_notification: _,
            #[cfg(debug_assertions)]
                search_selected_row: _,
            #[cfg(debug_assertions)]
                search_results: _,
            #[cfg(debug_assertions)]
                debug: _,
            tree: _,
            universe: _,
            selected_settings_page: _,
            behavior: _,
            task_msg: _,
            task_auth: _,
            settings: _,
            settings_snapshot: _,
            watcher: _,
            intel_channels: _,
            intel_watched: _,
            intel_executor: _,
            intel_resolver: _,
            intel_graph: _,
            intel_offsets: _,
            alarm_shared: _,
            patterns_editor: _,
            settings_ui: _,
            licenses: _,
            audio: _,
            database_updater: _,
            last_notification: _,
        } = self;

        if !self.initialized {
            let _span = tracing::info_span!("telescope_init").entered();

            egui_extras::install_image_loaders(ui.ctx());
            // Lets background threads wake the UI when they queue work for
            // it: app messages, intel detections, log records (see
            // `repaint`).
            crate::repaint::set_context(ui.ctx());
            // The window the native file dialogs belong to.
            self.settings_ui.set_window_owner(native_window_id(frame));

            self.tree = Some(self.create_tree());
            // Both again whenever the SDE is reloaded (`reload_sde`): on a
            // first run the universe is still empty here.
            self.sync_region_list();
            self.open_startup_regions();

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
        // Every map takes its messages each frame, drawn or not: a pane only
        // reads them in its `ui()`, which isn't called while its tab is
        // hidden or the Settings screen is open, and the broadcast channel
        // drops the oldest ones once it is full.
        self.drain_map_messages();
        // Over the Settings screen too: its Application page starts updates.
        self.database_updater.show(ui.ctx());

        // The Settings screen is full-window: while it is open it replaces the
        // maps, the log panel and the menu.
        if self.open[2] {
            self.show_settings_screen(ui);
            Self::finish_frame(ui);
            return;
        }

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
                        self.open_settings();
                    }
                    #[cfg(debug_assertions)]
                    if ui.button(t!("menu.debug")).clicked() {
                        self.open[1] = true;
                    }
                    ui.separator();
                    if ui.button(t!("menu.quit")).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button(t!("menu.help"), |ui| {
                    if ui.button(t!("menu.licenses")).clicked() {
                        self.open_licenses();
                    }
                    ui.separator();
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
        self.show_licenses_window(ui.ctx());

        // Debug menu (not in release builds)
        #[cfg(debug_assertions)]
        if self.open[1] {
            self.open_debug_menu(ui.ctx());
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let _span = tracing::info_span!("inserting map").entered();
            if let Some(tree) = &mut self.tree {
                // The log is its own bottom panel now: the maps get all the
                // remaining height.
                tree.set_height(ui.available_height());
                tree.ui(&mut self.behavior, ui);
            }
        });
        Self::finish_frame(ui);
    }
}

impl TelescopeApp {
    /// Opens the full-window Settings screen, snapshotting the settings so
    /// Cancel can revert the session, and makes sure the rules editor is
    /// loaded from the live rules.
    pub(crate) fn open_settings(&mut self) {
        if !self.open[2] {
            // Fresh channel list and activity for the Sources page.
            if let Err(error) = self.scan_intel_files() {
                tracing::debug!("could not scan the chat logs: {error}");
            }
            self.settings_snapshot = Some(self.settings.clone());
        }
        let graph = self.intel_graph.clone();
        self.patterns_editor.ensure_loaded(&graph);
        self.open[2] = true;
    }

    /// Discards every change made while the Settings screen was open and
    /// closes it.
    pub(crate) fn cancel_settings(&mut self) {
        if let Some(mut snapshot) = self.settings_snapshot.take() {
            // The log panel layout isn't part of the session: keep it.
            snapshot.set_layout(&self.settings.get_ui_state());
            self.settings = snapshot;
        }
        self.settings_ui.discard_drafts();
        // The language previewed while editing, and the start-up maps.
        crate::i18n::apply_language(&self.settings.get_ui_state().language);
        self.reset_startup_flags();
        self.apply_intel_settings();
        let graph = self.intel_graph.clone();
        self.patterns_editor.reset(&graph);
        self.open[2] = false;
    }

    /// Validates the edited graph and persists the settings and the graph
    /// without closing the Settings screen. An invalid graph keeps the errors
    /// shown and returns `false`.
    pub(crate) fn apply_settings(&mut self) -> bool {
        // The rule open in the node editor is applied too.
        if let Err(error) = self.patterns_editor.commit_open_editor() {
            self.patterns_editor.set_errors(vec![error]);
            return false;
        }
        let graph = self.patterns_editor.to_graph();
        let errors = graph.validate();
        if !errors.is_empty() {
            self.patterns_editor
                .set_errors(errors.iter().map(|error| error.to_string()).collect());
            return false;
        }
        self.patterns_editor.clear_errors();
        let sde_changed = self
            .settings_snapshot
            .as_ref()
            .is_some_and(|saved| saved.get_sde() != self.settings.get_sde());
        self.save_settings();
        if sde_changed {
            self.change_sde();
        }
        self.apply_node_style();
        self.apply_graph(graph);
        self.patterns_editor.mark_applied();
        // What was just applied is the new baseline for Cancel.
        self.settings_snapshot = Some(self.settings.clone());
        true
    }

    /// Applies the settings and closes the screen.
    pub(crate) fn accept_settings(&mut self) {
        if self.apply_settings() {
            self.settings_snapshot = None;
            self.open[2] = false;
        }
    }

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
                Message::ChannelActivity(channel, when) => {
                    // Last activity of the channel, for Settings -> Sources.
                    self.settings.note_channel_activity(&channel, when);
                }
                Message::UpdateIntelDirectory(directory_path) => {
                    match self.settings.set_intel(directory_path.as_path()) {
                        Ok(()) => {
                            if let Err(e) = self.scan_intel_files() {
                                self.notify_intel_error("UpdateIntelDirectory", e);
                            }
                        }
                        Err(e) => self.notify_intel_error("UpdateIntelDirectory", e),
                    }
                }
                Message::DatabaseUpdateProgress(phase) => {
                    self.database_updater.set_phase(phase);
                }
                Message::DatabaseUpdateInfo(info) => {
                    self.database_updater.set_info(info);
                }
                Message::DatabaseUpdated(rebuilt) => {
                    if rebuilt {
                        self.database_updater.finish();
                        self.reload_sde();
                    } else {
                        self.database_updater.hide();
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
                                if let Err(e) = self.scan_intel_files() {
                                    self.notify_intel_error("DefaultIntelDirectory", e);
                                }
                            }
                            Err(e) => self.notify_intel_error("DefaultIntelDirectory", e),
                        }
                    }
                }
                Message::ScanIntelFiles => {
                    let _ = self.scan_intel_files();
                }
                Message::SdePathPicked(path) => {
                    if let Err(e) = self.settings.set_sde(&path) {
                        self.notify_intel_error("SdePathPicked", e);
                    }
                }
                Message::DbPathPicked(path) => {
                    if let Err(e) = self.settings.set_db(&path) {
                        self.notify_intel_error("DbPathPicked", e);
                    }
                }
            };
        }
        // What the dispatch thread reads (it acts on detected lines on its
        // own, without this loop).
        self.sync_alarm_shared();
    }

    /// End of a frame: a message queued on the UI thread during it gets the
    /// frame that drains it (see `repaint::request`).
    fn finish_frame(ui: &egui::Ui) {
        if crate::repaint::take_pending() {
            ui.ctx().request_repaint();
        }
        tracing::info!(tracy.frame_mark = true);
    }

    /// Lets every map pane take the `MapSync` messages waiting for it.
    fn drain_map_messages(&mut self) {
        if let Some(tree) = self.tree.as_mut() {
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Pane(pane) = tile {
                    pane.event_manager();
                }
            }
        }
    }

    /// Redraws every map's nodes with the node style of the settings (the
    /// character glow set in Settings -> Maps).
    fn apply_node_style(&mut self) {
        let style = self.settings.get_node_style();
        if let Some(tree) = self.tree.as_mut() {
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Pane(pane) = tile {
                    pane.set_node_style(style);
                }
            }
        }
    }

    /// Marks the regions whose map opens at start-up as the settings say,
    /// dropping the choices made in Settings -> Maps and not applied.
    fn reset_startup_flags(&mut self) {
        let startup = self.settings.get_startup_regions().clone();
        for (region, data) in self.behavior.tile_data.iter_mut() {
            data.show_on_startup = startup.contains(region);
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
                // A region whose map was never created has nothing to show.
                if let (Some(tile_id), Some(tree)) = (region.get_tile_id(), self.tree.as_mut()) {
                    tree.set_visible(tile_id, true);
                    region.set_visible(true);
                }
            });
    }

    /// Called once before the first frame.
    #[tracing::instrument(skip(cc))]
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        cc.egui_ctx.set_fonts(Self::font_definitions());
        // Record why the GPU stops working, if it does (e.g. after sleep).
        #[cfg(not(target_arch = "wasm32"))]
        crate::gpu_diagnostics::install(cc);
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
        let tile_id = self
            .behavior
            .tile_data
            .get(&region_id)
            .and_then(|data| data.get_tile_id());
        if let (Some(tile_id), Some(tree)) = (tile_id, self.tree.as_mut()) {
            tree.tiles.toggle_visibility(tile_id);
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
            webb::map_alerts::ALERT_ICON,
            webb::map_alerts::CLEAR_ICON,
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
