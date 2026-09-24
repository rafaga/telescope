//! Settings page "Intelligence": alert distance, the EVE chat log directory and
//! the monitored channels.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, send_app_message};
use crate::app::settings::alerts_dir;
use eframe::egui;
use eframe::egui::{Button, FontId, IntoAtoms, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use native_tools::dialog::*;
use std::sync::Arc;

impl TelescopeApp {
    pub(super) fn show_intelligence_page(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(t!("settings.intelligence.alerts")).font(FontId::proportional(20.0)),
        );
        ui.horizontal(|ui| {
            let mut data = self.settings.get_warning_area();
            ui.label(t!("settings.intelligence.warn_before"));
            egui::ComboBox::new("warning_area", t!("settings.intelligence.warn_after"))
                .selected_text(data.to_string())
                // Narrow: it only ever holds 1-7. At the default width the
                // row no longer fits the window (see `show_chat_logs_row`).
                .width(48.0)
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
                    if let Ok(obj_dir) = alerts_dir().read_dir() {
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
            // Plays the selected sound (even before saving) so it can be
            // heard without waiting for a real alert.
            if ui
                .button("▶")
                .on_hover_text(t!("settings.intelligence.play_sound"))
                .clicked()
            {
                self.audio.play_alarm(&self.settings.get_alert_sound_path());
            }
            ui.end_row();
        });
        let mut center_on_alert = self.settings.get_center_on_alert();
        if ui
            .checkbox(
                &mut center_on_alert,
                t!("settings.intelligence.center_on_alert"),
            )
            .on_hover_text(t!("settings.intelligence.center_on_alert_hint"))
            .changed()
        {
            self.settings.set_center_on_alert(center_on_alert);
        }
        self.show_chat_logs_row(ui);
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
                // Fixed width, not resizable: with a single channel the
                // resize handles were left as stray vertical lines.
                .columns(Column::exact(230.0), 2)
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
    }

    /// The EVE chat log directory: its label on one line, then the path and
    /// the two buttons. The buttons are laid out right to left first so the
    /// path takes exactly the width that remains. As a single left-to-right
    /// row (label + a 280 px path + both buttons) it was wider than the
    /// Settings window, and egui then grows the window frame past its title
    /// bar, which stays at the fixed 700 px -- the title looked off-center.
    fn show_chat_logs_row(&mut self, ui: &mut egui::Ui) {
        let enabled = true;
        ui.label(t!("settings.intelligence.chat_logs"));
        let mut select_clicked = false;
        let mut default_clicked = false;
        // Exactly one row tall: a bare `with_layout` takes all the remaining
        // height and centers the row vertically in it.
        let row_size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(
            row_size,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let atoms = t!("settings.intelligence.default")
                    .into_owned()
                    .into_atoms();
                default_clicked = ui.add_enabled(enabled, Button::new(atoms)).clicked();
                let atoms2 = t!("settings.intelligence.select").into_owned().into_atoms();
                select_clicked = ui.add_enabled(enabled, Button::new(atoms2)).clicked();
                let mut str_intel = self.settings.get_intel().to_string_lossy().to_string();
                ui.add_enabled(
                    enabled,
                    TextEdit::singleline(&mut str_intel).desired_width(ui.available_width()),
                );
            },
        );
        if select_clicked {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let app_msg_tx = Arc::clone(&self.app_msg.0);
            self.dlg_intel_dir.open_file_dialog(move |result| {
                if let DialogResult::Ok(path) = result {
                    runtime.block_on(async {
                        let _span = tracing::info_span!("spawned intel message data").entered();
                        let _ = send_app_message(&app_msg_tx, Message::UpdateIntelDirectory(path))
                            .await;
                    });
                }
            });
        }
        if default_clicked {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let app_msg_tx = Arc::clone(&self.app_msg.0);
            runtime.block_on(async {
                let _ = send_app_message(&app_msg_tx, Message::DefaultIntelDirectory).await;
            });
        }
    }
}
