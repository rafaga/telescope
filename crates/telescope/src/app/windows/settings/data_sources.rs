//! Settings page "Data Sources": the SDE and player database paths, and the
//! button that checks for SDE updates.

use crate::app::TelescopeApp;
use eframe::egui;
use eframe::egui::FontId;
use eframe::egui::RichText;
use std::path::Path;
use std::sync::Arc;

impl TelescopeApp {
    pub(super) fn show_data_sources_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(t!("settings.data_sources.heading")).font(FontId::proportional(20.0)),
        );
        ui.horizontal(|ui| {
            ui.label(t!("settings.data_sources.sde"));
            let mut str_sde = self.settings.get_sde().to_string_lossy().to_string();
            if ui.text_edit_singleline(&mut str_sde).changed() {
                let _ = self.settings.set_sde(Path::new(&str_sde));
            }
        });
        ui.horizontal(|ui| {
            ui.label(t!("settings.data_sources.player_db"));
            let mut str_db = self.settings.get_db().to_string_lossy().to_string();
            if ui
                .text_edit_singleline(&mut str_db)
                .on_hover_text(t!("settings.data_sources.player_db_hint"))
                .changed()
            {
                let _ = self.settings.set_db(Path::new(&str_db));
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button(t!("settings.data_sources.check_updates"))
                .clicked()
            {
                let sde_cache_dir = Self::sde_build_cache_dir(&self.settings);
                crate::app::database_updater::DatabaseUpdater::spawn(
                    self.settings.get_sde().to_path_buf(),
                    sde_cache_dir.join("data"),
                    sde_cache_dir.join("sde"),
                    Arc::clone(&self.app_msg.0),
                    false,
                );
            }
        });
    }
}
