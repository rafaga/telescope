//! What the zKillboard source has learned, kept on disk between runs.
//!
//! Asking zKillboard and ESI again for what was already asked costs requests
//! the services ask clients to spare, and makes the tooltips wait. A killmail
//! never changes once it happened, and the counts of the last day stay good
//! for an hour, so [`KillStore`] keeps in an SQLite file (not encrypted, it
//! holds nothing private) next to the SDE:
//!
//! * the killmails of the last [`WINDOW`] the tooltips use, whether they were
//!   read from the live feed or from ESI,
//! * the counts of the last day, with the time they were asked,
//! * where the feed walk had got to, so that the next start reads only the
//!   files published meanwhile instead of the whole hour again.
//!
//! The store is a convenience, never a requirement: if the file can't be
//! opened or written the source goes on without it (a warning is logged once).

use super::kill_context::{KillRecord, WINDOW};
use super::kill_day::DayCount;
use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// How the store behaves. The defaults are what Telescope uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillStoreConfig {
    /// Name of the file, in the folder of the SDE database.
    pub file: String,
    /// How far back the killmails are kept (and used).
    pub retention: TimeDelta,
    /// How long the counts of the last day are kept.
    pub day_counts_kept: TimeDelta,
    /// The old rows are deleted after this many inserts.
    pub prune_every: u32,
    /// Whether the file is used only when the SDE database it sits next to
    /// exists (so that nothing creates the file in a folder that has no
    /// Telescope data, as a test run would).
    pub needs_sde: bool,
}

impl Default for KillStoreConfig {
    fn default() -> Self {
        Self {
            file: String::from("zkillboard_cache.sqlite"),
            retention: WINDOW,
            day_counts_kept: TimeDelta::hours(24),
            prune_every: 500,
            needs_sde: true,
        }
    }
}

/// The disk cache of the zKillboard source.
///
/// The file lives next to the SDE database, whose path is shared with the UI
/// and may change: the connection follows it.
pub(crate) struct KillStore {
    config: KillStoreConfig,
    sde: Arc<RwLock<PathBuf>>,
    open: Mutex<Option<(PathBuf, Connection)>>,
    inserts: AtomicU32,
    warned: AtomicBool,
}

impl KillStore {
    pub(crate) fn new(sde: Arc<RwLock<PathBuf>>, config: KillStoreConfig) -> Self {
        Self {
            config,
            sde,
            open: Mutex::new(None),
            inserts: AtomicU32::new(0),
            warned: AtomicBool::new(false),
        }
    }

    /// Seconds since the Unix epoch.
    fn secs(time: DateTime<Utc>) -> i64 {
        time.timestamp()
    }

    /// The moment `secs` seconds after the Unix epoch.
    fn time(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap_or(DateTime::UNIX_EPOCH)
    }

    /// The version of the layout of the file (`PRAGMA user_version`).
    fn schema_version() -> i64 {
        1
    }

    /// The columns of a killmail row, in the order [`Self::record_of`] reads.
    fn columns() -> &'static str {
        "killmail_id, time, system_id, victim, ships, pilots"
    }

    /// The key of the sequence number the feed walk had got to.
    fn feed_next_key() -> &'static str {
        "feed_next"
    }

    /// Creates the tables of the file when it is new. Fails when the file
    /// was written by a newer Telescope: it is left alone.
    fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > Self::schema_version() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        if version < Self::schema_version() {
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS killmails (
                    killmail_id INTEGER PRIMARY KEY,
                    time        INTEGER NOT NULL,
                    system_id   INTEGER NOT NULL,
                    victim      TEXT,
                    ships       TEXT NOT NULL,
                    pilots      INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS killmails_time ON killmails (time);
                CREATE TABLE IF NOT EXISTS day_counts (
                    system_id  INTEGER PRIMARY KEY,
                    kills      INTEGER NOT NULL,
                    more       INTEGER NOT NULL,
                    fetched_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS meta (
                    key   TEXT PRIMARY KEY,
                    value INTEGER NOT NULL
                );",
            )?;
            connection.execute_batch(&format!(
                "PRAGMA user_version = {};",
                Self::schema_version()
            ))?;
        }
        Ok(())
    }

    /// Opens the file at `path`, creating it and its tables when needed.
    fn connect(path: &Path) -> rusqlite::Result<Connection> {
        let connection = Connection::open(path)?;
        // The journal mode answers with a row: read it as one.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        connection.execute_batch("PRAGMA synchronous = NORMAL;")?;
        // What a previous run left in the write-ahead log goes into the file.
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Self::create_schema(&connection)?;
        Ok(connection)
    }

    /// Runs `work` on the connection, opening it first when needed. `None`
    /// when the store can't be used (see the module documentation).
    fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        let sde = self
            .sde
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if self.config.needs_sde && !sde.exists() {
            return None;
        }
        let path = sde.with_file_name(&self.config.file);
        let mut open = self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if open.as_ref().is_none_or(|(opened, _)| *opened != path) {
            *open = match Self::connect(&path) {
                Ok(connection) => Some((path.clone(), connection)),
                Err(error) => {
                    self.warn(&path, &error);
                    None
                }
            };
        }
        let (_, connection) = open.as_ref()?;
        match work(connection) {
            Ok(value) => Some(value),
            Err(error) => {
                self.warn(&path, &error);
                // Start from a fresh connection the next time.
                *open = None;
                None
            }
        }
    }

    fn warn(&self, path: &Path, error: &rusqlite::Error) {
        if !self.warned.swap(true, Ordering::AcqRel) {
            tracing::warn!(path = %path.display(), %error, "zKillboard cache not usable");
        }
    }

    /// Keeps `record`. A killmail already kept is left as it is.
    pub(crate) fn insert(&self, record: &KillRecord, now: DateTime<Utc>) {
        let (Ok(id), Ok(system), Ok(ships)) = (
            i64::try_from(record.killmail_id),
            i64::try_from(record.system_id),
            serde_json::to_string(&record.attacker_ships),
        ) else {
            return;
        };
        self.with(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO killmails \
                 (killmail_id, time, system_id, victim, ships, pilots) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    Self::secs(record.time),
                    system,
                    record.victim,
                    ships,
                    record.pilots
                ],
            )
        });
        if self.inserts.fetch_add(1, Ordering::AcqRel) % self.config.prune_every.max(1) == 0 {
            self.prune(now);
        }
    }

    /// Deletes what is older than what is used.
    pub(crate) fn prune(&self, now: DateTime<Utc>) {
        let killmails = now
            .checked_sub_signed(self.config.retention)
            .map_or(0, Self::secs);
        let counts = now
            .checked_sub_signed(self.config.day_counts_kept)
            .map_or(0, Self::secs);
        self.with(|connection| {
            connection.execute("DELETE FROM killmails WHERE time < ?1", [killmails])?;
            connection.execute("DELETE FROM day_counts WHERE fetched_at < ?1", [counts])
        });
    }

    /// A row of `killmails` (in the order of [`Self::columns()`]) as a record.
    fn record_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<KillRecord> {
        let ships: String = row.get(4)?;
        Ok(KillRecord {
            killmail_id: u64::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
            time: Self::time(row.get(1)?),
            system_id: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
            victim: row.get(3)?,
            attacker_ships: serde_json::from_str(&ships).unwrap_or_default(),
            pilots: row.get(5)?,
        })
    }

    /// The killmails of the last [`WINDOW`], oldest first.
    pub(crate) fn records(&self, now: DateTime<Utc>) -> Vec<KillRecord> {
        let oldest = now
            .checked_sub_signed(self.config.retention)
            .map_or(0, Self::secs);
        self.with(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {} FROM killmails WHERE time >= ?1 ORDER BY time",
                Self::columns()
            ))?;
            let rows = statement.query_map([oldest], Self::record_of)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
    }

    /// The killmail `killmail_id`, if it is kept.
    pub(crate) fn record(&self, killmail_id: u64) -> Option<KillRecord> {
        let id = i64::try_from(killmail_id).ok()?;
        self.with(|connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {} FROM killmails WHERE killmail_id = ?1",
                        Self::columns()
                    ),
                    [id],
                    Self::record_of,
                )
                .optional()
        })
        .flatten()
    }

    /// Keeps the counts of the last day, each with how long ago it was asked.
    pub(crate) fn save_days(&self, counts: &[(usize, DayCount, TimeDelta)], now: DateTime<Utc>) {
        self.with(|connection| {
            let transaction = connection.unchecked_transaction()?;
            for (system, count, age) in counts {
                let (Ok(system), Ok(kills)) = (i64::try_from(*system), i64::try_from(count.kills))
                else {
                    continue;
                };
                let asked = now.checked_sub_signed(*age).map_or(0, Self::secs);
                transaction.execute(
                    "INSERT OR REPLACE INTO day_counts (system_id, kills, more, fetched_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![system, kills, count.more, asked],
                )?;
            }
            transaction.commit()
        });
    }

    /// The counts of the last day asked at most `max_age` ago, with how long
    /// ago.
    pub(crate) fn days(
        &self,
        now: DateTime<Utc>,
        max_age: TimeDelta,
    ) -> Vec<(usize, DayCount, TimeDelta)> {
        let oldest = now.checked_sub_signed(max_age).map_or(0, Self::secs);
        self.with(|connection| {
            let mut statement = connection.prepare(
                "SELECT system_id, kills, more, fetched_at FROM day_counts \
                 WHERE fetched_at >= ?1",
            )?;
            let rows = statement.query_map([oldest], |row| {
                let asked = Self::time(row.get(3)?);
                Ok((
                    usize::try_from(row.get::<_, i64>(0)?).unwrap_or(0),
                    DayCount {
                        kills: usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        more: row.get(2)?,
                    },
                    (now - asked).max(TimeDelta::zero()),
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
    }

    /// Keeps the sequence number the feed walk had got to.
    pub(crate) fn set_feed_next(&self, sequence: u64) {
        let Ok(sequence) = i64::try_from(sequence) else {
            return;
        };
        self.with(|connection| {
            connection.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                params![Self::feed_next_key(), sequence],
            )
        });
    }

    /// The sequence number the feed walk had got to, if it is known.
    pub(crate) fn feed_next(&self) -> Option<u64> {
        self.with(|connection| {
            connection
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    [Self::feed_next_key()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        })
        .flatten()
        .and_then(|sequence| u64::try_from(sequence).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            static COUNT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "telescope-kill-store-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn store(&self) -> KillStore {
            KillStore::new(
                Arc::new(RwLock::new(self.0.join("sde.sqlite"))),
                KillStoreConfig {
                    needs_sde: false,
                    ..KillStoreConfig::default()
                },
            )
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(id: u64, time: DateTime<Utc>) -> KillRecord {
        KillRecord {
            killmail_id: id,
            time,
            system_id: 30002813,
            victim: Some(String::from("Merlin")),
            attacker_ships: vec![String::from("Rifter"), String::from("Sabre")],
            pilots: 2,
        }
    }

    fn ago(now: DateTime<Utc>, minutes: u64) -> DateTime<Utc> {
        now - TimeDelta::minutes(i64::try_from(minutes).unwrap())
    }

    #[test]
    fn the_killmails_of_the_last_hour_survive_a_restart() {
        let folder = Folder::new();
        let now = Utc::now();
        {
            let store = folder.store();
            store.insert(&record(1, ago(now, 30)), now);
            store.insert(&record(2, ago(now, 5)), now);
            store.insert(&record(3, ago(now, 90)), now);
        }
        let records = folder.store().records(now);
        let ids: Vec<u64> = records.iter().map(|record| record.killmail_id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(records[0].attacker_ships, vec!["Rifter", "Sabre"]);
        assert_eq!(records[0].victim.as_deref(), Some("Merlin"));
        assert_eq!(records[0].pilots, 2);
        assert_eq!(
            KillStore::secs(records[0].time),
            KillStore::secs(ago(now, 30))
        );
    }

    #[test]
    fn a_killmail_is_kept_once() {
        let folder = Folder::new();
        let now = Utc::now();
        let store = folder.store();
        store.insert(&record(1, ago(now, 3)), now);
        store.insert(&record(1, ago(now, 3)), now);
        assert_eq!(store.records(now).len(), 1);
        assert_eq!(store.record(1).map(|record| record.pilots), Some(2));
        assert!(store.record(2).is_none());
    }

    #[test]
    fn pruning_drops_what_is_older_than_the_window() {
        let folder = Folder::new();
        let now = Utc::now();
        let store = folder.store();
        store.insert(&record(1, ago(now, 90)), now);
        store.insert(&record(2, ago(now, 10)), now);
        store.prune(now);
        assert!(store.record(1).is_none());
        assert!(store.record(2).is_some());
    }

    #[test]
    fn the_day_counts_come_back_with_their_age() {
        let folder = Folder::new();
        let now = Utc::now();
        let store = folder.store();
        let count = DayCount {
            kills: 7,
            more: false,
        };
        store.save_days(
            &[
                (1, count, TimeDelta::minutes(10)),
                (2, count, TimeDelta::hours(3)),
            ],
            now,
        );
        let kept = folder.store().days(now, TimeDelta::hours(1));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, 1);
        assert_eq!(kept[0].1, count);
        assert_eq!(kept[0].2.num_seconds(), 10 * 60);
    }

    #[test]
    fn the_feed_position_is_remembered() {
        let folder = Folder::new();
        let store = folder.store();
        assert_eq!(store.feed_next(), None);
        store.set_feed_next(1234);
        store.set_feed_next(1300);
        assert_eq!(folder.store().feed_next(), Some(1300));
    }

    #[test]
    fn without_the_sde_nothing_is_written() {
        let folder = Folder::new();
        std::fs::create_dir_all(&folder.0).unwrap();
        let store = KillStore::new(
            Arc::new(RwLock::new(folder.0.join("sde.sqlite"))),
            KillStoreConfig::default(),
        );
        let now = Utc::now();
        store.insert(&record(1, now), now);
        assert!(!folder.0.join(KillStoreConfig::default().file).exists());
        // With the SDE database there, the file is made.
        std::fs::write(folder.0.join("sde.sqlite"), b"").unwrap();
        store.insert(&record(1, now), now);
        assert!(folder.0.join(KillStoreConfig::default().file).exists());
    }

    #[test]
    fn an_unusable_location_just_gives_nothing() {
        let store = KillStore::new(
            Arc::new(RwLock::new(PathBuf::from(
                "/no/such/folder/at/all/sde.sqlite",
            ))),
            KillStoreConfig {
                needs_sde: false,
                ..KillStoreConfig::default()
            },
        );
        let now = Utc::now();
        store.insert(&record(1, now), now);
        assert!(store.records(now).is_empty());
        assert_eq!(store.feed_next(), None);
    }

    #[test]
    fn a_newer_layout_is_left_alone() {
        let folder = Folder::new();
        let now = Utc::now();
        let path = folder.0.join(KillStoreConfig::default().file);
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE other (a); PRAGMA user_version = 99;")
            .unwrap();
        let store = folder.store();
        store.insert(&record(1, now), now);
        assert!(store.records(now).is_empty());
    }
}
