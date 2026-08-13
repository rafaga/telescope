use chrono::prelude::*;
use rusqlite::Error;

pub trait EsiObject {
    fn retrieve() -> Result<bool, Error>;
}

pub enum TelescopeDbError {
    NoConnection,
}

#[derive(Clone, PartialEq, Debug)]
pub struct AuthData {
    pub token: String,
    pub expiration: Option<DateTime<Utc>>,
    pub refresh_token: String,
}

/// OAuth token set returned by CCP after a successful authentication or
/// token refresh.
#[derive(Clone, PartialEq, Debug)]
pub struct TokenSet {
    pub token: String,
    pub refresh_token: String,
    pub expiration: Option<DateTime<Utc>>,
}

/// Data needed to complete the requested authentication flow.
#[derive(Clone, PartialEq, Debug)]
pub struct AuthorizeInfo {
    /// URL to open in the browser to initiate the authentication.
    pub url: String,
    /// PKCE verifier needed to authenticate the received code, when the
    /// `native-auth-flow` feature is enabled.
    pub pkce_verifier: Option<String>,
}

/// Claims extracted from the JWT issued by CCP after authentication.
#[derive(Clone, PartialEq, Debug)]
pub struct AuthClaims {
    /// Character name.
    pub name: String,
    /// Subject claim, with the format `CHARACTER:EVE:<character_id>`.
    pub sub: String,
}

/// Public information of a character as reported by ESI.
#[derive(Clone, PartialEq, Debug)]
pub struct CharacterPublicInfo {
    pub corporation_id: i32,
    pub alliance_id: Option<i32>,
}

impl AuthData {
    pub fn new() -> Self {
        profiling::function_scope!();

        AuthData {
            token: String::new(),
            expiration: None,
            refresh_token: String::new(),
        }
    }
}

impl Default for AuthData {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Character {
    pub id: i32,
    pub name: String,
    pub last_logon: DateTime<Utc>,
    pub corp: Option<Corporation>,
    pub alliance: Option<Alliance>,
    pub photo: Option<String>,
    pub location: i32,
}

impl Character {
    pub fn new() -> Self {
        profiling::function_scope!();

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

#[derive(Clone, PartialEq, Debug)]
pub struct Corporation {
    pub id: i32,
    pub name: String,
}

impl Corporation {
    pub fn new() -> Self {
        profiling::function_scope!();

        Corporation {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Corporation {
    fn default() -> Self {
        profiling::function_scope!();

        Self::new()
    }
}

impl BasicCatalog for Corporation {
    type Output = i32;

    fn id(&self) -> Self::Output {
        profiling::function_scope!();

        self.id
    }

    fn name(&self) -> &str {
        profiling::function_scope!();

        &self.name
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Alliance {
    pub id: i32,
    pub name: String,
}

impl Alliance {
    pub fn new() -> Self {
        profiling::function_scope!();

        Alliance {
            id: 0,
            name: String::new(),
        }
    }
}

impl Default for Alliance {
    fn default() -> Self {
        profiling::function_scope!();

        Self::new()
    }
}

impl BasicCatalog for Alliance {
    type Output = i32;

    fn id(&self) -> Self::Output {
        profiling::function_scope!();

        self.id
    }

    fn name(&self) -> &str {
        profiling::function_scope!();

        &self.name
    }
}

pub trait BasicCatalog {
    type Output;

    fn id(&self) -> Self::Output;
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // AuthData
    // ---------------------------------------------------------------------

    #[test]
    fn auth_data_new_is_empty() {
        let auth = AuthData::new();
        assert_eq!(auth.token, "");
        assert_eq!(auth.expiration, None);
        assert_eq!(auth.refresh_token, "");
        assert_eq!(auth, AuthData::default());
    }

    #[test]
    fn auth_data_clone_and_equality() {
        let mut auth = AuthData::new();
        auth.token = String::from("token");
        auth.refresh_token = String::from("refresh");
        auth.expiration = Some(Utc::now());

        let cloned = auth.clone();
        assert_eq!(auth, cloned);

        let mut other = cloned.clone();
        other.token = String::from("other");
        assert_ne!(auth, other);
    }

    // ---------------------------------------------------------------------
    // Character
    // ---------------------------------------------------------------------

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

    // ---------------------------------------------------------------------
    // Corporation
    // ---------------------------------------------------------------------

    #[test]
    fn corporation_new_is_zeroed() {
        let corp = Corporation::new();
        assert_eq!(corp.id, 0);
        assert_eq!(corp.name, "");
        assert_eq!(corp, Corporation::default());
    }

    #[test]
    fn corporation_implements_basic_catalog() {
        let corp = Corporation {
            id: 98000001,
            name: String::from("Acme Corp"),
        };
        assert_eq!(BasicCatalog::id(&corp), 98000001);
        assert_eq!(BasicCatalog::name(&corp), "Acme Corp");
    }

    // ---------------------------------------------------------------------
    // Alliance
    // ---------------------------------------------------------------------

    #[test]
    fn alliance_new_is_zeroed() {
        let alliance = Alliance::new();
        assert_eq!(alliance.id, 0);
        assert_eq!(alliance.name, "");
        assert_eq!(alliance, Alliance::default());
    }

    #[test]
    fn alliance_implements_basic_catalog() {
        let alliance = Alliance {
            id: 99000001,
            name: String::from("Acme Alliance"),
        };
        assert_eq!(BasicCatalog::id(&alliance), 99000001);
        assert_eq!(BasicCatalog::name(&alliance), "Acme Alliance");
    }
}
