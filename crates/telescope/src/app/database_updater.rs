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
//! forwards those into this struct's `set_status`/`hide`, and
//! [`TelescopeApp::ui`](super::TelescopeApp::ui) calls [`Self::show`]
//! once per frame, the same way it already calls
//! `open_about_window`/`open_settings_window`/`open_debug_menu`.
//!
//! There used to be a stub here that called a nonexistent
//! `eframe::run_ui_native` and never actually checked or built anything.

use eframe::egui::{self, Align2, Vec2};
use sde::Error;
use sde::builder::parser::{Parser, ParserConfig, ProjectedAxis};
use sde::builder::{extract, http, schema, sde_index};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tokio::sync::mpsc::Sender;

use super::messages::{Message, Type, send_app_message, try_send_app_message};

/// CCP's official SDE index/download root.
const SDE_URL: &str = "https://developers.eveonline.com/static-data/tranquility/";
/// dotlan's map SVGs, only fetched when `with_third_party` is enabled
/// (used to build `mapAbstractSystems`, see [`ParserConfig::with_third_party`]).
const MAPS_URL: &str = "http://evemaps.dotlan.net/svg/";
/// The `sde-builder` CLI also offers `"yaml"`; Telescope only ever needs
/// the smaller `jsonl` export the parser reads.
const SDE_VARIANT: &str = "jsonl";

/// Guards against two update runs racing each other (e.g. the automatic
/// startup check and a manual "Check for updates" click in Settings). A
/// second call to [`DatabaseUpdater::spawn`] while one is already
/// running is reported back through `app_msg` and otherwise ignored,
/// rather than letting two pipelines fight over the same `sde.db` file.
static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// UI state for the "updating the SDE database" window, owned by
/// `TelescopeApp` and painted every frame via [`Self::show`]. Doesn't
/// drive anything itself -- `Self::spawn` is the only thing that starts
/// an update, and it runs independently of whether this window is
/// currently visible.
#[derive(Default)]
pub struct DatabaseUpdater {
    visible: bool,
    status: String,
}

impl DatabaseUpdater {
    /// Paints the progress window if an update is currently running (see
    /// `Self::set_status`/`Self::hide`) -- a no-op otherwise.
    #[tracing::instrument(skip(self, ctx))]
    pub fn show(&self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        egui::Window::new("Updating EVE Online SDE database")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.status);
                });
            });
    }

    /// Shows the window (if it wasn't already) with `status` as its
    /// message. Called from `TelescopeApp::event_manager` on
    /// [`Message::DatabaseUpdateProgress`].
    pub fn set_status(&mut self, status: String) {
        self.visible = true;
        self.status = status;
    }

    /// Hides the window. Called from `TelescopeApp::event_manager` on
    /// [`Message::DatabaseUpdated`], which is sent once the
    /// check/build finishes (successfully or not).
    pub fn hide(&mut self) {
        self.visible = false;
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
    ) {
        // It detetcs if the database has a valid format.
        // Its checks the file typoe against the SQlite Magic header
        if sde_path.exists() {
            let mut file = File::open(&sde_path).unwrap();
            let mut buf = [0u8; 16];
            if match file.read_exact(&mut buf) {
                Ok(()) => &buf == b"SQLite format 3\0",
                Err(_) => false, // file it is too small or doesn't exist, so it is not a valid SQLite database
            } {
                return;
            } else {
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
                        "No SDE database path is configured (Settings -> Data Sources); skipping the update check.",
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
                let result =
                    Self::run(&sde_path, &data_dir, &sde_dir, with_third_party, &app_msg).await;
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
        app_msg: &Sender<Message>,
    ) -> Result<bool, Error> {
        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(String::from(
                "Checking for a new EVE Online SDE build...",
            )),
        )
        .await
        .ok();

        let client = http::build_client()?;
        let changed = sde_index::update_as_needed(&client, data_dir, SDE_URL, SDE_VARIANT).await?;

        let db_exists = sde_path.exists();
        if !changed && db_exists {
            return Ok(false);
        }

        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(String::from("Downloading the new SDE export...")),
        )
        .await
        .ok();

        if db_exists {
            std::fs::remove_file(sde_path)?;
        }
        if let Some(parent) = sde_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let zip_path = data_dir.join(format!("sde-{SDE_VARIANT}.zip"));
        extract::prepare_sde_directory(&zip_path, sde_dir)?;

        // Read back the build number `sde_index::update_as_needed` just
        // wrote (or confirmed unchanged) to `sde-{SDE_VARIANT}.build`,
        // purely to record it in `sdeFingerprint` below -- mirrors
        // `sde-builder`'s own CLI. `Ok` on read failure rather than
        // propagating it: a database with no recorded build number
        // (`sdeFingerprint.sdeBuild = NULL`) is still valid, so this
        // shouldn't abort the whole build.
        let build_number =
            std::fs::read_to_string(data_dir.join(format!("sde-{SDE_VARIANT}.build")))
                .ok()
                .map(|s| s.trim().to_string());

        send_app_message(
            app_msg,
            Message::DatabaseUpdateProgress(String::from(
                "Rebuilding the SDE database, this can take a minute...",
            )),
        )
        .await
        .ok();

        let mut connection = rusqlite::Connection::open(sde_path)?;
        schema::create_schema(&connection)?;

        // `force_isometric_position_2d: true`, matching `sde-builder`
        // itself: always compute `position2DX`/`Y` locally instead of
        // trusting CCP's own precomputed value (see `ParserConfig`'s
        // docs for why).
        let parser_config = ParserConfig {
            language: "en".to_string(),
            force_isometric_position_2d: true,
            isometric_projected_axis: ProjectedAxis::Y,
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
        sde_parser
            .build_database(&mut connection, &client, MAPS_URL, build_number.as_deref())
            .await?;

        Ok(true)
    }
}
