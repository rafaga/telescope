use crate::app::messages::{Message, Type};
use notify::EventHandler;
use notify::event::{CreateKind, ModifyKind};
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc::Sender;

pub struct IntelEventHandler {
    app_msg: Arc<Sender<Message>>,
    channels: Arc<Vec<String>>,
}

impl EventHandler for IntelEventHandler {
    fn handle_event(&mut self, event: Result<notify::Event, notify::Error>) {
        if self.channels.is_empty() {
            return;
        }
        if let Ok(event) = event {
            let app_sender_file = Arc::clone(&self.app_msg);
            match event.kind {
                notify::EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)) => {
                    if let Some(path) = event.paths[0].file_name() {
                        let file_name = path.to_string_lossy().to_string();
                        let splitted_file_name = file_name.split_once('_').unwrap();
                        if self
                            .channels
                            .binary_search(&splitted_file_name.0.to_string())
                            .is_ok()
                        {
                            thread::spawn(move || {
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .unwrap();
                                runtime.block_on(async {
                                    #[cfg(feature = "puffin")]
                                    puffin::profile_scope!("spawned Auth success message");

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
    pub fn new(channels: Arc<Vec<String>>, app_sender: Arc<Sender<Message>>) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();
        Self {
            app_msg: app_sender,
            channels,
        }
    }
}
