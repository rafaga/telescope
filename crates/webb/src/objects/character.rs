//! The [`Character`] entity and the public info ESI reports for it.

use super::alliance::Alliance;
use super::corporation::Corporation;
use chrono::prelude::*;
use serde::{Deserialize, Serialize};

/// The corner of the game window an overlay keeps its distance from.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayAnchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

impl OverlayAnchor {
    /// The name stored in the database (the same as in `telescope.toml`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
            Self::Center => "center",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "top_left" => Self::TopLeft,
            "top_right" => Self::TopRight,
            "bottom_left" => Self::BottomLeft,
            "bottom_right" => Self::BottomRight,
            "center" => Self::Center,
            _ => return None,
        })
    }
}

/// Where an overlay sits in the game window: its distance from an anchor
/// corner, so it keeps its place when the game is moved or resized.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayPosition {
    pub anchor: OverlayAnchor,
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
    /// Height of the game window the values were measured on (they scale
    /// with it); `None`: real pixels.
    #[serde(default)]
    pub reference_height: Option<u32>,
}

/// The overlay of a linked character: whether it is on and where the user
/// left it. Stored with the character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharacterOverlay {
    pub enabled: bool,
    /// `None` until the user places it.
    pub position: Option<OverlayPosition>,
}

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
    pub overlay: CharacterOverlay,
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
            overlay: CharacterOverlay::default(),
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
        assert_eq!(character.overlay, CharacterOverlay::default());
        assert!(!character.overlay.enabled);
        assert_eq!(character, Character::default());
    }

    #[test]
    fn anchors_round_trip_through_their_stored_names() {
        for anchor in [
            OverlayAnchor::TopLeft,
            OverlayAnchor::TopRight,
            OverlayAnchor::BottomLeft,
            OverlayAnchor::BottomRight,
            OverlayAnchor::Center,
        ] {
            assert_eq!(OverlayAnchor::parse(anchor.as_str()), Some(anchor));
        }
        assert_eq!(OverlayAnchor::parse("nowhere"), None);
    }
}
