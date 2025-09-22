use crate::objects::{Alliance, Character, Corporation};
use chrono::DateTime;
use hyper::body::Body;
use hyper_tls::HttpsConnector;
use rfesi::prelude::*;
use rusqlite::vtab::array;
use rusqlite::*;
use std::path::Path;
use http_body_util::{BodyExt, Empty};
use crate::objects::AuthData;
//use hyper::body::Bytes;
use bytes::Bytes;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};

#[cfg(feature = "crypted-db")]
use uuid::Uuid;

use self::player_database::PlayerDatabase;
pub mod player_database;

#[derive(Clone)]
pub struct EsiManager {
    pub esi: Esi,
    pub auth: AuthData,
    pub characters: Vec<Character>,
    pub path: String,
    pub active_character: Option<i32>,
}

impl EsiManager {
    pub(crate) fn get_standard_connection(&self) -> Result<Connection, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut flags = OpenFlags::default();
        flags.set(OpenFlags::SQLITE_OPEN_NO_MUTEX, false);
        flags.set(OpenFlags::SQLITE_OPEN_FULL_MUTEX, true);
        let connection = Connection::open_with_flags(self.path.clone(), flags)?;

        // we add the carray module disguised as rarray in rusqlite
        array::load_module(&connection)?;

        let query = "PRAGMA journey_mode=WAL;";
        let mut statement = connection.prepare(query)?;
        let _ = statement.execute([])?;
        

        #[cfg(feature = "crypted-db")]
        {
            let uuid = Uuid::new_v5(&Uuid::NAMESPACE_OID, "telescope".as_bytes());
            let query = ["PRAGMA key = '", uuid.to_string().as_str(), "'"].concat();
            let mut statement = connection.prepare(query.as_str())?;
            let _ = statement.execute([])?;
        }

        statement.finalize()?;
        Ok(connection)
    }

    // Alliance
    pub fn write_alliance(&mut self, alliance: &Alliance) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let players = PlayerDatabase::select_alliance(&conn, vec![alliance.id])?;
        let rows = if !players.is_empty() {
            PlayerDatabase::update_alliance(&conn, alliance)?
        } else {
            PlayerDatabase::insert_alliance(&conn, alliance)?
        };
        Ok(rows)
    }

    pub fn read_alliance(
        &mut self,
        alliance_vec: Option<Vec<i32>>,
    ) -> Result<Vec<Alliance>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result = if let Some(id_ally) = alliance_vec {
            PlayerDatabase::select_alliance(&conn, id_ally)?
        } else {
            PlayerDatabase::select_alliance(&conn, vec![])?
        };
        Ok(result)
    }

    pub fn remove_alliance(&mut self, alliance_vec: Option<Vec<i32>>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result = if let Some(id_ally) = alliance_vec {
            PlayerDatabase::delete_alliance(&conn, id_ally)?
        } else {
            PlayerDatabase::delete_alliance(&conn, vec![])?
        };
        Ok(result)
    }

    // Corporation
    pub fn write_corporation(&mut self, corp: &Corporation) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let corps = PlayerDatabase::select_corporation(&conn, vec![corp.id])?;
        let rows = if !corps.is_empty() {
            PlayerDatabase::update_corporation(&conn, corp)?
        } else {
            PlayerDatabase::insert_corporation(&conn, corp)?
        };
        Ok(rows)
    }

    pub fn read_corporation(
        &mut self,
        corporation_vec: Option<Vec<i32>>,
    ) -> Result<Vec<Corporation>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result = if let Some(id_corp) = corporation_vec {
            PlayerDatabase::select_corporation(&conn, id_corp)?
        } else {
            PlayerDatabase::select_corporation(&conn, vec![])?
        };
        Ok(result)
    }

    pub fn remove_corporation(
        &mut self,
        corporation_vec: Option<Vec<i32>>,
    ) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result = if let Some(id_ally) = corporation_vec {
            PlayerDatabase::delete_corporation(&conn, id_ally)?
        } else {
            PlayerDatabase::delete_corporation(&conn, vec![])?
        };
        Ok(result)
    }

    //Characters
    pub fn write_character(&mut self, char: &Character) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        // first we need to assure that Corporation and alliance exists on database
        if let Some(corp) = &char.corp {
            let _ = self.write_corporation(corp)?;
        }

        if let Some(alliance) = &char.alliance {
            let _ = self.write_alliance(alliance)?;
        }

        let players = PlayerDatabase::select_characters(&conn, vec![char.id])?;
        let rows = if !players.is_empty() {
            PlayerDatabase::update_character(&conn, char)?
        } else {
            PlayerDatabase::insert_character(&conn, char)?
        };
        Ok(rows)
    }

    pub fn read_characters(&mut self, char_vec: Option<Vec<i32>>) -> Result<Vec<Character>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result;
        if let Some(id_chars) = char_vec {
            result = PlayerDatabase::select_characters(&conn, id_chars)?;
        } else {
            result = PlayerDatabase::select_characters(&conn, vec![])?;
        };
        Ok(result)
    }

    pub fn remove_characters(&mut self, char_vec: Option<Vec<i32>>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let conn = self.get_standard_connection().unwrap();

        let result = if let Some(id_chars) = char_vec {
            PlayerDatabase::delete_characters(&conn, id_chars)?
        } else {
            PlayerDatabase::delete_characters(&conn, vec![])?
        };
        Ok(result)
    }

    pub fn new(
        useragent: &str,
        client_id: &str,
        _client_secret: &str,
        callback_url: &str,
        scope: Vec<&str>,
        database_path: String,
    ) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        #[cfg(not(feature = "native-auth-flow"))]
        let esi = EsiBuilder::new()
            .user_agent(useragent)
            .client_id(client_id)
            .client_secret(_client_secret)
            .callback_url(callback_url)
            .scope(scope.join(" ").as_str())
            .build()
            .unwrap();

        #[cfg(feature = "native-auth-flow")]
        let esi = EsiBuilder::new()
            .user_agent(useragent)
            .client_id(client_id)
            .callback_url(callback_url)
            .enable_application_authentication(true)
            .scope(scope.join(" ").as_str())
            .build()
            .unwrap();

        let mut obj = EsiManager {
            esi,
            auth: AuthData::new(),
            characters: Vec::new(),
            path: database_path,
            active_character: None,
        };

        // Path needs to be checked before invoking rusqlite to be effective
        let temp_path = Path::new(&obj.path);
        if let Ok(conn) = obj.get_standard_connection(){
            if !temp_path.exists() || !temp_path.is_file() {
                let _ = PlayerDatabase::create_database(&conn);
                let _ = PlayerDatabase::migrate_database();
            } else {
                // load existing players
                if let Ok(chars) = PlayerDatabase::select_characters(&conn, vec![]) {
                    obj.characters = chars;
                    if !obj.characters.is_empty() {
                        obj.auth = PlayerDatabase::select_auth(&conn).expect("Invalid Authetication data");
                    } 
                }
            }
        }
        obj
    }

    pub async fn get_location(&mut self, player_id: i32) -> Result<i32, String> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if !self.valid_token().await {
            return Err(String::from("Invalid Token"));
        }

        match self.esi.group_location().get_location(player_id).await {
            Ok(location) => {
                let player_location = location.solar_system_id;
                Ok(player_location)
            },
            Err(t_error) => {
                Err(t_error.to_string())
            }
        }
    }

    pub async fn valid_token(&self) -> bool {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut result = false;
        if self.esi.access_expiration.is_none() || self.esi.access_token.is_none() || self.esi.refresh_token.is_none() {
            return result;
        }
        if !self.auth.token.is_empty() && self.auth.expiration.is_some() && !self.auth.refresh_token.is_empty() {
            let current_datetime = chrono::Utc::now();
            //if auth.expiration =
            let offset =  self.auth.expiration.unwrap() - current_datetime;
            if offset.num_seconds()>= 20 {
                result = true;
            }
        }
        result
    }

    pub async fn refresh_token(&mut self) -> Result<usize,String> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if let Err(t_error) = self.esi.refresh_access_token(Some(&self.auth.refresh_token)).await {
            return Err(t_error.to_string());
        }
        self.auth.token = self.esi.access_token.as_ref().unwrap().clone();
        self.auth.expiration = chrono::DateTime::from_timestamp_millis(self.esi.access_expiration.unwrap());
        self.auth.refresh_token = self.esi.refresh_token.as_ref().unwrap().clone();
        if let Ok(conn) = self.get_standard_connection() {
            if let Err(t_error) = PlayerDatabase::update_auth(&conn, &self.auth){
                return Err(t_error.to_string());
            }
        }
        Ok(0)
        
    }

    #[tokio::main(flavor = "current_thread")]
    pub async fn get_player_photo(url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let https = HttpsConnector::new();
        let client = Client::builder(TokioExecutor::new()).build::<_, Empty<Bytes>>(https);
    
        let mut res = client.get(url.parse()?).await?;
        //assert_eq!(res.status(), 200);
        let mut photo:Vec<u8> = vec![];
        if res.status() == 200 {
            while !res.is_end_stream() {
                if let Some(data) = res.body_mut().frame().await.unwrap().expect("No data").data_mut() {
                    photo.extend_from_slice(data.as_ref());
                }
            }
        }
        Ok(photo)
    }

    pub async fn auth_user(
        &mut self,
        _auth_info: AuthenticationInformation,
        oauth_data: (String, String),
    ) -> Result<Option<Character>, Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        #[cfg(not(feature = "native-auth-flow"))]
        let verifier = None;

        #[cfg(feature = "native-auth-flow")]
        let verifier = _auth_info.pkce_verifier;

        let claims_option = self
            .esi
            .authenticate(oauth_data.0.as_str(), verifier)
            .await?;
        if let Some(claims) = claims_option {
            
            let mut player = Character::new();
            //let data = claims.unwrap();
            //character name
            player.name = claims.name;
            //character id
            let split: Vec<&str> = claims.sub.split(':').collect();
            player.id = split[2].parse::<i32>().unwrap();
            if !self.valid_token().await {
                self.auth.token = self.esi.access_token.as_ref().unwrap().to_string();
                self.auth.refresh_token = self.esi.refresh_token.as_ref().unwrap().to_string();

                //expiration Date
                self.auth.expiration = DateTime::from_timestamp_millis(self.esi.access_expiration.unwrap());
                if let Ok(conn) =  self.get_standard_connection() {
                    let _ =PlayerDatabase::update_auth(&conn, &self.auth);
                }
            }
            self.esi.update_spec().await?;
            let public_info = self
                .esi
                .group_character()
                .get_public_info(player.id)
                .await?;
            let corp_info = self
                .esi
                .group_corporation()
                .get_public_info(public_info.corporation_id)
                .await?;
            let corp = Corporation {
                id: public_info.corporation_id,
                name: corp_info.name,
            };
            player.corp = Some(corp);
            if let Some(ally_id) = public_info.alliance_id {
                let ally_info = self.esi.group_alliance().get_info(ally_id).await?;
                let ally = Alliance {
                    id: ally_id,
                    name: ally_info.name,
                };
                player.alliance = Some(ally);
            }
            let player_portraits = self.esi.group_character().get_portrait(player.id).await?;
            player.photo = Some(player_portraits.px128x128.unwrap());
            let player_location = self.esi.group_location().get_location(player.id).await?;
            player.location = player_location.solar_system_id;
            
            self.write_character(&player)?;
            Ok(Some(player))
        } else {
            Ok(None)
        }
    }
}
