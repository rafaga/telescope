//! Saving the settings edited in the Settings window.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};

impl TelescopeApp {
    /// Persists the changes made in the Settings window: records which
    /// regions open on startup, applies the intel channel selection (see
    /// [`TelescopeApp::apply_intel_settings`]) and writes the settings file.
    /// A failure to write is reported as an on-screen notification.
    #[tracing::instrument(skip(self))]
    pub(crate) fn save_settings(&mut self) {
        // Empty until the maps are built on the first frame: nothing to take.
        if !self.behavior.tile_data.is_empty() {
            let mut startup_regions: Vec<usize> = self
                .behavior
                .tile_data
                .iter()
                .filter(|(_, data)| data.show_on_startup)
                .map(|(region, _)| *region)
                .collect();
            startup_regions.sort_unstable();
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
