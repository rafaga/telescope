use crate::app::intel::IntelLogName;
use crate::app::messages::{Message, Type, send_app_message};
use notify::EventHandler;
use notify::event::ModifyKind;
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
                // This arm has to line up with what each backend actually
                // emits for a real content write, which differs per OS
                // (checked against notify-8.2.0's own backend source):
                //   - Windows (windows.rs, ReadDirectoryChangesW): every
                //     FILE_ACTION_MODIFIED is the untyped
                //     `Modify(ModifyKind::Any)` -- never `Data(_)` at all.
                //   - Linux (inotify.rs, IN_MODIFY): always
                //     `Modify(Data(DataChange::Any))` -- never the specific
                //     `DataChange::Content` this arm matched alone before.
                //   - macOS (fsevent.rs, kFSEventStreamEventFlagItemModified):
                //     correctly `Modify(Data(DataChange::Content))` -- the
                //     one platform where the original match actually worked.
                // `Data(_)` covers Linux (and macOS, a strict subset) in one
                // pattern; the bare `Any` arm is only reachable on Windows,
                // where "Data" is never used. Before this, a real new line
                // appended to a chatlog never triggered `IntelFileChanged`
                // on Windows *or* Linux -- it fell through to the catch-all
                // below and got logged as "Created", which is also why that
                // label kept showing up for events that weren't creations.
                notify::EventKind::Modify(ModifyKind::Data(_))
                | notify::EventKind::Modify(ModifyKind::Any) => {
                    if let Some(path) = event.paths.first().and_then(|p| p.file_name()) {
                        let file_name = path.to_string_lossy().to_string();
                        // not a chatlog name (or not a monitored channel): ignore
                        let is_monitored = IntelLogName::parse(&file_name).is_some_and(|log| {
                            channels.binary_search(&log.channel.to_string()).is_ok()
                        });
                        if is_monitored {
                            thread::spawn(move || {
                                let runtime = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .unwrap();
                                runtime.block_on(async {
                                    let _span = tracing::info_span!("spawned Auth success message")
                                        .entered();

                                    let _ = send_app_message(
                                        &app_sender_file,
                                        Message::IntelFileChanged(file_name.clone()),
                                    )
                                    .await;
                                    let _ = send_app_message(
                                        &app_sender_file,
                                        Message::GenericNotification((
                                            Type::Debug,
                                            String::from("Telescope"),
                                            String::from("IntelWatcher"),
                                            file_name + " Changed",
                                        )),
                                    )
                                    .await;
                                });
                            });
                        }
                    }
                }
                // Both broadened from `CreateKind::File`/`RemoveKind::File`
                // to the whole `CreateKind`/`RemoveKind` enum for the same
                // reason as the Modify arm above: Windows' FILE_ACTION_ADDED
                // and FILE_ACTION_REMOVED are always reported as the untyped
                // `CreateKind::Any`/`RemoveKind::Any` (windows.rs), never the
                // `File` variant either of these arms required, so on
                // Windows a new or deleted log file never triggered a
                // rescan -- it fell through to the catch-all below and got
                // logged as "Created" regardless of which of the two it
                // actually was. Linux/macOS already report the specific
                // `File`/`Folder` kind correctly in the common case, so this
                // is a no-op improvement there (it only additionally catches
                // each backend's own rarer ambiguous-kind fallback).
                notify::EventKind::Create(_) | notify::EventKind::Remove(_) => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    runtime.block_on(async {
                        let _ = send_app_message(&app_sender_file, Message::ScanIntelFiles).await;
                    });
                }
                // Genuinely unhandled kinds only now (Access, Rename,
                // Modify(Metadata(_)/Name(_)), the untyped `Any` default,
                // etc.) -- Create, Remove and content-relevant Modify events
                // are all handled above. Labels with the real `event.kind`
                // instead of a hardcoded " Created" that used to be printed
                // regardless of what actually happened, which is what made
                // unrelated events (e.g. a metadata/access-time touch right
                // after a real write) look like repeated duplicate
                // "Created" lines for the same file.
                kind => {
                    thread::spawn(move || {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        runtime.block_on(async {
                            let _ = send_app_message(
                                &app_sender_file,
                                Message::GenericNotification((
                                    Type::Debug,
                                    String::from("Telescope"),
                                    String::from("IntelWatcher"),
                                    format!(
                                        "{} {kind:?}",
                                        event
                                            .paths
                                            .first()
                                            .and_then(|p| p.file_name())
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_default()
                                    ),
                                )),
                            )
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
