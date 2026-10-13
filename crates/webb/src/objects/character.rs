//! The [`Character`] entity and the public info ESI reports for it.

use super::alliance::Alliance;
use super::corporation::Corporation;
use chrono::prelude::*;

/// Public information of a character as reported by ESI.
#[derive(Clone, PartialEq, Debug)]
pub struct CharacterPublicInfo {
    pub corporation_id: i64,
    pub alliance_id: Option<i64>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Character {
    pub id: i64,
    pub name: String,
    pub last_logon: DateTime<Utc>,
    pub corp: Option<Corporation>,
    pub alliance: Option<Alliance>,
    pub photo: Option<String>,
    pub location: i64,
}

impl Character {
    #[tracing::instrument]
    pub fn new() -> Self {
        Character {
            id: 0,
            name: String::new(),
            last_logon: DateTime::default(),
            corp: None,
            alliance: None,
            photo: None,
            location: 0,
        }
    }
}

impl Default for Character {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_new_is_zeroed() {
        let character = Character::new();
        assert_eq!(character.id, 0);
        assert_eq!(character.name, "");
        assert_eq!(character.last_logon, DateTime::<Utc>::default());
        assert_eq!(character.corp, None);
        assert_eq!(character.alliance, None);
        assert_eq!(character.photo, None);
        assert_eq!(character.location, 0);
        assert_eq!(character, Character::default());
    }
}
