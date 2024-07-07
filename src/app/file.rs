use std::fs::File;
use std::path::Path;

pub(crate) struct LogManager {
    obj_file: File,
}

impl LogManager {
    pub fn new() -> Self {
        Self {
            obj_file: File::open(Path::new("orale.log")).expect("woah!!"),
        }
    }
}
