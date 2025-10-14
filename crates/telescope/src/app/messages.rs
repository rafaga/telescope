use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use std::thread;
use std::{future::IntoFuture, net::SocketAddr};
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::time::{Duration, Instant, timeout_at};
use webb::auth_service::AuthService2;

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    SystemNotification((usize, Instant)),
    PlayerMoved((usize, usize)),
}

pub enum Type {
    Info,
    Error,
    Warning,
    Debug,
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
    Characters,
}

pub enum Message {
    EsiAuthSuccess((String, String)),
    GenericNotification((Type, String, String, String)),
    NewRegionalPane(usize),
    MapHidden(usize),
    MapShown(usize),
    PlayerNewLocation((i32, i32)),
    IntelFileChanged(String),
    UpdateIntelDirectory(),
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
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        // Set up a channel for communicating.
        // Build the runtime for the new thread.
        //
        // The runtime is created before spawning the thread
        // to more cleanly forward errors if the `unwrap()`
        // panics.

        Self { spawn: sender }
    }

    pub fn spawn(&self, msg: Message) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if self.spawn.blocking_send(msg).is_err() {
            panic!("The shared runtime has shut down.");
        }
    }
}

async fn handle_auth(time: usize, tx: Arc<Sender<Message>>) {
    let addr: SocketAddr = ([127, 0, 0, 1], 56123).into();
    let (atx, mut arx) = mpsc::channel::<(String, String)>(1);
    match TcpListener::bind(addr).await {
        Ok(listener) => {
            if let Ok((stream, _)) = listener.accept().await {
                let io = TokioIo::new(stream);
                let server = http1::Builder::new()
                    .serve_connection(io, AuthService2 { tx: Arc::new(atx) })
                    .into_future();

                let stx = Arc::clone(&tx);
                thread::spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    runtime.block_on(async {
                        #[cfg(feature = "puffin")]
                        puffin::profile_scope!("spawned Auth success message");

                        while let Some(result) = arx.recv().await {
                            let _send_result = stx.send(Message::EsiAuthSuccess(result)).await;
                        }
                    });
                });

                if let Err(t_error) =
                    timeout_at(Instant::now() + Duration::from_secs(time as u64), server).await
                {
                    let _ = tx
                        .send(Message::GenericNotification((
                            Type::Error,
                            String::from("MessageSpawner"),
                            String::from("handle_auth"),
                            t_error.to_string(),
                        )))
                        .await;
                } else {
                    //server.without_shutdown()
                }
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
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

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
        let cloned_msg_sender = Arc::clone(&msg_tx);
        std::thread::spawn(move || {
            rt.block_on(async move {
                #[cfg(feature = "puffin")]
                puffin::profile_scope!("spawned auth handler");

                while let Some(time) = recv.recv().await {
                    let cloned_msg_sender = Arc::clone(&cloned_msg_sender);
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
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if self.spawn.blocking_send(60).is_err() {
            panic!("The shared runtime has shut down.");
        }
    }
}
