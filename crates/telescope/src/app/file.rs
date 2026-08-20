use crate::app::messages::{Message, Type};
use notify::EventHandler;
use notify::event::{CreateKind, ModifyKind};
use std::sync::{Arc, RwLock};
use std::thread;
use tokio::sync::mpsc::Sender;

pub struct IntelEventHandler {
    app_msg: Arc<Sender<Message>>,
    // Shared with `TelescopeApp` so the monitored-channel list can be
    // updated in place after the user changes the intel directory or edits
    // the channel selection in Settings; `IntelEventHandler` is moved into
    // the `notify::Watcher` at construction time and can never be replaced,
    // so a plain snapshot here would go stale forever after the first save.
    channels: Arc<RwLock<Vec<String>>>,
}

impl EventHandler for IntelEventHandler {
    fn handle_event(&mut self, event: Result<notify::Event, notify::Error>) {
        let channels = match self.channels.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if channels.is_empty() {
            return;
        }
        if let Ok(event) = event {
            let app_sender_file = Arc::clone(&self.app_msg);
            match event.kind {
                notify::EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)) => {
                    if let Some(path) = event.paths[0].file_name() {
                        let file_name = path.to_string_lossy().to_string();
                        let splitted_file_name = file_name.split_once('_').unwrap();
                        if channels
                            .binary_search(&splitted_file_name.0.to_string())
                            .is_ok()
                        {
                            thread::spawn(move || {
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .unwrap();
                                runtime.block_on(async {
                                    let _span = tracing::info_span!("spawned Auth success message")
                                        .entered();

                                    let _ = app_sender_file
                                        .send(Message::IntelFileChanged(file_name.clone()))
                                        .await;
                                    let _ = app_sender_file
                                        .send(Message::GenericNotification((
                                            Type::Debug,
                                            String::from("Telescope"),
                                            String::from("IntelWatcher"),
                                            file_name + " Changed",
                                        )))
                                        .await;
                                });
                            });
                        }
                    }
                }
                notify::EventKind::Create(CreateKind::File) => {}
                _ => {
                    thread::spawn(move || {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        runtime.block_on(async {
                            let _ = app_sender_file
                                .send(Message::GenericNotification((
                                    Type::Debug,
                                    String::from("Telescope"),
                                    String::from("IntelWatcher"),
                                    event.paths[0]
                                        .file_name()
                                        .unwrap()
                                        .to_str()
                                        .unwrap()
                                        .to_owned()
                                        + " Created",
                                )))
                                .await;
                        });
                    });
                }
            }
        }
    }
}

impl IntelEventHandler {
    #[tracing::instrument(skip(app_sender))]
    pub fn new(channels: Arc<RwLock<Vec<String>>>, app_sender: Arc<Sender<Message>>) -> Self {
        Self {
            app_msg: app_sender,
            channels,
        }
    }
}
