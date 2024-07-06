pub struct AppData<'a> {
    pub user_agent: String,
    pub scope: Vec<&'a str>,
    pub secret_key: String,
    pub client_id: String,
    pub url: String,
}

impl<'a> AppData<'a> {
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
            secret_key: std::env::var("TELESCOPE_SECRET_KEY").expect("No ESI Secret Key its configured"),
            client_id: std::env::var("TELESCOPE_CLIENT_ID").expect("No ESI Client Id its configured"),
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from("telescope/dev"),
        }
    }
}
