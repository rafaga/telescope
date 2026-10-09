//! Static ESI application configuration ([`AppData`]): the SSO scopes requested,
//! the callback URL, the user agent and the client id.
//!
//! The client id is baked in at compile time from the `ESI_CLIENT_ID`
//! environment variable (see `BUILD.md`); the repository does not carry it.
//! There is no client secret: the login uses PKCE, which a desktop
//! application can do without one.

#![allow(clippy::option_env_unwrap)]

/// What Telescope sends as `User-Agent` to ESI: its name, its version (read
/// from the crate, so it never goes stale) and where to find its authors.
/// CCP asks every ESI client to say who is calling.
const USER_AGENT: &str = concat!(
    "telescope/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/rafaga/telescope)"
);

pub struct AppData<'a> {
    pub user_agent: String,
    pub scope: Vec<&'a str>,
    pub client_id: &'a str,
    pub url: String,
}

impl<'a> AppData<'a> {
    #[tracing::instrument]
    pub fn new() -> Self {
        Self::with_client_id(
            option_env!("ESI_CLIENT_ID")
                .expect("ESI_CLIENT_ID is not set: define it as an environment variable when building (see BUILD.md)."),
        )
    }

    /// The application data around the given client id: everything else is
    /// fixed.
    fn with_client_id(client_id: &'a str) -> Self {
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
            client_id,
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from(USER_AGENT),
        }
    }
}

#[cfg(test)]
impl<'a> AppData<'a> {
    /// Placeholder client id, so tests do not depend on the build
    /// environment.
    pub fn for_test() -> Self {
        Self::with_client_id("test-client")
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
    fn the_callback_is_local_and_the_client_id_is_kept() {
        let data = AppData::with_client_id("the-id");
        assert!(data.url.starts_with("http://localhost:"));
        assert!(data.user_agent.starts_with("telescope/"));
        assert_eq!(data.client_id, "the-id");
    }

    #[test]
    fn the_user_agent_names_the_app_its_version_and_where_to_reach_it() {
        let agent = AppData::for_test().user_agent;
        assert!(agent.starts_with("telescope/"), "{agent}");
        assert!(agent.contains(env!("CARGO_PKG_VERSION")), "{agent}");
        assert!(
            agent.contains("https://github.com/rafaga/telescope"),
            "{agent}"
        );
        assert!(!agent.starts_with("telescope/dev"), "{agent}");
        // It has to be a valid HTTP header value.
        assert!(
            http_value_is_valid(&agent),
            "not a valid header value: {agent}"
        );
    }

    fn http_value_is_valid(value: &str) -> bool {
        value.bytes().all(|b| (0x20..0x7f).contains(&b))
    }

    /// When the build environment has the client id, `new` hands it over.
    /// Without it it panics by design (see `BUILD.md`), so there is nothing
    /// to call.
    #[test]
    fn new_uses_the_build_environment_when_it_is_there() {
        let Some(id) = option_env!("ESI_CLIENT_ID") else {
            return;
        };
        let data = AppData::new();
        assert_eq!(data.client_id, id);
    }
}
