//! `TelescopeApp` methods that keep the local databases in sync: locating the
//! SDE build cache directory and reloading the SDE database without a restart.
//! (Storing a freshly authorized character lives in `character_link.rs`.)

use crate::app::TelescopeApp;
use crate::app::database_updater::DatabaseUpdater;
use crate::app::messages::Message;
use crate::app::messages::Type;
use crate::app::settings::Settings;
use crate::app::tiles::TileData;
use egui_tiles::Tile;
use sde::SdeManager;
use sde::objects::Universe;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The regions that get a map: k-space only, wormhole and abyssal regions
/// (ids from 11,000,000) have none.
fn kspace_regions(universe: &Universe) -> HashMap<usize, String> {
    universe
        .regions
        .values()
        .filter(|region| region.id < 11_000_000)
        .map(|region| (region.id as usize, region.name.clone()))
        .collect()
}

/// The listed regions that are no longer in `regions`.
fn regions_gone(
    tile_data: &HashMap<usize, TileData>,
    regions: &HashMap<usize, String>,
) -> Vec<usize> {
    tile_data
        .keys()
        .filter(|id| !regions.contains_key(id))
        .copied()
        .collect()
}

/// The startup regions that have no map open yet.
fn pending_startup_regions(tile_data: &HashMap<usize, TileData>) -> Vec<usize> {
    tile_data
        .iter()
        .filter(|(_, data)| data.show_on_startup && data.get_tile_id().is_none())
        .map(|(id, _)| *id)
        .collect()
}

impl TelescopeApp {
    /// Base scratch directory for `database_updater::DatabaseUpdater`'s
    /// background update/build (see `sde::builder::update::UpdatePaths`):
    /// callers join `"data"` onto it for the downloaded zip and the
    /// small `.build` file that avoids re-downloading unchanged data,
    /// and `"sde"` for the decompressed SDE tree. Kept next to `sde.db`
    /// itself (falling back to a relative `sde-build-cache` if
    /// `Settings::get_sde()` isn't configured yet, e.g. on a first run
    /// before the user has set a path in Settings -> Application) so
    /// it's obvious, on disk, what it belongs to; it isn't meant to be
    /// user-facing.
    pub(crate) fn sde_build_cache_dir(settings: &Settings) -> PathBuf {
        match settings.get_sde().parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("sde-build-cache"),
            _ => PathBuf::from("sde-build-cache"),
        }
    }

    /// Loads the SDE database again -- after `database_updater::DatabaseUpdater`
    /// rebuilt it, or when another one is picked in Settings -> Application --
    /// so it takes effect without restarting Telescope: the in-memory
    /// `universe`, the intel resolver, the list of regions, the systems of
    /// every open map and the startup regions that couldn't open before (a
    /// first run starts with no database).
    #[tracing::instrument(skip(self))]
    pub(crate) fn reload_sde(&mut self) {
        let loaded = SdeManager::new(self.settings.get_sde(), self.settings.get_factor())
            .and_then(|mut sde| sde.get_universe().map(|_| sde.universe));
        let universe = match loaded {
            Ok(universe) => universe,
            Err(error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("TelescopeApp"),
                    String::from("reload_sde"),
                    error.to_string(),
                )));
                return;
            }
        };
        self.universe = universe;
        // The intel detection resolves reported systems against this same
        // data.
        self.intel_resolver.replace(&self.universe);
        self.sync_alarm_jumps();
        self.behavior
            .set_path(self.settings.get_sde().to_path_buf());
        self.sync_region_list();
        if let Some(tree) = self.tree.as_mut() {
            for tile in tree.tiles.tiles_mut() {
                if let Tile::Pane(pane) = tile {
                    pane.reload_data(self.settings.get_sde());
                }
            }
        }
        self.open_startup_regions();
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("TelescopeApp"),
            String::from("reload_sde"),
            String::from("SDE data reloaded."),
        )));
    }

    /// Makes the list of regions (`self.behavior.tile_data`, the maps' ➕
    /// menu) match `self.universe`: new regions are added, those gone are
    /// removed with their map, and the others keep their state.
    pub(crate) fn sync_region_list(&mut self) {
        let regions = kspace_regions(&self.universe);
        let gone = regions_gone(&self.behavior.tile_data, &regions);
        for id in gone {
            if let Some(data) = self.behavior.tile_data.remove(&id)
                && let (Some(tile_id), Some(tree)) = (data.get_tile_id(), self.tree.as_mut())
            {
                tree.remove_recursively(tile_id);
            }
        }
        let startup = self.settings.get_startup_regions();
        for (id, name) in regions {
            self.behavior
                .tile_data
                .entry(id)
                .or_insert_with(|| TileData::new(name, startup.contains(&id)));
        }
    }

    /// Opens the map of every startup region (Settings -> Maps) that has
    /// none yet.
    pub(crate) fn open_startup_regions(&mut self) {
        for region in pending_startup_regions(&self.behavior.tile_data) {
            self.create_new_regional_pane(region);
        }
    }

    /// Checks for a newer SDE and rebuilds the database when there is one;
    /// `reload_sde` runs once it is done (see `database_updater`).
    pub(crate) fn start_sde_update(&self) {
        let sde_cache_dir = Self::sde_build_cache_dir(&self.settings);
        DatabaseUpdater::spawn(
            self.settings.get_sde().to_path_buf(),
            sde_cache_dir.join("data"),
            sde_cache_dir.join("sde"),
            Arc::clone(&self.app_msg.0),
            false,
            self.settings.get_data_source_urls().clone(),
        );
    }

    /// Another SDE database was applied in Settings -> Application: loaded
    /// at once when it is there, built first when it isn't.
    pub(crate) fn change_sde(&mut self) {
        if self.settings.get_sde().is_file() {
            self.reload_sde();
        } else {
            self.behavior
                .set_path(self.settings.get_sde().to_path_buf());
        }
        self.start_sde_update();
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

#[cfg(test)]
mod region_tests {
    use super::*;
    use egui_tiles::TileId;
    use sde::objects::Region;

    fn universe(regions: &[(u32, &str)]) -> Universe {
        let mut universe = Universe::new(1.0);
        for (id, name) in regions {
            let mut region = Region::new();
            region.id = *id;
            region.name = name.to_string();
            universe.regions.insert(*id, region);
        }
        universe
    }

    #[test]
    fn only_kspace_regions_get_a_map() {
        let universe = universe(&[
            (10000002, "The Forge"),
            (11000001, "A-R00001"),
            (12000001, "Abyss"),
        ]);
        let regions = kspace_regions(&universe);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[&10000002], "The Forge");
    }

    #[test]
    fn regions_missing_from_the_universe_are_reported_gone() {
        let mut tile_data = HashMap::new();
        tile_data.insert(1, TileData::new(String::from("kept"), false));
        tile_data.insert(2, TileData::new(String::from("removed"), false));
        let regions = HashMap::from([(1, String::from("kept"))]);
        assert_eq!(regions_gone(&tile_data, &regions), vec![2]);
    }

    #[test]
    fn only_startup_regions_without_a_map_are_pending() {
        let mut tile_data = HashMap::new();
        tile_data.insert(1, TileData::new(String::from("pending"), true));
        tile_data.insert(2, TileData::new(String::from("not startup"), false));
        let mut open = TileData::new(String::from("already open"), true);
        open.set_tile_id(Some(TileId::from_u64(7)));
        tile_data.insert(3, open);
        assert_eq!(pending_startup_regions(&tile_data), vec![1]);
    }
}
