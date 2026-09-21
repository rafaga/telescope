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
        let mut startup_regions = vec![];
        for region in self.behavior.tile_data.iter() {
            if region.1.show_on_startup {
                startup_regions.push(*region.0);
            }
        }
        if !startup_regions.is_empty() {
            self.settings.set_startup_regions(startup_regions);
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
