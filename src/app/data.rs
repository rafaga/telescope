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
                "esi-location.read_ship_type.v1",
                "esi-skills.read_skills.v1",
                "esi-search.search_structures.v1",
                "esi-clones.read_clones.v1",
                "esi-characters.read_contacts.v1",
                "esi-universe.read_structures.v1",
                "esi-ui.write_waypoint.v1",
                "esi-corporations.read_structures.v1",
                "esi-characters.read_chat_channels.v1",
                "esi-characters.read_standings.v1",
                "esi-location.read_online.v1",
                "esi-clones.read_implants.v1",
                "esi-characters.read_fatigue.v1",
                "esi-corporations.read_contacts.v1",
                "esi-corporations.read_standings.v1",
                "esi-corporations.read_starbases.v1",
                "esi-alliances.read_contacts.v1",
            ],
            secret_key: String::from("LVHIhVKjOMH488z8FWCPkSU8v4phsebvnJ92LLlB"),
            client_id: String::from("9efd379b98ed4b3fb8a8caabe5907f42"),
            url: String::from("http://localhost:56123/login"),
            user_agent: String::from("telescope/alpha"),
        }
    }
}
