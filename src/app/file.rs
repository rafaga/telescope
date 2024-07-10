use std::collections::HashMap;
use std::{fs::File,path::Path};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::{Receiver,  channel};
use tokio::runtime::Builder;

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

pub struct IntelWatcher {
    pub channels: HashMap<String,bool>,
}

impl IntelWatcher {

    pub fn new() -> Self {
        let mut obj = IntelWatcher{
            channels: HashMap::new(),
        };
        let _ = obj.scan_for_files();
        obj
    }

    fn scan_for_files(&mut self) -> Result<(),String>{
        self.channels.clear();
        let mut files = Vec::new();
        if let Some(os_dirs) = directories::BaseDirs::new() {
            let path = os_dirs.home_dir().join("Documents").join("EVE").join("logs").join("ChatLogs");
            let mut kat = path.as_path().into_iter();
            while let Some(file_path_str) = kat.next() {
                let file_path = Path::new(file_path_str);
                if file_path.is_file() {
                    files.push(file_path.file_name().unwrap());
                }
            }
            for file in files {
                if let Some((name,_file_date)) = file.to_string_lossy().split_once('_') {
                    self.channels.entry(String::from(name)).or_insert(false);
                }
            }
        } else {
            return Err(String::from("Error on path initialization"));
        }
        Ok(())
    }

    fn async_watcher(&mut self) -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
        let (tx, rx) = channel(10);
        let watcher = RecommendedWatcher::new(
            move |res| {
                
                let runtime = Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async {
                    tx.send(res).await.unwrap();
                })
            },
            Config::default(),
        )?;
        // Automatically select the best implementation for your platform.
        // You can also access each implementation directly e.g. INotifyWatcher.
        Ok((watcher, rx))
    }
    
    async fn async_watch<P: AsRef<Path>>(&mut self, path: P) -> notify::Result<()> {
        let (mut watcher, mut rx) = self.async_watcher()?;
    
        // Add a path to be watched. All files and directories at that path and
        // below will be monitored for changes.
        watcher.watch(path.as_ref(), RecursiveMode::NonRecursive)?;
    
        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => println!("changed: {:?}", event),
                Err(e) => println!("watch error: {:?}", e),
            }
        }
    
        Ok(())
    }

}