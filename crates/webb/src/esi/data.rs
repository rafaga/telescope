//! ESI client configuration ([`Data`]): user agent, client id and the OAuth
//! callback / authorize URLs.


pub struct Data{
    pub user_agent:String,
    pub client_id:String,
    pub callback_url: String,
    pub authorize_url: String,
    pub random_state: String,
}

impl Data{
    pub fn new() -> Self {
        Data { 
            user_agent: String::new(), 
            client_id: String::new(), 
            callback_url: String::new(),
            random_state: String::new(),
            authorize_url: String::new(),
        }
    }
}