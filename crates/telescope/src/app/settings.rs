//! User settings, persisted to a TOML file: data paths (SDE database, player
//! database, intel directory), map options and start-up regions, and the chat
//! channels that are available and monitored.
//!
//! `Settings` also scans the intel directory for chat logs and remembers how much
//! of each log has already been read.

use crate::app::intel::IntelLogName;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// as `sde.db`/`patterns.toml`/`telescope.toml`. Not user-editable, so
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
        // into Settings -> Data Sources first. Relative and next to
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
}

fn default_alert_duration_secs() -> u32 {
    Mapping::DEFAULT_ALERT_DURATION_SECS
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            startup_regions: vec![],
            warning_area: 4,
            center_on_alert: false,
            alert_duration_secs: Mapping::DEFAULT_ALERT_DURATION_SECS,
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
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Channels {
    #[serde(skip)]
    available: HashMap<String, bool>,
    #[serde(skip)]
    log_files: HashMap<String, (u64, DateTime<Utc>)>,
    monitored: Arc<Vec<String>>,
}

/// Layout and language of the main window, remembered between runs. Saved on
/// its own, as soon as it changes (see [`Settings::save_ui_state`]), without
/// the Settings window's Save button: the log panel layout isn't edited in
/// that window, and a language change is already visible on the next frame.
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

/// Where `DatabaseUpdater` fetches the SDE from. Not user-editable (there's
/// nowhere in Settings' UI to change it), so it isn't persisted to
/// `telescope.toml` -- see [`Settings::get_sde_url`]/[`Settings::get_maps_url`]/
/// [`Settings::get_sde_variant`].
pub(crate) struct DataSourceUrls {
    pub(crate) sde_url: String,
    pub(crate) maps_url: String,
    pub(crate) sde_variant: String,
}

impl Default for DataSourceUrls {
    fn default() -> Self {
        Self {
            // CCP's official SDE index/download root.
            sde_url: String::from("https://developers.eveonline.com/static-data/tranquility/"),
            // dotlan's map SVGs, only fetched when `with_third_party` is
            // enabled (used to build `mapAbstractSystems`, see
            // `sde::builder::parser::ParserConfig::with_third_party`).
            maps_url: String::from("http://evemaps.dotlan.net/svg/"),
            // The `sde-builder` CLI also offers `"yaml"`; Telescope only
            // ever needs the smaller `jsonl` export the parser reads.
            sde_variant: String::from("jsonl"),
        }
    }
}

/// Tuning for the on-screen notification log (`app::notifications`). Not
/// user-editable, so not persisted to `telescope.toml`.
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
    /// Vertical gap between character cards.
    pub(crate) card_spacing: f32,
}

impl Default for CharacterCardStyle {
    fn default() -> Self {
        Self {
            portrait_size: 80.0,
            empty_state_height: 200.0,
            card_spacing: 4.0,
        }
    }
}

/// Internal tuning values that live alongside the user-facing settings for
/// discoverability, but aren't part of `telescope.toml` and aren't shown in
/// the Settings window -- each field has its own `Default`, reproducing the
/// value a plain `const` used to hold before it moved here.
#[derive(Default)]
pub(crate) struct InternalDefaults {
    pub(crate) data_sources: DataSourceUrls,
    pub(crate) notifications: NotificationLimits,
    pub(crate) node_style: NodeStyle,
    pub(crate) character_card: CharacterCardStyle,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Settings {
    paths: FilePaths,
    mapping: Mapping,
    channels: Channels,
    // `default`: `telescope.toml` files from before this section existed.
    #[serde(default)]
    ui: UiState,
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
            internal: InternalDefaults::default(),
            channels: Channels {
                available: HashMap::new(),
                log_files: HashMap::new(),
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
        let ui = toml::Table::try_from(&state)
            .map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        document.insert(String::from("ui"), toml::Value::Table(ui));
        let text = toml::to_string(&document)
            .map_err(|t_error| SettingsError::Other(t_error.to_string()))?;
        std::fs::write(path, text).map_err(|t_error| SettingsError::Other(t_error.to_string()))
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

    pub(crate) fn scan_channels_logs(&mut self) -> Result<()> {
        self.channels.available.clear();
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

                self.channels
                    .available
                    .entry(log.channel.to_string())
                    .or_insert(false);

                self.channels
                    .log_files
                    .entry(format!("{}_{}", log.channel, log.suffix))
                    .and_modify(|hash_entry| {
                        hash_entry.1 = Utc::now();
                        hash_entry.0 = entry.metadata().unwrap().len();
                    })
                    .or_insert_with(|| (entry.metadata().unwrap().len(), Utc::now()));
            }
            Ok(())
        } else {
            Err(SettingsError::ReadError)
        }
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

    /// Sets the player database file. Unlike the SDE, the file doesn't have
    /// to exist yet (`EsiManager` creates it); only its directory does.
    /// Takes effect the next time Telescope starts.
    pub fn set_db(&mut self, path: &Path) -> Result<()> {
        let parent_exists = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.is_dir(),
            // A bare file name lives in the working directory.
            Some(_) => true,
            None => false,
        };
        if path.as_os_str().is_empty() || path.is_dir() || !parent_exists {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.db = path.to_path_buf();
        self.saved = false;
        Ok(())
    }

    pub fn set_sde(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Err(SettingsError::InvalidDirectory(
                path.to_string_lossy().to_string(),
            ));
        }
        self.paths.sde = path.to_path_buf();
        self.saved = false;
        Ok(())
    }

    /// Test-only escape hatch around [`Self::set_sde`]'s existence
    /// check, for exercising callers (e.g.
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

    pub fn its_saved(&self) -> bool {
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

    pub(crate) fn get_log_files_channels(&self) -> HashMap<String, (u64, DateTime<Utc>)> {
        self.channels.log_files.clone()
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

    pub(crate) fn set_log_files_channels(
        &mut self,
        new_log_channels: HashMap<String, (u64, DateTime<Utc>)>,
    ) {
        if self.channels.log_files != new_log_channels {
            self.channels.log_files = new_log_channels;
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

    /// CCP's SDE index/download root `DatabaseUpdater` fetches from.
    pub(crate) fn get_sde_url(&self) -> &str {
        &self.internal.data_sources.sde_url
    }

    /// dotlan's map SVG root `DatabaseUpdater` fetches from when built with
    /// third-party data.
    pub(crate) fn get_maps_url(&self) -> &str {
        &self.internal.data_sources.maps_url
    }

    /// The SDE export variant `DatabaseUpdater` downloads (`"jsonl"`).
    pub(crate) fn get_sde_variant(&self) -> &str {
        &self.internal.data_sources.sde_variant
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

    /// Geometry of a node's box on the regional maps.
    pub(crate) fn get_node_style(&self) -> NodeStyle {
        self.internal.node_style
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

    // Saving the layout patches only `[ui]`: an unsaved change made in the
    // Settings window must not reach the file this way.
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
        assert_eq!(loaded.get_ui_state(), state);
        assert_ne!(loaded.get_warning_area(), 9);
        assert!(!settings.its_saved());
    }

    #[test]
    fn db_defaults_to_a_real_file_next_to_the_app() {
        assert_eq!(Settings::default().get_db(), Path::new(FilePaths::DEFAULT_DB));
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
        let mut settings = Settings::default();
        settings.saved = true;

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
        assert_eq!(old.alert_duration_secs, Mapping::DEFAULT_ALERT_DURATION_SECS);
    }
}
