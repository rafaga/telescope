use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub(crate) struct FilePaths {
    #[serde(skip)]
    pub settings: String,
    pub sde_db: String,
    pub local_db: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Mapping {
    pub startup_regions: Vec<usize>,
    pub warning_area: String,  
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Manager {
    pub paths: FilePaths,
    pub mapping: Mapping,
    #[serde(skip)]
    pub factor: u64,
    #[serde(skip)]
    pub saved: bool,
}

impl Manager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn save(&mut self) {
        let file_path = Path::new(&self.paths.settings);
        let mut toml_file = File::options().write(true).open(file_path).expect("Unable to create settings file.");
        let toml_data = toml::to_string_pretty(self).unwrap();
        toml_file
            .write_all(toml_data.as_bytes())
            .expect("Unable to write settings on file.");
        self.saved = true;
    }

    pub(crate) fn create(&mut self) {
        let file_path = Path::new(&self.paths.settings);
        let toml_data = toml::to_string_pretty(self).unwrap();
        let mut toml_file = File::create_new(file_path).expect("Unable to create settings file.");
        toml_file
            .write_all(toml_data.as_bytes())
            .expect("Unable to write settings on new file.");
        self.saved = true;
    }

    pub(crate) fn load(&mut self) -> Self {
        let file_path = Path::new(&self.paths.settings);
        let mut toml_data = String::new();
        let mut toml_file = File::open(file_path).expect("Unable to create settings file.");
        toml_file
            .read_to_string(&mut toml_data)
            .expect("Unable to write settings on new file.");
        if let Ok(toml_formated_data) = toml::from_str::<Manager>(&toml_data) {
            if self.mapping.warning_area.parse::<i8>().is_err() {
                self.mapping.warning_area = String::from("1");
            }
            toml_formated_data
        } else {
            panic!("Invalid Data");
        }
    }
}

impl Default for Manager {
    fn default() -> Self {
        let settings_file = String::from("telescope.toml");
        let file_path = Path::new(&settings_file);

        let mut config = Self {
            paths: FilePaths {
                settings: settings_file.clone(),
                sde_db: String::from("assets/sde.db"),
                local_db: String::from("telescope.db"),
            },
            mapping: Mapping{
                startup_regions: vec![],
                warning_area: 4.to_string(),
            },
            factor: 50000000000000,
            saved: true,
        };

        if !file_path.is_file() {
            config.create();
        } else {
            config = config.load();
            config.paths.settings = settings_file.clone();
            config.factor = 50000000000000;
        }
        config.saved = true;
        config
    }
}
