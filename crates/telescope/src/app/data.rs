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
            secret_key: option_env!("ESI_SECRET_KEY")
                .expect("ESI_SECRET_KEY is not set: define it as an environment variable when building (see BUILD.md)."),
            client_id: option_env!("ESI_CLIENT_ID")
                .expect("ESI_CLIENT_ID is not set: define it as an environment variable when building (see BUILD.md)."),
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
        AppData {
            scope: Vec::new(),
            secret_key: "test-secret",
            client_id: "test-client",
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from("telescope/test"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ESI credentials are baked in at compile time from the build environment;
    /// without them `AppData::new` panics by design, so there is nothing to test.
    fn configured() -> bool {
        option_env!("ESI_SECRET_KEY").is_some() && option_env!("ESI_CLIENT_ID").is_some()
    }

    #[test]
    fn the_app_asks_for_the_scopes_it_uses() {
        if !configured() {
            return;
        }
        let data = AppData::new();
        for scope in [
            "publicData",
            "esi-location.read_location.v1",
            "esi-location.read_online.v1",
        ] {
            assert!(data.scope.contains(&scope), "{scope} missing");
        }
    }

    #[test]
    fn the_callback_is_local_and_the_credentials_are_present() {
        if !configured() {
            return;
        }
        let data = AppData::new();
        assert!(data.url.starts_with("http://localhost:"));
        assert!(data.user_agent.starts_with("telescope/"));
        assert!(!data.client_id.is_empty());
        assert!(!data.secret_key.is_empty());
    }
}
