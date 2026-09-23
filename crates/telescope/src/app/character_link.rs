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
pub(crate) fn remove_character(characters: &mut Vec<Character>, id: i32) -> Option<Character> {
    let index = characters.iter().position(|c| c.id == id)?;
    Some(characters.remove(index))
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
    pub(crate) fn unlink_character(&mut self, id: i32) {
        if let Err(t_error) = self.esi.remove_characters(Some(vec![id])) {
            self.notify_character_error("remove_characters", t_error.to_string());
            return;
        }
        remove_character(&mut self.esi.characters, id);
        self.remove_player_marker(id);
        if self.esi.active_character == Some(id) {
            self.esi.active_character = None;
        }
        if let Some(sender) = &self.char_msg
            && let Err(TrySendError::Full(_)) = sender.try_send(CharacterSync::Remove(id as usize))
        {
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
        let id = player.id as usize;
        if !upsert_character(&mut self.esi.characters, player) {
            // Already linked: its data and tokens were refreshed. Tell the
            // watchdog anyway, so it resumes the character if it had
            // stopped following it (e.g. a rejected token).
            if let Some(sender) = &self.char_msg {
                let _ = sender.try_send(CharacterSync::Add(id));
            }
            return;
        }
        if self.esi.characters.len() == 1 {
            // First character: any previous watchdog has run out of
            // characters, so start a fresh one.
            self.start_watchdog(vec![id]);
            return;
        }
        let delivered = match &self.char_msg {
            Some(sender) => match sender.try_send(CharacterSync::Add(id)) {
                Ok(()) => true,
                Err(TrySendError::Closed(_)) => false,
                Err(TrySendError::Full(_)) => {
                    self.notify_character_error(
                        "register_linked_character",
                        String::from("The location watchdog is busy; restarting it."),
                    );
                    false
                }
            },
            None => false,
        };
        if !delivered {
            // The watchdog stopped (e.g. an ESI error) or can't take the
            // message: restart it with every linked character.
            let ids = self.esi.characters.iter().map(|c| c.id as usize).collect();
            self.start_watchdog(ids);
        }
    }

    /// Tells the user, once at startup, when opening the player database
    /// required creating or migrating it (see `SchemaStatus`).
    pub(crate) fn report_player_database_status(&mut self) {
        let message = match self.esi.schema_status {
            Some(SchemaStatus::Migrated(version)) => format!(
                "The player database was updated (schema version {version} -> \
                 {SCHEMA_VERSION}). Characters whose login could not be kept will ask to \
                 be linked again."
            ),
            Some(SchemaStatus::Newer(version)) => format!(
                "The player database was written by a newer Telescope (schema version \
                 {version}, this one uses {SCHEMA_VERSION}); it was left untouched and \
                 may not work as expected."
            ),
            Some(SchemaStatus::Created | SchemaStatus::UpToDate) => return,
            None => String::from(
                "The player database could not be opened; linked characters are unavailable.",
            ),
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
    use super::{remove_character, upsert_character};
    use webb::objects::Character;

    fn character(id: i32, name: &str) -> Character {
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
}
