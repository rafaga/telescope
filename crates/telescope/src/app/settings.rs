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

#[derive(Serialize, Deserialize, Clone)]
struct FilePaths {
    #[serde(skip)]
    settings: PathBuf,
    intel: PathBuf,
    sde: PathBuf,
    db: PathBuf,
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
        Self {
            settings: Path::new("./telescope.toml").to_path_buf(),
            intel: tpath,
            sde: PathBuf::new(),
            db: PathBuf::new(),
        }
    }
}

impl FilePaths {}

#[derive(Serialize, Deserialize, Clone)]
struct Mapping {
    pub startup_regions: Vec<usize>,
    pub warning_area: u8,
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            startup_regions: vec![],
            warning_area: 4,
        }
    }
}

impl Mapping {}

#[derive(Serialize, Deserialize)]
pub(crate) struct Channels {
    #[serde(skip)]
    available: HashMap<String, bool>,
    #[serde(skip)]
    log_files: HashMap<String, (u64, DateTime<Utc>)>,
    monitored: Arc<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Settings {
    paths: FilePaths,
    mapping: Mapping,
    channels: Channels,
    #[serde(skip)]
    factor: i64,
    #[serde(skip)]
    region_factor: i64,
    #[serde(skip)]
    saved: bool,
}

impl TryFrom<PathBuf> for Settings {
    type Error = SettingsError;

    fn try_from(path: PathBuf) -> std::result::Result<Self, <Self as TryFrom<PathBuf>>::Error> {
        let mut toml_data = String::new();
        if path.exists() {
            if let Ok(mut toml_file) = File::open(path)
                && toml_file.read_to_string(&mut toml_data).is_ok()
            {
                if let Ok(mut toml_manager) = toml::from_str::<Settings>(&toml_data) {
                    toml_manager.saved = false;
                    toml_manager.factor = 50000000000000;
                    toml_manager.region_factor = -2;
                    Ok(toml_manager)
                } else {
                    Err(SettingsError::InvalidState)
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
            factor: 50000000000000,
            region_factor: -2,
            saved: false,
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
    pub(crate) fn save(&mut self) -> Result<bool> {
        if self.saved {
            return Ok(false);
        }
        let file_path = Path::new(&self.paths.settings);
        match File::options()
            .write(true)
            .create(true)
            .truncate(true)
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
            Err(_) => Err(SettingsError::WriteError),
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
        if self.get_intel().exists() {
            return Err(SettingsError::InvalidDirectory(String::new()));
        }
        if let Ok(mut directory) = self.get_intel().read_dir() {
            while let Some(Ok(entry)) = directory.next() {
                if let Some((name, file_date)) = entry.file_name().to_string_lossy().split_once('_')
                {
                    self.channels
                        .available
                        .entry(String::from(name))
                        .or_insert(false);
                    self.channels
                        .log_files
                        .entry(String::from(name) + "_" + file_date)
                        .and_modify(|hash_entry| {
                            hash_entry.1 = Utc::now();
                            hash_entry.0 = entry.metadata().unwrap().len();
                        })
                        .or_insert((entry.metadata().unwrap().len(), Utc::now()));
                }
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

    pub fn set_db(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
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

    pub(crate) fn get_startup_regions(&self) -> &Vec<usize> {
        self.mapping.startup_regions.as_ref()
    }

    pub(crate) fn set_startup_regions(&mut self, startup_regions: Vec<usize>) {
        self.mapping.startup_regions = startup_regions;
        self.saved = false;
    }

    pub(crate) fn get_factor(&self) -> i64 {
        self.factor
    }

    pub(crate) fn get_region_factor(&self) -> i64 {
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
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsError {
    FileNotFound(String),
    InvalidDirectory(String),
    InvalidState,
    ReadError,
    WriteError,
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileNotFound(path) => write!(f, "File not found: {path}"),
            Self::InvalidState => f.write_str("invalid state"),
            Self::ReadError => f.write_str("read error"),
            Self::WriteError => f.write_str("write error"),
            Self::InvalidDirectory(path) => write!(f, "Path not found: {path}"),
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
