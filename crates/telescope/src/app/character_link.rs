//! Linking and unlinking EVE characters: starting the SSO flow in the browser,
//! registering the character returned by a finished authentication, removing a
//! linked character, and keeping the location watchdog (see `watchdog.rs`) in
//! sync with the list of linked characters.
//!
//! The Settings -> Characters page (`windows/settings/characters.rs`) only
//! renders state and calls into the methods here.

use crate::app::TelescopeApp;
use crate::app::messages::AuthRequest;
use crate::app::messages::CharacterSync;
use crate::app::messages::LinkedCharacter;
use crate::app::messages::Message;
use crate::app::messages::Type;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use webb::esi::{SCHEMA_VERSION, SchemaStatus};
use webb::objects::Character;

/// Inserts `character` into `characters`, replacing an existing entry with the
/// same id (re-linking a character refreshes its data instead of duplicating
/// it). Returns `true` when the character was not linked before.
pub(crate) fn upsert_character(characters: &mut Vec<Character>, character: Character) -> bool {
    match characters.iter_mut().find(|c| c.id == character.id) {
        Some(existing) => {
            *existing = character;
            false
        }
        None => {
            characters.push(character);
            true
        }
    }
}

/// Removes the character with the given id, returning it if it was present.
pub(crate) fn remove_character(characters: &mut Vec<Character>, id: i64) -> Option<Character> {
    let index = characters.iter().position(|c| c.id == id)?;
    Some(characters.remove(index))
}

/// What became of a message sent to the location watchdog.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// The watchdog took it.
    Delivered,
    /// There is no watchdog, or it ended.
    Stopped,
    /// Its queue is full.
    Busy,
}

/// Sends `sync` to the watchdog, if there is one, without blocking.
pub(crate) fn deliver(sender: Option<&Sender<CharacterSync>>, sync: CharacterSync) -> Delivery {
    match sender {
        Some(sender) => match sender.try_send(sync) {
            Ok(()) => Delivery::Delivered,
            Err(TrySendError::Closed(_)) => Delivery::Stopped,
            Err(TrySendError::Full(_)) => Delivery::Busy,
        },
        None => Delivery::Stopped,
    }
}

/// The warning to show at startup about the player database, if any.
pub(crate) fn player_database_message(status: Option<SchemaStatus>) -> Option<String> {
    match status {
        Some(SchemaStatus::Migrated(version)) => Some(format!(
            "The player database was updated (schema version {version} ->              {SCHEMA_VERSION}). Characters whose login could not be kept will ask to              be linked again."
        )),
        Some(SchemaStatus::Newer(version)) => Some(format!(
            "The player database was written by a newer Telescope (schema version              {version}, this one uses {SCHEMA_VERSION}); it was left untouched and              may not work as expected."
        )),
        Some(SchemaStatus::Created | SchemaStatus::UpToDate) => None,
        None => Some(String::from(
            "The player database could not be opened; linked characters are unavailable.",
        )),
    }
}

impl TelescopeApp {
    /// Starts the local listener for the OAuth callback (`AuthSpawner`) and
    /// opens the EVE SSO login page in the browser. The listener finishes the
    /// authentication on its own thread, with a clone of the ESI manager, and
    /// reports back with `Message::CharacterAuthenticated`, handled by
    /// [`TelescopeApp::handle_character_authenticated`].
    #[tracing::instrument(skip(self))]
    pub(crate) fn start_character_link(&mut self) {
        let auth_info = match self.esi.get_authorize_url() {
            Ok(auth_info) => auth_info,
            Err(t_error) => {
                self.notify_character_error("get_authorize_url", t_error);
                return;
            }
        };
        let url = auth_info.url.clone();
        // Listener first, so it is already waiting when the browser redirects.
        if let Err(t_error) = self.task_auth.spawn(AuthRequest {
            esi: self.esi.clone(),
            auth_info,
        }) {
            self.notify_character_error("start_character_link", t_error);
            return;
        }
        if let Err(t_error) = open::that(url) {
            self.notify_character_error("open_authorize_url", t_error.to_string());
        }
    }

    /// Unlinks a character: deletes it from the player database first and,
    /// only if that succeeds, drops it from memory and stops watching its
    /// location, so memory and disk never disagree.
    #[tracing::instrument(skip(self))]
    pub(crate) fn unlink_character(&mut self, id: i64) {
        if let Err(t_error) = self.esi.remove_characters(Some(vec![id])) {
            self.notify_character_error("remove_characters", t_error.to_string());
            return;
        }
        remove_character(&mut self.esi.characters, id);
        self.remove_player_marker(id);
        if self.esi.active_character == Some(id) {
            self.esi.active_character = None;
        }
        if deliver(self.char_msg.as_deref(), CharacterSync::Remove(id)) == Delivery::Busy {
            self.notify_character_error(
                "unlink_character",
                String::from("The location watchdog is busy; restart Telescope to stop tracking this character."),
            );
        }
        if self.esi.characters.is_empty() {
            // Dropping the last sender also lets the watchdog shut down on its own.
            self.char_msg = None;
        }
    }

    /// Handles `Message::CharacterAuthenticated`: the auth thread already
    /// exchanged the tokens, fetched the character from ESI and stored it, so
    /// this only adopts the new session and starts tracking the character. No
    /// network calls happen here, on the UI thread.
    #[tracing::instrument(skip_all)]
    pub(crate) fn handle_character_authenticated(&mut self, linked: LinkedCharacter) {
        let LinkedCharacter { esi, character } = linked;
        self.esi.adopt_session(esi, character.id);
        self.register_linked_character(character);
    }

    /// Adds (or refreshes) a freshly authenticated character and makes sure
    /// the watchdog tracks it.
    fn register_linked_character(&mut self, player: Character) {
        let id = player.id;
        if !upsert_character(&mut self.esi.characters, player) {
            // Already linked: its data and tokens were refreshed. Tell the
            // watchdog anyway, so it resumes the character if it had
            // stopped following it (e.g. a rejected token).
            deliver(self.char_msg.as_deref(), CharacterSync::Add(id));
            return;
        }
        if self.esi.characters.len() == 1 {
            // First character: any previous watchdog has run out of
            // characters, so start a fresh one.
            self.start_watchdog(vec![id]);
            return;
        }
        let delivered = match deliver(self.char_msg.as_deref(), CharacterSync::Add(id)) {
            Delivery::Delivered => true,
            Delivery::Stopped => false,
            Delivery::Busy => {
                self.notify_character_error(
                    "register_linked_character",
                    String::from("The location watchdog is busy; restarting it."),
                );
                false
            }
        };
        if !delivered {
            // The watchdog stopped (e.g. an ESI error) or can't take the
            // message: restart it with every linked character.
            let ids = self.esi.characters.iter().map(|c| c.id).collect();
            self.start_watchdog(ids);
        }
    }

    /// Tells the user, once at startup, when opening the player database
    /// required creating or migrating it (see `SchemaStatus`).
    pub(crate) fn report_player_database_status(&mut self) {
        let Some(message) = player_database_message(self.esi.schema_status) else {
            return;
        };
        self.update_status_with_error((
            Type::Warning,
            String::from("EsiManager"),
            String::from("player_database"),
            message,
        ));
    }

    fn notify_character_error(&self, operation: &str, message: String) {
        self.task_msg.spawn(Message::GenericNotification((
            Type::Error,
            String::from("EsiManager"),
            String::from(operation),
            message,
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Delivery, SCHEMA_VERSION, SchemaStatus, deliver, player_database_message, remove_character,
        upsert_character,
    };
    use crate::app::messages::CharacterSync;
    use tokio::sync::mpsc;
    use webb::objects::Character;

    fn character(id: i64, name: &str) -> Character {
        let mut character = Character::new();
        character.id = id;
        character.name = String::from(name);
        character
    }

    #[test]
    fn upsert_adds_a_new_character() {
        let mut characters = vec![character(1, "A")];
        assert!(upsert_character(&mut characters, character(2, "B")));
        assert_eq!(characters.len(), 2);
    }

    #[test]
    fn upsert_replaces_an_already_linked_character() {
        let mut characters = vec![character(1, "A"), character(2, "B")];
        assert!(!upsert_character(
            &mut characters,
            character(2, "B renamed")
        ));
        assert_eq!(characters.len(), 2);
        assert_eq!(characters[1].name, "B renamed");
    }

    #[test]
    fn remove_returns_the_removed_character() {
        let mut characters = vec![character(1, "A"), character(2, "B")];
        let removed = remove_character(&mut characters, 1).map(|c| c.name);
        assert_eq!(removed.as_deref(), Some("A"));
        assert_eq!(characters.len(), 1);
    }

    #[test]
    fn remove_of_an_unknown_id_is_a_no_op() {
        let mut characters = vec![character(1, "A")];
        assert!(remove_character(&mut characters, 42).is_none());
        assert_eq!(characters.len(), 1);
    }

    #[test]
    fn a_running_watchdog_takes_the_message() {
        let (sender, mut receiver) = mpsc::channel(2);
        assert_eq!(
            deliver(Some(&sender), CharacterSync::Add(7)),
            Delivery::Delivered
        );
        assert!(matches!(receiver.try_recv(), Ok(CharacterSync::Add(7))));
    }

    #[test]
    fn no_watchdog_or_a_finished_one_means_stopped() {
        assert_eq!(deliver(None, CharacterSync::Add(1)), Delivery::Stopped);
        let (sender, receiver) = mpsc::channel(2);
        drop(receiver);
        assert_eq!(
            deliver(Some(&sender), CharacterSync::Remove(1)),
            Delivery::Stopped
        );
    }

    #[test]
    fn a_full_queue_is_busy_and_keeps_what_was_queued() {
        let (sender, mut receiver) = mpsc::channel(1);
        assert_eq!(
            deliver(Some(&sender), CharacterSync::Add(1)),
            Delivery::Delivered
        );
        assert_eq!(
            deliver(Some(&sender), CharacterSync::Add(2)),
            Delivery::Busy
        );
        assert!(matches!(receiver.try_recv(), Ok(CharacterSync::Add(1))));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn a_healthy_player_database_needs_no_warning() {
        assert_eq!(player_database_message(Some(SchemaStatus::Created)), None);
        assert_eq!(player_database_message(Some(SchemaStatus::UpToDate)), None);
    }

    #[test]
    fn a_migrated_database_names_both_versions() {
        let message = player_database_message(Some(SchemaStatus::Migrated(1))).unwrap();
        assert!(message.contains("schema version 1"), "{message}");
        assert!(message.contains(&SCHEMA_VERSION.to_string()), "{message}");
        assert!(message.contains("linked again"), "{message}");
    }

    #[test]
    fn a_newer_database_is_reported_as_left_untouched() {
        let message = player_database_message(Some(SchemaStatus::Newer(99))).unwrap();
        assert!(message.contains("newer Telescope"), "{message}");
        assert!(message.contains("99"), "{message}");
        assert!(message.contains("untouched"), "{message}");
    }

    #[test]
    fn a_database_that_could_not_be_opened_is_reported() {
        let message = player_database_message(None).unwrap();
        assert!(message.contains("could not be opened"), "{message}");
    }
}
