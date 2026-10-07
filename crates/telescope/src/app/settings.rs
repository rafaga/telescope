//! User settings, persisted to a TOML file: data paths (SDE database, player
//! database, intel directory), map options and start-up regions, and the chat
//! channels that are available and monitored.
//!
//! `Settings` also scans the intel directory for chat logs and remembers how much
//! of each log has already been read.

use crate::app::database_updater::is_sqlite;
use crate::app::intel::IntelLogName;
use sde::builder::BuildUrls;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use std::{
    error,
    fmt::{Display, Formatter},
};

// `default`: a settings file may leave out any path (the shipped template
// leaves out `intel`, whose default depends on the user's home folder).
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct FilePaths {
    #[serde(skip)]
    settings: PathBuf,
    intel: PathBuf,
    sde: PathBuf,
    db: PathBuf,
    /// Directory the bundled alarm sounds live in (see `app::audio`'s module
    /// docs), relative to wherever Telescope is run from -- same convention
    /// as `sde.db`/`telescope.toml`. Not user-editable, so
    /// it isn't persisted to `telescope.toml`.
    #[serde(skip)]
    alerts_dir: PathBuf,
    // Just the file name (e.g. "1_campana_info.wav"), resolved against
    // `alerts_dir` by `Settings::get_alert_sound_path` -- not the full path,
    // so a future change to `alerts_dir` doesn't require migrating every
    // `telescope.toml` already on disk.
    alert_sound: PathBuf,
}

impl Default for FilePaths {
    fn default() -> Self {
        let os_dirs = directories::BaseDirs::new().unwrap();
        let tpath = os_dirs
            .home_dir()
            .join("Documents")
            .join("EVE")
            .join("logs")
            .join("ChatLogs");
        // A real default path (instead of the previous empty `PathBuf`)
        // so `database_updater::DatabaseUpdater` has somewhere to build
        // `sde.db` on a first run without the user having to type a path
        // into Settings -> Application first. Relative and next to
        // `telescope.toml` (i.e. wherever Telescope is run from) rather
        // than an OS data/home directory -- `sde.db` is meant to sit
        // alongside the app, not get tucked away somewhere the user has
        // to go look for it.
        //
        // Same for `db` (the ESI/player database managed by
        // `webb::esi::EsiManager`): relative, so its parent (the working
        // directory) always exists and `EsiManager::new` can create it on
        // a first run. It used to default to an empty path, which SQLite
        // treats as a *private temporary* database -- a fresh, empty one
        // per connection, deleted on close -- so linking a character
        // failed (no tables) and nothing was ever persisted.
        Self {
            settings: Path::new("telescope.toml").to_path_buf(),
            intel: tpath,
            sde: Path::new("sde.db").to_path_buf(),
            db: PathBuf::from(Self::DEFAULT_DB),
            alerts_dir: PathBuf::from("assets/alerts"),
            alert_sound: PathBuf::from("1_campana_info.wav"),
        }
    }
}

impl FilePaths {
    /// Default file name of the player (ESI) database, relative to the
    /// working directory (next to `telescope.toml`).
    pub(crate) const DEFAULT_DB: &str = "telescope.db";
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Mapping {
    pub startup_regions: Vec<usize>,
    pub warning_area: u8,
    /// Center every map on the linked character an alert sounded for.
    /// `serde(default)`: settings files written before this option existed
    /// load with it off.
    #[serde(default)]
    pub center_on_alert: bool,
    /// How long the visual alert of an intel report stays on the maps, in
    /// seconds (it fades out over that time). `serde(default)`: settings
    /// files written before this option existed get the default.
    #[serde(default = "default_alert_duration_secs")]
    pub alert_duration_secs: u32,
    /// Strongest opacity of the glow over the node of a system with a linked
    /// character, 0 (off) to 1. `serde(default)`: settings files written
    /// before this option existed get the default.
    #[serde(default = "default_glow_intensity")]
    pub glow_intensity: f32,
}

fn default_alert_duration_secs() -> u32 {
    Mapping::DEFAULT_ALERT_DURATION_SECS
}

fn default_glow_intensity() -> f32 {
    Mapping::DEFAULT_GLOW_INTENSITY
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            startup_regions: vec![],
            warning_area: 4,
            center_on_alert: false,
            alert_duration_secs: Mapping::DEFAULT_ALERT_DURATION_SECS,
            glow_intensity: Mapping::DEFAULT_GLOW_INTENSITY,
        }
    }
}

impl Mapping {
    /// Default of [`Mapping::alert_duration_secs`]: four minutes.
    pub(crate) const DEFAULT_ALERT_DURATION_SECS: u32 = 240;
    /// Shortest visual alert the Settings slider allows, in seconds.
    pub(crate) const MIN_ALERT_DURATION_SECS: u32 = 10;
    /// Longest visual alert the Settings slider allows, in seconds.
    pub(crate) const MAX_ALERT_DURATION_SECS: u32 = 600;
    /// Default of [`Mapping::glow_intensity`].
    pub(crate) const DEFAULT_GLOW_INTENSITY: f32 = 0.6;
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Channels {
    /// Every channel with a log in the intel directory (plus the monitored
    /// ones without any), and whether it is monitored in the Settings
    /// window: the draft of `monitored`.
    #[serde(skip)]
    available: HashMap<String, bool>,
    /// When each channel's log last changed: the files' modification times
    /// at the last scan, updated live as monitored logs are read.
    #[serde(skip)]
    activity: HashMap<String, SystemTime>,
    monitored: Arc<Vec<String>>,
}

/// Layout and language of the main window, remembered between runs. The
/// layout is saved on its own, as soon as it changes (see
/// [`Settings::save_ui_state`]): the log panel isn't edited in the Settings
/// window. The language is, so it is saved with the other settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub(crate) struct UiState {
    /// Whether the log panel at the bottom is expanded.
    pub log_expanded: bool,
    /// Height of the expanded log panel, in points.
    pub log_height: f32,
    /// Interface language: `"auto"` (the operating system's) or the name of
    /// a `locales/` file such as `"es"`. See `crate::i18n`.
    pub language: String,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            log_expanded: true,
            log_height: 110.0,
            language: String::from(crate::i18n::AUTO),
        }
    }
}

/// What the on-screen log shows, edited in Settings -> Application.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub(crate) struct LogOptions {
    /// Whether `Type::Debug` messages are printed in the log. Hiding them
    /// only keeps them off the screen: they are still received and handled
    /// like any other message.
    pub show_debug: bool,
}

impl Default for LogOptions {
    /// Debug messages are shown in debug builds and hidden in release ones.
    fn default() -> Self {
        Self {
            show_debug: cfg!(debug_assertions),
        }
    }
}

/// Tuning for the on-screen notification log (`app::notifications`). Not
/// user-editable, so not persisted to `telescope.toml`.
#[derive(Clone)]
pub(crate) struct NotificationLimits {
    /// Maximum number of entries kept in `TelescopeApp::app_messages`
    /// before the oldest are dropped.
    pub(crate) max_app_messages: usize,
    /// How long an incoming notification must differ from the one
    /// immediately before it to be shown, rather than collapsed as a
    /// duplicate.
    pub(crate) dedup_window: std::time::Duration,
    /// Smallest height the expanded log panel can be dragged to, in points.
    pub(crate) log_panel_min_height: f32,
}

impl Default for NotificationLimits {
    fn default() -> Self {
        Self {
            max_app_messages: 500,
            dedup_window: std::time::Duration::from_secs(1),
            log_panel_min_height: 60.0,
        }
    }
}

/// Geometry of a node's box on the regional maps (`app::tiles::Template`),
/// before the map's `zoom` multiplier. Not user-editable, so not persisted
/// to `telescope.toml`. `Copy`: read once per [`app::tiles::Template`]
/// (constructed per pane, not per frame) and handed around by value.
/// `Debug`: recorded as a field by the `#[tracing::instrument]` on
/// `RegionPane::new`/`TelescopeApp::generate_pane`, which don't skip it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeStyle {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) corner_radius: f32,
    /// Width of the border (drawn outside the box).
    pub(crate) border: f32,
    /// Strongest opacity of the character glow over the node background,
    /// so the label on top of it stays readable.
    pub(crate) glow_max_alpha: f32,
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            width: 90.0,
            height: 35.0,
            corner_radius: 10.0,
            border: 2.0,
            glow_max_alpha: 0.6,
        }
    }
}

/// Layout of the Settings -> Characters page (`windows::settings::characters`).
/// Not user-editable, so not persisted to `telescope.toml`.
#[derive(Clone, Copy)]
pub(crate) struct CharacterCardStyle {
    /// Side of the square character portrait, in points.
    pub(crate) portrait_size: f32,
    /// Height of the placeholder shown when no character is linked.
    pub(crate) empty_state_height: f32,
}

impl Default for CharacterCardStyle {
    fn default() -> Self {
        Self {
            portrait_size: 64.0,
            empty_state_height: 120.0,
        }
    }
}

/// Internal tuning values that live alongside the user-facing settings for
/// discoverability, but aren't part of `telescope.toml` and aren't shown in
/// the Settings window -- each field has its own `Default`, reproducing the
/// value a plain `const` used to hold before it moved here.
#[derive(Default, Clone)]
pub(crate) struct InternalDefaults {
    /// Where `DatabaseUpdater` fetches the SDE from. Not user-editable
    /// (there's nowhere in Settings' UI to change it), so it isn't
    /// persisted to `telescope.toml` -- see
    /// [`Settings::get_data_source_urls`].
    ///
    /// [`sde::builder::BuildUrls`] rather than a Telescope-owned struct:
    /// as of `sde` 0.5.0 (already a dependency, with the `builder` feature
    /// enabled for `database_updater.rs`) its `Default` impl already
    /// returns the exact CCP SDE / dotlan maps / `jsonl` values Telescope
    /// needs, so a local copy of the same three defaults would just be
    /// duplication to keep in sync by hand.
    pub(crate) data_sources: BuildUrls,
    pub(crate) notifications: NotificationLimits,
    pub(crate) node_style: NodeStyle,
    pub(crate) character_card: CharacterCardStyle,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Settings {
    paths: FilePaths,
    mapping: Mapping,
    channels: Channels,
    // `default`: `telescope.toml` files from before this section existed.
    #[serde(default)]
    ui: UiState,
    // `default`: `telescope.toml` files from before this section existed.
    #[serde(default)]
    log: LogOptions,
    #[serde(skip)]
    internal: InternalDefaults,
    #[serde(skip)]
    factor: f64,
    #[serde(skip)]
    region_factor: f64,
    #[serde(skip)]
    saved: bool,
}

impl TryFrom<PathBuf> for Settings {
    type Error = SettingsError;

    fn try_from(path: PathBuf) -> std::result::Result<Self, <Self as TryFrom<PathBuf>>::Error> {
        let mut toml_data = String::new();
        if path.exists() {
            if let Ok(mut toml_file) = File::open(&path)
                && toml_file.read_to_string(&mut toml_data).is_ok()
            {
                match toml::from_str::<Settings>(&toml_data) {
                    Ok(mut toml_manager) => {
                        toml_manager.paths.settings = path.to_path_buf();
                        // `telescope.toml` files written before `db` had a
                        // real default carry `db = ""`; see
                        // `FilePaths::default` for why that can't be kept.
                        if toml_manager.paths.db.as_os_str().is_empty() {
                            toml_manager.paths.db = PathBuf::from(FilePaths::DEFAULT_DB);
                        }
                        toml_manager.factor = 50000000000000.0;
                        toml_manager.region_factor = -2.0;
                        toml_manager.saved = false;
                        Ok(toml_manager)
                    }
                    Err(e) => Err(SettingsError::Other(e.to_string())),
                }
            } else {
                Err(SettingsError::ReadError)
            }
        } else {
            Err(SettingsError::FileNotFound(
                path.to_string_lossy().to_string(),
            ))
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        let mut config = Self {
            paths: FilePaths::default(),
            mapping: Mapping::default(),
            factor: 50000000000000.0,
            region_factor: -2.0,
            saved: false,
            ui: UiState::default(),
            log: LogOptions::default(),
            internal: InternalDefaults::default(),
            channels: Channels {
                available: HashMap::new(),
                activity: HashMap::new(),
                monitored: Arc::new(Vec::new()),
            },
        };
        let _ = config.scan_channels_logs();
        config
    }
}

impl Settings {
    pub(crate) fn get_ui_state(&self) -> UiState {
        self.ui.clone()
    }

    /// Stores the main window layout and writes it to `telescope.toml` right
    /// away -- only its `[ui]` table, patched into the file as it is on disk,
    /// so settings edited in the Settings window but not saved yet stay
    /// unsaved. When the file doesn't exist yet, the next regular save
    /// writes the layout with everything else.
    pub(crate) fn save_ui_state(&mut self, state: UiState) -> Result<()> {
        self.ui = state.clone();
        let path = Path::new(&self.paths.settings);
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(path)
            .map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        let mut document: toml::Table =
            toml::from_str(&text).map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        let mut ui = toml::Table::try_from(&state)
            .map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        // The language is edited in the Settings window like any other
        // setting: it reaches the file only with the rest of them (`save`).
        match document
            .get("ui")
            .and_then(|table| table.get("language"))
            .cloned()
        {
            Some(language) => {
                ui.insert(String::from("language"), language);
            }
            None => {
                ui.remove("language");
            }
        }
        document.insert(String::from("ui"), toml::Value::Table(ui));
        let text = toml::to_string(&document)
            .map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        std::fs::write(path, text).map_err(|t_error| SettingsError::Other(t_error.to_string()))
    }

    /// Whether debug messages are printed in the log.
    pub(crate) fn get_show_debug_log(&self) -> bool {
        self.log.show_debug
    }

    pub(crate) fn set_show_debug_log(&mut self, value: bool) {
        if self.log.show_debug != value {
            self.log.show_debug = value;
            self.saved = false;
        }
    }

    /// Takes the log panel layout of `state`, leaving the language as it is.
    pub(crate) fn set_layout(&mut self, state: &UiState) {
        self.ui.log_expanded = state.log_expanded;
        self.ui.log_height = state.log_height;
    }

    pub(crate) fn save(&mut self) -> Result<bool> {
        if self.saved {
            return Ok(false);
        }
        let file_path = Path::new(&self.paths.settings);
        let mut ancestors = file_path.ancestors();
        if ancestors.next().is_none() {
            return Err(SettingsError::FileNotFound(String::new()));
        }
        match File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .read(true)
            .open(file_path)
        {
            Ok(mut toml_file) => {
                if let Ok(toml_data) = toml::to_string(self)
                    && toml_file.write_all(toml_data.as_bytes()).is_ok()
                {
                    self.saved = true;
                    Ok(true)
                } else {
                    Err(SettingsError::WriteError)
                }
            }
            Err(e) => Err(SettingsError::Other(e.to_string())),
        }
    }

    pub(crate) fn get_cloned_monitored_channels(&self) -> Arc<Vec<String>> {
        self.channels.monitored.clone()
    }

    pub(crate) fn set_monitored_channels(&mut self, monitored_channels: Vec<String>) {
        self.channels.monitored = Arc::new(monitored_channels);
        self.saved = false;
    }

    /// Lists the channels with a log in the intel directory and when each
    /// last changed. A channel keeps the monitored flag it had (a change not
    /// saved yet survives a rescan); one seen for the first time is flagged
    /// if it is monitored. Monitored channels without any log stay listed,
    /// so saving never drops them.
    pub(crate) fn scan_channels_logs(&mut self) -> Result<()> {
        let previous = std::mem::take(&mut self.channels.available);
        self.channels.activity.clear();
        let monitored = Arc::clone(&self.channels.monitored);
        let flag = |channel: &str| {
            previous
                .get(channel)
                .copied()
                .unwrap_or_else(|| monitored.iter().any(|name| name == channel))
        };
        for channel in monitored.iter() {
            self.channels
                .available
                .insert(channel.clone(), flag(channel));
        }
        if !self.get_intel().exists() {
            return Err(SettingsError::InvalidDirectory(String::new()));
        }

        if let Ok(mut directory) = self.get_intel().read_dir() {
            while let Some(Ok(entry)) = directory.next() {
                let file_name = entry.file_name();
                let full_name = file_name.to_string_lossy();

                let Some(log) = IntelLogName::parse(&full_name) else {
                    // not a valid chatlog file name, ignoring it
                    continue;
                };

                let channel = log.channel.to_string();
                let monitored_flag = flag(&channel);
                self.channels
                    .available
                    .entry(channel.clone())
                    .or_insert(monitored_flag);
                if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                    let latest = self.channels.activity.entry(channel).or_insert(modified);
                    if modified > *latest {
                        *latest = modified;
                    }
                }
            }
            Ok(())
        } else {
            Err(SettingsError::ReadError)
        }
    }

    /// When each channel's log last changed (see [`Channels`]).
    pub(crate) fn get_channel_activity(&self) -> &HashMap<String, SystemTime> {
        &self.channels.activity
    }

    /// Records that `channel`'s log changed at `when` (a monitored log was
    /// just read).
    pub(crate) fn note_channel_activity(&mut self, channel: &str, when: SystemTime) {
        self.channels.activity.insert(channel.to_string(), when);
    }

    pub fn set_intel(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.intel = path.to_path_buf();
        self.saved = false;
        Ok(())
    }

    pub fn get_intel(&self) -> &Path {
        self.paths.intel.as_path()
    }

    pub fn get_settings(&self) -> &Path {
        self.paths.settings.as_path()
    }

    pub fn get_sde(&self) -> &Path {
        self.paths.sde.as_path()
    }

    pub fn get_db(&self) -> &Path {
        self.paths.db.as_path()
    }

    /*pub fn set_settings(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.settings = path.to_path_buf();
        Ok(())
    }*/

    /// Whether `path` can be a file to create or open: it isn't empty or a
    /// folder, and its folder exists. The file itself doesn't have to.
    fn is_file_path_in_existing_folder(path: &Path) -> bool {
        let parent_exists = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.is_dir(),
            // A bare file name lives in the working directory.
            Some(_) => true,
            None => false,
        };
        !path.as_os_str().is_empty() && !path.is_dir() && parent_exists
    }

    /// Sets the player database file. The file doesn't have to exist yet
    /// (`EsiManager` creates it); only its directory does. Takes effect the
    /// next time Telescope starts.
    pub fn set_db(&mut self, path: &Path) -> Result<()> {
        if !Self::is_file_path_in_existing_folder(path) {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.db = path.to_path_buf();
        self.saved = false;
        Ok(())
    }

    /// Sets the SDE database file. Like the player database, the file
    /// doesn't have to exist yet (`DatabaseUpdater` builds it there when the
    /// change is applied); only its directory does.
    ///
    /// A file that does exist must be empty or a SQLite database: the
    /// updater replaces whatever it finds at this path with the database it
    /// builds, so it must not be some other file the user typed by mistake.
    pub fn set_sde(&mut self, path: &Path) -> Result<()> {
        let holds_something_else = path.is_file()
            && std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
            && !is_sqlite(path);
        if !Self::is_file_path_in_existing_folder(path) || holds_something_else {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.sde = path.to_path_buf();
        self.saved = false;
        Ok(())
    }

    /// Test-only escape hatch around [`Self::set_sde`]'s path
    /// checks, for exercising callers (e.g.
    /// `TelescopeApp::sde_build_cache_dir`) against paths -- like an
    /// empty one -- that can legitimately show up in a `Settings` loaded
    /// from an old `telescope.toml` (predating
    /// `FilePaths::default`'s current, always-non-empty `sde` default)
    /// but that `set_sde` itself would otherwise refuse to construct in
    /// a test.
    #[cfg(test)]
    pub(crate) fn set_sde_for_test(&mut self, path: &Path) {
        self.paths.sde = path.to_path_buf();
    }

    /// Settings whose files all live in `dir`, with no channels: what the
    /// tests build an app on, so nothing outside `dir` is read or written.
    #[cfg(test)]
    pub(crate) fn in_dir_for_test(dir: &Path) -> Self {
        let mut settings = Self::default();
        settings.paths.settings = dir.join("telescope.toml");
        settings.paths.intel = dir.join("ChatLogs");
        settings.paths.sde = dir.join("sde.db");
        settings.paths.db = dir.join("players.db");
        settings.channels = Channels {
            available: HashMap::new(),
            activity: HashMap::new(),
            monitored: Arc::new(Vec::new()),
        };
        settings
    }

    /// Whether everything was written by the last `save` (the Settings
    /// window now compares against its snapshot instead).
    #[cfg(test)]
    pub(crate) fn its_saved(&self) -> bool {
        self.saved
    }

    pub(crate) fn get_warning_area(&self) -> u8 {
        self.mapping.warning_area
    }

    pub(crate) fn set_warning_area(&mut self, new_limit: u8) {
        self.mapping.warning_area = new_limit;
        self.saved = false;
    }

    /// How long the visual alert of an intel report lasts on the maps.
    pub(crate) fn get_alert_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.get_alert_duration_secs()))
    }

    pub(crate) fn get_alert_duration_secs(&self) -> u32 {
        self.mapping.alert_duration_secs.clamp(
            Mapping::MIN_ALERT_DURATION_SECS,
            Mapping::MAX_ALERT_DURATION_SECS,
        )
    }

    pub(crate) fn set_alert_duration_secs(&mut self, secs: u32) {
        self.mapping.alert_duration_secs = secs.clamp(
            Mapping::MIN_ALERT_DURATION_SECS,
            Mapping::MAX_ALERT_DURATION_SECS,
        );
        self.saved = false;
    }

    pub(crate) fn get_center_on_alert(&self) -> bool {
        self.mapping.center_on_alert
    }

    pub(crate) fn set_center_on_alert(&mut self, value: bool) {
        self.mapping.center_on_alert = value;
        self.saved = false;
    }

    pub(crate) fn get_startup_regions(&self) -> &Vec<usize> {
        self.mapping.startup_regions.as_ref()
    }

    pub(crate) fn set_startup_regions(&mut self, startup_regions: Vec<usize>) {
        self.mapping.startup_regions = startup_regions;
        self.saved = false;
    }

    pub(crate) fn get_factor(&self) -> f64 {
        self.factor
    }

    pub(crate) fn get_region_factor(&self) -> f64 {
        self.region_factor
    }

    pub(crate) fn get_available_channels(&self) -> HashMap<String, bool> {
        self.channels.available.clone()
    }

    pub(crate) fn set_available_channels(&mut self, new_available_channels: HashMap<String, bool>) {
        if self.channels.available != new_available_channels {
            self.channels.available = new_available_channels;
            self.saved = false;
        }
    }

    /// Where the alarm sounds actually are: `paths.alerts_dir` under the
    /// first place Telescope's shipped files are found (the working
    /// directory, or the executable's folder once installed -- see
    /// `crate::app_dirs`), or `paths.alerts_dir` itself if none has it.
    pub(crate) fn alerts_dir(&self) -> PathBuf {
        crate::app_dirs::find_resource(&self.paths.alerts_dir)
            .unwrap_or_else(|| self.paths.alerts_dir.clone())
    }

    /// The selected alarm sound's file name (e.g. `"1_campana_info.wav"`),
    /// not a path you can open directly -- see [`Self::get_alert_sound_path`]
    /// for that.
    pub(crate) fn get_alert_sound(&self) -> &Path {
        self.paths.alert_sound.as_path()
    }

    /// [`Self::get_alert_sound`] resolved against [`Self::alerts_dir`], ready
    /// to hand to `File::open` -- what `app::audio::AlarmPlayer::play_alarm`
    /// actually plays.
    pub(crate) fn get_alert_sound_path(&self) -> PathBuf {
        self.alerts_dir().join(&self.paths.alert_sound)
    }

    /// CCP's SDE index/download root, dotlan's map SVG root, and the SDE
    /// export variant (`"jsonl"`) `DatabaseUpdater` fetches from -- bundled
    /// as a single [`sde::builder::BuildUrls`] (rather than three separate
    /// getters) so `DatabaseUpdater::spawn`/`run` can each take one param
    /// for this instead of three, keeping both under clippy's
    /// `too_many_arguments` threshold.
    pub(crate) fn get_data_source_urls(&self) -> &BuildUrls {
        &self.internal.data_sources
    }

    /// Cap on `TelescopeApp::app_messages`, the on-screen notification log.
    pub(crate) fn get_max_app_messages(&self) -> usize {
        self.internal.notifications.max_app_messages
    }

    /// How close together two notifications have to arrive to be collapsed
    /// as duplicates.
    pub(crate) fn get_notification_dedup_window(&self) -> std::time::Duration {
        self.internal.notifications.dedup_window
    }

    /// Smallest height the expanded log panel can be dragged to.
    pub(crate) fn get_log_panel_min_height(&self) -> f32 {
        self.internal.notifications.log_panel_min_height
    }

    /// Geometry of a node's box on the regional maps, with the character
    /// glow at [`Mapping::glow_intensity`].
    pub(crate) fn get_node_style(&self) -> NodeStyle {
        NodeStyle {
            glow_max_alpha: self.mapping.glow_intensity,
            ..self.internal.node_style
        }
    }

    /// Opacity of the character glow, 0 (off) to 1.
    pub(crate) fn get_glow_intensity(&self) -> f32 {
        self.mapping.glow_intensity
    }

    pub(crate) fn set_glow_intensity(&mut self, intensity: f32) {
        let intensity = intensity.clamp(0.0, 1.0);
        if self.mapping.glow_intensity != intensity {
            self.mapping.glow_intensity = intensity;
            self.saved = false;
        }
    }

    /// Sets the interface language (see `crate::i18n`) as an unsaved change:
    /// it is written with the rest of the settings.
    pub(crate) fn set_language(&mut self, language: &str) {
        if self.ui.language != language {
            self.ui.language = language.to_string();
            self.saved = false;
        }
    }

    /// Layout of the Settings -> Characters page.
    pub(crate) fn get_character_card_style(&self) -> CharacterCardStyle {
        self.internal.character_card
    }

    /// `name` is just a file name (what `windows::settings::intelligence`'s
    /// picker lists from reading [`Self::alerts_dir`]), not a path -- this
    /// joins it against that directory to check it really exists before
    /// accepting it.
    pub fn set_alert_sound(&mut self, name: &str) -> Result<()> {
        let full = self.alerts_dir().join(name);
        if !full.exists() {
            return Err(SettingsError::InvalidDirectory(
                full.to_string_lossy().to_string(),
            ));
        }
        self.paths.alert_sound = PathBuf::from(name);
        self.saved = false;
        Ok(())
    }

    /// Test-only escape hatch around [`Self::set_alert_sound`]'s existence
    /// check, for the same reason [`Self::set_sde_for_test`] exists: `cargo
    /// test` runs this crate's test binary with its working directory set
    /// to the package root (`crates/telescope`), not the workspace root
    /// `alerts_dir` is actually relative to, so the real check can never
    /// pass in a test without reaching outside the test process to change
    /// its working directory -- which this deliberately avoids, to not
    /// risk interfering with any other test that (now or later) reads a
    /// relative path while this one has it pointed elsewhere. Also sets
    /// `saved = false`, unlike `set_sde_for_test`, since tests that need
    /// this (the dirty-flag and save/reload round-trip tests) need that
    /// side effect specifically, and skipping the filesystem check doesn't
    /// change what a real accepted name would have done to it.
    #[cfg(test)]
    pub(crate) fn set_alert_sound_for_test(&mut self, name: &str) {
        self.paths.alert_sound = PathBuf::from(name);
        self.saved = false;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsError {
    FileNotFound(String),
    InvalidDirectory(String),
    ReadError,
    WriteError,
    Other(String),
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound(path) => write!(f, "File not found: {path}"),
            Self::ReadError => f.write_str("read error"),
            Self::WriteError => f.write_str("write error"),
            Self::InvalidDirectory(path) => write!(f, "Path not found: {path}"),
            Self::Other(message) => write!(f, "Other Error: {message}"),
        }
    }
}

impl error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

pub type Result<T> = std::result::Result<T, SettingsError>;
impl SettingsError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Creates (and clears) a scratch directory under the OS temp dir,
    /// unique to this test process, so parallel test runs don't collide.
    fn temp_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "telescope-settings-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    // Regression test for the intel-directory-change bug: `set_intel` only
    // accepts paths that already exist, so `scan_channels_logs` must scan
    // successfully for exactly the directories it can ever actually be
    // called with -- an inverted `if self.get_intel().exists()` guard here
    // used to bail out before scanning in precisely that (only realistic)
    // case, silently leaving `available` empty and making the Settings UI
    // report "No intel channels detected" no matter what was in the folder.
    #[test]
    fn scan_channels_logs_populates_available_channels_for_an_existing_directory() {
        let dir = temp_dir("existing");
        fs::write(dir.join("Local_20230101_000000_12345.txt"), b"").unwrap();
        fs::write(dir.join("wc.Vale+Tribute_20230101_000000_12345.txt"), b"").unwrap();

        let mut settings = Settings::default();
        settings.set_intel(&dir).unwrap();

        assert!(settings.scan_channels_logs().is_ok());

        let available = settings.get_available_channels();
        assert!(available.contains_key("Local"));
        assert!(available.contains_key("wc.Vale+Tribute"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_channels_logs_errors_when_the_directory_does_not_exist() {
        let mut settings = Settings::default();
        // `set_intel` itself rejects nonexistent paths, so the field is set
        // directly here to reach `scan_channels_logs` with a path that
        // doesn't exist -- exercising the one branch this guard is actually
        // meant to cover.
        settings.paths.intel = PathBuf::from("/nonexistent/telescope-test-path-xyz");

        assert!(settings.scan_channels_logs().is_err());
    }

    // An old `telescope.toml` has no `[ui]` table: the defaults apply.
    #[test]
    fn ui_state_defaults_when_missing_from_the_file() {
        let dir = temp_dir("ui-missing");
        let path = dir.join("telescope.toml");
        let settings = Settings::default();
        let mut document: toml::Table =
            toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        document.remove("ui");
        fs::write(&path, toml::to_string(&document).unwrap()).unwrap();

        let loaded = Settings::try_from(path).unwrap();
        assert_eq!(loaded.get_ui_state(), UiState::default());
    }

    // A `[ui]` table written before `language` existed keeps its layout and
    // follows the operating system's language.
    #[test]
    fn language_defaults_to_auto_in_an_older_ui_table() {
        let dir = temp_dir("ui-no-language");
        let path = dir.join("telescope.toml");
        let settings = Settings::default();
        let mut document: toml::Table =
            toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        let mut ui = toml::Table::new();
        ui.insert(String::from("log_expanded"), toml::Value::Boolean(false));
        document.insert(String::from("ui"), toml::Value::Table(ui));
        fs::write(&path, toml::to_string(&document).unwrap()).unwrap();

        let loaded = Settings::try_from(path).unwrap().get_ui_state();
        assert!(!loaded.log_expanded);
        assert_eq!(loaded.language, crate::i18n::AUTO);
    }

    // Saving the layout patches only `[ui]`, and not its language: an
    // unsaved change made in the Settings window must not reach the file
    // this way.
    #[test]
    fn save_ui_state_only_writes_the_ui_table() {
        let dir = temp_dir("ui-save");
        let path = dir.join("telescope.toml");
        let mut settings = Settings::default();
        settings.paths.settings = path.clone();
        settings.save().unwrap();
        settings.set_warning_area(9);

        let state = UiState {
            log_expanded: false,
            log_height: 222.0,
            language: String::from("es"),
        };
        settings.save_ui_state(state.clone()).unwrap();

        let loaded = Settings::try_from(path).unwrap();
        assert_eq!(
            loaded.get_ui_state(),
            UiState {
                language: UiState::default().language,
                ..state
            }
        );
        assert_ne!(loaded.get_warning_area(), 9);
        assert!(!settings.its_saved());
    }

    #[test]
    fn db_defaults_to_a_real_file_next_to_the_app() {
        assert_eq!(
            Settings::default().get_db(),
            Path::new(FilePaths::DEFAULT_DB)
        );
    }

    // Regression test: a `telescope.toml` from before `db` had a default
    // carries `db = ""`, which SQLite opens as a throwaway temp database.
    #[test]
    fn an_empty_db_in_an_old_toml_falls_back_to_the_default() {
        let dir = temp_dir("empty-db");
        let path = dir.join("telescope.toml");
        let mut settings = Settings::default();
        settings.paths.db = PathBuf::new();
        fs::write(&path, toml::to_string(&settings).unwrap()).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("db = \"\""));

        let loaded = Settings::try_from(path).unwrap();
        assert_eq!(loaded.get_db(), Path::new(FilePaths::DEFAULT_DB));
    }

    #[test]
    fn set_db_accepts_a_new_file_in_an_existing_directory() {
        let dir = temp_dir("set-db-new");
        let mut settings = Settings::default();
        let db = dir.join("new.db");
        settings.set_db(&db).unwrap();
        assert_eq!(settings.get_db(), db.as_path());
        assert!(!settings.its_saved());
    }

    #[test]
    fn set_db_rejects_missing_directories_directories_and_empty_paths() {
        let dir = temp_dir("set-db-bad");
        let mut settings = Settings::default();
        assert!(settings.set_db(&dir.join("missing").join("x.db")).is_err());
        assert!(settings.set_db(&dir).is_err());
        assert!(settings.set_db(Path::new("")).is_err());
        assert_eq!(settings.get_db(), Path::new(FilePaths::DEFAULT_DB));
    }

    #[test]
    fn alert_sound_default_is_1_campana_info_wav() {
        let settings = Settings::default();
        assert_eq!(settings.get_alert_sound(), Path::new("1_campana_info.wav"));
    }

    #[test]
    fn get_alert_sound_path_joins_it_against_alerts_dir() {
        let mut settings = Settings::default();
        settings.set_alert_sound_for_test("9_trino_marimba.wav");

        assert_eq!(
            settings.get_alert_sound_path(),
            FilePaths::default().alerts_dir.join("9_trino_marimba.wav")
        );
    }

    // `set_alert_sound` itself can't be exercised against a real,
    // known-good file name here -- see `set_alert_sound_for_test`'s doc
    // comment for why -- but a name that doesn't exist under `alerts_dir`
    // has to fail regardless of the test binary's working directory, so
    // this much of the real function is still safe to cover directly.
    #[test]
    fn set_alert_sound_rejects_an_unknown_sound_name() {
        let mut settings = Settings::default();

        let result = settings.set_alert_sound("this-sound-does-not-exist.wav");

        assert!(matches!(result, Err(SettingsError::InvalidDirectory(_))));
        // A rejected name must not have touched the stored value.
        assert_eq!(settings.get_alert_sound(), Path::new("1_campana_info.wav"));
    }

    #[test]
    fn set_alert_sound_for_test_marks_settings_as_unsaved() {
        let mut settings = Settings {
            saved: true,
            ..Settings::default()
        };

        settings.set_alert_sound_for_test("7_gong_solemne.wav");

        assert!(!settings.its_saved());
    }

    #[test]
    fn alert_sound_survives_a_save_and_reload_round_trip() {
        let dir = temp_dir("alert-sound-roundtrip");
        let mut settings = Settings::default();
        settings.paths.settings = dir.join("telescope.toml");
        settings.set_alert_sound_for_test("7_gong_solemne.wav");

        assert_eq!(settings.save(), Ok(true));

        let reloaded = Settings::try_from(dir.join("telescope.toml")).unwrap();
        assert_eq!(reloaded.get_alert_sound(), Path::new("7_gong_solemne.wav"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shipped_settings_template_loads() {
        let text = include_str!("../../../../assets/telescope.default.toml");
        let settings: Settings = toml::from_str(text).unwrap();
        assert_eq!(settings.paths.intel, FilePaths::default().intel);
        assert_eq!(settings.ui.language, crate::i18n::AUTO);
        assert!(settings.get_startup_regions().is_empty());
        assert!(!settings.get_center_on_alert());
    }

    #[test]
    fn the_alert_duration_defaults_and_is_clamped() {
        let mut settings = Settings::default();
        assert_eq!(
            settings.get_alert_duration_secs(),
            Mapping::DEFAULT_ALERT_DURATION_SECS
        );
        settings.set_alert_duration_secs(1);
        assert_eq!(
            settings.get_alert_duration_secs(),
            Mapping::MIN_ALERT_DURATION_SECS
        );
        settings.set_alert_duration_secs(100_000);
        assert_eq!(
            settings.get_alert_duration_secs(),
            Mapping::MAX_ALERT_DURATION_SECS
        );
        let old: Mapping = toml::from_str("startup_regions = []\nwarning_area = 4\n").unwrap();
        assert_eq!(
            old.alert_duration_secs,
            Mapping::DEFAULT_ALERT_DURATION_SECS
        );
    }

    /// Settings that count as already saved, to see which calls dirty them.
    fn saved_settings() -> Settings {
        Settings {
            saved: true,
            ..Settings::default()
        }
    }

    #[test]
    fn plain_setters_store_the_value_and_mark_the_settings_unsaved() {
        let mut settings = saved_settings();
        settings.set_startup_regions(vec![3, 1]);
        assert_eq!(settings.get_startup_regions(), &vec![3, 1]);
        assert!(!settings.its_saved());

        let mut settings = saved_settings();
        settings.set_monitored_channels(vec![String::from("Intel")]);
        assert_eq!(*settings.get_cloned_monitored_channels(), vec!["Intel"]);
        assert!(!settings.its_saved());
    }

    #[test]
    fn unchanged_values_do_not_dirty_the_settings() {
        let mut settings = saved_settings();
        let available = settings.get_available_channels();
        settings.set_available_channels(available);
        let glow = settings.get_glow_intensity();
        settings.set_glow_intensity(glow);
        let language = settings.ui.language.clone();
        settings.set_language(&language);
        assert!(settings.its_saved());

        settings.set_available_channels(HashMap::from([(String::from("Intel"), true)]));
        assert!(!settings.its_saved());
    }

    #[test]
    fn the_glow_intensity_is_clamped_and_reaches_the_node_style() {
        let mut settings = saved_settings();
        settings.set_glow_intensity(7.0);
        assert_eq!(settings.get_glow_intensity(), 1.0);
        assert_eq!(settings.get_node_style().glow_max_alpha, 1.0);
        settings.set_glow_intensity(-2.0);
        assert_eq!(settings.get_glow_intensity(), 0.0);
        assert!(!settings.its_saved());
    }

    #[test]
    fn changing_the_language_is_an_unsaved_change() {
        let mut settings = saved_settings();
        settings.set_language("es");
        assert_eq!(settings.ui.language, "es");
        assert!(!settings.its_saved());
    }

    #[test]
    fn channel_activity_is_recorded_per_channel() {
        let mut settings = Settings::default();
        let first = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10);
        let second = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20);
        settings.note_channel_activity("Intel", first);
        settings.note_channel_activity("Intel", second);
        settings.note_channel_activity("Alliance", first);
        let activity = settings.get_channel_activity();
        assert_eq!(activity.len(), 2);
        assert_eq!(activity["Intel"], second);
    }

    #[test]
    fn set_sde_takes_a_file_that_may_not_exist_yet_in_a_folder_that_does() {
        let dir = temp_dir("set-sde");
        let mut settings = saved_settings();

        // The updater builds the database there: it doesn't have to exist.
        let new = dir.join("new-location.db");
        assert_eq!(settings.set_sde(&new), Ok(()));
        assert_eq!(settings.get_sde(), new.as_path());
        assert!(!settings.its_saved());

        // Not a folder, not a file in a folder that isn't there, not nothing.
        for invalid in [
            dir.clone(),
            dir.join("no-folder").join("sde.db"),
            PathBuf::new(),
        ] {
            let mut settings = saved_settings();
            assert_eq!(
                settings.set_sde(&invalid),
                Err(SettingsError::InvalidDirectory(
                    invalid.to_string_lossy().to_string()
                )),
                "{invalid:?}"
            );
            assert!(settings.its_saved());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_sde_refuses_an_existing_file_that_is_not_a_database() {
        // The updater replaces what it finds at this path.
        let dir = temp_dir("set-sde-other");
        let other = dir.join("notes.txt");
        fs::write(&other, "my notes").unwrap();
        let mut settings = saved_settings();
        assert!(settings.set_sde(&other).is_err());
        assert!(settings.its_saved());

        // An empty file, and a SQLite database, are fine.
        let empty = dir.join("empty.db");
        fs::write(&empty, b"").unwrap();
        assert_eq!(settings.set_sde(&empty), Ok(()));
        let database = dir.join("sde.db");
        fs::write(&database, b"SQLite format 3\0and the rest of the page").unwrap();
        assert_eq!(settings.set_sde(&database), Ok(()));
        assert_eq!(settings.get_sde(), database.as_path());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_internal_defaults_are_usable() {
        let settings = Settings::default();
        assert!(settings.get_max_app_messages() > 0);
        assert!(settings.get_log_panel_min_height() > 0.0);
        assert!(settings.get_notification_dedup_window() > std::time::Duration::ZERO);
        assert!(settings.get_factor().is_finite());
        assert!(settings.get_region_factor().is_finite());
        assert!(!settings.get_data_source_urls().sde_url.is_empty());
        assert_eq!(settings.get_settings(), Settings::default().get_settings());
        let card = settings.get_character_card_style();
        assert!(card.portrait_size > 0.0);
    }

    #[test]
    fn debug_messages_follow_the_build_by_default_and_the_choice_is_saved() {
        let dir = temp_dir("debug-log");
        let path = dir.join("telescope.toml");
        let mut settings = Settings::default();
        settings.paths.settings = path.clone();
        assert_eq!(settings.get_show_debug_log(), cfg!(debug_assertions));

        // The opposite of the default, so it is written and read back.
        settings.set_show_debug_log(!cfg!(debug_assertions));
        assert!(!settings.its_saved());
        settings.save().unwrap();
        let loaded = Settings::try_from(path).unwrap();
        assert_eq!(loaded.get_show_debug_log(), !cfg!(debug_assertions));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_without_the_log_table_gets_the_default() {
        let dir = temp_dir("debug-log-old");
        let path = dir.join("telescope.toml");
        let mut settings = Settings::default();
        settings.paths.settings = path.clone();
        settings.save().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let without_log: String = text.split("[log]").next().unwrap().to_string();
        fs::write(&path, without_log).unwrap();
        assert_eq!(
            Settings::try_from(path).unwrap().get_show_debug_log(),
            cfg!(debug_assertions)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_layout_takes_the_panel_and_keeps_the_language() {
        let mut settings = Settings::default();
        settings.set_language("es");
        settings.set_layout(&UiState {
            log_expanded: false,
            log_height: 321.0,
            language: String::from("fr"),
        });
        assert!(!settings.ui.log_expanded);
        assert_eq!(settings.ui.log_height, 321.0);
        assert_eq!(settings.ui.language, "es");
    }

    #[test]
    fn saving_the_layout_without_a_file_only_keeps_it_in_memory() {
        let dir = temp_dir("ui-no-file");
        let mut settings = Settings::default();
        settings.paths.settings = dir.join("telescope.toml");
        let state = UiState {
            log_expanded: false,
            log_height: 200.0,
            language: String::from("auto"),
        };
        assert_eq!(settings.save_ui_state(state.clone()), Ok(()));
        assert_eq!(settings.ui, state);
        assert!(!dir.join("telescope.toml").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_the_layout_patches_only_the_ui_table_of_the_file() {
        let dir = temp_dir("ui-patch");
        let path = dir.join("telescope.toml");
        fs::write(
            &path,
            "[other]\nkept = 1\n\n[ui]\nlanguage = \"es\"\nlog_expanded = true\nlog_height = 50.0\n",
        )
        .unwrap();
        let mut settings = Settings::default();
        settings.paths.settings = path.clone();
        settings
            .save_ui_state(UiState {
                log_expanded: false,
                log_height: 240.0,
                // Ignored: the language only reaches the file with `save`.
                language: String::from("fr"),
            })
            .unwrap();

        let document: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(document["other"]["kept"].as_integer(), Some(1));
        assert_eq!(document["ui"]["language"].as_str(), Some("es"));
        assert_eq!(document["ui"]["log_expanded"].as_bool(), Some(false));
        assert_eq!(document["ui"]["log_height"].as_float(), Some(240.0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_the_layout_does_not_invent_a_language_the_file_lacks() {
        let dir = temp_dir("ui-patch-no-language");
        let path = dir.join("telescope.toml");
        fs::write(&path, "[other]\nkept = 1\n").unwrap();
        let mut settings = Settings::default();
        settings.paths.settings = path.clone();
        settings.save_ui_state(UiState::default()).unwrap();

        let document: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(document["ui"].get("language").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_the_layout_over_an_unreadable_file_is_an_error() {
        let dir = temp_dir("ui-bad-file");
        let path = dir.join("telescope.toml");
        fs::write(&path, "this is = = not toml").unwrap();
        let mut settings = Settings::default();
        settings.paths.settings = path;
        assert!(matches!(
            settings.save_ui_state(UiState::default()),
            Err(SettingsError::Other(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_errors_read_well_and_have_no_source() {
        use std::error::Error as _;
        for (error, text) in [
            (
                SettingsError::FileNotFound(String::from("a")),
                "File not found: a",
            ),
            (
                SettingsError::InvalidDirectory(String::from("b")),
                "Path not found: b",
            ),
            (SettingsError::ReadError, "read error"),
            (SettingsError::WriteError, "write error"),
            (SettingsError::Other(String::from("c")), "Other Error: c"),
        ] {
            assert_eq!(error.to_string(), text);
            assert!(error.source().is_none());
        }
    }
}
