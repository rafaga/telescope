//! Encryption of the player database (the `crypted-db` feature).
//!
//! The database is a SQLCipher file whose key is derived from the machine's
//! identifier (see `native_tools::get_*_unique_id`): a copy of the file is of
//! no use on another machine. The key is a raw 256-bit key, SHA-256 of the
//! identifier, so opening a connection -- which the manager does for every
//! query -- doesn't pay SQLCipher's passphrase derivation each time.
//!
//! [`prepare`] runs once, before the database is first opened, and brings an
//! existing file to that key:
//!
//! * a plain SQLite file (a build without `crypted-db`) is exported to an
//!   encrypted copy that replaces it;
//! * a file encrypted with the identifier itself as passphrase (the scheme
//!   before the raw key) is re-keyed.

use rusqlite::{Connection, Error, OpenFlags};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Value used when the operating system can't provide an identifier (the
/// same fallback `native_tools` returns as its error).
const FALLBACK_UNIQUE_ID: &str = "t313/sc0p3";

/// Separates this key from any other use of the same identifier.
const KEY_CONTEXT: &str = "telescope/player-database/v1:";

/// First 16 bytes of every plain SQLite file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// This machine's identifier, read once (on Linux it may ask D-Bus).
fn machine_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        #[cfg(target_os = "windows")]
        let id = native_tools::get_windows_unique_id();
        #[cfg(target_os = "macos")]
        let id = native_tools::get_macos_unique_id();
        #[cfg(target_os = "linux")]
        let id = native_tools::get_linux_unique_id();
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        let id: Result<String, String> = Err(String::from(FALLBACK_UNIQUE_ID));
        // Each of them returns the fallback as its error.
        id.unwrap_or_else(|fallback| fallback)
    })
}

/// The raw key for `id`, as SQLCipher's `x'<64 hex digits>'` literal.
fn raw_key(id: &str) -> String {
    let digest = Sha256::digest(format!("{KEY_CONTEXT}{id}").as_bytes());
    let mut key = String::from("x'");
    for byte in digest {
        let _ = write!(key, "{byte:02X}");
    }
    key.push('\'');
    key
}

/// This machine's key.
fn key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| raw_key(machine_id()))
}

/// Keys `connection` with this machine's key. It must be the first thing
/// done on the connection.
pub(crate) fn apply_key(connection: &Connection) -> Result<(), Error> {
    // A raw key goes in double quotes; it is only hex digits.
    connection.execute_batch(&format!("PRAGMA key = \"{}\";", key()))
}

/// What [`prepare`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preparation {
    /// Nothing: no file yet, or already under this machine's key.
    Ready,
    /// A plain SQLite file was encrypted.
    Encrypted,
    /// A file under the old passphrase key was re-keyed.
    Rekeyed,
}

/// Brings the database at `path` to this machine's key (see the module
/// docs). A file no key opens is left as it is, and the error says so.
pub(crate) fn prepare(path: &Path) -> Result<Preparation, Error> {
    // Trying a key that doesn't fit is how the file is recognized, and
    // SQLCipher logs every such attempt to stderr as an error ("hmac check
    // failed"). Its log is silenced while probing; an actual failure is
    // still returned (and logged by the caller).
    let quiet = QuietCipherLog::new();
    let result = prepare_with(path, machine_id());
    drop(quiet);
    result
}

/// Silences SQLCipher's log (a process-wide setting) until dropped, then
/// puts back the level it had.
struct QuietCipherLog {
    previous: Option<String>,
}

impl QuietCipherLog {
    fn new() -> Self {
        let previous = Connection::open_in_memory().ok().and_then(|connection| {
            let level = connection
                .query_row("PRAGMA cipher_log_level", [], |row| row.get::<_, String>(0))
                .ok()?;
            connection
                .query_row("PRAGMA cipher_log_level = NONE", [], |_| Ok(()))
                .ok()?;
            Some(level)
        });
        Self { previous }
    }
}

impl Drop for QuietCipherLog {
    fn drop(&mut self) {
        if let Some(level) = &self.previous
            && let Ok(connection) = Connection::open_in_memory()
        {
            // Only SQLCipher's own level names reach here.
            let _ =
                connection.query_row(
                    &format!("PRAGMA cipher_log_level = {level}"),
                    [],
                    |_| Ok(()),
                );
        }
    }
}

fn prepare_with(path: &Path, id: &str) -> Result<Preparation, Error> {
    let Ok(header) = read_header(path) else {
        // No file (or not readable yet): it is created under the key.
        return Ok(Preparation::Ready);
    };
    if header.is_empty() {
        return Ok(Preparation::Ready);
    }
    if header == SQLITE_HEADER {
        encrypt_plain(path, &raw_key(id))?;
        return Ok(Preparation::Encrypted);
    }
    if opens_with(path, &format!("PRAGMA key = \"{}\";", raw_key(id))) {
        return Ok(Preparation::Ready);
    }
    // The scheme before the raw key: the identifier as passphrase. Also the
    // fallback value, which was the key on Linux (and wherever the system
    // had no identifier).
    for passphrase in [id, FALLBACK_UNIQUE_ID] {
        let pragma = format!("PRAGMA key = {};", quote(passphrase));
        if opens_with(path, &pragma) {
            // Re-keying rewrites the file in place: keep a copy until it
            // is done, and put it back if it fails.
            let backup = side_path(path, ".rekeying");
            std::fs::copy(path, &backup).map_err(io_error)?;
            if let Err(error) = rekey(path, &pragma, &raw_key(id)) {
                let _ = std::fs::copy(&backup, path);
                let _ = std::fs::remove_file(&backup);
                return Err(error);
            }
            let _ = std::fs::remove_file(&backup);
            return Ok(Preparation::Rekeyed);
        }
    }
    Err(Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
        Some(format!(
            "{} is encrypted with a key this machine doesn't have",
            path.display()
        )),
    ))
}

/// The first 16 bytes of the file (fewer for a shorter file).
fn read_header(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut header = Vec::with_capacity(16);
    std::fs::File::open(path)?
        .take(16)
        .read_to_end(&mut header)?;
    Ok(header)
}

/// A SQL string literal.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Whether `key_pragma` opens the database at `path`.
fn opens_with(path: &Path, key_pragma: &str) -> bool {
    let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
    else {
        return false;
    };
    connection.execute_batch(key_pragma).is_ok()
        && connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok()
}

/// Changes the key of the database at `path` from `key_pragma` to `raw_key`.
fn rekey(path: &Path, key_pragma: &str, raw_key: &str) -> Result<(), Error> {
    let connection = Connection::open(path)?;
    connection.execute_batch(key_pragma)?;
    // Re-keying rewrites every page; SQLCipher doesn't do it in WAL mode.
    connection.pragma_update_and_check(None, "journal_mode", "DELETE", |row| {
        row.get::<_, String>(0)
    })?;
    connection.execute_batch(&format!("PRAGMA rekey = \"{raw_key}\";"))?;
    connection.close().map_err(|(_, error)| error)
}

/// Replaces the plain database at `path` with an encrypted copy under
/// `raw_key`. The copy is written next to it first: if anything fails the
/// plain file is still there.
fn encrypt_plain(path: &Path, raw_key: &str) -> Result<(), Error> {
    let encrypted = side_path(path, ".encrypting");
    let _ = std::fs::remove_file(&encrypted);
    {
        let connection = Connection::open(path)?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        connection.execute(
            &format!("ATTACH DATABASE ?1 AS encrypted KEY \"{raw_key}\""),
            [encrypted.to_string_lossy()],
        )?;
        connection.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))?;
        connection.execute_batch(&format!(
            "PRAGMA encrypted.user_version = {version}; DETACH DATABASE encrypted;"
        ))?;
        connection.close().map_err(|(_, error)| error)?;
    }
    // The plain file and its write-ahead log hold the tokens in clear.
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(side_path(path, suffix));
    }
    std::fs::rename(&encrypted, path).map_err(io_error)
}

/// A file system error as a database error.
fn io_error(error: std::io::Error) -> Error {
    Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        Some(error.to_string()),
    )
}

/// `path` with `suffix` appended to its file name.
fn side_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("webb-cipher-{name}-{}.db", std::process::id()));
        cleanup(&path);
        path
    }

    fn cleanup(path: &Path) {
        for suffix in ["", "-wal", "-shm", ".encrypting", ".rekeying"] {
            let _ = std::fs::remove_file(side_path(path, suffix));
        }
    }

    /// A database with one row and a schema version, keyed with `pragma`
    /// (plain when `None`).
    fn create(path: &Path, pragma: Option<&str>) {
        let connection = Connection::open(path).unwrap();
        if let Some(pragma) = pragma {
            connection.execute_batch(pragma).unwrap();
        }
        connection
            .execute_batch(
                "CREATE TABLE auth (id INTEGER, token TEXT);
                 INSERT INTO auth VALUES (1, 'secret-token');
                 PRAGMA user_version = 7;",
            )
            .unwrap();
    }

    /// The row and version, read with this `id`'s raw key.
    fn read(path: &Path, id: &str) -> (String, i64) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(&format!("PRAGMA key = \"{}\";", raw_key(id)))
            .unwrap();
        let token = connection
            .query_row("SELECT token FROM auth WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        (token, version)
    }

    fn contains(path: &Path, needle: &str) -> bool {
        std::fs::read(path)
            .unwrap()
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    #[test]
    fn the_raw_key_is_64_hex_digits_and_depends_on_the_id() {
        let key = raw_key("machine");
        assert!(key.starts_with("x'") && key.ends_with('\''));
        assert_eq!(key.len(), 2 + 64 + 1);
        assert_ne!(key, raw_key("other machine"));
        assert_eq!(key, raw_key("machine"));
    }

    #[test]
    fn probing_restores_the_cipher_log_level() {
        let level = || {
            Connection::open_in_memory()
                .unwrap()
                .query_row("PRAGMA cipher_log_level", [], |row| row.get::<_, String>(0))
                .unwrap()
        };
        let before = level();
        {
            let _quiet = QuietCipherLog::new();
            assert_eq!(level(), "NONE");
        }
        assert_eq!(level(), before);
    }

    #[test]
    fn a_missing_file_is_ready() {
        let path = temp_path("missing");
        assert_eq!(prepare_with(&path, "machine").unwrap(), Preparation::Ready);
        assert!(!path.exists());
    }

    #[test]
    fn a_plain_database_is_encrypted_keeping_its_data() {
        let path = temp_path("plain");
        create(&path, None);
        assert!(contains(&path, "secret-token"));

        assert_eq!(
            prepare_with(&path, "machine").unwrap(),
            Preparation::Encrypted
        );
        assert!(!contains(&path, "secret-token"));
        assert_eq!(read(&path, "machine"), (String::from("secret-token"), 7));
        // Now it is ready as it is.
        assert_eq!(prepare_with(&path, "machine").unwrap(), Preparation::Ready);
        cleanup(&path);
    }

    #[test]
    fn a_database_under_the_old_passphrase_is_rekeyed() {
        for passphrase in ["machine", FALLBACK_UNIQUE_ID, "it's"] {
            let path = temp_path("legacy");
            create(&path, Some(&format!("PRAGMA key = {};", quote(passphrase))));
            let id = if passphrase == FALLBACK_UNIQUE_ID {
                "machine"
            } else {
                passphrase
            };
            assert_eq!(prepare_with(&path, id).unwrap(), Preparation::Rekeyed);
            assert_eq!(read(&path, id), (String::from("secret-token"), 7));
            assert!(!side_path(&path, ".rekeying").exists());
            cleanup(&path);
        }
    }

    #[test]
    fn a_database_of_another_machine_is_left_alone() {
        let path = temp_path("foreign");
        create(
            &path,
            Some(&format!("PRAGMA key = \"{}\";", raw_key("other machine"))),
        );
        let before = std::fs::read(&path).unwrap();
        let quiet = QuietCipherLog::new();
        assert!(prepare_with(&path, "machine").is_err());
        drop(quiet);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        cleanup(&path);
    }
}
