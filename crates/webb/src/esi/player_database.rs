use crate::esi::Error;
use crate::objects::{Alliance, AuthData, BasicCatalog, Character, Corporation};
use chrono::{DateTime, Utc};
use rusqlite::vtab::array;
use rusqlite::{Connection, ToSql, params};
use std::rc::Rc;

pub(crate) struct PlayerDatabase {}

impl PlayerDatabase {
    pub(crate) fn create_database(conn: &Connection) -> Result<bool, Error> {

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
        let mut query = String::from("UPDATE char SET name = :name, corporation = :corp,");
        if character.alliance.is_some() {
            query += " alliance = :alliance,";
        }
        query += "lastlogon = :last_logon, location = :location WHERE id = :id;";
        let mut statement = conn.prepare(query.as_str()).unwrap();

        let fecha = character.last_logon.to_rfc3339();
        let mut params: Vec<(&str, &dyn ToSql)> = vec![
            (":name", &character.name),
            (":corp", &character.corp.as_ref().unwrap().id),
            (":last_logon", &fecha),
            (":location", &character.location),
            (":id", &character.id),
        ];

        if let Some(alliance) = character.alliance.as_ref() {
            params.push((":alliance", &alliance.id));
        }
        let rows: usize = statement.execute(params.as_slice())?;
        //PlayerDatabase::update_auth(conn, character.id, character.auth.as_ref().unwrap())?;
        Ok(rows)
    }

    pub(crate) fn select_auth(conn: &Connection) -> Result<AuthData, Error> {

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

        /*let mut query = String::from("INSERT INTO char (id,");
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
        let rows = statement.raw_execute()?;*/

        let fecha = player.last_logon.to_rfc3339();
        let mut query = [
            String::from("INSERT INTO char (id,name,lastLogon,location"),
            String::from(" VALUES (:id,:name,:last_logon,:location"),
        ];
        let mut params: Vec<(&str, &dyn ToSql)> = vec![
            (":name", &player.name),
            (":last_logon", &fecha),
            (":location", &player.location),
            (":id", &player.id),
        ];

        if let Some(corp) = player.corp.as_ref() {
            query[0] += ",corporation";
            query[1] += ",:corp";
            params.push((":corp", &corp.id));
        }

        if let Some(alliance) = player.alliance.as_ref() {
            query[0] += ",alliance";
            query[1] += ",:alliance";
            params.push((":alliance", &alliance.id));
        }

        if let Some(photo) = player.photo.as_ref() {
            query[0] += ",portrait";
            query[1] += ",:portrait";
            params.push((":portrait", photo));
        }

        query[0] += ")";
        query[1] += ")";
        let mut statement = conn
            .prepare((query[0].clone() + &query[1]).as_str())
            .unwrap();
        let rows: usize = statement.execute(params.as_slice())?;

        //PlayerDatabase::insert_auth(conn,player.id,player.auth.as_ref().unwrap())?;
        Ok(rows)
    }

    fn repeat_vars(count: usize) -> String {

        assert_ne!(count, 0);
        let mut s = "?,".repeat(count);
        // Remove trailing comma
        s.pop();
        s
    }

    pub(crate) fn migrate_database() -> Result<bool, Error> {
        // TODO: migration database schema goes here
        Ok(true)
    }

    pub(crate) fn delete_characters(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {

        PlayerDatabase::delete_general(conn, "char", ids)
    }

    // Corporation
    pub(crate) fn select_corporation(
        conn: &Connection,
        ids: Vec<i32>,
    ) -> Result<Vec<Corporation>, Error> {

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

        PlayerDatabase::update_catalog(conn, "corp", corp)
    }

    pub(crate) fn insert_corporation(
        conn: &Connection,
        corp: &Corporation,
    ) -> Result<usize, Error> {

        PlayerDatabase::insert_catalog(conn, "corp", corp)
    }

    pub(crate) fn delete_corporation(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {

        PlayerDatabase::delete_general(conn, "corp", ids)
    }

    // Alliance
    pub(crate) fn select_alliance(
        conn: &Connection,
        ids: Vec<i32>,
    ) -> Result<Vec<Alliance>, Error> {

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

        PlayerDatabase::update_catalog(conn, "alliance", ally)
    }

    pub(crate) fn insert_alliance(conn: &Connection, ally: &Alliance) -> Result<usize, Error> {

        PlayerDatabase::insert_catalog(conn, "alliance", ally)
    }
    pub(crate) fn delete_alliance(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {

        PlayerDatabase::delete_general(conn, "alliance", ids)
    }

    // function to delete values
    fn delete_general(conn: &Connection, table: &str, ids: Vec<i32>) -> Result<usize, Error> {

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

        let query = format!("UPDATE {} SET name = ? WHERE id = ?;", table);
        let mut statement = conn.prepare(&query)?;
        let params = rusqlite::params![obj.name(), obj.id()];
        let rows = statement.execute(params)?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("cannot open in-memory database");
        array::load_module(&conn).expect("cannot load rarray module");
        conn
    }

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut statement = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<usize, String>(0))
            .unwrap();
        rows.map(|name| name.unwrap()).collect()
    }

    fn sample_alliance() -> Alliance {
        Alliance {
            id: 99000001,
            name: String::from("Acme Alliance"),
        }
    }

    fn sample_corporation() -> Corporation {
        Corporation {
            id: 98000001,
            name: String::from("Acme Corp"),
        }
    }

    fn sample_character() -> Character {
        let mut character = Character::new();
        character.id = 90000001;
        character.name = String::from("Test Pilot");
        character.corp = Some(sample_corporation());
        character.alliance = Some(sample_alliance());
        character.photo = Some(String::from(
            "https://images.evetech.net/characters/90000001/portrait",
        ));
        character.last_logon = DateTime::from_timestamp(1750000000, 0).unwrap();
        character.location = 30000001;
        character
    }

    // ---------------------------------------------------------------------
    // Schema
    // ---------------------------------------------------------------------

    #[test]
    fn create_database_creates_all_tables() {
        let conn = memory_connection();
        assert!(PlayerDatabase::create_database(&conn).unwrap());

        let tables = table_names(&conn);
        assert!(tables.contains(&String::from("char")));
        assert!(tables.contains(&String::from("corp")));
        assert!(tables.contains(&String::from("alliance")));
        assert!(tables.contains(&String::from("metadata")));
    }

    #[test]
    fn create_database_seeds_metadata_and_empty_auth() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();

        let db_version: String = conn
            .query_row("SELECT value FROM metadata WHERE id = 'db'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(db_version, "0");

        let auth = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(auth.token, "");
        assert_eq!(auth.refresh_token, "");
        assert_eq!(auth.expiration, None);
    }

    #[test]
    fn migrate_database_returns_true() {
        assert!(PlayerDatabase::migrate_database().unwrap());
    }

    // ---------------------------------------------------------------------
    // repeat_vars
    // ---------------------------------------------------------------------

    #[test]
    fn repeat_vars_generates_placeholders() {
        assert_eq!(PlayerDatabase::repeat_vars(1), "?");
        assert_eq!(PlayerDatabase::repeat_vars(3), "?,?,?");
    }

    #[test]
    #[should_panic]
    fn repeat_vars_panics_with_zero() {
        PlayerDatabase::repeat_vars(0);
    }

    // ---------------------------------------------------------------------
    // Alliance
    // ---------------------------------------------------------------------

    #[test]
    fn alliance_crud_roundtrip() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        let alliance = sample_alliance();

        // insert
        assert_eq!(
            PlayerDatabase::insert_alliance(&conn, &alliance).unwrap(),
            1
        );
        let stored = PlayerDatabase::select_alliance(&conn, vec![]).unwrap();
        assert_eq!(stored, vec![alliance.clone()]);

        // select by id
        let stored = PlayerDatabase::select_alliance(&conn, vec![alliance.id]).unwrap();
        assert_eq!(stored, vec![alliance.clone()]);

        // unknown id selects nothing
        let stored = PlayerDatabase::select_alliance(&conn, vec![12345]).unwrap();
        assert!(stored.is_empty());

        // update
        let renamed = Alliance {
            id: alliance.id,
            name: String::from("Renamed Alliance"),
        };
        assert_eq!(PlayerDatabase::update_alliance(&conn, &renamed).unwrap(), 1);
        let stored = PlayerDatabase::select_alliance(&conn, vec![alliance.id]).unwrap();
        assert_eq!(stored, vec![renamed]);

        // delete
        assert_eq!(
            PlayerDatabase::delete_alliance(&conn, vec![alliance.id]).unwrap(),
            1
        );
        let stored = PlayerDatabase::select_alliance(&conn, vec![]).unwrap();
        assert!(stored.is_empty());
    }

    #[test]
    fn delete_alliance_with_empty_ids_deletes_nothing() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        PlayerDatabase::insert_alliance(&conn, &sample_alliance()).unwrap();

        assert_eq!(PlayerDatabase::delete_alliance(&conn, vec![]).unwrap(), 0);
        assert_eq!(
            PlayerDatabase::select_alliance(&conn, vec![])
                .unwrap()
                .len(),
            1
        );
    }

    // ---------------------------------------------------------------------
    // Corporation
    // ---------------------------------------------------------------------

    #[test]
    fn corporation_crud_roundtrip() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        let corp = sample_corporation();

        // insert
        assert_eq!(PlayerDatabase::insert_corporation(&conn, &corp).unwrap(), 1);
        let stored = PlayerDatabase::select_corporation(&conn, vec![]).unwrap();
        assert_eq!(stored, vec![corp.clone()]);

        // select by id
        let stored = PlayerDatabase::select_corporation(&conn, vec![corp.id]).unwrap();
        assert_eq!(stored, vec![corp.clone()]);

        // update
        let renamed = Corporation {
            id: corp.id,
            name: String::from("Renamed Corp"),
        };
        assert_eq!(
            PlayerDatabase::update_corporation(&conn, &renamed).unwrap(),
            1
        );
        let stored = PlayerDatabase::select_corporation(&conn, vec![corp.id]).unwrap();
        assert_eq!(stored, vec![renamed]);

        // delete
        assert_eq!(
            PlayerDatabase::delete_corporation(&conn, vec![corp.id]).unwrap(),
            1
        );
        let stored = PlayerDatabase::select_corporation(&conn, vec![]).unwrap();
        assert!(stored.is_empty());
    }

    #[test]
    fn delete_corporation_with_empty_ids_deletes_nothing() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        PlayerDatabase::insert_corporation(&conn, &sample_corporation()).unwrap();

        assert_eq!(
            PlayerDatabase::delete_corporation(&conn, vec![]).unwrap(),
            0
        );
        assert_eq!(
            PlayerDatabase::select_corporation(&conn, vec![])
                .unwrap()
                .len(),
            1
        );
    }

    // ---------------------------------------------------------------------
    // Character
    // ---------------------------------------------------------------------

    #[test]
    fn character_insert_and_select_roundtrip() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        // select_characters resolves corp and alliance with subqueries, so
        // they must exist in their tables first.
        PlayerDatabase::insert_corporation(&conn, &sample_corporation()).unwrap();
        PlayerDatabase::insert_alliance(&conn, &sample_alliance()).unwrap();

        let character = sample_character();
        assert_eq!(
            PlayerDatabase::insert_character(&conn, &character).unwrap(),
            1
        );

        let stored = PlayerDatabase::select_characters(&conn, vec![]).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0], character);

        let stored = PlayerDatabase::select_characters(&conn, vec![character.id]).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Test Pilot");
    }

    #[test]
    fn character_without_relations_stores_nulls() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();

        let mut character = sample_character();
        character.corp = None;
        character.alliance = None;
        character.photo = None;
        PlayerDatabase::insert_character(&conn, &character).unwrap();

        let stored = PlayerDatabase::select_characters(&conn, vec![]).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0], character);
    }

    #[test]
    fn character_update_changes_fields() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        PlayerDatabase::insert_corporation(&conn, &sample_corporation()).unwrap();
        PlayerDatabase::insert_alliance(&conn, &sample_alliance()).unwrap();

        let character = sample_character();
        PlayerDatabase::insert_character(&conn, &character).unwrap();

        let mut updated = character.clone();
        updated.name = String::from("Renamed Pilot");
        updated.location = 30000002;
        assert_eq!(
            PlayerDatabase::update_character(&conn, &updated).unwrap(),
            1
        );

        let stored = PlayerDatabase::select_characters(&conn, vec![character.id]).unwrap();
        assert_eq!(stored, vec![updated]);
    }

    #[test]
    fn character_delete_removes_rows() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        // foreign keys are enforced, so the parents must exist first
        PlayerDatabase::insert_corporation(&conn, &sample_corporation()).unwrap();
        PlayerDatabase::insert_alliance(&conn, &sample_alliance()).unwrap();
        PlayerDatabase::insert_character(&conn, &sample_character()).unwrap();

        assert_eq!(PlayerDatabase::delete_characters(&conn, vec![]).unwrap(), 0);
        assert_eq!(
            PlayerDatabase::delete_characters(&conn, vec![90000001]).unwrap(),
            1
        );
        assert!(
            PlayerDatabase::select_characters(&conn, vec![])
                .unwrap()
                .is_empty()
        );
    }

    // ---------------------------------------------------------------------
    // Auth
    // ---------------------------------------------------------------------

    #[test]
    fn auth_insert_and_select_roundtrip() {
        let conn = memory_connection();
        array::load_module(&conn).unwrap();
        conn.execute(
            "CREATE TABLE metadata (id VARCHAR(255) PRIMARY KEY, value VARCHAR(255) NOT NULL);",
            [],
        )
        .unwrap();

        let mut auth = AuthData::new();
        auth.token = String::from("access-token");
        auth.refresh_token = String::from("refresh-token");
        auth.expiration = Some(DateTime::from_timestamp(1760000000, 0).unwrap());

        // one row per field: token, refresh_token, expiration
        assert_eq!(PlayerDatabase::insert_auth(&conn, &auth).unwrap(), 3);

        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(stored, auth);
    }

    #[test]
    fn auth_insert_without_expiration_stores_none() {
        let conn = memory_connection();
        conn.execute(
            "CREATE TABLE metadata (id VARCHAR(255) PRIMARY KEY, value VARCHAR(255) NOT NULL);",
            [],
        )
        .unwrap();

        let mut auth = AuthData::new();
        auth.token = String::from("access-token");
        auth.refresh_token = String::from("refresh-token");
        PlayerDatabase::insert_auth(&conn, &auth).unwrap();

        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(stored.token, "access-token");
        assert_eq!(stored.refresh_token, "refresh-token");
        assert_eq!(stored.expiration, None);
    }

    #[test]
    fn auth_update_persists_new_values() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();

        let mut auth = AuthData::new();
        auth.token = String::from("new-access-token");
        auth.refresh_token = String::from("new-refresh-token");
        auth.expiration = Some(DateTime::from_timestamp(1760000000, 0).unwrap());

        assert_eq!(PlayerDatabase::update_auth(&conn, &auth).unwrap(), 3);

        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(stored, auth);
    }
}
