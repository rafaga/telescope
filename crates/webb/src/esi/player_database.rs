//! SQLite schema and queries of the local player database: characters,
//! corporations, alliances and the authorization tokens of the linked characters.

use crate::esi::Error;
use crate::objects::{Alliance, AuthData, BasicCatalog, Character, Corporation};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, ToSql, params};
use std::collections::HashMap;

pub(crate) struct PlayerDatabase {}

/// Schema version this build writes and expects, stored in the `metadata`
/// row `db`. To change the schema: bump it, update `create_database` (new
/// databases start at the latest version directly) and append the script
/// that takes a database from the previous version to this one to
/// [`MIGRATIONS`].
///
/// - 0: one token set for every character, in `metadata`.
/// - 1: one token set per character, in the `auth` table.
pub const SCHEMA_VERSION: i32 = 1;

/// A migration script: changes only what its version step needs.
type Migration = fn(&Connection) -> Result<(), Error>;

/// `MIGRATIONS[n]` takes a database from schema version `n` to `n + 1`.
const MIGRATIONS: &[Migration] = &[PlayerDatabase::migrate_0_to_1];

// One migration per version step, always.
const _: () = assert!(MIGRATIONS.len() == SCHEMA_VERSION as usize);

/// Creation script of the `auth` table, shared by `create_database` and
/// the 0 -> 1 migration.
const CREATE_AUTH_TABLE: &str = "CREATE TABLE auth (id INTEGER PRIMARY KEY \
    REFERENCES char(id) ON DELETE CASCADE ON UPDATE CASCADE, \
    token TEXT NOT NULL, refresh_token TEXT NOT NULL, expiration TEXT NOT NULL)";

/// What [`PlayerDatabase::ensure_schema`] found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatus {
    /// There was no schema (new or empty file): it was created.
    Created,
    /// The schema was at the given older version: the pending migration
    /// scripts were run, keeping the stored data.
    Migrated(i32),
    /// Already at [`SCHEMA_VERSION`]: nothing to do.
    UpToDate,
    /// Written by a newer Telescope, at the given version: left untouched.
    Newer(i32),
}

impl PlayerDatabase {
    /// Brings the database to [`SCHEMA_VERSION`]: creates the schema when
    /// there is none, runs the pending migration scripts (and only those)
    /// when it is older, and does nothing when it is current or newer.
    /// Creation and migration run in a single transaction, so a failure
    /// leaves the database as it was.
    #[tracing::instrument]
    pub(crate) fn ensure_schema(conn: &Connection) -> Result<SchemaStatus, Error> {
        match PlayerDatabase::schema_version(conn)? {
            None => {
                let transaction = conn.unchecked_transaction()?;
                PlayerDatabase::create_database(&transaction)?;
                transaction.commit()?;
                Ok(SchemaStatus::Created)
            }
            Some(version) if version < SCHEMA_VERSION => {
                let transaction = conn.unchecked_transaction()?;
                for step in version.max(0)..SCHEMA_VERSION {
                    MIGRATIONS[step as usize](&transaction)?;
                }
                PlayerDatabase::set_schema_version(&transaction, SCHEMA_VERSION)?;
                transaction.commit()?;
                Ok(SchemaStatus::Migrated(version))
            }
            Some(version) if version > SCHEMA_VERSION => Ok(SchemaStatus::Newer(version)),
            Some(_) => Ok(SchemaStatus::UpToDate),
        }
    }

    /// Schema version stored in the database, or `None` when it has no
    /// schema yet (no `metadata` table, e.g. a brand-new or empty file).
    #[tracing::instrument]
    pub(crate) fn schema_version(conn: &Connection) -> Result<Option<i32>, Error> {
        let query = "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'metadata'";
        let tables: i64 = conn.query_row(query, [], |row| row.get(0))?;
        if tables == 0 {
            return Ok(None);
        }
        let query = "SELECT value FROM metadata WHERE id = 'db'";
        let value: String = conn.query_row(query, [], |row| row.get(0))?;
        value.trim().parse::<i32>().map(Some).map_err(|t_error| {
            Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(t_error))
        })
    }

    fn set_schema_version(conn: &Connection, version: i32) -> Result<(), Error> {
        let query = "UPDATE metadata SET value = ?1 WHERE id = 'db'";
        conn.execute(query, [version.to_string()])?;
        Ok(())
    }

    /// 0 -> 1: tokens move from a single set in `metadata` to one set per
    /// character in the new `auth` table. The stored set is kept for the
    /// character it belongs to (read from the token's own `sub` claim, no
    /// network needed); the other characters have no token and must be
    /// linked again. Characters, corporations and alliances are untouched.
    #[tracing::instrument]
    fn migrate_0_to_1(conn: &Connection) -> Result<(), Error> {
        conn.execute(CREATE_AUTH_TABLE, [])?;

        let value = |id: &str| -> Result<Option<String>, Error> {
            let mut statement = conn.prepare("SELECT value FROM metadata WHERE id = ?1")?;
            let mut rows = statement.query([id])?;
            Ok(match rows.next()? {
                Some(row) => Some(row.get(0)?),
                None => None,
            })
        };
        let token = value("token")?.unwrap_or_default();
        let refresh_token = value("refresh_token")?.unwrap_or_default();
        let expiration = value("expiration")?.unwrap_or_default();

        if let Some(character_id) = jwt_character_id(&token)
            && !refresh_token.is_empty()
            && !PlayerDatabase::select_characters(conn, vec![character_id])?.is_empty()
        {
            let mut auth = AuthData::new();
            auth.token = token;
            auth.refresh_token = refresh_token;
            if let Ok(utc_dt) = DateTime::parse_from_rfc3339(&expiration) {
                auth.expiration = Some(utc_dt.to_utc());
            }
            PlayerDatabase::insert_auth(conn, character_id, &auth)?;
        }

        let query = "DELETE FROM metadata WHERE id IN ('token', 'refresh_token', 'expiration')";
        conn.execute(query, [])?;
        Ok(())
    }

    #[tracing::instrument]
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

        // OAuth tokens, one set per character (an EVE SSO token only works
        // for the character that logged in).
        statement = conn.prepare(CREATE_AUTH_TABLE)?;
        statement.execute([])?;

        // Telescope Metadata
        let query =
            "CREATE TABLE metadata (id VARCHAR(255) PRIMARY KEY,value VARCHAR(255) NOT NULL);";
        statement = conn.prepare(query)?;
        statement.execute([])?;
        let query = "INSERT INTO metadata (id,value) VALUES (?,?)";
        statement = conn.prepare(query)?;
        statement.execute(["db", SCHEMA_VERSION.to_string().as_str()])?;
        Ok(true)
    }

    #[tracing::instrument]
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
    #[tracing::instrument]
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

    /// Token sets of every linked character, by character id.
    #[tracing::instrument]
    pub(crate) fn select_auth(conn: &Connection) -> Result<HashMap<i32, AuthData>, Error> {
        let query = "SELECT id, token, refresh_token, expiration FROM auth";
        let mut statement = conn.prepare(query)?;
        let mut rows = statement.query([])?;
        let mut result = HashMap::new();
        while let Some(row) = rows.next()? {
            let mut auth = AuthData::new();
            auth.token = row.get(1)?;
            auth.refresh_token = row.get(2)?;
            let expiration: String = row.get(3)?;
            if let Ok(utc_dt) = DateTime::parse_from_rfc3339(&expiration) {
                auth.expiration = Some(utc_dt.to_utc());
            }
            result.insert(row.get(0)?, auth);
        }
        Ok(result)
    }

    /// Stores the token set of a character: updates its row, or inserts it
    /// when the character has none yet.
    #[tracing::instrument(skip(auth_data))]
    pub(crate) fn save_auth(
        conn: &Connection,
        character_id: i32,
        auth_data: &AuthData,
    ) -> Result<usize, Error> {
        let rows = PlayerDatabase::update_auth(conn, character_id, auth_data)?;
        if rows > 0 {
            return Ok(rows);
        }
        PlayerDatabase::insert_auth(conn, character_id, auth_data)
    }

    #[tracing::instrument(skip(auth_data))]
    pub(crate) fn insert_auth(
        conn: &Connection,
        character_id: i32,
        auth_data: &AuthData,
    ) -> Result<usize, Error> {
        let query =
            "INSERT INTO auth (id, token, refresh_token, expiration) VALUES (?1, ?2, ?3, ?4)";
        let mut statement = conn.prepare(query)?;
        let rows = statement.execute(params![
            character_id,
            auth_data.token,
            auth_data.refresh_token,
            PlayerDatabase::expiration_text(auth_data),
        ])?;
        Ok(rows)
    }

    #[tracing::instrument(skip(auth_data))]
    pub(crate) fn update_auth(
        conn: &Connection,
        character_id: i32,
        auth_data: &AuthData,
    ) -> Result<usize, Error> {
        let query = "UPDATE auth SET token = ?1, refresh_token = ?2, expiration = ?3 WHERE id = ?4";
        let mut statement = conn.prepare(query)?;
        let rows = statement.execute(params![
            auth_data.token,
            auth_data.refresh_token,
            PlayerDatabase::expiration_text(auth_data),
            character_id,
        ])?;
        Ok(rows)
    }

    #[tracing::instrument]
    pub(crate) fn delete_auth(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        PlayerDatabase::delete_general(conn, "auth", ids)
    }

    fn expiration_text(auth_data: &AuthData) -> String {
        auth_data
            .expiration
            .map(|expiration| expiration.to_rfc3339())
            .unwrap_or_default()
    }

    #[tracing::instrument]
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

    #[tracing::instrument]
    fn repeat_vars(count: usize) -> String {
        assert_ne!(count, 0);
        let mut s = "?,".repeat(count);
        // Remove trailing comma
        s.pop();
        s
    }

    #[tracing::instrument]
    pub(crate) fn delete_characters(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        PlayerDatabase::delete_general(conn, "char", ids)
    }

    // Corporation
    #[tracing::instrument]
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

    #[tracing::instrument]
    pub(crate) fn update_corporation(
        conn: &Connection,
        corp: &Corporation,
    ) -> Result<usize, Error> {
        PlayerDatabase::update_catalog(conn, "corp", corp)
    }

    #[tracing::instrument]
    pub(crate) fn insert_corporation(
        conn: &Connection,
        corp: &Corporation,
    ) -> Result<usize, Error> {
        PlayerDatabase::insert_catalog(conn, "corp", corp)
    }

    #[tracing::instrument]
    pub(crate) fn delete_corporation(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        PlayerDatabase::delete_general(conn, "corp", ids)
    }

    // Alliance
    #[tracing::instrument]
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

    #[tracing::instrument]
    pub(crate) fn update_alliance(conn: &Connection, ally: &Alliance) -> Result<usize, Error> {
        PlayerDatabase::update_catalog(conn, "alliance", ally)
    }

    #[tracing::instrument]
    pub(crate) fn insert_alliance(conn: &Connection, ally: &Alliance) -> Result<usize, Error> {
        PlayerDatabase::insert_catalog(conn, "alliance", ally)
    }
    #[tracing::instrument]
    pub(crate) fn delete_alliance(conn: &Connection, ids: Vec<i32>) -> Result<usize, Error> {
        PlayerDatabase::delete_general(conn, "alliance", ids)
    }

    // function to delete values
    #[tracing::instrument]
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
    #[tracing::instrument(skip(obj))]
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
    #[tracing::instrument(skip(obj))]
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

/// Character id from the `sub` claim (`CHARACTER:EVE:<id>`) of an EVE SSO
/// access token (a JWT), without verifying it: only used to tell which
/// character an already stored token belongs to.
fn jwt_character_id(token: &str) -> Option<i32> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims["sub"].as_str()?.split(':').nth(2)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::vtab::array;

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
    fn create_database_stamps_the_version_and_starts_without_tokens() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();

        assert_eq!(
            PlayerDatabase::schema_version(&conn).unwrap(),
            Some(SCHEMA_VERSION)
        );
        assert!(table_names(&conn).contains(&String::from("auth")));
        assert!(PlayerDatabase::select_auth(&conn).unwrap().is_empty());
    }

    #[test]
    fn schema_version_is_none_without_schema() {
        let conn = memory_connection();
        assert_eq!(PlayerDatabase::schema_version(&conn).unwrap(), None);
    }

    #[test]
    fn ensure_schema_creates_then_reports_up_to_date() {
        let conn = memory_connection();
        assert_eq!(
            PlayerDatabase::ensure_schema(&conn).unwrap(),
            SchemaStatus::Created
        );
        assert!(table_names(&conn).contains(&String::from("char")));
        assert_eq!(
            PlayerDatabase::ensure_schema(&conn).unwrap(),
            SchemaStatus::UpToDate
        );
    }

    /// Unsigned JWT whose `sub` claim is the given character (enough for
    /// `jwt_character_id`, which doesn't verify the signature).
    fn fake_jwt(character_id: i32) -> String {
        use base64::Engine;
        let encode =
            |json: String| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        format!(
            "{}.{}.signature",
            encode(String::from(r#"{"alg":"RS256"}"#)),
            encode(format!(
                r#"{{"sub":"CHARACTER:EVE:{character_id}","name":"Old"}}"#
            ))
        )
    }

    /// Version 0 schema, as older builds created it (single token set in
    /// `metadata`, no `auth` table), with characters 1 and 2 and the given
    /// stored access token.
    fn create_version_0_schema(conn: &Connection, token: &str) {
        conn.execute_batch(
            "CREATE TABLE char (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL,
                corporation INTEGER REFERENCES corp(id) ON DELETE CASCADE ON UPDATE CASCADE,
                alliance INTEGER REFERENCES alliance(id) ON DELETE CASCADE ON UPDATE CASCADE,
                portrait BLOB, lastLogon DATETIME NOT NULL, location INTEGER NOT NULL);
             CREATE TABLE corp (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL);
             CREATE TABLE alliance (id INTEGER PRIMARY KEY, name VARCHAR(255) NOT NULL);
             INSERT INTO corp (id,name) VALUES (98000001,'Acme Corp');
             CREATE TABLE metadata (id VARCHAR(255) PRIMARY KEY,value VARCHAR(255) NOT NULL);
             INSERT INTO metadata (id,value) VALUES ('db','0'), ('refresh_token','old-refresh'),
                ('expiration','2026-01-01T00:00:00+00:00');
             INSERT INTO char (id,name,corporation,lastLogon,location) VALUES
                (1,'One',98000001,'2026-01-01T00:00:00+00:00',30000142),
                (2,'Two',98000001,'2026-01-01T00:00:00+00:00',30000142);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO metadata (id,value) VALUES ('token',?1)",
            [token],
        )
        .unwrap();
    }

    fn token_rows_in_metadata(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM metadata WHERE id <> 'db'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn ensure_schema_migrates_version_0_keeping_data() {
        let conn = memory_connection();
        create_version_0_schema(&conn, &fake_jwt(2));

        assert_eq!(
            PlayerDatabase::ensure_schema(&conn).unwrap(),
            SchemaStatus::Migrated(0)
        );
        assert_eq!(
            PlayerDatabase::schema_version(&conn).unwrap(),
            Some(SCHEMA_VERSION)
        );
        // Characters and corporations are kept.
        assert_eq!(
            PlayerDatabase::select_characters(&conn, vec![])
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            PlayerDatabase::select_corporation(&conn, vec![])
                .unwrap()
                .len(),
            1
        );
        // The stored token went to the character it belongs to.
        let auth = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[&2].token, fake_jwt(2));
        assert_eq!(auth[&2].refresh_token, "old-refresh");
        assert!(auth[&2].expiration.is_some());
        assert_eq!(token_rows_in_metadata(&conn), 0);

        assert_eq!(
            PlayerDatabase::ensure_schema(&conn).unwrap(),
            SchemaStatus::UpToDate
        );
    }

    #[test]
    fn migration_skips_a_token_it_cannot_attribute() {
        let conn = memory_connection();
        // Not a JWT (and, below, a JWT for a character that isn't linked).
        create_version_0_schema(&conn, "opaque-token");
        PlayerDatabase::ensure_schema(&conn).unwrap();
        assert!(PlayerDatabase::select_auth(&conn).unwrap().is_empty());
        assert_eq!(
            PlayerDatabase::select_characters(&conn, vec![])
                .unwrap()
                .len(),
            2
        );

        let conn = memory_connection();
        create_version_0_schema(&conn, &fake_jwt(99));
        PlayerDatabase::ensure_schema(&conn).unwrap();
        assert!(PlayerDatabase::select_auth(&conn).unwrap().is_empty());
    }

    #[test]
    fn a_failed_migration_leaves_the_database_as_it_was() {
        let conn = memory_connection();
        create_version_0_schema(&conn, &fake_jwt(1));
        // Makes the migration's CREATE TABLE fail.
        conn.execute("CREATE TABLE auth (id INTEGER)", []).unwrap();

        assert!(PlayerDatabase::ensure_schema(&conn).is_err());
        assert_eq!(PlayerDatabase::schema_version(&conn).unwrap(), Some(0));
        assert_eq!(token_rows_in_metadata(&conn), 3);
    }

    #[test]
    fn jwt_character_id_reads_the_sub_claim() {
        assert_eq!(jwt_character_id(&fake_jwt(95279738)), Some(95279738));
        assert_eq!(jwt_character_id("not-a-jwt"), None);
        assert_eq!(jwt_character_id(""), None);
    }

    #[test]
    fn ensure_schema_leaves_a_newer_schema_untouched() {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        conn.execute("UPDATE metadata SET value = '99' WHERE id = 'db'", [])
            .unwrap();
        assert_eq!(
            PlayerDatabase::ensure_schema(&conn).unwrap(),
            SchemaStatus::Newer(99)
        );
    }

    #[test]
    fn update_auth_without_schema_is_an_error_not_a_panic() {
        let conn = memory_connection();
        assert!(PlayerDatabase::update_auth(&conn, 1, &AuthData::new()).is_err());
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

    /// Creates the schema plus the characters the auth rows will point to
    /// (foreign keys are enforced).
    fn database_with_characters(ids: &[i32]) -> Connection {
        let conn = memory_connection();
        PlayerDatabase::create_database(&conn).unwrap();
        for id in ids {
            let mut character = Character::new();
            character.id = *id;
            character.name = format!("Pilot {id}");
            PlayerDatabase::insert_character(&conn, &character).unwrap();
        }
        conn
    }

    fn sample_auth(token: &str) -> AuthData {
        let mut auth = AuthData::new();
        auth.token = format!("{token}-access");
        auth.refresh_token = format!("{token}-refresh");
        auth.expiration = Some(DateTime::from_timestamp(1760000000, 0).unwrap());
        auth
    }

    #[test]
    fn auth_is_stored_per_character() {
        let conn = database_with_characters(&[1, 2]);

        assert_eq!(
            PlayerDatabase::insert_auth(&conn, 1, &sample_auth("one")).unwrap(),
            1
        );
        assert_eq!(
            PlayerDatabase::insert_auth(&conn, 2, &sample_auth("two")).unwrap(),
            1
        );

        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[&1], sample_auth("one"));
        assert_eq!(stored[&2], sample_auth("two"));
    }

    #[test]
    fn auth_without_expiration_stores_none() {
        let conn = database_with_characters(&[1, 2]);
        let mut auth = sample_auth("one");
        auth.expiration = None;
        PlayerDatabase::insert_auth(&conn, 1, &auth).unwrap();

        assert_eq!(
            PlayerDatabase::select_auth(&conn).unwrap()[&1].expiration,
            None
        );
    }

    #[test]
    fn save_auth_inserts_then_updates_only_that_character() {
        let conn = database_with_characters(&[1, 2]);
        PlayerDatabase::save_auth(&conn, 1, &sample_auth("one")).unwrap();
        PlayerDatabase::save_auth(&conn, 2, &sample_auth("two")).unwrap();

        assert_eq!(
            PlayerDatabase::save_auth(&conn, 1, &sample_auth("one-new")).unwrap(),
            1
        );
        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[&1], sample_auth("one-new"));
        assert_eq!(stored[&2], sample_auth("two"));
    }

    #[test]
    fn delete_auth_removes_only_the_given_characters() {
        let conn = database_with_characters(&[1, 2]);
        PlayerDatabase::save_auth(&conn, 1, &sample_auth("one")).unwrap();
        PlayerDatabase::save_auth(&conn, 2, &sample_auth("two")).unwrap();

        assert_eq!(PlayerDatabase::delete_auth(&conn, vec![1]).unwrap(), 1);
        let stored = PlayerDatabase::select_auth(&conn).unwrap();
        assert!(!stored.contains_key(&1));
        assert!(stored.contains_key(&2));
    }
}
