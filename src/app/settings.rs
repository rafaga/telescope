
use std::path::Path;
use serde::{Serialize,Deserialize};
use std::fs::File;
use std::io::{Write,Read};

#[derive(Serialize,Deserialize)]
pub(crate) struct FilePaths {
    pub settings:String,
    pub sde_db:String,
    pub local_db: String,
}

#[derive(Serialize,Deserialize)]
pub(crate) struct Manager{
    pub paths: FilePaths,
    #[serde(skip)]
    pub factor: u64,
    pub startup_regions: Vec<usize>,
}

impl Manager {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn save(&self) {
        let file_path = Path::new(&self.paths.settings);
        let toml_data = toml::to_string_pretty(self).unwrap();
        let mut toml_file = File::create_new(file_path).expect("Unable to create settings file.");
        toml_file.write_all(toml_data.as_bytes()).expect("Unable to write settings on new file.");
    }

    pub(crate) fn load(&self) -> Self {
        let file_path = Path::new(&self.paths.settings);
        let mut toml_data = String::new();
        let mut toml_file = File::open(file_path).expect("Unable to create settings file.");
        toml_file.read_to_string(&mut toml_data).expect("Unable to write settings on new file.");
        if let Ok(toml_formated_data) = toml::from_str(&toml_data){
            toml_formated_data
        } else {
            panic!("Invalid Data");
        }
    }
}

impl Default for Manager{
    fn default() -> Self {
        let settings_file = String::from("telescope.toml");
        let file_path = Path::new(&settings_file);

        let mut config = Self {
            paths: FilePaths{
                settings: settings_file.clone(),
                sde_db: String::from("assets/sde.db"),
                local_db: String::from("telescope.db"),
            },
            startup_regions:vec![],
            factor: 50000000000000
        };

        if !file_path.is_file() {
            config.save();
        } else {
            config = config.load();
        }
        config
    }
}