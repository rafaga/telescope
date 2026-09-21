//! The Settings window frame: the page menu, the selected page and the Save button.
//! Each page lives in its own submodule (`intelligence`, `data_sources`,
//! `characters`).

use crate::app::TelescopeApp;
use crate::app::messages::SettingsPage;
use eframe::egui;
use eframe::egui::Button;
use eframe::egui::Color32;
use egui_extras::Column;
use egui_extras::TableBuilder;

mod characters;
mod data_sources;
mod intelligence;

impl TelescopeApp {
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_settings_window(&mut self, ctx: &egui::Context) {
        // Copied out (and written back below) instead of borrowing
        // `self.open[2]` for the whole `show` call: the page methods called
        // inside the closure need `&mut self` as a whole.
        let mut open = self.open[2];
        egui::Window::new("Settings")
            .movable(true)
            .resizable(false)
            .fixed_size([700.0, 510.0])
            .movable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let row_height = 25.0;
                        let pages = SettingsPage::ALL;
                        ui.push_id("settings_menu", |ui| {
                            TableBuilder::new(ui)
                                .column(Column::resizable(Column::exact(150.0), false))
                                .striped(false)
                                .vscroll(false)
                                .body(|body| {
                                    body.rows(row_height, pages.len(), |mut row| {
                                        let current_page = pages[row.index()];
                                        row.col(|ui: &mut egui::Ui| {
                                            let selected =
                                                self.selected_settings_page == current_page;
                                            if ui
                                                .selectable_label(selected, current_page.title())
                                                .clicked()
                                            {
                                                self.selected_settings_page = current_page;
                                            };
                                        });
                                    });
                                });
                        });
                        ui.add_space(480.0 - (pages.len() as f32 * row_height));
                    });
                    ui.separator();
                    ui.push_id("settings_config", |ui| {
                        ui.vertical(|ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                match self.selected_settings_page {
                                    SettingsPage::Intelligence => self.show_intelligence_page(ui),
                                    SettingsPage::DataSources => self.show_data_sources_page(ui),
                                    SettingsPage::Characters => self.show_characters_page(ui),
                                }
                            });
                        });
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(650.00);
                });
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Save")).clicked() {
                        self.save_settings();
                    }
                    if !self.settings.its_saved() {
                        ui.colored_label(Color32::YELLOW, "⚠ unsaved changes");
                    }
                });
            });
        self.open[2] = open;
    }
}
