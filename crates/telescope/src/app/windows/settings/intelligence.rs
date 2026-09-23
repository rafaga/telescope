//! Settings page "Intelligence": alert distance, the EVE chat log directory, the
//! monitored channels and the maps shown at start-up.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, send_app_message};
use crate::app::settings::ALERTS_DIR;
use eframe::egui;
use eframe::egui::{Button, FontId, IntoAtoms, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use native_tools::dialog::*;
use std::path::Path;
use std::sync::Arc;

impl TelescopeApp {
    pub(super) fn show_intelligence_page(&mut self, ui: &mut egui::Ui) {
        let mut keys: Vec<usize> = self.behavior.tile_data.keys().copied().collect();
        keys.sort_unstable();
        let num_rows = keys.len().div_ceil(3);
        ui.label(
            RichText::new(t!("settings.intelligence.alerts")).font(FontId::proportional(20.0)),
        );
        ui.horizontal(|ui| {
            let mut data = self.settings.get_warning_area();
            ui.label(t!("settings.intelligence.warn_before"));
            egui::ComboBox::new("warning_area", t!("settings.intelligence.warn_after"))
                .selected_text(data.to_string())
                .show_ui(ui, |ui| {
                    for i in 1u8..8 {
                        if ui.selectable_value(&mut data, i, i.to_string()).changed() {
                            self.settings.set_warning_area(i);
                        }
                    }
                });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            // Compared against each candidate below via `selectable_value`,
            // so it has to be the currently selected sound's own name, not
            // (as this used to read) a leftover clone of the warning-area
            // combo's `u8` above -- that made the dropdown's highlighted
            // entry meaningless, though the closed box's label was still
            // right since that comes from `get_alert_sound()` directly.
            let mut current = self
                .settings
                .get_alert_sound()
                .to_string_lossy()
                .into_owned();
            ui.label(t!("settings.intelligence.alert_sound"));
            egui::ComboBox::new("alert_sound", "")
                .selected_text(current.clone())
                .show_ui(ui, |ui| {
                    if let Ok(obj_dir) = Path::new(ALERTS_DIR).read_dir() {
                        for file in obj_dir.flatten() {
                            if let Some(name) = file.file_name().to_str()
                                && ui
                                    .selectable_value(&mut current, name.to_string(), name)
                                    .changed()
                            {
                                let _ = self.settings.set_alert_sound(name);
                            }
                        }
                    }
                });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let enabled = true;
            ui.label(t!("settings.intelligence.chat_logs"));
            let mut str_intel = self.settings.get_intel().to_string_lossy().to_string();
            ui.add_enabled(enabled, TextEdit::singleline(&mut str_intel));
            let atoms2 = t!("settings.intelligence.select").into_owned().into_atoms();
            if ui.add_enabled(enabled, Button::new(atoms2)).clicked() {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let app_msg_tx = Arc::clone(&self.app_msg.0);
                self.dlg_intel_dir.open_file_dialog(move |result| {
                    if let DialogResult::Ok(path) = result {
                        runtime.block_on(async {
                            let _span = tracing::info_span!("spawned intel message data").entered();
                            let _ =
                                send_app_message(&app_msg_tx, Message::UpdateIntelDirectory(path))
                                    .await;
                        });
                    }
                });
            }
            let atoms = t!("settings.intelligence.default")
                .into_owned()
                .into_atoms();
            if ui.add_enabled(enabled, Button::new(atoms)).clicked() {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let app_msg_tx = Arc::clone(&self.app_msg.0);
                runtime.block_on(async {
                    let _ = send_app_message(&app_msg_tx, Message::DefaultIntelDirectory).await;
                });
            }
        });
        let row_height = 18.0;
        let mut available_channels = self.settings.get_available_channels();
        // clone keys to avoid borrowing available_channels while we later mutably borrow it
        let mut channels: Vec<String> = available_channels.keys().cloned().collect();
        channels.sort_unstable();
        ui.add_space(12.00);
        ui.label(
            RichText::new(t!("settings.intelligence.monitored")).font(FontId::proportional(20.0)),
        );
        ui.label(t!("settings.intelligence.monitored_help"));
        ui.push_id("chan_tbl", |ui| {
            TableBuilder::new(ui)
                .columns(Column::resizable(Column::exact(230.0), true), 2)
                .striped(true)
                .vscroll(true)
                .body(|mut body| {
                    if !channels.is_empty() {
                        body.rows(row_height, channels.len().div_ceil(2), |mut row| {
                            let index = row.index() * 2;
                            row.col(|ui: &mut egui::Ui| {
                                let key = &channels[index];
                                let chan = available_channels.get_mut(key).unwrap();
                                ui.checkbox(chan, key);
                            });
                            if index < channels.len() - 1 {
                                row.col(|ui: &mut egui::Ui| {
                                    let key = &channels[index + 1];
                                    let chan = available_channels.get_mut(key).unwrap();
                                    ui.checkbox(chan, key);
                                });
                            }
                        });
                    } else {
                        body.row(row_height, |mut row| {
                            row.col(|ui| {
                                ui.label(t!("settings.intelligence.no_channels"));
                            });
                        });
                    }
                });
        });
        self.settings.set_available_channels(available_channels);
        ui.add_space(12.00);
        ui.label(
            RichText::new(t!("settings.intelligence.startup_maps"))
                .font(FontId::proportional(20.0)),
        );
        ui.label(t!("settings.intelligence.startup_help"))
            .with_new_rect(ui.available_rect_before_wrap());
        ui.push_id("rgn_tbl", |ui| {
            TableBuilder::new(ui)
                .column(Column::resizable(Column::exact(150.0), false))
                .column(Column::resizable(Column::exact(150.0), false))
                .column(Column::resizable(Column::exact(150.0), false))
                .striped(true)
                .vscroll(false)
                .body(|body| {
                    body.rows(row_height, num_rows, |mut row| {
                        let key_index = row.index() * 3;
                        row.col(|ui: &mut egui::Ui| {
                            let region = self.behavior.tile_data.get_mut(&keys[key_index]).unwrap();
                            let name = region.get_name();
                            //let checked = &mut self.behavior.tile_data.get_mut(&region.get_id()).unwrap().show_on_startup;
                            ui.checkbox(&mut region.show_on_startup, name);
                        });
                        let mut t_key_index = key_index + 1;
                        if t_key_index < keys.len() {
                            row.col(|ui: &mut egui::Ui| {
                                let region =
                                    self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                let name = region.get_name();
                                ui.checkbox(&mut region.show_on_startup, name);
                            });
                        }
                        t_key_index += 1;
                        if t_key_index < keys.len() {
                            row.col(|ui: &mut egui::Ui| {
                                let region =
                                    self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                let name = region.get_name();
                                ui.checkbox(&mut region.show_on_startup, name);
                            });
                        }
                    });
                });
        });
    }
}
