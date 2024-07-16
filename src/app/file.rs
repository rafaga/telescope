use std::collections::HashMap;
use std::{fs::File,path::Path};
use std::thread;
use std::sync::Arc;
use notify::{Config, Event, RecommendedWatcher, Watcher, RecursiveMode};
use tokio::sync::mpsc::{Receiver,  channel};
use tokio::runtime::Builder;
use tokio::sync::broadcast::Sender as BCSender;
use tokio::sync::mpsc::Sender;
use crate::app::messages::{Message,MapSync};

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
    watcher: Option<RecommendedWatcher>,
    app_msg: Arc<Sender<Message>>,
    map_msg: Arc<BCSender<MapSync>>,
}

impl IntelWatcher {
    pub fn new(app_sender: Arc<Sender<Message>>, map_syncer: Arc<BCSender<MapSync>>) -> Self {
        let mut a = Self {
            channels: HashMap::new(),
            watcher: None,
            app_msg: app_sender,
            map_msg: map_syncer,
        };
        let _ = a.scan_for_files();
        a
    }

    pub fn scan_for_files(&mut self) -> Result<(),String>{
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

    fn async_watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
        let (ntx, nrx) = channel(10);
        let watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(runtime) = Builder::new_current_thread().enable_all().build(){
                    runtime.block_on(async {
                        let _r = ntx.send(res).await;
                    });
                }
            },
            Config::default(),
        )?;
        // Automatically select the best implementation for your platform.
        // You can also access each implementation directly e.g. INotifyWatcher.
        Ok((watcher, nrx))
    }

    fn translate_raw_message(message: String) {
    }

    pub fn start_watcher(&mut self) -> Result<(),String> {
        if let Ok((mut watcher,mut fs_rx)) = Self::async_watcher() {
            if let Some(os_dirs) = directories::BaseDirs::new() {
                let path = os_dirs.home_dir().join("Documents").join("EVE").join("logs").join("ChatLogs");
                if watcher.watch(&path, RecursiveMode::NonRecursive).is_ok() {
                    self.watcher = Some(watcher);
                    let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                    let app_sender = Arc::clone(&self.app_msg); 
                    thread::spawn(move || {
                        runtime.block_on(async {
                            while let Some(res) = fs_rx.recv().await {
                                match res {
                                    Ok(event) => {
                                        let path = event.paths;
                                        Self::translate_raw_message(String::new());
                                    },
                                    Err(t_error) => {

                                    },
                                }
                            }
                        });
                    });
                }
            }
        }
        Ok(())
    }

}