
pub struct AppData<'a>{
    pub user_agent:String,
    pub scope:Vec<&'a str>,
    pub secret_key:String,
    pub client_id:String,
    pub url:String,
}

impl<'a> AppData<'a>{
    pub fn new() -> Self{
        AppData{
            scope: vec![""],
            secret_key: String::new(),
            client_id: String::new(),
            url:  String::new(),
            user_agent: String::new(),
        }
    }   
 }