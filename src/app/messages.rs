use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use hyper::Server;
use std::net::SocketAddr;
use webb::auth_service::MakeSvc;
use tokio::time::{timeout_at, Instant, Duration};
use std::sync::Arc;

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    SystemNotification(usize),
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

async fn handle_message(message: Message) {
    match message {
        Message::EsiAuthSuccess((a,b)) => {},
        Message::GenericNotification((a,b,c,d)) => {},
        Message::MapHidden(id) => {},
        Message::MapShown(id) => {},
        Message::NewRegionalPane(id) => {},
    }
}


async fn handle_auth(time: usize, tx: Arc<mpsc::Sender<Message>>) {
    let addr: SocketAddr = ([127, 0, 0, 1], 56123).into();
    let mut result = (String::new(), String::new());
    let server = Server::bind(&addr)
        .serve(MakeSvc::new())
        .with_graceful_shutdown(async {
            let msg = rx.await.ok();
            match msg {
                Ok(data) => {
                    let _ = tx
                        .send(Message::EsiAuthSuccess(data))
                        .await;
                }
                Err(t_error) => {
                    let _ = tx
                        .send(Message::GenericNotification(
                            (Type::Error,
                            String::from("EsiManager"),
                            String::from("launch_auth_server"),
                            t_error.to_string())
                        ))
                        .await;
                }
            }
        });
    let _ = timeout_at(Instant::now() + Duration::from_secs(60), server).await;
    //Ok(result);
}

pub struct TaskSpawner {
    msg_spawn: mpsc::Sender<Message>,
    auth_spawn: mpsc::Sender<usize>,
    join_handlers: Vec<JoinHandle<()>>
}

impl TaskSpawner {
    pub fn new() -> TaskSpawner {
        // Set up a channel for communicating.
        let (send, mut recv) = mpsc::channel(30);
        let (auth_send, mut auth_recv) = mpsc::channel(5);

        let mut join_handlers = Vec::new();
        // Build the runtime for the new thread.
        //
        // The runtime is created before spawning the thread
        // to more cleanly forward errors if the `unwrap()`
        // panics.
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        std::thread::spawn(move || {
            let handle = rt.spawn(async move {
                while let Some(time) = recv.recv().await {
                    tokio::spawn(handle_message(time));
                }

                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
            join_handlers.push(handle);
        });

        std::thread::spawn(move || {
            let handle = rt.spawn(async move {
                while let Some(time) = auth_recv.recv().await {
                    tokio::spawn(handle_auth(time,Arc::new(send)));
                }

                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
            join_handlers.push(handle);
        });

        TaskSpawner {
            msg_spawn: send,
            auth_spawn: auth_send,
            join_handlers
        }
    }

    pub fn spawn_msg_task(&self, msg: Message) {
        match self.msg_spawn.blocking_send(msg) {
            Ok(()) => {},
            Err(_) => panic!("The shared runtime has shut down."),
        }
    }

    pub fn spawn_auth_task(&self, time: usize) {
        match self.auth_spawn.blocking_send(time) {
            Ok(()) => {},
            Err(_) => panic!("The shared runtime has shut down."),
        }
    }
}


