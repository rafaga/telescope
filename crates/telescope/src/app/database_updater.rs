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

use eframe::egui::{self, Align2, RichText, Vec2};
use egui_panels::StatusKind;
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

/// The phases of an SDE update, in the order they run; each is sent to the
/// window as [`Message::DatabaseUpdateProgress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdePhase {
    /// Asking CCP's SDE index whether there is a newer build.
    Checking,
    /// Downloading and decompressing the new export.
    Downloading,
    /// Parsing the export into a new `sde.db`.
    Rebuilding,
}

impl SdePhase {
    const ALL: [Self; 3] = [Self::Checking, Self::Downloading, Self::Rebuilding];

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
            Self::Rebuilding => t!("sde_update.step_rebuild"),
        }
        .into_owned()
    }

    /// What is happening now, under the steps.
    fn detail(self) -> String {
        match self {
            Self::Checking => t!("sde_update.checking"),
            Self::Downloading => t!("sde_update.downloading"),
            Self::Rebuilding => t!("sde_update.rebuilding"),
        }
        .into_owned()
    }
}

/// Width of the update window's content.
const WINDOW_WIDTH: f32 = 400.0;

/// UI state for the "updating the SDE database" window, owned by
/// `TelescopeApp` and painted every frame via [`Self::show`]. Doesn't
/// drive anything itself -- `Self::spawn` is the only thing that starts
/// an update, and it runs independently of whether this window is
/// currently visible.
#[derive(Default)]
pub struct DatabaseUpdater {
    /// The phase running, `None` while no update is.
    phase: Option<SdePhase>,
    /// When the window was shown, for the elapsed time.
    started: Option<Instant>,
}

impl DatabaseUpdater {
    /// Paints the progress window if an update is currently running (see
    /// `Self::set_phase`/`Self::hide`) -- a no-op otherwise. It is drawn
    /// like the Settings screen (`egui_panels`): a title with the elapsed
    /// time, a progress rail, the three phases as steps and what the
    /// current one is doing.
    #[tracing::instrument(skip(self, ctx))]
    pub fn show(&self, ctx: &egui::Context) {
        let Some(phase) = self.phase else {
            return;
        };
        egui::Window::new(t!("sde_update.title"))
            .id(egui::Id::new("sde_update_window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(egui_panels::dialog_frame(ctx))
            .show(ctx, |ui| {
                let theme = egui_panels::Theme::get(ui.ctx());
                let palette = theme.palette(ui.visuals());
                ui.set_width(WINDOW_WIDTH);
                ui.spacing_mut().item_spacing.y = theme.section_spacing;

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t!("sde_update.title"))
                                .size(theme.section_title_size + 2.0)
                                .color(palette.strong_text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let elapsed = self
                                .started
                                .map_or(0, |started| started.elapsed().as_secs());
                            egui_panels::badge(
                                ui,
                                &format!("{}:{:02}", elapsed / 60, elapsed % 60),
                            )
                            .on_hover_text(t!("sde_update.elapsed"));
                        });
                    });
                    ui.label(
                        RichText::new(t!("sde_update.description"))
                            .size(theme.small_size)
                            .color(palette.muted_text),
                    );
                });

                // Half a phase in while it runs, so the rail moves at once.
                let target = (phase.index() as f32 + 0.5) / SdePhase::ALL.len() as f32;
                let fraction = ui.ctx().animate_value_with_time(
                    egui::Id::new("sde_update_progress"),
                    target,
                    0.4,
                );
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_height(4.0)
                        .corner_radius(2)
                        .fill(palette.accent),
                );

                let steps: Vec<String> = SdePhase::ALL.iter().map(|phase| phase.step()).collect();
                let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
                egui_panels::progress_steps(ui, &steps, phase.index());

                ui.separator();
                egui_panels::status(ui, StatusKind::Info, &phase.detail());
                ui.label(
                    RichText::new(t!("sde_update.hint"))
                        .size(theme.small_size)
                        .color(palette.muted_text),
                );
            });
        // The elapsed time counts even while nothing else repaints.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// Shows the window (if it wasn't already) at `phase`. Called from
    /// `TelescopeApp::event_manager` on [`Message::DatabaseUpdateProgress`].
    pub fn set_phase(&mut self, phase: SdePhase) {
        if self.phase.is_none() {
            self.started = Some(Instant::now());
        }
        self.phase = Some(phase);
    }

    /// Hides the window. Called from `TelescopeApp::event_manager` on
    /// [`Message::DatabaseUpdated`], which is sent once the
    /// check/build finishes (successfully or not).
    pub fn hide(&mut self) {
        self.phase = None;
        self.started = None;
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
                    Ok(rebuilt) => {
                        if rebuilt {
                            Self::notify(
                                &app_msg,
                                Type::Info,
                                "SDE database updated successfully.",
                            )
                            .await;
                        } else {
                            Self::notify(
                                &app_msg,
                                Type::Info,
                                "SDE database is already up to date.",
                            )
                            .await;
                        }
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
    /// Returns `Ok(true)` if the database was (re)built, `Ok(false)` if it
    /// was already up to date (nothing to do). Errors -- no network, a
    /// malformed zip, a SQL failure -- are returned rather than panicking;
    /// the caller decides how to surface them.
    async fn run(
        sde_path: &std::path::Path,
        data_dir: &std::path::Path,
        sde_dir: &std::path::Path,
        with_third_party: bool,
        urls: &BuildUrls,
        app_msg: &Sender<Message>,
    ) -> Result<bool, Error> {
        send_app_message(app_msg, Message::DatabaseUpdateProgress(SdePhase::Checking))
            .await
            .ok();

        let client = http::build_client()?;
        let changed =
            sde_index::update_as_needed(&client, data_dir, &urls.sde_url, &urls.sde_variant)
                .await?;

        let db_exists = is_sqlite(sde_path);
        if !changed && db_exists {
            return Ok(false);
        }

        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(SdePhase::Downloading),
        )
        .await
        .ok();

        if let Some(parent) = sde_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let zip_path = data_dir.join(format!("sde-{}.zip", urls.sde_variant));
        extract::prepare_sde_directory(&zip_path, sde_dir)?;

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
        // Closed before the file is moved or removed.
        drop(connection);
        if let Err(error) = built {
            let _ = std::fs::remove_file(&building);
            return Err(error);
        }
        // One step on every platform: the old database stays whole until
        // the new one replaces it.
        std::fs::rename(&building, sde_path)?;

        Ok(true)
    }
}

/// Whether `path` is a file starting with the SQLite header (a missing,
/// unreadable or too short file isn't).
fn is_sqlite(path: &std::path::Path) -> bool {
    let mut header = [0u8; 16];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == b"SQLite format 3 ")
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
    use super::{building_path, is_sqlite};

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
