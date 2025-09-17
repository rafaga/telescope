#![allow(clippy::option_env_unwrap)]
pub struct AppData<'a> {
    pub user_agent: String,
    pub scope: Vec<&'a str>,
    pub secret_key: &'a str,
    pub client_id: &'a str,
    pub url: String,
}

impl<'a> AppData<'a> {
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

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
            secret_key: option_env!("TELESCOPE_SECRET_KEY")
                .expect("ESI Secret Key it is undefined, add it to Cargo.toml in env section."),
            client_id: option_env!("TELESCOPE_CLIENT_ID")
                .expect("ESI Client Id it is undefined, add it to Cargo.toml in env section"),
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from("telescope/dev"),
        }
    }
}
