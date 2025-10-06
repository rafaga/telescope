use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, remove_file};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
pub(crate) struct FilePaths {
    #[serde(skip)]
    pub settings: String,
    #[serde(skip)]
    pub internal_intel: Option<PathBuf>,
    pub default_behavior: bool,
    pub intel: String,
    pub sde_db: String,
    pub local_db: String,
}

#[derive(Serialize, Deserialize)]
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

impl Manager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn save(&mut self) {
        let file_path = Path::new(&self.paths.settings);
        let mut toml_file = File::options()
            .write(true)
            .open(file_path)
            .expect("Unable to create settings file.");
        let toml_data = toml::to_string(self).unwrap();
        toml_file
            .write_all(toml_data.as_bytes())
            .expect("Unable to write settings on file.");
        self.saved = true;
    }

    pub(crate) fn create(&mut self) {
        let file_path = Path::new(&self.paths.settings);
        let toml_data = toml::to_string(self).unwrap();
        let mut toml_file = File::create_new(file_path).expect("Unable to create settings file.");
        toml_file
            .write_all(toml_data.as_bytes())
            .expect("Unable to write settings on new file.");
        self.saved = true;
    }

    pub(crate) fn check_intel_directory(&self) -> Result<Option<PathBuf>, String> {
        let intel_path = Path::new(&self.paths.intel);
        if intel_path.exists() {
            Ok(Some(intel_path.to_path_buf()))
        } else if let Some(os_dirs) = directories::BaseDirs::new() {
            let t_path = os_dirs
                .home_dir()
                .join("Documents")
                .join("EVE")
                .join("logs")
                .join("ChatLogs");
            if t_path.exists() {
                Ok(Some(t_path))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    pub(crate) fn load(&mut self) -> Result<(), String> {
        let file_path = Path::new(&self.paths.settings);
        let mut toml_data = String::new();
        let mut toml_file = File::open(file_path).expect("Unable to create settings file.");
        toml_file
            .read_to_string(&mut toml_data)
            .expect("Unable to write settings on new file.");
        if let Ok(toml_formatted_data) = toml::from_str::<Manager>(&toml_data) {
            self.mapping = toml_formatted_data.mapping;
            self.channels.monitored = toml_formatted_data.channels.monitored;
            self.paths.local_db = toml_formatted_data.paths.local_db;
            self.paths.sde_db = toml_formatted_data.paths.sde_db;
            self.paths.default_behavior = toml_formatted_data.paths.default_behavior;
            self.paths.intel = toml_formatted_data.paths.intel;
            self.scan_for_files()?;
            if self.mapping.warning_area.parse::<i8>().is_err() {
                self.mapping.warning_area = String::from("1");
            }
            for channel in self.channels.monitored.iter() {
                self.channels
                    .available
                    .entry(channel.to_string())
                    .and_modify(|val| *val = true);
            }
            Ok(())
        } else {
            Err(String::from("Invalid Data"))
        }
    }

    pub fn scan_for_files(&mut self) -> Result<bool, String> {
        self.channels.available.clear();
        match &self.paths.internal_intel {
            Some(path) => {
                if let Ok(mut directory) = path.as_path().read_dir() {
                    while let Some(Ok(entry)) = directory.next() {
                        if let Some((name, file_date)) =
                            entry.file_name().to_string_lossy().split_once('_')
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
                    Ok(true)
                } else {
                    Err(String::from("Error on Intel path setup"))
                }
            }
            None => Ok(false),
        }
    }
}

impl Default for Manager {
    fn default() -> Self {
        let settings_file = String::from("telescope.toml");
        let file_path = Path::new(&settings_file);
        let mut path = None;

        if let Some(os_dirs) = directories::BaseDirs::new() {
            let t_path = os_dirs
                .home_dir()
                .join("Documents")
                .join("EVE")
                .join("logs")
                .join("ChatLogs");
            if t_path.exists() {
                path = Some(t_path)
            }
        }

        let mut config = Self {
            paths: FilePaths {
                internal_intel: path,
                settings: settings_file.clone(),
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

        if !file_path.is_file() || (config.load().is_err() && remove_file(file_path).is_ok()) {
            config.create();
            let _result = config.scan_for_files();
        }
        config.saved = true;
        config
    }
}
