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
use webb::objects::Character;

/// Time between two location polls of every character.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// A character followed by the watchdog.
struct Tracked {
    id: i64,
    /// Last known solar system (0 = unknown yet).
    location: i64,
    /// Its token was rejected or couldn't be renewed: skipped until the
    /// character is linked again (`CharacterSync::Add`).
    paused: bool,
}

impl Tracked {
    fn new(id: i64) -> Self {
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
fn character_label(characters: &[Character], id: i64) -> String {
    characters
        .iter()
        .find(|character| character.id == id)
        .map(|character| character.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn relink_notification(characters: &[Character], id: i64, error: &str) -> Message {
    Message::GenericNotification((
        Type::Warning,
        String::from("Telescope App"),
        String::from("start_watchdog"),
        format!(
            "{} is no longer tracked: its EVE login is not valid anymore ({error}). \
             Link the character again in Settings -> Characters.",
            character_label(characters, id)
        ),
    ))
}

/// What the watchdog does after a link/unlink request.
#[derive(Debug, PartialEq, Eq)]
enum SyncOutcome {
    /// Poll every character now (a character was linked).
    PollNow,
    /// Nothing else to do; keep waiting for the next poll.
    Wait,
    /// No character left to follow: the watchdog ends.
    Stop,
}

/// Applies a link/unlink request to the followed characters.
fn apply_sync(tracked: &mut Vec<Tracked>, sync: Option<CharacterSync>) -> SyncOutcome {
    match sync {
        Some(CharacterSync::Add(id)) => {
            // Resume it and forget its last location, so the next poll
            // reports it even if unchanged.
            match tracked.iter_mut().find(|item| item.id == id) {
                Some(item) => *item = Tracked::new(id),
                None => tracked.push(Tracked::new(id)),
            }
            SyncOutcome::PollNow
        }
        Some(CharacterSync::Remove(id)) => {
            tracked.retain(|item| item.id != id);
            if tracked.is_empty() {
                SyncOutcome::Stop
            } else {
                SyncOutcome::Wait
            }
        }
        // Every sender is gone: the app dropped this watchdog (no characters
        // left, or a new one replaced it).
        None => {
            tracked.clear();
            SyncOutcome::Stop
        }
    }
}

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    pub fn start_watchdog(&mut self, character_id: Vec<i64>) {
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
                        let id = item.id;
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
                                                character_label(&t_esi.characters, id)
                                            ),
                                        )),
                                    )
                                    .await;
                                }
                                Err(t_error) => {
                                    item.paused = true;
                                    let _ = send_app_message(
                                        &app_sender,
                                        relink_notification(&t_esi.characters, id, &t_error),
                                    )
                                    .await;
                                    continue;
                                }
                            }
                        }
                        match t_esi.get_location(id).await {
                            Ok(new_location) => {
                                if item.location != new_location {
                                    item.location = new_location;
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
                                    relink_notification(&t_esi.characters, id, &t_error),
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
                                            character_label(&t_esi.characters, id)
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
                            message = receiver.recv() => {
                                if matches!(message, Some(CharacterSync::Add(_))) {
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
                                }
                                match apply_sync(&mut character_ids, message) {
                                    SyncOutcome::PollNow => poll_now = true,
                                    SyncOutcome::Wait => {}
                                    SyncOutcome::Stop => break,
                                }
                            }
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
    use super::*;

    fn character(id: i64, name: &str) -> Character {
        let mut character = Character::new();
        character.id = id;
        character.name = name.to_string();
        character
    }

    fn ids(tracked: &[Tracked]) -> Vec<i64> {
        tracked.iter().map(|item| item.id).collect()
    }

    #[test]
    fn only_401_and_403_mean_the_token_was_rejected() {
        assert!(is_auth_rejection("Invalid HTTP status code received: 403"));
        assert!(is_auth_rejection("Invalid HTTP status code received: 401"));
        assert!(!is_auth_rejection("Invalid HTTP status code received: 502"));
        assert!(!is_auth_rejection("Invalid Token"));
    }

    #[test]
    fn a_character_is_named_or_falls_back_to_its_id() {
        let characters = [character(1, "Alice")];
        assert_eq!(character_label(&characters, 1), "Alice");
        assert_eq!(character_label(&characters, 99), "99");
    }

    #[test]
    fn the_relink_warning_names_the_character_and_the_cause() {
        let characters = [character(1, "Alice")];
        match relink_notification(&characters, 1, "401") {
            Message::GenericNotification((Type::Warning, _, _, text)) => {
                assert!(text.contains("Alice"), "{text}");
                assert!(text.contains("(401)"), "{text}");
                assert!(text.contains("Settings -> Characters"), "{text}");
            }
            _ => panic!("expected a warning notification"),
        }
    }

    #[test]
    fn a_new_tracked_character_starts_active_with_no_location() {
        let tracked = Tracked::new(5);
        assert_eq!(
            (tracked.id, tracked.location, tracked.paused),
            (5, 0, false)
        );
    }

    #[test]
    fn linking_a_new_character_adds_it_and_polls_now() {
        let mut tracked = vec![Tracked::new(1)];
        let outcome = apply_sync(&mut tracked, Some(CharacterSync::Add(2)));
        assert_eq!(outcome, SyncOutcome::PollNow);
        assert_eq!(ids(&tracked), vec![1, 2]);
    }

    #[test]
    fn relinking_resumes_a_paused_character_and_forgets_its_location() {
        let mut tracked = vec![Tracked::new(1)];
        tracked[0].paused = true;
        tracked[0].location = 30000142;
        let outcome = apply_sync(&mut tracked, Some(CharacterSync::Add(1)));
        assert_eq!(outcome, SyncOutcome::PollNow);
        assert_eq!(ids(&tracked), vec![1]);
        assert!(!tracked[0].paused);
        assert_eq!(tracked[0].location, 0);
    }

    #[test]
    fn unlinking_keeps_waiting_until_no_character_is_left() {
        let mut tracked = vec![Tracked::new(1), Tracked::new(2)];
        assert_eq!(
            apply_sync(&mut tracked, Some(CharacterSync::Remove(1))),
            SyncOutcome::Wait
        );
        assert_eq!(ids(&tracked), vec![2]);
        assert_eq!(
            apply_sync(&mut tracked, Some(CharacterSync::Remove(2))),
            SyncOutcome::Stop
        );
        assert!(tracked.is_empty());
    }

    #[test]
    fn unlinking_an_unknown_character_changes_nothing() {
        let mut tracked = vec![Tracked::new(1)];
        assert_eq!(
            apply_sync(&mut tracked, Some(CharacterSync::Remove(9))),
            SyncOutcome::Wait
        );
        assert_eq!(ids(&tracked), vec![1]);
    }

    #[test]
    fn a_closed_channel_stops_the_watchdog_and_clears_the_list() {
        let mut tracked = vec![Tracked::new(1), Tracked::new(2)];
        assert_eq!(apply_sync(&mut tracked, None), SyncOutcome::Stop);
        assert!(tracked.is_empty());
    }
}
