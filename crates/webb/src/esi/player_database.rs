use crate::esi::Error;
use crate::objects::{Alliance, AuthData, BasicCatalog, Character, Corporation};
use chrono::{DateTime, Utc};
use rusqlite::vtab::array;
use rusqlite::{Connection, ToSql, params};
use std::rc::Rc;

pub(crate) struct PlayerDatabase {}

impl PlayerDatabase {
    pub(crate) fn create_database(conn: &Connection) -> Result<bool, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        //Character Public Data
        let mut query =
            String::from("CREATE TABLE char (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL,");
        query += " corporation INTEGER REFERENCES corp(id) ON DELETE CASCADE ON UPDATE CASCADE,";
        query += " alliance INTEGER REFERENCES alliance(id) ON DELETE CASCADE ON UPDATE CASCADE,";
        query += " portrait BLOB, lastLogon DATETIME NOT NULL, location INTEGER NOT NULL)";
        let mut statement = conn.prepare(&query)?;
        statement.execute([])?;

        // Corporations
        let mut query = "CREATE TABLE corp (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL)";
        let mut statement = conn.prepare(query)?;
        statement.execute([])?;

        // Alliances
        query = "CREATE TABLE alliance (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL)";
        statement = conn.prepare(query)?;
        statement.execute([])?;

        // Telescope Metadata
        let mut query =
            "CREATE TABLE metadata (id VARCHAR(255) PRIMARY KEY,value VARCHAR(255) NOT NULL);";
        statement = conn.prepare(query)?;
        statement.execute([])?;
        query = "INSERT INTO metadata (id,value) VALUES (?,?)";
        statement = conn.prepare(query)?;
        statement.execute(["db", "0"])?;

        PlayerDatabase::insert_auth(conn, &AuthData::new())?;
        Ok(true)
    }

    pub(crate) fn select_characters(
        conn: &Connection,
        ids: Vec<i32>,
    ) -> Result<Vec<Character>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut result = Vec::new();
        let mut query = String::from(
            "SELECT id, name, corporation, alliance, portrait, lastLogon, location FROM char",
        );
        if !ids.is_empty() {
            let vars = PlayerDatabase::repeat_vars(ids.len());
            query = format!(
                "SELECT id, name, corporation, alliance, portrait, lastLogon, location FROM char WHERE id IN ({})",
                vars
            );
        }
        let mut statement = conn.prepare(&query)?;
        let mut rows = statement.query(rusqlite::params_from_iter(ids))?;
        while let Some(row) = rows.next()? {
            let dt = row.get::<usize, String>(5)?.parse::<DateTime<Utc>>();
            let mut char = Character::new();
            char.id = row.get(0)?;
            char.name = row.get(1)?;
            char.photo = row.get(4)?;
            char.corp = if let Ok(value) = row.get::<usize, i32>(2) {
                Some(PlayerDatabase::select_corporation(conn, vec![value])?[0].clone())
            } else {
                None
            };
            char.alliance = if let Ok(value) = row.get::<usize, i32>(3) {
                Some(PlayerDatabase::select_alliance(conn, vec![value])?[0].clone())
            } else {
                None
            };
            if let Ok(time) = dt {
                let utc_dt = DateTime::from_naive_utc_and_offset(time.naive_utc(), Utc);
                char.last_logon = utc_dt;
            }
            char.location = row.get::<usize, i32>(6)?;
            result.push(char);
        }
        Ok(result)
    }

    // Updated
    pub(crate) fn update_character(
        conn: &Connection,
        character: &Character,
    ) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut query = if character.alliance.is_none() {
            String::from("UPDATE char SET name = ?, corporation = ?, ")
        } else {
            String::from("UPDATE char SET name = ?, alliance = ?, corporation = ?, ")
        };
        query += "lastlogon = ?, location = ? WHERE id = ?;";
        let mut statement = conn.prepare(query.as_str()).unwrap();
        let params = if character.alliance.is_none() {
            rusqlite::params![
                character.name,
                character.corp.as_ref().unwrap().id,
                character.last_logon.to_string(),
                character.location,
                character.id
            ]
        } else {
            rusqlite::params![
                character.name,
                character.alliance.as_ref().unwrap().id,
                character.corp.as_ref().unwrap().id,
                character.last_logon.to_string(),
                character.location,
                character.id
            ]
        };
        let rows: usize = statement.execute(params)?;
        //PlayerDatabase::update_auth(conn, character.id, character.auth.as_ref().unwrap())?;
        Ok(rows)
    }

    pub(crate) fn select_auth(conn: &Connection) -> Result<AuthData, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let values = vec![
            String::from("token"),
            String::from("expiration"),
            String::from("refresh_token"),
        ];
        let mut result = AuthData::new();
        let query = String::from("SELECT id, value FROM metadata WHERE id IN rarray(?1)");

        let mut statement = conn.prepare(&query)?;
        let id_list: array::Array = Rc::new(
            values
                .into_iter()
                .map(rusqlite::types::Value::from)
                .collect::<Vec<rusqlite::types::Value>>(),
        );
        let mut rows = statement.query([id_list])?;
        while let Some(row) = rows.next()? {
            let field: String = row.get(0)?;
            if field.as_str() == "token" {
                result.token = row.get(1)?;
            }
            if field.as_str() == "expiration" {
                let date_as_string = row.get::<usize, String>(1)?;

                if let Ok(utc_dt) = DateTime::parse_from_rfc3339(&date_as_string) {
                    result.expiration = Some(utc_dt.to_utc());
                }
            }
            if field.as_str() == "refresh_token" {
                result.refresh_token = row.get(1)?;
            }
        }
        Ok(result)
    }

    pub(crate) fn insert_auth(conn: &Connection, auth_data: &AuthData) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut data: Vec<(String, String)> = Vec::new();
        let mut query = String::from("INSERT INTO metadata (id,value)");
        query += " VALUES (?1,?2)";
        data.push((String::from("token"), auth_data.token.clone()));
        data.push((
            String::from("refresh_token"),
            auth_data.refresh_token.clone(),
        ));
        if let Some(expiration_date) = auth_data.expiration {
            data.push((String::from("expiration"), expiration_date.to_rfc3339()));
        } else {
            data.push((String::from("expiration"), String::new()));
        }

        let mut rows = 0;
        for item in data {
            let mut statement = conn.prepare(&query)?;
            let affected_rows = statement.execute(params![item.0, item.1])?;
            rows += affected_rows;
        }
        Ok(rows)
    }

    pub(crate) fn update_auth(conn: &Connection, auth_data: &AuthData) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let query = String::from("UPDATE metadata SET value = ?1 WHERE id = ?2;");
        let mut data: Vec<(String, String)> = Vec::new();
        data.push((String::from("token"), auth_data.token.clone()));
        data.push((
            String::from("refresh_token"),
            auth_data.refresh_token.clone(),
        ));
        if let Some(expiration_date) = auth_data.expiration {
            data.push((String::from("expiration"), expiration_date.to_rfc3339()));
        } else {
            data.push((String::from("expiration"), String::new()));
        }
        let mut rows = 0;
        for item in data {
            let mut statement = conn.prepare(&query).unwrap();
            let affected_rows = statement.execute(params![item.1, item.0])?;
            statement.finalize()?;
            rows += affected_rows;
        }
        Ok(rows)
    }

    pub(crate) fn insert_character(conn: &Connection, player: &Character) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut query = String::from("INSERT INTO char (id,");
        query += "name,corporation,alliance,portrait,lastLogon,location) VALUES (?,?,?,?,?,?,?)";
        let mut statement = conn.prepare(query.as_str())?;
        let dt = player.last_logon.to_rfc3339();
        statement.raw_bind_parameter(1, player.id)?;
        statement.raw_bind_parameter(2, &player.name)?;
        if player.corp.is_some() {
            statement.raw_bind_parameter(3, player.corp.as_ref().unwrap().id)?;
        }
        if player.alliance.is_some() {
            statement.raw_bind_parameter(4, player.alliance.as_ref().unwrap().id)?;
        }
        if player.photo.is_some() {
            statement.raw_bind_parameter(5, player.photo.clone().unwrap())?;
        }
        statement.raw_bind_parameter(6, dt)?;
        statement.raw_bind_parameter(7, player.location)?;
        let rows = statement.raw_execute()?;
        //PlayerDatabase::insert_auth(conn,player.id,player.auth.as_ref().unwrap())?;
        Ok(rows)
    }

    fn repeat_vars(count: usize) -> String {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        assert_ne!(count, 0);
        let mut s = "?,".repeat(count);
        // Remove trailing comma
        s.pop();
        s
    }

    pub(crate) fn migrate_database() -> Result<bool, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();
        // TODO: migration database schema goes here
        Ok(true)
    }

    pub(crate) fn delete_characters(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::delete_general(conn, "char", ids)
    }

    // Corporation
    pub(crate) fn select_corporation(
        conn: &Connection,
        ids: Vec<i32>,
    ) -> Result<Vec<Corporation>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut result = Vec::new();
        let mut query = String::from("SELECT id,name FROM corp");
        if !ids.is_empty() {
            let vars = PlayerDatabase::repeat_vars(ids.len());
            query = format!("SELECT id,name FROM corp WHERE id IN ({})", vars);
        }
        let mut statement = conn.prepare(&query)?;
        let mut rows = statement.query(rusqlite::params_from_iter(ids))?;
        while let Some(row) = rows.next()? {
            let corp = Corporation {
                id: row.get::<usize, i32>(0)?,
                name: row.get::<usize, String>(1)?,
            };
            result.push(corp);
        }
        Ok(result)
    }

    pub(crate) fn update_corporation(
        conn: &Connection,
        corp: &Corporation,
    ) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::update_catalog(conn, "corp", corp)
    }

    pub(crate) fn insert_corporation(
        conn: &Connection,
        corp: &Corporation,
    ) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::insert_catalog(conn, "corp", corp)
    }

    pub(crate) fn delete_corporation(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::delete_general(conn, "corp", ids)
    }

    // Alliance
    pub(crate) fn select_alliance(
        conn: &Connection,
        ids: Vec<i32>,
    ) -> Result<Vec<Alliance>, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut result = Vec::new();
        let mut query = String::from("SELECT id,name FROM alliance");
        if !ids.is_empty() {
            let vars = PlayerDatabase::repeat_vars(ids.len());
            query = format!("SELECT id,name FROM alliance WHERE id IN ({})", vars);
        }
        let mut statement = conn.prepare(&query)?;
        let mut rows = statement.query(rusqlite::params_from_iter(ids))?;
        while let Some(row) = rows.next()? {
            let ally = Alliance {
                id: row.get::<usize, i32>(0)?,
                name: row.get::<usize, String>(1)?,
            };
            result.push(ally);
        }
        Ok(result)
    }

    pub(crate) fn update_alliance(conn: &Connection, ally: &Alliance) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::update_catalog(conn, "alliance", ally)
    }

    pub(crate) fn insert_alliance(conn: &Connection, ally: &Alliance) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::insert_catalog(conn, "alliance", ally)
    }
    pub(crate) fn delete_alliance(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        PlayerDatabase::delete_general(conn, "alliance", ids)
    }

    // function to delete values
    fn delete_general(conn: &Connection, table: &str, ids: Vec<i32>) -> Result<usize, Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!(table);

        if !ids.is_empty() {
            let vars = PlayerDatabase::repeat_vars(ids.len());
            let query = format!("DELETE FROM {} WHERE id IN ({})", table, vars);
            let mut statement = conn.prepare(&query)?;
            if let Ok(rows) = statement.execute(rusqlite::params_from_iter(ids)) {
                Ok(rows)
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    // generic Function to insert new values on a catalog
    fn insert_catalog<B: BasicCatalog>(
        conn: &Connection,
        table: &str,
        obj: &B,
    ) -> Result<usize, Error>
    where
        <B as BasicCatalog>::Output: ToSql,
    {
        #[cfg(feature = "puffin")]
        puffin::profile_function!(table);

        let query = format!("INSERT INTO {} (id,name) VALUES (?,?);", table);
        let mut statement = conn.prepare(&query)?;
        let params = rusqlite::params![obj.id(), obj.name()];
        let rows = statement.execute(params)?;
        Ok(rows)
    }

    // generic Function to update values on a catalog
    fn update_catalog<B: BasicCatalog>(
        conn: &Connection,
        table: &str,
        obj: &B,
    ) -> Result<usize, Error>
    where
        <B as BasicCatalog>::Output: ToSql,
    {
        #[cfg(feature = "puffin")]
        puffin::profile_function!(table);

        let query = format!("UPDATE {} SET name = ? WHERE id = ?;", table);
        let mut statement = conn.prepare(&query)?;
        let params = rusqlite::params![obj.name(), obj.id()];
        let rows = statement.execute(params)?;
        Ok(rows)
    }
}
