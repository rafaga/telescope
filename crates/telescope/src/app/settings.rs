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

#[derive(Serialize, Deserialize)]
pub(crate) struct FilePaths {
    #[serde(skip)]
    pub settings: PathBuf,
    #[serde(skip)]
    pub internal_intel: Option<PathBuf>,
    pub default_behavior: bool,
    pub intel: String,
    pub sde_db: String,
    pub local_db: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Mapping {
    pub startup_regions: Vec<usize>,
    pub warning_area: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Channels {
    #[serde(skip)]
    pub available: HashMap<String, bool>,
    #[serde(skip)]
    pub log_files: HashMap<String, (u64, DateTime<Utc>)>,
    pub monitored: Arc<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Manager {
    pub paths: FilePaths,
    pub mapping: Mapping,
    pub channels: Channels,
    #[serde(skip)]
    pub factor: i64,
    #[serde(skip)]
    pub region_factor: i64,
    #[serde(skip)]
    pub saved: bool,
}

impl TryFrom<PathBuf> for Manager {
    type Error = ManagerError;

    fn try_from(path: PathBuf) -> std::result::Result<Self, <Self as TryFrom<PathBuf>>::Error> {
        let mut toml_data = String::new();
        if path.exists() {
            if let Ok(mut toml_file) = File::open(path)
                && toml_file.read_to_string(&mut toml_data).is_ok()
            {
                if let Ok(mut toml_manager) = toml::from_str::<Manager>(&toml_data) {
                    toml_manager.saved = false;
                    toml_manager.factor = 50000000000000;
                    toml_manager.region_factor = -2;
                    if !toml_manager.paths.intel.is_empty()
                        && let Ok(pbuf) = toml_manager.verify_intel_path()
                    {
                        toml_manager.paths.internal_intel = Some(pbuf.clone());
                        toml_manager.paths.intel = pbuf.to_string_lossy().to_string();
                    }
                    if toml_manager.scan_channels_logs().is_ok() {
                        Ok(toml_manager)
                    } else {
                        Err(ManagerError::ReadError)
                    }
                } else {
                    Err(ManagerError::InvalidState)
                }
            } else {
                Err(ManagerError::ReadError)
            }
        } else {
            Err(ManagerError::FileNotFound(
                path.to_string_lossy().to_string(),
            ))
        }
    }
}

impl Manager {
    pub(crate) fn new() -> Self {
        let settings_file = String::from("telescope.toml");
        let file_path = Path::new(&settings_file);
        let mut config = Self {
            paths: FilePaths {
                internal_intel: None,
                settings: file_path.to_path_buf(),
                default_behavior: false,
                intel: String::new(),
                sde_db: String::from("assets/sde.db"),
                local_db: String::from("telescope.db"),
            },
            mapping: Mapping {
                startup_regions: vec![],
                warning_area: 4.to_string(),
            },
            factor: 50000000000000,
            region_factor: -2,
            saved: true,
            channels: Channels {
                available: HashMap::new(),
                log_files: HashMap::new(),
                monitored: Arc::new(Vec::new()),
            },
        };

        let _ = config.verify_intel_path();
        config
    }

    pub(crate) fn write(&mut self) -> Result<()> {
        let file_path = Path::new(&self.paths.settings);
        match File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(file_path)
        {
            Ok(mut toml_file) => {
                let toml_data = toml::to_string(self).unwrap();
                toml_file
                    .write_all(toml_data.as_bytes())
                    .expect("Unable to write settings on file.");
                self.saved = true;
                Ok(())
            }
            Err(_) => Err(ManagerError::WriteError),
        }
    }

    fn verify_intel_path(&mut self) -> Result<PathBuf> {
        let k_path = if let Some(t_path) = self.paths.internal_intel.clone() {
            t_path
        } else {
            if let Some(os_dirs) = directories::BaseDirs::new() {
                os_dirs
                    .home_dir()
                    .join("Documents")
                    .join("EVE")
                    .join("logs")
                    .join("ChatLogs")
            } else {
                return Err(ManagerError::ReadError);
            }
        };
        if k_path.exists() {
            let _ = self.scan_channels_logs();
            Ok(k_path.to_path_buf())
        } else {
            Err(ManagerError::InvalidDirectory(
                k_path.to_string_lossy().to_string(),
            ))
        }
    }

    fn scan_channels_logs(&mut self) -> Result<()> {
        self.channels.available.clear();
        if self.paths.internal_intel.is_none() {
            return Err(ManagerError::InvalidDirectory(String::new()));
        }
        let path = &self.paths.internal_intel.clone().unwrap();
        if let Ok(mut directory) = path.read_dir() {
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
            Err(ManagerError::ReadError)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ManagerError {
    FileNotFound(String),
    InvalidDirectory(String),
    InvalidState,
    ReadError,
    WriteError,
}

impl Display for ManagerError {
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

impl error::Error for ManagerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

pub type Result<T> = std::result::Result<T, ManagerError>;

/*impl From<std::io::Error> for ManagerError {
    fn from(e: std::io::Error) -> Self { Self::Io(e.to_string()) }
}
impl From<String> for ManagerError {
    fn from(s: String) -> Self { Self::Other(s) }
}
impl From<&str> for ManagerError {
    fn from(s: &str) -> Self { Self::Other(s.to_string()) }
}*/

impl ManagerError {
    //pub fn not_found(name: impl Into<String>) -> Self { Self::NotFound(name.into()) }
}
