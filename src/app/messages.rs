use hyper::Server;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::time::{timeout_at, Duration, Instant};
use webb::auth_service::MakeSvc;

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    SystemNotification(usize),
    PlayerMoved((usize, usize)),
}

pub enum Type {
    Info,
    Error,
    Warning,
}

#[derive(Clone)]
pub enum Target {
    System,
    Region,
}

#[derive(PartialEq)]
pub enum SettingsPage {
    Intelligence,
    DataSources,
}

pub enum Message {
    EsiAuthSuccess((String, String)),
    GenericNotification((Type, String, String, String)),
    NewRegionalPane(usize),
    MapHidden(usize),
    MapShown(usize),
}

pub enum CharacterSync {
    Add(usize),
    Remove(usize),
}

pub struct MessageSpawner {
    spawn: Arc<mpsc::Sender<Message>>,
}

impl MessageSpawner {
    pub fn new(sender: Arc<mpsc::Sender<Message>>) -> Self {
        // Set up a channel for communicating.
        // Build the runtime for the new thread.
        //
        // The runtime is created before spawning the thread
        // to more cleanly forward errors if the `unwrap()`
        // panics.

        Self { spawn: sender }
    }

    pub fn spawn(&self, msg: Message) {
        if self.spawn.blocking_send(msg).is_err() {
            panic!("The shared runtime has shut down.");
        }
    }
}

async fn handle_auth(time: usize, tx: Arc<Sender<Message>>) {
    let addr: SocketAddr = ([127, 0, 0, 1], 56123).into();
    let (atx, mut arx) = mpsc::channel::<(String, String)>(1);
    match Server::try_bind(&addr) {
        Ok(builder) => {
            let server = builder
                .serve(MakeSvc::new(Arc::new(atx)))
                .with_graceful_shutdown(async {
                    while let Some(result) = arx.recv().await {
                        let _ = tx.send(Message::EsiAuthSuccess(result)).await;
                    }
                });
            let result =
                timeout_at(Instant::now() + Duration::from_secs(time as u64), server).await;
            if let Err(t_error) = result {
                let _ = tx
                    .send(Message::GenericNotification((
                        Type::Error,
                        String::from("MessageSpawner"),
                        String::from("handle_auth"),
                        t_error.to_string(),
                    )))
                    .await;
            } else {
                let _ = tx
                    .send(Message::GenericNotification((
                        Type::Info,
                        String::from("MessageSpawner"),
                        String::from("handle_auth"),
                        String::from("logged in"),
                    )))
                    .await;
            }
        }
        Err(t_error) => {
            let _ = tx
                .send(Message::GenericNotification((
                    Type::Error,
                    String::from("MessageSpawner"),
                    String::from("handle_auth"),
                    t_error.to_string(),
                )))
                .await;
        }
    };
}

pub struct AuthSpawner {
    spawn: Arc<mpsc::Sender<usize>>,
}

impl AuthSpawner {
    pub fn new(msg_tx: Arc<mpsc::Sender<Message>>) -> Self {
        // Set up a channel for communicating.
        let (send, mut recv) = mpsc::channel(3);
        let arc_send = Arc::new(send);

        let obj = Self { spawn: arc_send };
        // Build the runtime for the new thread.
        //
        // The runtime is created before spawning the thread
        // to more cleanly forward errors if the `unwrap()`
        // panics.
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        std::thread::spawn(move || {
            rt.block_on(async move {
                while let Some(time) = recv.recv().await {
                    let cloned_msg_sender = Arc::clone(&msg_tx);
                    tokio::spawn(handle_auth(time, cloned_msg_sender));
                }
                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
        });

        obj
    }

    pub fn spawn(&self) {
        if self.spawn.blocking_send(60).is_err() {
            panic!("The shared runtime has shut down.");
        }
    }
}
