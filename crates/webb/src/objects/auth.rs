//! Authorization data: the stored token, the token set CCP returns, the data
//! of the authentication flow and the JWT claims.

use chrono::prelude::*;

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
    /// The random `state` sent in [`Self::url`]. The SSO callback must bring
    /// it back unchanged: a callback with any other value did not come from
    /// this login (see `EsiManagerCore::auth_user`).
    pub state: String,
    /// PKCE verifier needed to authenticate the received code.
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

impl AuthData {
    #[tracing::instrument]
    pub fn new() -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
