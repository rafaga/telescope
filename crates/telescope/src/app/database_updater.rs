//! A small window showing progress while the local EVE Online SDE
//! database (`sde.db`) is checked for updates and, if needed, rebuilt.
//!
//! There is no single "check and build" entry point in the `sde` crate
//! itself (as of the `test` branch this workspace's `[patch.crates-io]`
//! points `sde` at -- see the workspace root `Cargo.toml`) -- only the
//! individual pieces `sde-builder`'s own CLI (`sde`'s `src/bin/cli.rs`)
//! calls in sequence: [`sde_index::update_as_needed`] (check CCP's SDE
//! index) -> [`extract::prepare_sde_directory`] (decompress the zip) ->
//! [`schema::create_schema`] (create the tables) ->
//! [`Parser::build_database`] (parse the SDE into them). `Self::run`
//! below calls those same four functions in the same order the CLI
//! does, for the same reason the CLI does: there is currently nowhere
//! else this sequence lives. It intentionally goes no further than
//! that -- no SDE parsing, downloading, or database logic is
//! reimplemented here, only this crate's usual
//! background-thread-plus-message-channel wiring around calls into
//! `sde::builder`.
//!
//! This module's actual job -- the reason it exists as more than a
//! function call -- is the window: [`DatabaseUpdater`] holds the
//! progress text, [`DatabaseUpdater::show`] paints it, and
//! [`DatabaseUpdater::spawn`] runs the four-step check/build above on
//! its own thread (so the egui UI thread never blocks on network I/O),
//! the same pattern
//! [`super::messages::AuthSpawner`]/[`super::TelescopeApp::start_watchdog`]
//! already use. It reports back through
//! [`Message::DatabaseUpdateProgress`]/[`Message::DatabaseUpdated`] and
//! [`Message::GenericNotification`] rather than touching any UI state
//! directly; [`TelescopeApp::event_manager`](super::TelescopeApp::event_manager)
//! forwards those into this struct's `set_phase`/`hide`, and
//! [`TelescopeApp::ui`](super::TelescopeApp::ui) calls [`Self::show`]
//! once per frame, the same way it already calls
//! `open_about_window`/`open_settings_window`/`open_debug_menu`.
//!
//! There used to be a stub here that called a nonexistent
//! `eframe::run_ui_native` and never actually checked or built anything.

use eframe::egui::{self, Align2, Margin, RichText, Stroke, Vec2};
use sde::Error;
use sde::builder::parser::{Parser, ParserConfig, Position2DMode, ProjectedAxis};
use sde::builder::{BuildUrls, extract, http, schema, sde_index};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;
use tokio::sync::mpsc::Sender;

use super::messages::{Message, Type, send_app_message, try_send_app_message};

/// Guards against two update runs racing each other (e.g. the automatic
/// startup check and a manual "Check for updates" click in Settings). A
/// second call to [`DatabaseUpdater::spawn`] while one is already
/// running is reported back through `app_msg` and otherwise ignored,
/// rather than letting two pipelines fight over the same `sde.db` file.
static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Set by the window's Cancel button, read by `DatabaseUpdater::run` at the
/// end of each phase: the phase that is running finishes, the next one does
/// not start.
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The phases of an SDE update, in the order they run; each is sent to the
/// window as [`Message::DatabaseUpdateProgress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdePhase {
    /// Asking CCP's SDE index which build is the latest.
    Checking,
    /// Downloading the new export.
    Downloading,
    /// Decompressing the export.
    Extracting,
    /// Parsing the export into a new `sde.db`.
    Rebuilding,
    /// Checking the new database before it replaces the old one.
    Verifying,
}

/// Something learned while an update runs, shown next to the steps and in
/// the window's log; sent as [`Message::DatabaseUpdateInfo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdeInfo {
    /// The build installed (if any) and the latest one CCP offers.
    Versions {
        /// The build number recorded by the last update.
        installed: Option<String>,
        /// The latest build number in CCP's index.
        available: String,
    },
    /// The export was downloaded: its size in bytes.
    Downloaded(u64),
    /// The new database passed its integrity check.
    Verified,
}

impl SdePhase {
    const ALL: [Self; 5] = [
        Self::Checking,
        Self::Downloading,
        Self::Extracting,
        Self::Rebuilding,
        Self::Verifying,
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|phase| *phase == self)
            .unwrap_or_default()
    }

    /// The phase's line in the list of steps.
    fn step(self) -> String {
        match self {
            Self::Checking => t!("sde_update.step_check"),
            Self::Downloading => t!("sde_update.step_download"),
            Self::Extracting => t!("sde_update.step_extract"),
            Self::Rebuilding => t!("sde_update.step_rebuild"),
            Self::Verifying => t!("sde_update.step_verify"),
        }
        .into_owned()
    }

    /// What is happening now: the phase's line in the log.
    fn detail(self) -> String {
        match self {
            Self::Checking => t!("sde_update.checking"),
            Self::Downloading => t!("sde_update.downloading"),
            Self::Extracting => t!("sde_update.extracting"),
            Self::Rebuilding => t!("sde_update.rebuilding"),
            Self::Verifying => t!("sde_update.verifying"),
        }
        .into_owned()
    }
}

/// Width of the update window.
const WINDOW_WIDTH: f32 = 568.0;

/// Room left at each side of the window on a small screen.
const WINDOW_MARGIN: f32 = 24.0;

/// Height of the window's own title bar.
const TITLE_BAR_HEIGHT: f32 = 34.0;

/// Height of the log area.
const LOG_HEIGHT: f32 = 92.0;

/// How an update run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// `sde.db` was (re)built.
    Rebuilt,
    /// The database was already up to date.
    UpToDate,
    /// The user cancelled before the rebuild started.
    Cancelled,
}

/// UI state for the "updating the SDE database" window, owned by
/// `TelescopeApp` and painted every frame via [`Self::show`]. Doesn't
/// drive anything itself -- `Self::spawn` is the only thing that starts
/// an update, and it runs independently of whether this window is
/// currently visible.
#[derive(Default)]
pub struct DatabaseUpdater {
    /// The phase running, `None` while no update is.
    phase: Option<SdePhase>,
    /// The update is over and the new database is in place; the window
    /// stays until the user accepts it.
    finished: bool,
    /// When the running phase began.
    phase_started: Option<Instant>,
    /// The installed and the latest build, once known.
    versions: Option<(Option<String>, String)>,
    /// The size of the downloaded export, once known.
    downloaded: Option<u64>,
    /// One line per event, each led by the time of day.
    log: Vec<String>,
}

/// `m:ss` for a duration.
fn clock(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl DatabaseUpdater {
    /// Paints the progress window if an update is currently running (see
    /// `Self::set_phase`/`Self::hide`) -- a no-op otherwise. It is drawn
    /// like a dialog of the Settings screen (`egui_panels`): a title bar
    /// with a close button, a progress bar, the three
    /// phases as steps, a log of what began and when, and a Cancel button
    /// that takes effect at the end of the running phase (not during the
    /// rebuild, which can't be interrupted).
    #[tracing::instrument(skip(self, ctx))]
    pub fn show(&mut self, ctx: &egui::Context) {
        let Some(phase) = self.phase else {
            return;
        };
        let mut accepted = false;
        egui::Window::new(t!("sde_update.title"))
            .id(egui::Id::new("sde_update_window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui_panels::dialog_frame(ctx).inner_margin(Margin::ZERO))
            .show(ctx, |ui| {
                let theme = egui_panels::Theme::get(ui.ctx());
                let palette = theme.palette(ui.visuals());
                let cancelling = CANCEL_REQUESTED.load(Ordering::SeqCst);
                let finished = self.finished;
                let can_cancel = matches!(
                    phase,
                    SdePhase::Checking | SdePhase::Downloading | SdePhase::Extracting
                ) && !cancelling
                    && !finished;
                // Narrower than usual when the application window is.
                let window_width = WINDOW_WIDTH
                    .min(ui.ctx().content_rect().width() - 2.0 * WINDOW_MARGIN)
                    .max(280.0);
                ui.set_width(window_width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                // Title bar.
                ui.allocate_ui_with_layout(
                    Vec2::new(window_width, TITLE_BAR_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(t!("sde_update.title"))
                                .size(theme.section_title_size)
                                .color(palette.strong_text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            let hover = if finished {
                                t!("sde_update.accept")
                            } else {
                                t!("sde_update.cancel")
                            };
                            if close_button(ui, can_cancel || finished)
                                .on_hover_text(hover)
                                .clicked()
                            {
                                if finished {
                                    accepted = true;
                                } else {
                                    Self::request_cancel();
                                }
                            }
                        });
                    },
                );
                egui_panels::divider(ui);

                egui::Frame::NONE
                    .inner_margin(Margin {
                        left: 18,
                        right: 18,
                        top: 16,
                        bottom: 18,
                    })
                    .show(ui, |ui| {
                        ui.set_width(window_width - 36.0);
                        ui.spacing_mut().item_spacing.y = 14.0;

                        let info = match &self.versions {
                            Some((installed, available)) => t!(
                                "sde_update.versions",
                                installed = installed
                                    .clone()
                                    .unwrap_or_else(|| t!("sde_update.none").into_owned()),
                                available = available
                            )
                            .into_owned(),
                            None => t!("sde_update.description").into_owned(),
                        };
                        ui.label(
                            RichText::new(info)
                                .size(theme.small_size)
                                .color(palette.muted_text),
                        );

                        // Half a phase in while it runs, so the bar moves at once.
                        let target = if finished {
                            1.0
                        } else {
                            (phase.index() as f32 + 0.5) / SdePhase::ALL.len() as f32
                        };
                        let current = if finished {
                            SdePhase::ALL.len()
                        } else {
                            phase.index()
                        };
                        let fraction = ui.ctx().animate_value_with_time(
                            egui::Id::new("sde_update_progress"),
                            target,
                            0.4,
                        );
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let width = ui.available_width() - 44.0;
                            ui.add_sized(
                                [width, 8.0],
                                egui::ProgressBar::new(fraction)
                                    .desired_height(8.0)
                                    .corner_radius(4)
                                    .fill(palette.accent),
                            );
                            ui.label(
                                RichText::new(format!("{:.0} %", fraction * 100.0))
                                    .size(theme.small_size)
                                    .color(palette.muted_text),
                            );
                        });

                        let steps: Vec<String> =
                            SdePhase::ALL.iter().map(|phase| phase.step()).collect();
                        let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
                        let notes: Vec<String> = (0..steps.len())
                            .map(|index| match index {
                                0 => self
                                    .versions
                                    .as_ref()
                                    .filter(|_| current > 0)
                                    .map(|(_, available)| {
                                        t!("sde_update.available", build = available).into_owned()
                                    })
                                    .unwrap_or_default(),
                                1 => self
                                    .downloaded
                                    .filter(|_| current > 1)
                                    .map(format_size)
                                    .unwrap_or_default(),
                                _ => String::new(),
                            })
                            .enumerate()
                            .map(|(index, note)| {
                                if index == current && note.is_empty() {
                                    self.phase_started
                                        .map(|began| clock(began.elapsed()))
                                        .unwrap_or_default()
                                } else {
                                    note
                                }
                            })
                            .collect();
                        egui_panels::progress_steps_with_notes(ui, &steps, &notes, current);

                        // What began and when.
                        egui::Frame::NONE
                            .fill(ui.visuals().extreme_bg_color)
                            .stroke(Stroke::new(1.0, palette.card_stroke))
                            .corner_radius(3)
                            .inner_margin(Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 2.0;
                                egui::ScrollArea::vertical()
                                    .id_salt("sde_update_log")
                                    .max_height(LOG_HEIGHT - 16.0)
                                    .auto_shrink([false, false])
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for line in &self.log {
                                            ui.label(
                                                RichText::new(line)
                                                    .monospace()
                                                    .size(theme.small_size - 1.0)
                                                    .color(palette.muted_text),
                                            );
                                        }
                                    });
                            });

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let accept = ui.add_enabled_ui(finished, |ui| {
                                        egui_panels::button(
                                            ui,
                                            t!("sde_update.accept").into_owned(),
                                            if finished {
                                                egui_panels::Variant::Primary
                                            } else {
                                                egui_panels::Variant::Secondary
                                            },
                                        )
                                    });
                                    if accept.inner.clicked() {
                                        accepted = true;
                                    }
                                    let label = if cancelling {
                                        t!("sde_update.cancelling")
                                    } else {
                                        t!("sde_update.cancel")
                                    };
                                    let cancel = ui.add_enabled_ui(can_cancel, |ui| {
                                        egui_panels::button(
                                            ui,
                                            label.into_owned(),
                                            egui_panels::Variant::Ghost,
                                        )
                                    });
                                    if cancel.inner.clicked() {
                                        Self::request_cancel();
                                    }
                                },
                            );
                        });
                    });
            });
        if accepted {
            self.hide();
            return;
        }
        // The elapsed time counts even while nothing else repaints.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// Asks the running update to stop at the end of its current phase.
    fn request_cancel() {
        CANCEL_REQUESTED.store(true, Ordering::SeqCst);
    }

    /// Shows the window (if it wasn't already) at `phase`. Called from
    /// `TelescopeApp::event_manager` on [`Message::DatabaseUpdateProgress`].
    pub fn set_phase(&mut self, phase: SdePhase) {
        if self.phase.is_none() {
            self.finished = false;
            self.versions = None;
            self.downloaded = None;
            self.log.clear();
        }
        self.log_line(phase.detail());
        self.phase_started = Some(Instant::now());
        self.phase = Some(phase);
    }

    /// Records something learned during the update. Called from
    /// `TelescopeApp::event_manager` on [`Message::DatabaseUpdateInfo`].
    pub fn set_info(&mut self, info: SdeInfo) {
        match info {
            SdeInfo::Versions {
                installed,
                available,
            } => {
                let line = t!(
                    "sde_update.log_versions",
                    installed = installed
                        .clone()
                        .unwrap_or_else(|| t!("sde_update.none").into_owned()),
                    available = available
                )
                .into_owned();
                self.log_line(line);
                self.versions = Some((installed, available));
            }
            SdeInfo::Downloaded(bytes) => {
                self.log_line(
                    t!("sde_update.log_downloaded", size = format_size(bytes)).into_owned(),
                );
                self.downloaded = Some(bytes);
            }
            SdeInfo::Verified => {
                self.log_line(t!("sde_update.log_verified").into_owned());
            }
        }
    }

    /// Adds a line to the log, led by the time of day.
    fn log_line(&mut self, text: String) {
        self.log.push(format!(
            "{}  {text}",
            chrono::Local::now().format("%H:%M:%S")
        ));
    }

    /// The new database is in place: every step is done and the window
    /// waits for the user to accept it. Called from
    /// `TelescopeApp::event_manager` on [`Message::DatabaseUpdated`] when
    /// the database was rebuilt.
    pub fn finish(&mut self) {
        if self.phase.is_some() {
            self.finished = true;
            self.phase_started = None;
            self.log_line(t!("sde_update.log_done").into_owned());
        }
    }

    /// Hides the window. Called from `TelescopeApp::event_manager` on
    /// [`Message::DatabaseUpdated`] when nothing was rebuilt (up to date,
    /// cancelled or failed), and when the user accepts a finished update.
    pub fn hide(&mut self) {
        self.phase = None;
        self.finished = false;
        self.phase_started = None;
        self.versions = None;
        self.downloaded = None;
        self.log.clear();
    }

    /// Spawns the update check/build on its own thread, with its own
    /// single-threaded tokio runtime (matching
    /// `messages::AuthSpawner`/`TelescopeApp::start_watchdog`), so
    /// callers on the egui UI thread never block on network I/O.
    ///
    /// `sde_path` is where `sde.db` lives (or will be created --
    /// `Settings::get_sde()`). `data_dir` is scratch space for the
    /// downloaded zip and the small `.build` file `sde_index` uses to
    /// detect a new build next time; `sde_dir` is where that zip gets
    /// decompressed to. Neither needs to already exist. `with_third_party`
    /// mirrors `sde-builder`'s `--with-third-party` flag -- off by
    /// default, since none of that data comes from CCP's official
    /// export.
    ///
    /// Progress is reported through `app_msg` as
    /// [`Message::DatabaseUpdateProgress`] (drives this window) and
    /// [`Message::GenericNotification`] (the app's on-screen log,
    /// start/finish/error only -- not one per phase, unlike the
    /// window), and completion as [`Message::DatabaseUpdated`] so the
    /// caller can hide the window and reload `TelescopeApp::universe`
    /// once a new `sde.db` is in place.
    #[tracing::instrument(skip(app_msg))]
    pub fn spawn(
        sde_path: PathBuf,
        data_dir: PathBuf,
        sde_dir: PathBuf,
        app_msg: Arc<Sender<Message>>,
        with_third_party: bool,
        urls: BuildUrls,
    ) {
        // It detetcs if the database has a valid format.
        // Its checks the file typoe against the SQlite Magic header
        // A file that isn't a SQLite database is rebuilt (`run` treats it as
        // missing); a valid one still goes through the update check.
        if sde_path.exists() && !is_sqlite(&sde_path) {
            let _ = try_send_app_message(
                &app_msg,
                Message::GenericNotification((
                    Type::Warning,
                    String::from("DatabaseUpdater"),
                    String::from("spawn"),
                    String::from("The SDE database is corrupted, rebuilding it."),
                )),
            );
        }
        // An empty path means `Settings::get_sde()` isn't configured
        // (shouldn't happen for a fresh `Settings::default()` anymore,
        // see `settings::FilePaths::default`, but an existing
        // `telescope.toml` saved before that default existed can still
        // have one). `rusqlite::Connection::open("")` doesn't error --
        // SQLite treats an empty filename as a private on-disk temporary
        // database, deleted as soon as the connection closes -- so
        // without this guard every startup would silently download and
        // build the whole SDE into a database nobody could ever read,
        // instead of either persisting it or failing loudly.
        if sde_path.as_os_str().is_empty() {
            let _ = try_send_app_message(
                &app_msg,
                Message::GenericNotification((
                    Type::Warning,
                    String::from("DatabaseUpdater"),
                    String::from("spawn"),
                    String::from(
                        "No SDE database path is configured (Settings -> Application); skipping the update check.",
                    ),
                )),
            );
            return;
        }
        if UPDATE_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            let _ = try_send_app_message(
                &app_msg,
                Message::GenericNotification((
                    Type::Info,
                    String::from("DatabaseUpdater"),
                    String::from("spawn"),
                    String::from("An SDE update check is already running, skipping."),
                )),
            );
            return;
        }

        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build the database-updater runtime");
            runtime.block_on(async move {
                let _span = tracing::info_span!("spawned database updater").entered();
                let result = Self::run(
                    &sde_path,
                    &data_dir,
                    &sde_dir,
                    with_third_party,
                    &urls,
                    &app_msg,
                )
                .await;
                match result {
                    Ok(outcome) => {
                        match outcome {
                            Outcome::Rebuilt => {
                                Self::notify(
                                    &app_msg,
                                    Type::Info,
                                    "SDE database updated successfully.",
                                )
                                .await;
                            }
                            Outcome::UpToDate => {
                                Self::notify(
                                    &app_msg,
                                    Type::Info,
                                    "SDE database is already up to date.",
                                )
                                .await;
                            }
                            Outcome::Cancelled => {
                                Self::notify(&app_msg, Type::Info, "SDE update cancelled.").await;
                            }
                        }
                        let rebuilt = outcome == Outcome::Rebuilt;
                        let _ = send_app_message(&app_msg, Message::DatabaseUpdated(rebuilt)).await;
                    }
                    Err(err) => {
                        let _ = send_app_message(
                            &app_msg,
                            Message::GenericNotification((
                                Type::Error,
                                String::from("DatabaseUpdater"),
                                String::from("run"),
                                err.to_string(),
                            )),
                        )
                        .await;
                        let _ = send_app_message(&app_msg, Message::DatabaseUpdated(false)).await;
                    }
                }
            });
            UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);
        });
    }

    /// Sends both a status update for the progress window
    /// (`Message::DatabaseUpdateProgress`) and a line in the app's
    /// on-screen log (`Message::GenericNotification`) -- used for the
    /// start/finish/error messages, which are worth keeping in the log
    /// after the window closes; per-phase progress during `Self::run`
    /// only updates the window (see `Self::run`'s own progress sends).
    async fn notify(app_msg: &Sender<Message>, kind: Type, message: &str) {
        let _ = send_app_message(
            app_msg,
            Message::GenericNotification((
                kind,
                String::from("DatabaseUpdater"),
                String::from("run"),
                message.to_string(),
            )),
        )
        .await;
    }

    /// Checks CCP's SDE index and, if a newer build is available (or
    /// `sde_path` doesn't exist yet), downloads it and rebuilds `sde.db`
    /// from scratch -- the same four steps `sde-builder`'s CLI runs:
    /// [`sde_index::update_as_needed`] -> [`extract::prepare_sde_directory`]
    /// -> [`schema::create_schema`] -> [`Parser::build_database`].
    ///
    /// Whether to rebuild is decided entirely from observed state -- no
    /// separate "force" flag: a rebuild happens whenever
    /// [`sde_index::update_as_needed`] reports a new build (`changed`) or
    /// `sde_path` doesn't exist yet (`!db_exists`). Skipping only requires
    /// both "nothing changed" and "the database is already there".
    ///
    /// Returns whether the database was rebuilt, was already up to date or
    /// the user cancelled (see [`Outcome`]). Errors -- no network, a
    /// malformed zip, a SQL failure -- are returned rather than panicking;
    /// the caller decides how to surface them.
    async fn run(
        sde_path: &std::path::Path,
        data_dir: &std::path::Path,
        sde_dir: &std::path::Path,
        with_third_party: bool,
        urls: &BuildUrls,
        app_msg: &Sender<Message>,
    ) -> Result<Outcome, Error> {
        send_app_message(app_msg, Message::DatabaseUpdateProgress(SdePhase::Checking))
            .await
            .ok();

        let client = http::build_client()?;
        let build_file = data_dir.join(format!("sde-{}.build", urls.sde_variant));
        let installed = std::fs::read_to_string(&build_file)
            .ok()
            .map(|build| build.trim().to_string())
            .filter(|build| !build.is_empty());
        // Only for the window: `sde_index::update_as_needed` reads the index
        // again and decides on its own.
        let available = http::fetch_text(&client, &format!("{}latest.jsonl", urls.sde_url))
            .await
            .ok()
            .and_then(|index| latest_build(&index));
        if let Some(available) = &available {
            send_app_message(
                app_msg,
                Message::DatabaseUpdateInfo(SdeInfo::Versions {
                    installed: installed.clone(),
                    available: available.clone(),
                }),
            )
            .await
            .ok();
        }

        let db_exists = is_sqlite(sde_path);
        let zip_path = data_dir.join(format!("sde-{}.zip", urls.sde_variant));
        let expect_download = !db_exists
            || !zip_path.exists()
            || available
                .as_ref()
                .is_none_or(|available| installed.as_ref() != Some(available));
        if expect_download {
            send_app_message(
                app_msg,
                Message::DatabaseUpdateProgress(SdePhase::Downloading),
            )
            .await
            .ok();
        }
        let changed =
            sde_index::update_as_needed(&client, data_dir, &urls.sde_url, &urls.sde_variant)
                .await?;
        if !changed && db_exists {
            return Ok(Outcome::UpToDate);
        }
        if let Ok(zip) = std::fs::metadata(&zip_path) {
            send_app_message(
                app_msg,
                Message::DatabaseUpdateInfo(SdeInfo::Downloaded(zip.len())),
            )
            .await
            .ok();
        }
        if Self::cancelled(data_dir, urls) {
            return Ok(Outcome::Cancelled);
        }

        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(SdePhase::Extracting),
        )
        .await
        .ok();

        if let Some(parent) = sde_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        extract::prepare_sde_directory(&zip_path, sde_dir)?;
        if Self::cancelled(data_dir, urls) {
            return Ok(Outcome::Cancelled);
        }

        // Read back the build number `sde_index::update_as_needed` just
        // wrote (or confirmed unchanged) to `sde-{sde_variant}.build`,
        // purely to record it in `sdeFingerprint` below -- mirrors
        // `sde-builder`'s own CLI. `Ok` on read failure rather than
        // propagating it: a database with no recorded build number
        // (`sdeFingerprint.sdeBuild = NULL`) is still valid, so this
        // shouldn't abort the whole build.
        let build_number =
            std::fs::read_to_string(data_dir.join(format!("sde-{}.build", urls.sde_variant)))
                .ok()
                .map(|s| s.trim().to_string());

        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(SdePhase::Rebuilding),
        )
        .await
        .ok();

        // Built next to the database and moved over it only once complete:
        // a failed download or build keeps the database there was.
        let building = building_path(sde_path);
        if building.exists() {
            std::fs::remove_file(&building)?;
        }
        let mut connection = rusqlite::Connection::open(&building)?;
        schema::create_schema(&connection)?;

        // Local projection instead of CCP's precomputed `position2D`:
        // that value is a hand-adjusted schematic of the in-game map,
        // not a projection of the 3D coordinates, and covers k-space
        // only. `Orthogonal(Y)` is the north-up top-down: EVE's
        // galactic plane is the X-Z plane (x = east, z = north) with y
        // as the vertical axis, so dropping y gives east = screen
        // right, north = screen up -- the community-canonical
        // orientation -- and, being a true projection, it covers every
        // system in scope (w-space included). Same default
        // `sde-builder`'s own CLI uses.
        let parser_config = ParserConfig {
            language: "en".to_string(),
            position_2d: Position2DMode::Orthogonal(ProjectedAxis::Y),
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: true,
            with_gates: true,
            with_moons: true,
            verbose: false,
            with_third_party,
        };
        let sde_parser = Parser::new(sde_dir, parser_config);
        let built = sde_parser
            .build_database(
                &mut connection,
                &client,
                &urls.maps_url,
                build_number.as_deref(),
            )
            .await;
        let verified = match built {
            Ok(_) => {
                send_app_message(
                    app_msg,
                    Message::DatabaseUpdateProgress(SdePhase::Verifying),
                )
                .await
                .ok();
                Self::verify(&connection)
            }
            Err(error) => Err(error),
        };
        // Closed before the file is moved or removed.
        drop(connection);
        if let Err(error) = verified {
            let _ = std::fs::remove_file(&building);
            return Err(error);
        }
        send_app_message(app_msg, Message::DatabaseUpdateInfo(SdeInfo::Verified))
            .await
            .ok();
        // One step on every platform: the old database stays whole until
        // the new one replaces it.
        std::fs::rename(&building, sde_path)?;

        Ok(Outcome::Rebuilt)
    }

    /// SQLite's own quick integrity check of the database just built.
    fn verify(connection: &rusqlite::Connection) -> Result<(), Error> {
        let result: String =
            connection.pragma_query_value(None, "quick_check", |row| row.get(0))?;
        if result == "ok" {
            return Ok(());
        }
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some(format!(
                "the new SDE database failed its integrity check: {result}"
            )),
        )
        .into())
    }

    /// Whether the user asked to stop. A cancelled run forgets the build
    /// number it recorded, so the next check still sees the export as new
    /// instead of taking the old database for up to date.
    fn cancelled(data_dir: &std::path::Path, urls: &BuildUrls) -> bool {
        let cancelled = CANCEL_REQUESTED.load(Ordering::SeqCst);
        if cancelled {
            let _ = std::fs::remove_file(data_dir.join(format!("sde-{}.build", urls.sde_variant)));
        }
        cancelled
    }
}

/// The latest build number in the text of CCP's SDE index (`latest.jsonl`),
/// `None` when it has none: the window then simply has no version to show.
fn latest_build(index: &str) -> Option<String> {
    index.lines().find_map(|line| {
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if !compact.contains("\"_key\":\"sde\"") {
            return None;
        }
        let rest = compact.split("\"buildNumber\":").nth(1)?;
        let build: String = rest
            .trim_start_matches('"')
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        (!build.is_empty()).then_some(build)
    })
}

/// A byte count for people: `48.2 MB`.
fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

/// The title bar's close button: a 22 px square with a drawn cross, the same
/// hover as a side navigation item. Disabled it is dimmed and inert.
fn close_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let theme = egui_panels::Theme::get(ui.ctx());
    let palette = theme.palette(ui.visuals());
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), sense);
    if ui.is_rect_visible(rect) {
        if enabled && response.hovered() {
            ui.painter().rect_filled(rect, 3.0, palette.hover_fill);
        }
        let color = if enabled {
            palette.muted_text
        } else {
            palette.muted_text.gamma_multiply(0.4)
        };
        let arm = 4.0;
        let center = rect.center();
        let stroke = Stroke::new(1.4, color);
        ui.painter().line_segment(
            [center + Vec2::new(-arm, -arm), center + Vec2::new(arm, arm)],
            stroke,
        );
        ui.painter().line_segment(
            [center + Vec2::new(-arm, arm), center + Vec2::new(arm, -arm)],
            stroke,
        );
    }
    response
}

/// Whether `path` is a file starting with the SQLite header (a missing,
/// unreadable or too short file isn't).
fn is_sqlite(path: &std::path::Path) -> bool {
    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == b"SQLite format 3\0")
}

/// Where a new SDE database is built before it replaces `sde_path`
/// (`sde.db` -> `sde.db.building`).
fn building_path(sde_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = sde_path.as_os_str().to_owned();
    name.push(".building");
    std::path::PathBuf::from(name)
}

#[cfg(test)]
mod building_path_tests {
    use super::{building_path, format_size, is_sqlite, latest_build};

    #[test]
    fn the_latest_build_is_read_from_the_index() {
        let index =
            "{\"_key\": \"sde\", \"buildNumber\": 3458726, \"releaseDate\": \"2026-08-06\"}\r\n";
        assert_eq!(latest_build(index).as_deref(), Some("3458726"));
        let text =
            "{\"_key\":\"other\",\"buildNumber\":1}\n{\"_key\":\"sde\",\"buildNumber\":\"77\"}";
        assert_eq!(latest_build(text).as_deref(), Some("77"));
        assert_eq!(
            latest_build("{\"_key\": \"other\", \"buildNumber\": 1}"),
            None
        );
        assert_eq!(latest_build(""), None);
    }

    #[test]
    fn sizes_are_shown_in_megabytes_or_kilobytes() {
        assert_eq!(format_size(50_540_000), "48.2 MB");
        assert_eq!(format_size(2048), "2 KB");
    }

    #[test]
    fn only_a_file_with_the_sqlite_header_is_a_database() {
        let dir = std::env::temp_dir().join(format!("telescope-is-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let valid = dir.join("valid.db");
        std::fs::write(&valid, b"SQLite format 3\0rest of the page").unwrap();
        let broken = dir.join("broken.db");
        std::fs::write(&broken, b"not a database at all").unwrap();
        let short = dir.join("short.db");
        std::fs::write(&short, b"SQLite").unwrap();

        assert!(is_sqlite(&valid));
        assert!(!is_sqlite(&broken));
        assert!(!is_sqlite(&short));
        assert!(!is_sqlite(&dir.join("missing.db")));
        let _ = std::fs::remove_dir_all(&dir);
    }
    use std::path::Path;

    #[test]
    fn the_new_database_is_built_next_to_the_old_one() {
        assert_eq!(
            building_path(Path::new("data/sde.db")),
            Path::new("data/sde.db.building")
        );
        assert_eq!(
            building_path(Path::new("sde.db")),
            Path::new("sde.db.building")
        );
    }
}
