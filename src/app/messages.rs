use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::time;

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


#[derive(Clone)]
pub struct TaskSpawner {
    spawn: mpsc::Sender<Message>,
}

impl TaskSpawner {
    pub fn new() -> TaskSpawner {
        // Set up a channel for communicating.
        let (send, mut recv) = mpsc::channel(16);

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
            rt.block_on(async move {
                while let Some(task) = recv.recv().await {
                    tokio::spawn(handle_message(task));
                }

                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
        });

        TaskSpawner {
            spawn: send,
        }
    }

    pub fn spawn_task(&self, msg: Message) {
        match self.spawn.blocking_send(msg) {
            Ok(()) => {},
            Err(_) => panic!("The shared runtime has shut down."),
        }
    }
}

async fn handle_auth(time: usize) {


    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let (tx, rx) = channel::<(String, String)>();
    crate::SHARED_TX.lock().await.replace(tx);
    let mut result = (String::new(), String::new());
    let server = Server::bind(&addr)
        .serve(MakeSvc::new())
        .with_graceful_shutdown(async {
            let msg = rx.await.ok();
            if let Some(values) = msg {
                result = values;
            }
        });
    let _ = timeout_at(Instant::now() + Duration::from_secs(60), server).await;
    Ok(result)

    let tx = Arc::clone(&self.app_msg.0);
    let future = async move {
        match webb::esi::EsiManager::launch_auth_server(56123) {
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
        };
    };
    self.tpool.spawn_ok(future);
}

pub struct AuthSpawner {
    spawn: mpsc::Sender<usize>,
}

impl AuthSpawner {
    pub fn new() -> AuthSpawner {
        // Set up a channel for communicating.
        let (send, mut recv) = mpsc::channel(5);

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
            rt.block_on(async move {
                while let Some(time) = recv.recv().await {
                    tokio::spawn(handle_auth(time));
                }

                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
        });

        AuthSpawner {
            spawn: send,
        }
    }

    pub fn spawn_task(&self, time: usize) {
        match self.spawn.blocking_send(time) {
            Ok(()) => {},
            Err(_) => panic!("The shared runtime has shut down."),
        }
    }
}


