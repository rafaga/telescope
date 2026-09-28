//! The Settings screen: a full-window frame with the page menu, the selected
//! page and the global Cancel / Accept bar. Each page lives in its own
//! submodule (`general`, `intelligence`, `characters`, `patterns`);
//! `data_sources` is a section of the General page.

use crate::app::TelescopeApp;
use crate::app::messages::SettingsPage;
use eframe::egui;
use eframe::egui::Color32;

mod characters;
mod data_sources;
mod general;
mod intelligence;
pub(crate) mod patterns;

impl TelescopeApp {
    /// Draws the Settings screen over the whole window: a left page menu, the
    /// selected page and a bottom bar with Cancel and Accept.
    #[tracing::instrument(skip(self, ui))]
    pub(crate) fn show_settings_screen(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("settings_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button(t!("settings.cancel")).clicked() {
                    self.cancel_settings();
                }
                if ui.button(t!("settings.apply")).clicked() {
                    self.apply_settings();
                }
                if ui.button(t!("settings.accept")).clicked() {
                    self.accept_settings();
                }
                if !self.settings.its_saved() {
                    ui.colored_label(Color32::YELLOW, t!("settings.unsaved"));
                }
            });
        });
        egui::Panel::left("settings_nav")
            .resizable(false)
            .show(ui, |ui| {
                ui.set_min_width(180.0);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(t!("settings.title")).strong());
                ui.separator();
                for page in SettingsPage::ALL {
                    let selected = self.selected_settings_page == page;
                    if ui.selectable_label(selected, page.title()).clicked() {
                        self.selected_settings_page = page;
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.selected_settings_page {
                SettingsPage::General => self.show_general_page(ui),
                SettingsPage::Intelligence => self.show_intelligence_page(ui),
                SettingsPage::Patterns => self.show_patterns_page(ui),
                SettingsPage::Characters => self.show_characters_page(ui),
            });
        });
    }
}
