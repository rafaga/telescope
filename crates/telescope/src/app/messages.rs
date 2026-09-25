//! Messages used to communicate between the UI, the background tasks and the file
//! watcher: the [`Message`] enum (the app's central event type), the map and
//! character sync messages, and the [`MessageSpawner`] / [`send_app_message`]
//! helpers that deliver them.

use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use sputnik::map_alerts::IntelAlert;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Duration, Instant, sleep_until, timeout_at};
use webb::auth_service::AuthService2;
use webb::esi::EsiManager;
use webb::objects::AuthorizeInfo;
use webb::objects::Character;

/// How long the local OAuth callback listener waits for the browser to
/// come back from the EVE SSO login page.
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    /// An intel report on a solar system: its visual alert (unless it is a
    /// `clear` report) and its line in the node tooltip.
    SystemAlert(IntelAlert),
    /// Plays (or clears) an animation on a node of every map that has it;
    /// used by the Debug window to preview the node effects. Only the
    /// `#[cfg(debug_assertions)]` Debug window (see `app/windows.rs`'s
    /// `mod debug`) ever sends it, but the variant and its handling arms stay
    /// compiled in every profile -- gating them with the UI is what used to
    /// drop the animation match arms entirely in release. The `allow` below
    /// covers release, where nothing constructs it.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    NodeEffect((usize, NodeEffect)),
}

/// Node animations offered by `egui-map` (see `egui_map::map::NodeHandle`):
/// one-off events that end on their own, lasting states that run until
/// cleared, and `Clear` itself. Only ever constructed by the Debug window's
/// animation preview, but kept compiled in every profile -- see the note on
/// `MapSync::NodeEffect` above.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NodeEffect {
    #[default]
    Pulse,
    Ripple,
    Countdown,
    ScaleIn,
    Crosshair,
    Halo,
    Blink,
    Orbit,
    Clear,
}

#[cfg_attr(not(debug_assertions), allow(dead_code))]
impl NodeEffect {
    pub const ALL: [NodeEffect; 9] = [
        NodeEffect::Pulse,
        NodeEffect::Ripple,
        NodeEffect::Countdown,
        NodeEffect::ScaleIn,
        NodeEffect::Crosshair,
        NodeEffect::Halo,
        NodeEffect::Blink,
        NodeEffect::Orbit,
        NodeEffect::Clear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NodeEffect::Pulse => "Pulse (one-off)",
            NodeEffect::Ripple => "Ripple (one-off)",
            NodeEffect::Countdown => "Countdown (one-off)",
            NodeEffect::ScaleIn => "Scale in (one-off)",
            NodeEffect::Crosshair => "Crosshair (one-off)",
            NodeEffect::Halo => "Halo (lasting)",
            NodeEffect::Blink => "Blink (lasting)",
            NodeEffect::Orbit => "Orbit (lasting)",
            NodeEffect::Clear => "Clear effects",
        }
    }

    /// Applies the effect to a map node.
    pub fn apply(self, node: egui_map::map::NodeHandle<'_>) {
        let now = Instant::now().into();
        match self {
            NodeEffect::Pulse => node.pulse(now),
            NodeEffect::Ripple => node.ripple(now),
            NodeEffect::Countdown => node.countdown(now),
            NodeEffect::ScaleIn => node.scale_in(now),
            NodeEffect::Crosshair => node.crosshair(now),
            NodeEffect::Halo => node.halo(),
            NodeEffect::Blink => node.blink(),
            NodeEffect::Orbit => node.orbit(),
            NodeEffect::Clear => node.clear(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Info,
    Error,
    Warning,
    Debug,
}

#[derive(Clone)]
pub enum Target {
    System,
    /// Never constructed outside the Debug window (`center_on_target`'s
    /// handler for it in both map panes is an unfinished no-op stub), but
    /// kept compiled in every profile so those arms stay put -- see the note
    /// on `MapSync::NodeEffect`. The `allow` below covers release, where
    /// nothing constructs it.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    Region,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    General,
    Intelligence,
    Characters,
}

impl SettingsPage {
    /// Every settings page, in the order the Settings window menu lists
    /// them. A new page is one variant above, one entry here, one arm in
    /// `title` and one arm in the Settings window's page `match`.
    pub const ALL: [SettingsPage; 3] = [
        SettingsPage::General,
        SettingsPage::Intelligence,
        SettingsPage::Characters,
    ];

    /// Label shown for this page in the Settings window menu.
    pub fn title(self) -> String {
        match self {
            SettingsPage::General => t!("settings.pages.general"),
            SettingsPage::Intelligence => t!("settings.pages.intelligence"),
            SettingsPage::Characters => t!("settings.pages.characters"),
        }
        .into_owned()
    }
}

/// A character that finished the SSO flow on the auth thread, together with
/// the manager clone that authenticated it (its token state is adopted by the
/// UI's manager, see `EsiManagerCore::adopt_session`).
pub struct LinkedCharacter {
    pub esi: EsiManager,
    pub character: Character,
}

pub enum Message {
    /// Sent by the auth thread (`handle_auth`) once a character finished the
    /// SSO flow and its data was fetched from ESI and stored.
    CharacterAuthenticated(Box<LinkedCharacter>),
    GenericNotification((Type, String, String, String)),
    NewRegionalPane(usize),
    MapHidden(usize),
    MapShown(usize),
    PlayerNewLocation((i32, i32)),
    IntelFileChanged(String),
    UpdateIntelDirectory(PathBuf),
    DefaultIntelDirectory,
    /// Sent by `database_updater::DatabaseUpdater` while its background
    /// update check/build is running, one per phase -- drives the
    /// status text in `database_updater::DatabaseUpdater`'s progress
    /// window (`DatabaseUpdater::set_status`).
    DatabaseUpdateProgress(String),
    /// Sent by `database_updater::DatabaseUpdater` once its background
    /// update check finishes; also hides the progress window
    /// (`DatabaseUpdater::hide`). `true` means `sde.db` was (re)built
    /// and should be reloaded (see `TelescopeApp::handle_database_updated`);
    /// `false` means it was already up to date, or the check/build
    /// failed (the failure itself was already reported separately via a
    /// `GenericNotification`).
    DatabaseUpdated(bool),
    /// Sent by `IntelEventHandler` when a new intel file is created in the
    /// monitored directory or when the application initializes, to trigger
    ///  a scan of all intel files.
    ScanIntelFiles,
}

impl Message {
    /// Returns the variant's name, for lightweight tagging of spans/events
    /// (e.g. in Tracy) without dumping potentially large or arbitrary
    /// payloads (`GenericNotification`'s error text, `IntelFileChanged`'s
    /// path, etc.) into every trace.
    pub fn kind(&self) -> &'static str {
        match self {
            Message::CharacterAuthenticated(_) => "CharacterAuthenticated",
            Message::GenericNotification(_) => "GenericNotification",
            Message::NewRegionalPane(_) => "NewRegionalPane",
            Message::MapHidden(_) => "MapHidden",
            Message::MapShown(_) => "MapShown",
            Message::PlayerNewLocation(_) => "PlayerNewLocation",
            Message::IntelFileChanged(_) => "IntelFileChanged",
            Message::UpdateIntelDirectory(_) => "UpdateIntelDirectory",
            Message::DefaultIntelDirectory => "DefaultIntelDirectory",
            Message::DatabaseUpdateProgress(_) => "DatabaseUpdateProgress",
            Message::DatabaseUpdated(_) => "DatabaseUpdated",
            Message::ScanIntelFiles => "ScanIntelFiles",
        }
    }
}

pub enum CharacterSync {
    Add(usize),
    Remove(usize),
}

pub struct MessageSpawner {
    spawn: Arc<mpsc::Sender<Message>>,
}

impl MessageSpawner {
    #[tracing::instrument(skip(sender))]
    pub fn new(sender: Arc<mpsc::Sender<Message>>) -> Self {
        // Set up a channel for communicating.
        // Build the runtime for the new thread.
        //
        // The runtime is created before spawning the thread
        // to more cleanly forward errors if the `unwrap()`
        // panics.

        Self { spawn: sender }
    }

    #[tracing::instrument(skip(self, msg), fields(kind = msg.kind()))]
    pub fn spawn(&self, msg: Message) {
        // `try_send`, not `blocking_send`: every call site for this reaches
        // it from the UI thread during `TelescopeApp::update()` (directly,
        // or via a pane's `event_manager()`/`node_ui()` called from the same
        // `update()`), and the only thing that ever drains this channel --
        // `TelescopeApp::event_manager`'s `while let Ok(message) =
        // self.app_msg.1.try_recv()` -- also runs on that same UI thread,
        // once, near the top of that same `update()`. If a single frame
        // ever queued more messages than the channel's capacity (`app.rs`'s
        // `mpsc::channel::<messages::Message>(40)`), a `blocking_send` here
        // would block the UI thread waiting for room that only a `recv()`
        // on this same, now-blocked thread could free -- a self-deadlock
        // that freezes the whole app. `try_send` trades that hang for the
        // rare, non-fatal loss of a single log line, which is a strictly
        // better failure mode for a diagnostics channel.
        match self.spawn.try_send(msg) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => {
                panic!("The shared runtime has shut down.");
            }
            Err(mpsc::error::TrySendError::Full(msg)) => {
                tracing::warn!(
                    kind = msg.kind(),
                    "app message channel is full; dropping message"
                );
            }
        }
    }
}

/// Sends `msg` on `tx`, tagging the resulting Tracy zone with the
/// message's `kind()`. Centralizes instrumentation for the many call
/// sites that hold their own `Sender<Message>`/`Arc<Sender<Message>>`
/// clone and send directly, instead of going through `MessageSpawner`.
/// Never records the payload itself (see `Message::kind`).
#[tracing::instrument(skip(tx, msg), fields(kind = msg.kind()))]
pub async fn send_app_message(
    tx: &Sender<Message>,
    msg: Message,
) -> Result<(), mpsc::error::SendError<Message>> {
    tx.send(msg).await
}

/// Non-async counterpart of [`send_app_message`], for the `try_send` call
/// sites.
#[tracing::instrument(skip(tx, msg), fields(kind = msg.kind()))]
pub fn try_send_app_message(
    tx: &Sender<Message>,
    msg: Message,
) -> Result<(), mpsc::error::TrySendError<Message>> {
    tx.try_send(msg)
}

/// One "link a character" attempt, handed to [`AuthSpawner::spawn`]: a clone
/// of the UI's ESI manager to authenticate with (so the network calls never
/// touch the UI thread) and the authorization info the browser was sent to.
pub struct AuthRequest {
    pub esi: EsiManager,
    pub auth_info: AuthorizeInfo,
}

fn auth_notification(kind: Type, message: String) -> Message {
    Message::GenericNotification((
        kind,
        String::from("AuthSpawner"),
        String::from("handle_auth"),
        message,
    ))
}

/// Local port the EVE SSO redirects the browser back to (see the callback URL
/// in `AppData`).
const AUTH_CALLBACK_PORT: u16 = 56123;

/// How long the in-flight HTTP connections get to finish sending the
/// confirmation page once the callback arrived, before they are dropped.
const AUTH_RESPONSE_GRACE: Duration = Duration::from_secs(2);

/// Waits for the SSO callback, then hands it to [`complete_auth`] on its own
/// thread.
///
/// The listener only lives while waiting: it accepts every connection the
/// browser opens (preconnects, favicon, the real `/login` redirect), serves
/// them without keep-alive, and is dropped -- freeing the port -- as soon as
/// the callback arrives or [`AUTH_TIMEOUT`] expires. That way a second
/// character can be linked right after the first one: the old code kept the
/// port bound (and a keep-alive connection open) for the whole timeout, so
/// the next login either couldn't bind the port or was delivered to the
/// previous, already finished, attempt.
async fn handle_auth(request: AuthRequest, tx: Arc<Sender<Message>>) {
    let addr: SocketAddr = ([127, 0, 0, 1], AUTH_CALLBACK_PORT).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(t_error) => {
            let message = format!(
                "Can't listen for the EVE SSO login on {addr}: {t_error}. Is another Telescope running?"
            );
            let _ = send_app_message(&tx, auth_notification(Type::Error, message)).await;
            return;
        }
    };

    let (atx, mut arx) = mpsc::channel::<(String, String)>(1);
    let service = AuthService2 { tx: Arc::new(atx) };
    let mut connections = JoinSet::new();
    let deadline = Instant::now() + AUTH_TIMEOUT;
    let response = loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let service = service.clone();
                    connections.spawn(async move {
                        let _ = http1::Builder::new()
                            .keep_alive(false)
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
                Err(t_error) => {
                    let _ = send_app_message(&tx, auth_notification(Type::Error, t_error.to_string()))
                        .await;
                    return;
                }
            },
            Some(response) = arx.recv() => break response,
            () = sleep_until(deadline) => {
                let message = format!(
                    "No EVE SSO login arrived within {} seconds; press Add again to retry.",
                    AUTH_TIMEOUT.as_secs()
                );
                let _ = send_app_message(&tx, auth_notification(Type::Warning, message)).await;
                return;
            }
        }
    };

    // Free the port right away so another link can start, and give the
    // browser a moment to receive the confirmation page.
    drop(listener);
    let _ = timeout_at(Instant::now() + AUTH_RESPONSE_GRACE, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    drop(connections);

    // The ESI calls run on their own thread, outside this (abortable) task:
    // once the callback arrived, a newer link request must not cancel it.
    let AuthRequest { esi, auth_info } = request;
    thread::spawn(move || complete_auth(esi, auth_info, response, tx));
}

/// Finishes an authentication off the UI thread (token exchange + character,
/// corporation and alliance lookups, storing the character) and reports the
/// outcome as a single [`Message`].
fn complete_auth(
    mut esi: EsiManager,
    auth_info: AuthorizeInfo,
    response: (String, String),
    tx: Arc<Sender<Message>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(t_error) => {
            let _ = try_send_app_message(&tx, auth_notification(Type::Error, t_error.to_string()));
            return;
        }
    };
    runtime.block_on(async {
        let _span = tracing::info_span!("auth completion").entered();
        let message = match esi.auth_user(auth_info, response).await {
            Ok(Some(character)) => {
                Message::CharacterAuthenticated(Box::new(LinkedCharacter { esi, character }))
            }
            Ok(None) => auth_notification(
                Type::Info,
                String::from(
                    "Apparently there was some kind of trouble authenticating the player.",
                ),
            ),
            Err(t_error) => auth_notification(Type::Error, t_error.to_string()),
        };
        let _ = send_app_message(&tx, message).await;
    });
}

pub struct AuthSpawner {
    spawn: Arc<mpsc::Sender<AuthRequest>>,
}

impl AuthSpawner {
    #[tracing::instrument(skip(msg_tx))]
    pub fn new(msg_tx: Arc<mpsc::Sender<Message>>) -> Self {
        // Set up a channel for communicating.
        let (send, mut recv) = mpsc::channel::<AuthRequest>(3);
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
                let _span = tracing::info_span!("spawned auth handler").entered();
                // Only one login can wait for its callback at a time (they
                // share the port): a new request replaces a pending one --
                // e.g. the user closed the browser tab and pressed Add
                // again. Awaiting the aborted task makes sure its listener
                // is dropped before the new one binds the port.
                let mut pending: Option<JoinHandle<()>> = None;
                while let Some(request) = recv.recv().await {
                    if let Some(previous) = pending.take()
                        && !previous.is_finished()
                    {
                        previous.abort();
                        let _ = previous.await;
                    }
                    let cloned_msg_sender = Arc::clone(&cloned_msg_sender);
                    pending = Some(tokio::spawn(handle_auth(request, cloned_msg_sender)));
                }
                // Once all senders have gone out of scope,
                // the `.recv()` call returns None and it will
                // exit from the while loop and shut down the
                // thread.
            });
        });

        obj
    }

    /// Queues a link attempt without blocking the caller (the UI thread).
    #[tracing::instrument(skip_all)]
    pub fn spawn(&self, request: AuthRequest) -> Result<(), String> {
        self.spawn
            .try_send(request)
            .map_err(|t_error| match t_error {
                mpsc::error::TrySendError::Full(_) => {
                    String::from("Too many character links in progress; try again in a minute.")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    String::from("The authentication service has shut down.")
                }
            })
    }
}

#[cfg(test)]
mod auth_spawner_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn request(db_name: &str) -> AuthRequest {
        let db = std::env::temp_dir().join(format!("{db_name}-{}.db", std::process::id()));
        let esi = EsiManager::new(
            "telescope-test",
            "test-client-id",
            "test-client-secret",
            "http://localhost:56123/login",
            vec!["publicData"],
            &db,
        );
        let auth_info = esi.get_authorize_url().unwrap();
        AuthRequest { esi, auth_info }
    }

    /// Sends the SSO redirect like a browser would, retrying while the
    /// listener is still starting. Returns the HTTP status line.
    fn send_callback() -> String {
        let start = std::time::Instant::now();
        loop {
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", AUTH_CALLBACK_PORT)) {
                stream
                    .write_all(b"GET /login?code=abc&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                    .unwrap();
                let mut reply = String::new();
                let _ = stream.read_to_string(&mut reply);
                return reply.lines().next().unwrap_or_default().to_owned();
            }
            assert!(start.elapsed().as_secs() < 10, "listener never came up");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Waits for the message `complete_auth` sends for one callback (with a
    /// fake code it is an error notification, but it proves the callback was
    /// received and processed).
    fn wait_for_outcome(rx: &mut mpsc::Receiver<Message>) {
        let start = std::time::Instant::now();
        loop {
            match rx.try_recv() {
                Ok(Message::GenericNotification((_, source, _, _))) if source == "AuthSpawner" => {
                    return;
                }
                Ok(Message::CharacterAuthenticated(_)) => return,
                _ => {}
            }
            assert!(start.elapsed().as_secs() < 60, "no auth outcome received");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    // Regression test: linking a second character right after the first
    // failed with "Address already in use" because the first listener kept
    // the port for the whole timeout. Also covers a pending login being
    // replaced by a new Add.
    #[test]
    fn consecutive_logins_each_get_a_listener() {
        let (tx, mut rx) = mpsc::channel::<Message>(40);
        let spawner = AuthSpawner::new(Arc::new(tx));

        spawner.spawn(request("auth-first")).unwrap();
        assert!(send_callback().contains("200"));
        wait_for_outcome(&mut rx);

        // Immediately again: the port must already be free.
        spawner.spawn(request("auth-second")).unwrap();
        assert!(send_callback().contains("200"));
        wait_for_outcome(&mut rx);

        // A login that never completes is replaced by the next Add.
        spawner.spawn(request("auth-abandoned")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        spawner.spawn(request("auth-retry")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(send_callback().contains("200"));
        wait_for_outcome(&mut rx);
        while let Ok(message) = rx.try_recv() {
            if let Message::GenericNotification((_, _, _, text)) = message {
                assert!(!text.contains("in use"), "unexpected: {text}");
            }
        }
    }
}
