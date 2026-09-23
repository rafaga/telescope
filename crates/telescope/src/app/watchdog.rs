//! Background task that polls ESI for the location of each linked character,
//! refreshing the access token when it expires, and reports the changes to the UI.

use crate::app::TelescopeApp;
use crate::app::messages::CharacterSync;
use crate::app::messages::Message;
use crate::app::messages::Type;
use crate::app::messages::send_app_message;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::{Instant, sleep_until};
use webb::esi::EsiManager;

/// Time between two location polls of every character.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A character followed by the watchdog.
struct Tracked {
    id: usize,
    /// Last known solar system (0 = unknown yet).
    location: usize,
    /// Its token was rejected or couldn't be renewed: skipped until the
    /// character is linked again (`CharacterSync::Add`).
    paused: bool,
}

impl Tracked {
    fn new(id: usize) -> Self {
        Self {
            id,
            location: 0,
            paused: false,
        }
    }
}

/// Whether an ESI error means the character's token was rejected.
fn is_auth_rejection(error: &str) -> bool {
    error.contains("status code received: 401") || error.contains("status code received: 403")
}

/// Character name for messages, falling back to its id.
fn character_label(esi: &EsiManager, id: i32) -> String {
    esi.characters
        .iter()
        .find(|character| character.id == id)
        .map(|character| character.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn relink_notification(esi: &EsiManager, id: i32, error: &str) -> Message {
    Message::GenericNotification((
        Type::Warning,
        String::from("Telescope App"),
        String::from("start_watchdog"),
        format!(
            "{} is no longer tracked: its EVE login is not valid anymore ({error}). \
             Link the character again in Settings -> Characters.",
            character_label(esi, id)
        ),
    ))
}

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    pub fn start_watchdog(&mut self, character_id: Vec<usize>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (sender, mut receiver) = mpsc::channel::<CharacterSync>(10);
        let app_sender = Arc::clone(&self.app_msg.0);
        let mut t_esi = self.esi.clone();
        thread::spawn(move || {
            runtime.block_on(async {
                let _span = tracing::info_span!("spawned watchdog").entered();

                let mut character_ids = vec![];
                // The app's copy may hold stale tokens (a previous watchdog
                // could have refreshed them); the database has the latest.
                if let Err(t_error) = t_esi.reload_auth() {
                    let _ = send_app_message(
                        &app_sender,
                        Message::GenericNotification((
                            Type::Error,
                            String::from("Telescope App"),
                            String::from("start_watchdog - reload_auth"),
                            t_error.to_string(),
                        )),
                    )
                    .await;
                }
                if let Err(t_error) = t_esi.update_spec().await {
                    let _ = send_app_message(
                        &app_sender,
                        Message::GenericNotification((
                            Type::Error,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            t_error.to_string(),
                        )),
                    )
                    .await;
                } else {
                    let _ = send_app_message(
                        &app_sender,
                        Message::GenericNotification((
                            Type::Info,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            String::from("Starting watchdog"),
                        )),
                    )
                    .await;
                }
                for char_id in character_id {
                    character_ids.push(Tracked::new(char_id));
                }
                while !character_ids.is_empty() {
                    // Each character is polled with its own token; a failure
                    // only affects that character, never the others.
                    for item in character_ids.iter_mut().filter(|item| !item.paused) {
                        let id = item.id as i32;
                        if !t_esi.valid_token(id).await {
                            match t_esi.refresh_token(id).await {
                                Ok(_) => {
                                    let _ = send_app_message(
                                        &app_sender,
                                        Message::GenericNotification((
                                            Type::Debug,
                                            String::from("Telescope App"),
                                            String::from("start_watchdog"),
                                            format!(
                                                "token of {} refreshed successfully",
                                                character_label(&t_esi, id)
                                            ),
                                        )),
                                    )
                                    .await;
                                }
                                Err(t_error) => {
                                    item.paused = true;
                                    let _ = send_app_message(
                                        &app_sender,
                                        relink_notification(&t_esi, id, &t_error),
                                    )
                                    .await;
                                    continue;
                                }
                            }
                        }
                        match t_esi.get_location(id).await {
                            Ok(new_location) => {
                                if item.location != (new_location as usize) {
                                    item.location = new_location as usize;
                                    // The app updates the character and every
                                    // map pane's marker from this message.
                                    let _ = send_app_message(
                                        &app_sender,
                                        Message::PlayerNewLocation((id, new_location)),
                                    )
                                    .await;
                                }
                            }
                            // 401/403: ESI rejects this character's token
                            // (revoked, or scopes changed); retrying won't help.
                            Err(t_error) if is_auth_rejection(&t_error) => {
                                item.paused = true;
                                let _ = send_app_message(
                                    &app_sender,
                                    relink_notification(&t_esi, id, &t_error),
                                )
                                .await;
                            }
                            Err(t_error) => {
                                let _ = send_app_message(
                                    &app_sender,
                                    Message::GenericNotification((
                                        Type::Error,
                                        String::from("Telescope App"),
                                        format!(
                                            "start_watchdog - get_location - {}",
                                            character_label(&t_esi, id)
                                        ),
                                        t_error.to_string(),
                                    )),
                                )
                                .await;
                            }
                        }
                    }
                    // Wait for the next poll, but react to link/unlink
                    // requests right away: a newly linked character gets its
                    // first location in seconds instead of after a full cycle.
                    let next_poll = Instant::now() + POLL_INTERVAL;
                    let mut poll_now = false;
                    while !poll_now {
                        tokio::select! {
                            message = receiver.recv() => match message {
                                Some(CharacterSync::Add(char_id)) => {
                                    // The new (or re-linked) character's tokens
                                    // were stored by the auth thread; pick them up.
                                    if let Err(t_error) = t_esi.reload_auth() {
                                        let _ = send_app_message(
                                            &app_sender,
                                            Message::GenericNotification((
                                                Type::Error,
                                                String::from("Telescope App"),
                                                String::from("start_watchdog - reload_auth"),
                                                t_error.to_string(),
                                            )),
                                        )
                                        .await;
                                    }
                                    // Resume it and forget its last location, so
                                    // the next poll reports it even if unchanged.
                                    match character_ids.iter_mut().find(|item| item.id == char_id) {
                                        Some(item) => *item = Tracked::new(char_id),
                                        None => character_ids.push(Tracked::new(char_id)),
                                    }
                                    poll_now = true;
                                }
                                Some(CharacterSync::Remove(char_id)) => {
                                    character_ids.retain(|item| item.id != char_id);
                                    if character_ids.is_empty() {
                                        break;
                                    }
                                }
                                // Every sender is gone: the app dropped this
                                // watchdog (no characters left, or a new one
                                // replaced it).
                                None => {
                                    character_ids.clear();
                                    break;
                                }
                            },
                            () = sleep_until(next_poll) => poll_now = true,
                        }
                    }
                }
                let _ = send_app_message(
                    &app_sender,
                    Message::GenericNotification((
                        Type::Info,
                        String::from("Telescope App"),
                        String::from("start_watchdog"),
                        String::from("Watchdog ended"),
                    )),
                )
                .await;
            });
        });

        self.char_msg = Some(Arc::new(sender));
    }
}

#[cfg(test)]
mod tests {
    use super::is_auth_rejection;

    #[test]
    fn only_401_and_403_mean_the_token_was_rejected() {
        assert!(is_auth_rejection("Invalid HTTP status code received: 403"));
        assert!(is_auth_rejection("Invalid HTTP status code received: 401"));
        assert!(!is_auth_rejection("Invalid HTTP status code received: 502"));
        assert!(!is_auth_rejection("Invalid Token"));
    }
}
