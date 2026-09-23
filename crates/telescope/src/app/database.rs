//! `TelescopeApp` methods that keep the local databases in sync: locating the
//! SDE build cache directory and reacting to a finished SDE database update.
//! (Storing a freshly authorized character lives in `character_link.rs`.)

use crate::app::TelescopeApp;
use crate::app::messages::Message;
use crate::app::messages::Type;
use crate::app::settings::Settings;
use sde::SdeManager;
use std::path::PathBuf;

impl TelescopeApp {
    /// Base scratch directory for `database_updater::DatabaseUpdater`'s
    /// background update/build (see `sde::builder::update::UpdatePaths`):
    /// callers join `"data"` onto it for the downloaded zip and the
    /// small `.build` file that avoids re-downloading unchanged data,
    /// and `"sde"` for the decompressed SDE tree. Kept next to `sde.db`
    /// itself (falling back to a relative `sde-build-cache` if
    /// `Settings::get_sde()` isn't configured yet, e.g. on a first run
    /// before the user has set a path in Settings -> Data Sources) so
    /// it's obvious, on disk, what it belongs to; it isn't meant to be
    /// user-facing.
    pub(crate) fn sde_build_cache_dir(settings: &Settings) -> PathBuf {
        match settings.get_sde().parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("sde-build-cache"),
            _ => PathBuf::from("sde-build-cache"),
        }
    }

    /// Reloads `self.universe` from `sde.db` after
    /// `database_updater::DatabaseUpdater` has (re)built it.
    ///
    /// Note this only refreshes the in-memory `universe` data (used e.g.
    /// to look up region/system names and coordinates); every map pane
    /// (`RegionPane`/`UniversePane`, see `tiles.rs`) and the search box
    /// already open their own `SdeManager` against `settings.get_sde()`
    /// on demand, so they pick up the new database automatically. What
    /// does *not* happen live is regenerating the region *tile* list
    /// built once at startup (`create_tree`/the `self.behavior.tile_data`
    /// entries) -- a region that didn't exist in `sde.db` before this
    /// update (i.e. this was the very first successful build) won't get
    /// a tile until Telescope is restarted.
    #[tracing::instrument(skip(self))]
    pub(crate) fn handle_database_updated(&mut self) {
        match SdeManager::new(self.settings.get_sde(), self.settings.get_factor()) {
            Ok(mut sde) => match sde.get_universe() {
                Ok(_) => {
                    self.universe = sde.universe;
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Info,
                        String::from("TelescopeApp"),
                        String::from("handle_database_updated"),
                        String::from(
                            "SDE data reloaded. Restart Telescope to see newly available regions on the map.",
                        ),
                    )));
                }
                Err(t_error) => {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Error,
                        String::from("TelescopeApp"),
                        String::from("handle_database_updated"),
                        t_error.to_string(),
                    )));
                }
            },
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("TelescopeApp"),
                    String::from("handle_database_updated"),
                    t_error.to_string(),
                )));
            }
        }
    }
}

#[cfg(test)]
mod sde_build_cache_dir_tests {
    use crate::app::TelescopeApp;
    use crate::app::settings::Settings;
    use std::path::Path;

    // `Settings::default()`'s `sde` path is always non-empty now (see
    // `settings::FilePaths::default`), so this is the realistic case:
    // the cache dir sits next to `sde.db`, not off on its own.
    #[test]
    fn sits_next_to_the_configured_sde_database() {
        let settings = Settings::default();
        let cache_dir = TelescopeApp::sde_build_cache_dir(&settings);
        assert_eq!(cache_dir.file_name(), Some("sde-build-cache".as_ref()));
        assert_eq!(cache_dir.parent(), settings.get_sde().parent());
    }

    // Regression test for the empty-path case `DatabaseUpdater::run`
    // also guards against directly: an unconfigured (or pre-default,
    // loaded-from-an-old-`telescope.toml`) `sde` path has no parent
    // directory to sit next to, so this must fall back to a relative
    // path instead of panicking or joining onto nothing.
    #[test]
    fn falls_back_to_a_relative_path_when_sde_is_unconfigured() {
        let mut settings = Settings::default();
        settings.set_sde_for_test(Path::new(""));
        let cache_dir = TelescopeApp::sde_build_cache_dir(&settings);
        assert_eq!(cache_dir, Path::new("sde-build-cache"));
    }
}
