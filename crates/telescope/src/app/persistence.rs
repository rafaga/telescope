//! Saving the settings edited in the Settings window.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use crate::app::tiles::TileData;
use std::collections::HashMap;

/// The regions flagged to open on startup, sorted (the form the settings
/// keep them in, so an unchanged selection compares equal).
fn startup_regions(tile_data: &HashMap<usize, TileData>) -> Vec<usize> {
    let mut regions: Vec<usize> = tile_data
        .iter()
        .filter(|(_, data)| data.show_on_startup)
        .map(|(region, _)| *region)
        .collect();
    regions.sort_unstable();
    regions
}

impl TelescopeApp {
    /// Persists the changes made in the Settings window: records which
    /// regions open on startup, applies the intel channel selection (see
    /// [`TelescopeApp::apply_intel_settings`]) and writes the settings file.
    /// A failure to write is reported as an on-screen notification.
    #[tracing::instrument(skip(self))]
    pub(crate) fn save_settings(&mut self) {
        // Empty until the maps are built on the first frame: nothing to take.
        if !self.behavior.tile_data.is_empty() {
            let startup_regions = startup_regions(&self.behavior.tile_data);
            if startup_regions != *self.settings.get_startup_regions() {
                self.settings.set_startup_regions(startup_regions);
            }
        }
        self.apply_intel_settings();
        if let Err(e) = self.settings.save() {
            self.task_msg.spawn(Message::GenericNotification((
                Type::Error,
                String::from("TelescopeApp"),
                String::from("save_settings"),
                e.to_string(),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles(regions: &[(usize, bool)]) -> HashMap<usize, TileData> {
        regions
            .iter()
            .map(|(id, startup)| (*id, TileData::new(format!("region {id}"), *startup)))
            .collect()
    }

    #[test]
    fn only_the_flagged_regions_are_kept_and_they_come_sorted() {
        let data = tiles(&[(30, true), (10, true), (20, false), (40, true)]);
        assert_eq!(startup_regions(&data), vec![10, 30, 40]);
    }

    #[test]
    fn no_flagged_region_gives_an_empty_selection() {
        assert!(startup_regions(&tiles(&[(1, false)])).is_empty());
        assert!(startup_regions(&HashMap::new()).is_empty());
    }
}
