//! Static ESI application configuration ([`AppData`]): the SSO scopes requested,
//! the callback URL, the user agent and the client id / secret key.
//!
//! The client id and secret key are baked in at compile time from the
//! `ESI_CLIENT_ID` and `ESI_SECRET_KEY` environment variables (see `BUILD.md`);
//! the repository does not carry them.

#![allow(clippy::option_env_unwrap)]
pub struct AppData<'a> {
    pub user_agent: String,
    pub scope: Vec<&'a str>,
    pub secret_key: &'a str,
    pub client_id: &'a str,
    pub url: String,
}

impl<'a> AppData<'a> {
    #[tracing::instrument]
    pub fn new() -> Self {
        Self::with_credentials(
            option_env!("ESI_CLIENT_ID")
                .expect("ESI_CLIENT_ID is not set: define it as an environment variable when building (see BUILD.md)."),
            option_env!("ESI_SECRET_KEY")
                .expect("ESI_SECRET_KEY is not set: define it as an environment variable when building (see BUILD.md)."),
        )
    }

    /// The application data around the given credentials: everything but the
    /// client id and the secret key is fixed.
    fn with_credentials(client_id: &'a str, secret_key: &'a str) -> Self {
        AppData {
            scope: vec![
                "publicData",
                "esi-location.read_location.v1",
                "esi-clones.read_clones.v1",
                "esi-characters.read_contacts.v1",
                "esi-ui.write_waypoint.v1",
                "esi-location.read_online.v1",
                "esi-corporations.read_standings.v1",
                "esi-alliances.read_contacts.v1",
            ],
            secret_key,
            client_id,
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from("telescope/dev"),
        }
    }
}

#[cfg(test)]
impl<'a> AppData<'a> {
    /// Placeholder credentials, so tests do not depend on the build
    /// environment.
    pub fn for_test() -> Self {
        Self::with_credentials("test-client", "test-secret")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_asks_for_the_scopes_it_uses() {
        let data = AppData::for_test();
        for scope in [
            "publicData",
            "esi-location.read_location.v1",
            "esi-location.read_online.v1",
        ] {
            assert!(data.scope.contains(&scope), "{scope} missing");
        }
        // No scope is asked for twice.
        let unique: std::collections::HashSet<&&str> = data.scope.iter().collect();
        assert_eq!(unique.len(), data.scope.len());
    }

    #[test]
    fn the_callback_is_local_and_the_credentials_are_kept() {
        let data = AppData::with_credentials("the-id", "the-secret");
        assert!(data.url.starts_with("http://localhost:"));
        assert!(data.user_agent.starts_with("telescope/"));
        assert_eq!(data.client_id, "the-id");
        assert_eq!(data.secret_key, "the-secret");
    }

    /// When the build environment has the credentials, `new` hands them over.
    /// Without them it panics by design (see `BUILD.md`), so there is nothing
    /// to call.
    #[test]
    fn new_uses_the_build_environment_when_it_is_there() {
        let (Some(id), Some(secret)) =
            (option_env!("ESI_CLIENT_ID"), option_env!("ESI_SECRET_KEY"))
        else {
            return;
        };
        let data = AppData::new();
        assert_eq!(data.client_id, id);
        assert_eq!(data.secret_key, secret);
    }
}
