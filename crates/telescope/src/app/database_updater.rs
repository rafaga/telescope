//! A small window showing progress while the local EVE Online SDE
//! database (`sde.db`) is checked for updates and, if needed, rebuilt.
//!
//! `Self::run` first asks [`update::prepare`] whether sde-deltas'
//! build-to-build deltas can bring `sde.db` up to date: they're applied to
//! the mirror of the SDE the previous full build left behind (one zip),
//! and `sde.db` then only has its recorded build moved or is rebuilt from
//! that mirror, unpacked for the occasion, without downloading CCP's
//! export. When they can't, it runs the same pieces `sde-builder`'s own
//! CLI (`sde::builder::pipeline`) calls in sequence:
//! [`sde_index::update_as_needed`] (check CCP's SDE index) ->
//! [`extract::prepare_sde_directory`] (decompress the zip) ->
//! [`schema::create_schema`] (create the tables) ->
//! [`Parser::build_database`] (parse the SDE into them) ->
//! [`update::create_mirror`] (keep what the next delta update needs) ->
//! [`pipeline::clean_downloads`] (remove what was downloaded), so what
//! stays on disk is the database, the mirror's zip and dotlan's maps. It
//! intentionally goes no further than that -- no SDE parsing, downloading,
//! or database logic is reimplemented here, only this crate's usual
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
use sde::builder::parser::{Parser, ParserConfig, Position2DMode, ProjectedAxis};
use sde::builder::pipeline::{self, Workspace};
use sde::builder::update::{self, FullReason, UpdateOptions, UpdatePlan};
use sde::builder::{BuildUrls, extract, http, schema, sde_index};
use sde::{Error, SdeManager};
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
    /// This many build-to-build deltas from sde-deltas are being applied
    /// instead of downloading the export.
    Deltas(usize),
    /// The deltas reached this build without changing anything `sde.db`
    /// holds: only its recorded build moved.
    Bumped(String),
    /// Deltas couldn't be used, the export is downloaded instead: why.
    FullBuild(String),
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
    /// The database's recorded build moved to a newer one whose changes
    /// don't touch anything it holds: nothing to reload.
    Bumped,
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
    /// How many deltas are applied instead, when that's the case.
    deltas: Option<usize>,
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
                                1 => match self.deltas {
                                    Some(count) => {
                                        t!("sde_update.deltas", count = count).into_owned()
                                    }
                                    None => self
                                        .downloaded
                                        .filter(|_| current > 1)
                                        .map(format_size)
                                        .unwrap_or_default(),
                                },
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
            self.deltas = None;
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
            SdeInfo::Deltas(count) => {
                self.log_line(t!("sde_update.log_deltas", count = count).into_owned());
                self.deltas = Some(count);
            }
            SdeInfo::Bumped(build) => {
                self.log_line(t!("sde_update.log_bumped", build = build).into_owned());
            }
            SdeInfo::FullBuild(reason) => {
                self.log_line(t!("sde_update.log_full", reason = reason).into_owned());
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
        self.deltas = None;
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
                            Outcome::Bumped => {
                                Self::notify(
                                    &app_msg,
                                    Type::Info,
                                    "SDE database moved to the latest build: nothing the maps use changed.",
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

    /// Brings `sde.db` up to CCP's latest SDE build.
    ///
    /// First through sde-deltas ([`update::prepare`]): the mirror of the SDE
    /// a previous full build left in `sde_dir` is brought up to date with
    /// the build-to-build deltas, and then `sde.db` either stays as it is,
    /// only has its recorded build moved ([`Outcome::Bumped`]), or is
    /// rebuilt from the mirror -- without downloading CCP's export.
    ///
    /// When deltas can't be used ([`UpdatePlan::Full`]: no mirror yet, a
    /// schema change, sde-deltas unreachable or lagging...), the same steps
    /// `sde-builder`'s CLI runs: [`sde_index::update_as_needed`] ->
    /// [`extract::prepare_sde_directory`] -> [`schema::create_schema`] ->
    /// [`Parser::build_database`], and then [`update::create_mirror`] so the
    /// next update can use deltas. On that path a rebuild happens whenever
    /// [`sde_index::update_as_needed`] reports a new build (`changed`) or
    /// `sde_path` doesn't exist yet (`!db_exists`).
    ///
    /// Returns how the update ended (see [`Outcome`]). Errors -- no network,
    /// a malformed zip, a SQL failure -- are returned rather than panicking;
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
        // The database's own fingerprint knows its build (delta updates move
        // it without touching the `.build` file, which describes the zip);
        // the `.build` file stands in for a database without one.
        let installed = installed_build(sde_path).or_else(|| {
            std::fs::read_to_string(&build_file)
                .ok()
                .map(|build| build.trim().to_string())
                .filter(|build| !build.is_empty())
        });
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
        let parser_config = Self::parser_config(with_third_party);
        let workspace = Workspace::new(
            sde_path.to_path_buf(),
            data_dir.to_path_buf(),
            sde_dir.to_path_buf(),
        );
        let archive = workspace.mirror_archive();

        let plan = update::prepare(
            &client,
            urls,
            sde_path,
            sde_dir,
            &archive,
            &parser_config,
            UpdateOptions::default(),
            |progress| {
                if progress.done == 0 && progress.total > 0 {
                    try_send_app_message(
                        app_msg,
                        Message::DatabaseUpdateProgress(SdePhase::Downloading),
                    )
                    .ok();
                    try_send_app_message(
                        app_msg,
                        Message::DatabaseUpdateInfo(SdeInfo::Deltas(progress.total)),
                    )
                    .ok();
                }
            },
        )
        .await;
        match plan {
            UpdatePlan::UpToDate { .. } => return Ok(Outcome::UpToDate),
            UpdatePlan::Bump { to, .. } => {
                update::set_sde_build(sde_path, &to)?;
                send_app_message(app_msg, Message::DatabaseUpdateInfo(SdeInfo::Bumped(to)))
                    .await
                    .ok();
                return Ok(Outcome::Bumped);
            }
            UpdatePlan::Rebuild { to, mirror_dir, .. } => {
                // `prepare` unpacked the mirror into the SDE directory for
                // this rebuild; it goes whatever happens next, the archive
                // keeps it.
                if Self::cancelled(data_dir, urls) {
                    let _ = update::release_working_copy(&mirror_dir);
                    return Ok(Outcome::Cancelled);
                }
                let rebuilt = Self::rebuild(
                    sde_path,
                    &mirror_dir,
                    parser_config,
                    urls,
                    Some(&to),
                    app_msg,
                )
                .await;
                let released = update::release_working_copy(&mirror_dir);
                rebuilt?;
                released?;
                return Ok(Outcome::Rebuilt);
            }
            UpdatePlan::Full { reason } => {
                if matches!(reason, FullReason::SchemaChanged(_)) {
                    Self::notify(
                        app_msg,
                        Type::Warning,
                        &format!("SDE schema change: {reason}"),
                    )
                    .await;
                }
                // A mirror is there but sde-deltas is out of reach: a database
                // already at CCP's latest build needs no full download.
                let deltas_out_of_reach = matches!(
                    reason,
                    FullReason::Unavailable(_) | FullReason::Lagging { .. }
                );
                if deltas_out_of_reach && db_exists && installed.is_some() && installed == available
                {
                    return Ok(Outcome::UpToDate);
                }
                // A first run has no mirror yet: nothing worth telling.
                if reason != FullReason::NoMirror {
                    send_app_message(
                        app_msg,
                        Message::DatabaseUpdateInfo(SdeInfo::FullBuild(reason.to_string())),
                    )
                    .await
                    .ok();
                }
            }
        }

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

        extract::prepare_sde_directory(&zip_path, sde_dir)?;
        if Self::cancelled(data_dir, urls) {
            return Ok(Outcome::Cancelled);
        }

        // Read back the build number `sde_index::update_as_needed` just
        // wrote (or confirmed unchanged) to `sde-{sde_variant}.build`, to
        // record it in `sdeFingerprint` and the mirror below -- mirrors
        // `sde-builder`'s own CLI. `Ok` on read failure rather than
        // propagating it: a database with no recorded build number
        // (`sdeFingerprint.sdeBuild = NULL`) is still valid, so this
        // shouldn't abort the whole build.
        let build_number =
            std::fs::read_to_string(data_dir.join(format!("sde-{}.build", urls.sde_variant)))
                .ok()
                .map(|s| s.trim().to_string());

        let sde_parser = Self::rebuild(
            sde_path,
            sde_dir,
            parser_config,
            urls,
            build_number.as_deref(),
            app_msg,
        )
        .await?;

        // Keep only what the parser read, as one zip, so the next update can
        // use deltas instead of this download, and remove everything
        // downloaded or decompressed from CCP: what stays on disk is the
        // database, that zip and dotlan's maps. The database is already in
        // place: a failure here only costs the next update a full download
        // again.
        if let Some(build) = &build_number {
            match update::create_mirror(sde_dir, &archive, &sde_parser, build) {
                Ok(_) => {
                    if let Err(error) = pipeline::clean_downloads(&workspace, &urls.sde_variant) {
                        Self::notify(
                            app_msg,
                            Type::Warning,
                            &format!("Couldn't remove the downloaded SDE export: {error}"),
                        )
                        .await;
                    }
                }
                Err(error) => {
                    // A half-reduced SDE directory is of no use: empty it
                    // (the zip stays, so a retry doesn't download it again).
                    let _ = update::release_working_copy(sde_dir);
                    Self::notify(
                        app_msg,
                        Type::Warning,
                        &format!("Couldn't keep a mirror of the SDE for delta updates: {error}"),
                    )
                    .await;
                }
            }
        }

        Ok(Outcome::Rebuilt)
    }

    /// The parser settings Telescope builds `sde.db` with.
    ///
    /// Local projection instead of CCP's precomputed `position2D`: that
    /// value is a hand-adjusted schematic of the in-game map, not a
    /// projection of the 3D coordinates, and covers k-space only.
    /// `Orthogonal(Y)` is the north-up top-down: EVE's galactic plane is the
    /// X-Z plane (x = east, z = north) with y as the vertical axis, so
    /// dropping y gives east = screen right, north = screen up -- the
    /// community-canonical orientation -- and, being a true projection, it
    /// covers every system in scope (w-space included). Same default
    /// `sde-builder`'s own CLI uses.
    fn parser_config(with_third_party: bool) -> ParserConfig {
        ParserConfig {
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
        }
    }

    /// Builds a new `sde.db` from the SDE files in `sde_dir` (CCP's export
    /// or the mirror), recording `build` in its fingerprint, and returns the
    /// parser that read them (it knows which fields it read, see
    /// [`update::create_mirror`]).
    ///
    /// Built next to the database and moved over it only once complete
    /// and verified: a failed build keeps the database there was.
    async fn rebuild(
        sde_path: &std::path::Path,
        sde_dir: &std::path::Path,
        parser_config: ParserConfig,
        urls: &BuildUrls,
        build: Option<&str>,
        app_msg: &Sender<Message>,
    ) -> Result<Parser, Error> {
        let client = http::build_client()?;
        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(SdePhase::Rebuilding),
        )
        .await
        .ok();

        if let Some(parent) = sde_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let building = building_path(sde_path);
        if building.exists() {
            std::fs::remove_file(&building)?;
        }
        let mut connection = rusqlite::Connection::open(&building)?;
        schema::create_schema(&connection)?;

        let sde_parser = Parser::new(sde_dir, parser_config);
        let built = sde_parser
            .build_database(&mut connection, &client, &urls.maps_url, build)
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

        Ok(sde_parser)
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

/// The SDE build recorded in `sde.db`'s fingerprint, when it has an intact
/// one.
fn installed_build(sde_path: &std::path::Path) -> Option<String> {
    let manager = SdeManager::new(sde_path, 1.0).ok()?;
    match manager.get_fingerprint() {
        Ok(Some((fingerprint, true))) => fingerprint.sde_build,
        _ => None,
    }
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

/// The update lock and cancel flag are process-wide: the tests that touch
/// them (or read them through `run`, `spawn` and `show`) take this first.
#[cfg(test)]
static FLAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn serial() -> std::sync::MutexGuard<'static, ()> {
    FLAG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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

#[cfg(test)]
mod updater_state_tests {
    use super::*;

    #[test]
    fn the_phases_run_in_order_and_have_distinct_texts() {
        let phases = [
            SdePhase::Checking,
            SdePhase::Downloading,
            SdePhase::Extracting,
            SdePhase::Rebuilding,
            SdePhase::Verifying,
        ];
        for (expected, phase) in phases.iter().enumerate() {
            assert_eq!(phase.index(), expected);
            assert!(!phase.step().is_empty());
            assert!(!phase.detail().is_empty());
        }
        let steps: std::collections::HashSet<String> =
            phases.iter().map(|phase| phase.step()).collect();
        assert_eq!(steps.len(), phases.len());
    }

    #[test]
    fn the_clock_shows_minutes_and_padded_seconds() {
        assert_eq!(clock(std::time::Duration::from_secs(0)), "0:00");
        assert_eq!(clock(std::time::Duration::from_secs(9)), "0:09");
        assert_eq!(clock(std::time::Duration::from_secs(75)), "1:15");
        assert_eq!(clock(std::time::Duration::from_secs(3600)), "60:00");
    }

    #[test]
    fn the_first_phase_opens_the_window_with_a_clean_state() {
        let mut updater = DatabaseUpdater::default();
        assert!(updater.phase.is_none());
        updater.set_phase(SdePhase::Checking);
        assert_eq!(updater.phase, Some(SdePhase::Checking));
        assert!(updater.phase_started.is_some());
        assert_eq!(updater.log.len(), 1);
        // "HH:MM:SS  text"
        assert_eq!(updater.log[0].as_bytes()[2], b':');
        assert!(updater.log[0].contains("  "));
    }

    #[test]
    fn later_phases_keep_what_was_learned() {
        let mut updater = DatabaseUpdater::default();
        updater.set_phase(SdePhase::Checking);
        updater.set_info(SdeInfo::Versions {
            installed: None,
            available: String::from("100"),
        });
        updater.set_phase(SdePhase::Downloading);
        assert_eq!(updater.versions, Some((None, String::from("100"))));
        assert_eq!(updater.phase, Some(SdePhase::Downloading));
        assert_eq!(updater.log.len(), 3);
    }

    #[test]
    fn each_kind_of_info_is_recorded_and_logged() {
        let mut updater = DatabaseUpdater::default();
        updater.set_phase(SdePhase::Checking);
        updater.set_info(SdeInfo::Versions {
            installed: Some(String::from("90")),
            available: String::from("100"),
        });
        assert_eq!(
            updater.versions,
            Some((Some(String::from("90")), String::from("100")))
        );
        updater.set_info(SdeInfo::Downloaded(50_540_000));
        assert_eq!(updater.downloaded, Some(50_540_000));
        assert!(updater.log.last().unwrap().contains("48.2 MB"));
        let before = updater.log.len();
        updater.set_info(SdeInfo::Verified);
        assert_eq!(updater.log.len(), before + 1);
    }

    #[test]
    fn finishing_needs_a_running_update_and_keeps_the_window_open() {
        let mut updater = DatabaseUpdater::default();
        updater.finish();
        assert!(!updater.finished);
        assert!(updater.log.is_empty());

        updater.set_phase(SdePhase::Verifying);
        updater.finish();
        assert!(updater.finished);
        assert!(updater.phase_started.is_none());
        assert!(updater.phase.is_some());
    }

    #[test]
    fn hiding_forgets_everything_and_a_new_update_starts_clean() {
        let mut updater = DatabaseUpdater::default();
        updater.set_phase(SdePhase::Checking);
        updater.set_info(SdeInfo::Downloaded(1));
        updater.finish();
        updater.hide();
        assert!(updater.phase.is_none());
        assert!(!updater.finished);
        assert!(updater.downloaded.is_none());
        assert!(updater.log.is_empty());

        updater.set_phase(SdePhase::Checking);
        assert_eq!(updater.log.len(), 1);
        assert!(!updater.finished);
    }

    #[test]
    fn a_fresh_database_passes_its_integrity_check() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        assert!(DatabaseUpdater::verify(&connection).is_ok());
    }

    #[test]
    fn a_cancelled_run_forgets_the_recorded_build_number() {
        let dir = std::env::temp_dir().join(format!("telescope-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let urls = BuildUrls::default();
        let build_file = dir.join(format!("sde-{}.build", urls.sde_variant));
        std::fs::write(&build_file, "100").unwrap();

        let _guard = super::serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        assert!(!DatabaseUpdater::cancelled(&dir, &urls));
        assert!(build_file.exists());

        DatabaseUpdater::request_cancel();
        assert!(DatabaseUpdater::cancelled(&dir, &urls));
        assert!(!build_file.exists());

        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `run` and `spawn` against a local HTTP server that plays CCP's SDE index:
/// no test here reaches the real network or a real `sde.db`.
#[cfg(test)]
mod updater_run_tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc;

    const INDEX: &str = "{\"_key\": \"sde\", \"buildNumber\": 100, \"releaseDate\": \"x\"}\n";
    const ZIP_PATH: &str = "/eve-online-static-data-100-jsonl.zip";

    /// Answers each request with the body registered for its path, or 404.
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match std::io::Read::read(&mut stream, &mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = match routes.iter().find(|(route, _)| *route == path) {
                    Some((_, body)) => ("200 OK", body.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    /// An address nothing listens on.
    fn dead_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/", listener.local_addr().unwrap())
    }

    fn urls(base: &str) -> BuildUrls {
        BuildUrls {
            sde_variant: String::from("jsonl"),
            sde_url: base.to_string(),
            maps_url: base.to_string(),
            deltas_url: format!("{base}deltas/"),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "telescope-updater-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file with the SQLite header, standing for the installed `sde.db`.
    fn installed_database(dir: &Path) -> PathBuf {
        let path = dir.join("sde.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute("CREATE TABLE marker (x)", []).unwrap();
        path
    }

    fn summary(message: &Message) -> String {
        match message {
            Message::DatabaseUpdateProgress(phase) => format!("progress:{phase:?}"),
            Message::DatabaseUpdateInfo(SdeInfo::Versions {
                installed,
                available,
            }) => format!("versions:{installed:?}->{available}"),
            Message::DatabaseUpdateInfo(SdeInfo::Downloaded(bytes)) => {
                format!("downloaded:{bytes}")
            }
            Message::DatabaseUpdateInfo(SdeInfo::Verified) => String::from("verified"),
            Message::DatabaseUpdateInfo(SdeInfo::Deltas(count)) => format!("deltas:{count}"),
            Message::DatabaseUpdateInfo(SdeInfo::Bumped(build)) => format!("bumped:{build}"),
            Message::DatabaseUpdateInfo(SdeInfo::FullBuild(reason)) => format!("full:{reason}"),
            Message::DatabaseUpdated(rebuilt) => format!("updated:{rebuilt}"),
            Message::GenericNotification((_, _, _, text)) => format!("note:{text}"),
            _ => String::from("other"),
        }
    }

    fn drain(receiver: &mut mpsc::Receiver<Message>) -> Vec<String> {
        std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|message| summary(&message))
            .collect()
    }

    fn run_update(
        sde_path: &Path,
        data_dir: &Path,
        urls: &BuildUrls,
    ) -> (Result<Outcome, Error>, Vec<String>) {
        let (sender, mut receiver) = mpsc::channel(64);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(DatabaseUpdater::run(
            sde_path,
            data_dir,
            &data_dir.join("sde"),
            false,
            urls,
            &sender,
        ));
        (result, drain(&mut receiver))
    }

    fn build_file(dir: &Path) -> PathBuf {
        dir.join("sde-jsonl.build")
    }

    fn zip_file(dir: &Path) -> PathBuf {
        dir.join("sde-jsonl.zip")
    }

    /// Waits for the thread `spawn` started to release the update lock.
    fn wait_until_idle() {
        for _ in 0..1000 {
            if !UPDATE_IN_PROGRESS.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the update thread did not finish");
    }

    // ---- run ----

    #[test]
    fn an_installed_current_build_is_up_to_date_and_downloads_nothing() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("current");
        let database = installed_database(&dir);
        std::fs::write(build_file(&dir), "100\n").unwrap();
        std::fs::write(zip_file(&dir), "old zip").unwrap();
        let base = serve(vec![("/latest.jsonl", INDEX.as_bytes().to_vec())]);

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert_eq!(result.unwrap(), Outcome::UpToDate);
        assert_eq!(
            messages,
            ["progress:Checking", "versions:Some(\"100\")->100"]
        );
        assert_eq!(std::fs::read_to_string(zip_file(&dir)).unwrap(), "old zip");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_build_is_downloaded_and_a_cancel_stops_before_extracting() {
        let _guard = serial();
        let dir = temp_dir("cancel");
        let database = installed_database(&dir);
        std::fs::write(build_file(&dir), "99").unwrap();
        std::fs::write(zip_file(&dir), "old zip").unwrap();
        let payload = b"not really a zip".to_vec();
        let base = serve(vec![
            ("/latest.jsonl", INDEX.as_bytes().to_vec()),
            (ZIP_PATH, payload.clone()),
        ]);
        CANCEL_REQUESTED.store(true, Ordering::SeqCst);

        let (result, messages) = run_update(&database, &dir, &urls(&base));
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);

        assert_eq!(result.unwrap(), Outcome::Cancelled);
        assert_eq!(
            messages,
            [
                "progress:Checking".to_string(),
                "versions:Some(\"99\")->100".to_string(),
                "progress:Downloading".to_string(),
                format!("downloaded:{}", payload.len()),
            ]
        );
        // The zip was replaced, and the recorded build forgotten so the next
        // check does not take the old database for current.
        assert_eq!(std::fs::read(zip_file(&dir)).unwrap(), payload);
        assert!(!build_file(&dir).exists());
        assert!(is_sqlite(&database));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_zip_fails_the_update_and_keeps_the_installed_database() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("badzip");
        let database = installed_database(&dir);
        let before = std::fs::read(&database).unwrap();
        std::fs::write(build_file(&dir), "99").unwrap();
        let base = serve(vec![
            ("/latest.jsonl", INDEX.as_bytes().to_vec()),
            (ZIP_PATH, b"not really a zip".to_vec()),
        ]);

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert!(result.is_err());
        assert!(messages.contains(&String::from("progress:Extracting")));
        assert!(!messages.contains(&String::from("progress:Rebuilding")));
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert!(!building_path(&database).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_download_is_an_error_and_keeps_the_recorded_build() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("nodownload");
        let database = installed_database(&dir);
        std::fs::write(build_file(&dir), "99").unwrap();
        // The index is there, the zip is not (404).
        let base = serve(vec![("/latest.jsonl", INDEX.as_bytes().to_vec())]);

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert!(result.is_err());
        assert!(messages.contains(&String::from("progress:Downloading")));
        assert_eq!(std::fs::read_to_string(build_file(&dir)).unwrap(), "99");
        assert!(is_sqlite(&database));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_with_an_installed_database_counts_as_up_to_date() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("offline");
        let database = installed_database(&dir);

        let (result, messages) = run_update(&database, &dir, &urls(&dead_server()));

        assert_eq!(result.unwrap(), Outcome::UpToDate);
        assert_eq!(messages, ["progress:Checking", "progress:Downloading"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offline_without_a_database_fails_and_builds_nothing() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("firstrun");
        let database = dir.join("sub").join("sde.db");

        let (result, messages) = run_update(&database, &dir, &urls(&dead_server()));

        assert!(result.is_err());
        assert!(messages.contains(&String::from("progress:Extracting")));
        assert!(!database.exists());
        assert!(!building_path(&database).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_index_without_the_sde_build_is_treated_like_no_index() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("noindex");
        let database = installed_database(&dir);
        let base = serve(vec![(
            "/latest.jsonl",
            b"{\"_key\": \"other\", \"buildNumber\": 1}\n".to_vec(),
        )]);

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert_eq!(result.unwrap(), Outcome::UpToDate);
        assert!(
            !messages
                .iter()
                .any(|message| message.starts_with("versions"))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- run, with sde-deltas ----

    /// CCP's index at build 101, released long after any test runs (so
    /// sde-deltas never counts as lagging).
    const INDEX_101: &str =
        "{\"_key\": \"sde\", \"buildNumber\": 101, \"releaseDate\": \"2999-01-01T00:00:00Z\"}\n";

    /// Where the mirror's zip lives for the tests: `run_update` uses `dir`
    /// as the data directory.
    fn mirror_archive(dir: &Path) -> PathBuf {
        Workspace::new(dir.join("sde.db"), dir.to_path_buf(), dir.join("sde")).mirror_archive()
    }

    /// A `sde.db` fingerprinted at build 100 with Telescope's settings, and
    /// in `dir` a mirror (as one zip) at build 100 of a single `types` table
    /// whose `name` the parser reads.
    fn delta_ready(dir: &Path) -> PathBuf {
        let config = DatabaseUpdater::parser_config(false);
        let sde_dir = dir.join("sde");
        std::fs::create_dir_all(&sde_dir).unwrap();
        std::fs::write(
            sde_dir.join("types.jsonl"),
            "{\"_key\":1,\"name\":{\"en\":\"A\"}}\n",
        )
        .unwrap();
        let mut usage = sde::builder::usage::FieldUsage::default();
        usage.insert("types", "name");
        // Kept the way a build leaves it: one zip in the data directory,
        // and nothing decompressed.
        let mirror =
            sde::builder::mirror::Mirror::create(&sde_dir, &usage, "100", &config).unwrap();
        mirror.pack(&mirror_archive(dir)).unwrap();
        update::release_working_copy(&sde_dir).unwrap();

        let path = dir.join("sde.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        schema::create_schema(&connection).unwrap();
        let fingerprint = sde::objects::SdeFingerprint {
            sde_build: Some(String::from("100")),
            language: config.language.clone(),
            position_2d: config.position_2d,
            map_kspace: config.map_kspace,
            map_wspace: config.map_wspace,
            map_abyssal: config.map_abyssal,
            map_void: config.map_void,
            with_gates: config.with_gates,
            with_moons: config.with_moons,
            with_third_party: config.with_third_party,
            with_icebelts: None,
            with_triglavian_status: None,
            with_jove_observatories: None,
            with_special_ore: None,
        };
        let (force, axis) = fingerprint.position_2d.fingerprint_columns();
        connection
            .execute(
                "INSERT INTO sdeFingerprint (id, sdeBuild, language, forceIsometricPosition2d, \
                 isometricProjectedAxis, mapKspace, mapWspace, mapAbyssal, mapVoid, withGates, \
                 withMoons, withThirdParty, hash) \
                 VALUES (1, '100', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    fingerprint.language,
                    force,
                    axis,
                    fingerprint.map_kspace,
                    fingerprint.map_wspace,
                    fingerprint.map_abyssal,
                    fingerprint.map_void,
                    fingerprint.with_gates,
                    fingerprint.with_moons,
                    fingerprint.with_third_party,
                    fingerprint.hash(),
                ],
            )
            .unwrap();
        path
    }

    /// sde-deltas publishing build 101 (from 100), whose manifest lists
    /// `tables` and whose delta is `delta`, next to CCP's index at 101.
    fn delta_routes(tables: &str, delta: &str) -> Vec<(&'static str, Vec<u8>)> {
        use sha2::Digest;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(delta.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();
        let sha256: String = sha2::Sha256::digest(&compressed)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        vec![
            ("/latest.jsonl", INDEX_101.as_bytes().to_vec()),
            (
                "/deltas/index.json",
                br#"{"formatVersion":1,"firstBuild":101,"latestBuild":101,"builds":[
                    {"build":101,"lastBuild":100,"releaseDate":"2999-01-01T00:00:00Z","verification":"ok"}]}"#
                    .to_vec(),
            ),
            (
                "/deltas/101/manifest.json",
                format!(
                    r#"{{"formatVersion":1,"build":101,"lastBuild":100,"tables":{tables},
                        "files":{{"delta.jsonl.gz":{{"bytes":1,"sha256":"{sha256}"}}}}}}"#
                )
                .into_bytes(),
            ),
            ("/deltas/101/delta.jsonl.gz", compressed),
        ]
    }

    #[test]
    fn a_build_changing_nothing_the_maps_use_only_moves_the_recorded_build() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("bump");
        let database = delta_ready(&dir);
        let base = serve(delta_routes(r#"{"skins":{"added":1}}"#, ""));

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert_eq!(result.unwrap(), Outcome::Bumped);
        assert_eq!(
            messages,
            [
                "progress:Checking",
                "versions:Some(\"100\")->101",
                "progress:Downloading",
                "deltas:1",
                "bumped:101",
            ]
        );
        assert_eq!(installed_build(&database).as_deref(), Some("101"));
        // Nothing was downloaded from CCP.
        assert!(!zip_file(&dir).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancelling_after_the_mirror_was_unpacked_leaves_nothing_in_the_sde_directory() {
        let _guard = serial();
        let dir = temp_dir("delta-cancel");
        let database = delta_ready(&dir);
        let before = std::fs::read(&database).unwrap();
        // A change to a field the parser reads: the database must be rebuilt
        // from the mirror, which `prepare` unpacks into `dir/sde`.
        let delta = r#"{"table":"types","id":1,"op":"changed","fields":[{"path":"name.en","old":"A","new":"B"}]}"#;
        let base = serve(delta_routes(r#"{"types":{"changed":1}}"#, delta));
        CANCEL_REQUESTED.store(true, Ordering::SeqCst);

        let (result, _) = run_update(&database, &dir, &urls(&base));
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);

        assert_eq!(result.unwrap(), Outcome::Cancelled);
        assert!(
            std::fs::read_dir(dir.join("sde"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "maps")
        );
        // The deltas stay applied in the archive for the next run, and the
        // database wasn't touched.
        let mirror = sde::builder::mirror::Mirror::archive_meta(&mirror_archive(&dir)).unwrap();
        assert_eq!(mirror.build, "101");
        assert_eq!(std::fs::read(&database).unwrap(), before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_database_at_the_latest_delta_is_up_to_date() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("delta-current");
        let database = delta_ready(&dir);
        let base = serve(vec![
            ("/latest.jsonl", INDEX.as_bytes().to_vec()),
            (
                "/deltas/index.json",
                br#"{"formatVersion":1,"firstBuild":100,"latestBuild":100,"builds":[
                    {"build":100,"lastBuild":99,"releaseDate":"x","verification":"ok"}]}"#
                    .to_vec(),
            ),
        ]);

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert_eq!(result.unwrap(), Outcome::UpToDate);
        assert_eq!(
            messages,
            ["progress:Checking", "versions:Some(\"100\")->100"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_schema_change_warns_and_falls_back_to_the_full_download() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let dir = temp_dir("schema");
        let database = delta_ready(&dir);
        let before = std::fs::read(&database).unwrap();
        let delta = r#"{"table":"types","op":"schema","kind":"drop_path","path":"name.en"}"#;
        // CCP's zip isn't there: the full download fails after the warning.
        let base = serve(delta_routes(r#"{"types":{"changed":0}}"#, delta));

        let (result, messages) = run_update(&database, &dir, &urls(&base));

        assert!(result.is_err());
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("note:SDE schema change")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("full:schema change")),
            "{messages:?}"
        );
        assert_eq!(std::fs::read(&database).unwrap(), before);
        // The mirror didn't move, and nothing is left unpacked.
        let mirror = sde::builder::mirror::Mirror::archive_meta(&mirror_archive(&dir)).unwrap();
        assert_eq!(mirror.build, "100");
        assert!(
            std::fs::read_dir(dir.join("sde"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "maps")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn notify_sends_a_notification_with_the_text() {
        let (sender, mut receiver) = mpsc::channel(4);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(DatabaseUpdater::notify(&sender, Type::Info, "hello"));
        assert_eq!(drain(&mut receiver), ["note:hello"]);
    }

    // ---- spawn ----

    #[test]
    fn spawn_skips_an_unconfigured_database_path() {
        let _guard = serial();
        let dir = temp_dir("nopath");
        let (sender, mut receiver) = mpsc::channel(8);

        DatabaseUpdater::spawn(
            PathBuf::new(),
            dir.clone(),
            dir.join("sde"),
            Arc::new(sender),
            false,
            urls(&dead_server()),
        );

        let messages = drain(&mut receiver);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("No SDE database path"), "{messages:?}");
        assert!(!UPDATE_IN_PROGRESS.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spawn_reports_a_corrupted_database_and_a_run_already_going() {
        let _guard = serial();
        let dir = temp_dir("running");
        let database = dir.join("sde.db");
        std::fs::write(&database, "this is not sqlite").unwrap();
        let (sender, mut receiver) = mpsc::channel(8);
        UPDATE_IN_PROGRESS.store(true, Ordering::SeqCst);

        DatabaseUpdater::spawn(
            database.clone(),
            dir.clone(),
            dir.join("sde"),
            Arc::new(sender),
            false,
            urls(&dead_server()),
        );
        UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);

        let messages = drain(&mut receiver);
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].contains("corrupted"), "{messages:?}");
        assert!(messages[1].contains("already running"), "{messages:?}");
        // Nothing touched the file.
        assert_eq!(
            std::fs::read_to_string(&database).unwrap(),
            "this is not sqlite"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn spawn_and_collect(database: &Path, dir: &Path, base: &str) -> Vec<String> {
        let (sender, mut receiver) = mpsc::channel(64);
        DatabaseUpdater::spawn(
            database.to_path_buf(),
            dir.to_path_buf(),
            dir.join("sde"),
            Arc::new(sender),
            false,
            urls(base),
        );
        wait_until_idle();
        drain(&mut receiver)
    }

    #[test]
    fn a_spawned_check_of_a_current_database_reports_and_finishes() {
        let _guard = serial();
        let dir = temp_dir("spawn-current");
        let database = installed_database(&dir);
        std::fs::write(build_file(&dir), "100").unwrap();
        std::fs::write(zip_file(&dir), "old zip").unwrap();
        let base = serve(vec![("/latest.jsonl", INDEX.as_bytes().to_vec())]);

        let messages = spawn_and_collect(&database, &dir, &base);

        assert_eq!(
            messages.last().map(String::as_str),
            Some("updated:false"),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message == "note:SDE database is already up to date."),
            "{messages:?}"
        );
        assert!(!UPDATE_IN_PROGRESS.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_spawned_update_that_fails_reports_the_error_and_releases_the_lock() {
        let _guard = serial();
        let dir = temp_dir("spawn-fail");
        let database = dir.join("sde.db");

        let messages = spawn_and_collect(&database, &dir, &dead_server());

        assert_eq!(
            messages.last().map(String::as_str),
            Some("updated:false"),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("note:") && message.len() > "note:".len()),
            "{messages:?}"
        );
        assert!(!database.exists());
        assert!(!UPDATE_IN_PROGRESS.load(Ordering::SeqCst));
        // And a second update can start afterwards.
        let again = spawn_and_collect(&database, &dir, &dead_server());
        assert_eq!(again.last().map(String::as_str), Some("updated:false"));
        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- The window ----

    fn frame(ctx: &egui::Context, draw: impl FnMut(&egui::Context)) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 700.0),
            )),
            ..egui::RawInput::default()
        };
        let mut draw = draw;
        let mut output = ctx.run_ui(input, |ui| draw(ui.ctx()));
        output.textures_delta.clear();
    }

    #[test]
    fn the_window_only_exists_while_an_update_runs() {
        let _guard = serial();
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
        let ctx = egui::Context::default();
        let mut updater = DatabaseUpdater::default();
        let id = egui::Id::new("sde_update_window");

        frame(&ctx, |ctx| updater.show(ctx));
        assert!(ctx.memory(|memory| memory.area_rect(id)).is_none());

        updater.set_phase(SdePhase::Checking);
        frame(&ctx, |ctx| updater.show(ctx));
        frame(&ctx, |ctx| updater.show(ctx));
        assert!(ctx.memory(|memory| memory.area_rect(id)).is_some());

        updater.hide();
        frame(&ctx, |ctx| updater.show(ctx));
        assert!(updater.phase.is_none());
    }

    #[test]
    fn the_window_draws_every_phase_with_and_without_details() {
        let _guard = serial();
        for cancelling in [false, true] {
            CANCEL_REQUESTED.store(cancelling, Ordering::SeqCst);
            for phase in SdePhase::ALL {
                for finished in [false, true] {
                    let ctx = egui::Context::default();
                    let mut updater = DatabaseUpdater::default();
                    updater.set_phase(phase);
                    updater.set_info(SdeInfo::Versions {
                        installed: None,
                        available: String::from("100"),
                    });
                    updater.set_info(SdeInfo::Downloaded(50_540_000));
                    updater.set_info(SdeInfo::Verified);
                    if finished {
                        updater.finish();
                    }
                    frame(&ctx, |ctx| updater.show(ctx));
                    frame(&ctx, |ctx| updater.show(ctx));
                    assert!(
                        ctx.memory(|memory| memory.area_rect(egui::Id::new("sde_update_window")))
                            .is_some(),
                        "{phase:?} finished={finished} cancelling={cancelling}"
                    );
                }
            }
        }
        CANCEL_REQUESTED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn the_close_button_is_a_small_square_that_only_clicks_when_enabled() {
        let ctx = egui::Context::default();
        let (mut enabled_size, mut disabled_size) = (Vec2::ZERO, Vec2::ZERO);
        let (mut enabled_click, mut disabled_click) = (false, true);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 700.0),
            )),
            ..egui::RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let enabled = close_button(ui, true);
            let disabled = close_button(ui, false);
            enabled_size = enabled.rect.size();
            disabled_size = disabled.rect.size();
            enabled_click = enabled.sense.senses_click();
            disabled_click = disabled.sense.senses_click();
        });
        output.textures_delta.clear();
        assert_eq!(enabled_size, Vec2::splat(22.0));
        assert_eq!(disabled_size, Vec2::splat(22.0));
        assert!(enabled_click);
        assert!(!disabled_click);
    }
}
