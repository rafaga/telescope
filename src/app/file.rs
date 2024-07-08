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

pub struct IntelChannelWatcher {
    pub channels: Vec<String>,
}

impl IntelChannelWatcher {

    pub fn new() -> Self {
        IntelChannelWatcher{
            channels: Vec::new()
        }
    }

    fn scan_for_files(&mut self,) {
        let path;
        self.channels.clear();
        if cfg!(target_os = "macos") {
            path = Path::new("");
        } else if cfg!(target_os = "windows") {
            path = Path::new("");
        } else if cfg!(target_os = "unix") {
            path = Path::new("");
        }
    }

    fn async_watcher(&mut self ) -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
        let (mut tx, rx) = channel(1);
    
        // Automatically select the best implementation for your platform.
        // You can also access each implementation directly e.g. INotifyWatcher.
        let watcher = RecommendedWatcher::new(
            move |res| {
                let runtime = Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async {
                    tx.send(res).await.unwrap();
                })
            },
            Config::default(),
        )?;
    
        Ok((watcher, rx))
    }
    
    async fn async_watch<P: AsRef<Path>>(&mut self, path: P) -> notify::Result<()> {
        let (mut watcher, mut rx) = self.async_watcher()?;
    
        // Add a path to be watched. All files and directories at that path and
        // below will be monitored for changes.
        watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;
    
        while let Some(res) = rx.recv().await {
            match res {
                Ok(event) => println!("changed: {:?}", event),
                Err(e) => println!("watch error: {:?}", e),
            }
        }
    
        Ok(())
    }

}