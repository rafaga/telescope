//! The guard's store in the encrypted player database. The guard itself (what
//! it remembers, the pace and the pauses) lives in the `zkill` crate.

use std::sync::Mutex;

pub(crate) use zkill::guard::{GuardConfig, GuardStore, RequestGuard};

/// The guard's store in the encrypted player database: one row for each
/// endpoint (see [`zkill::guard::Endpoint`]).
pub(crate) struct PlayerGuardStore {
    esi: Mutex<webb::esi::EsiManager>,
    key: &'static str,
}

impl PlayerGuardStore {
    /// The row of the database's `metadata` table the R2Z2 guard uses.
    pub(crate) const R2Z2: &'static str = "zkillboard_guard_r2z2";
    /// The row the query API's guard uses.
    pub(crate) const API: &'static str = "zkillboard_guard_api";
    /// The row of the single guard there was before one for each endpoint. It
    /// is read, never written, for an endpoint with no row of its own yet, so
    /// a ban remembered then still counts for both.
    const LEGACY: &'static str = "zkillboard_guard";

    pub(crate) fn new(esi: webb::esi::EsiManager, key: &'static str) -> Self {
        Self {
            esi: Mutex::new(esi),
            key,
        }
    }
}

impl GuardStore for PlayerGuardStore {
    fn load(&self) -> Option<String> {
        let esi = self
            .esi
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let read = |key: &str| match esi.read_metadata(key) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, key, "could not read the zKillboard guard");
                None
            }
        };
        read(self.key).or_else(|| read(Self::LEGACY))
    }

    fn save(&self, value: &str) {
        let esi = self
            .esi
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(error) = esi.write_metadata(self.key, value) {
            tracing::warn!(%error, "could not save the zKillboard guard");
        }
    }
}
