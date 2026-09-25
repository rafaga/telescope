//! The debug window: search the SDE by name and act on a result (center the maps
//! on it, optionally emitting a notification), plus an "Advanced" section with
//! testing tools: simulate an intel line, test alerts, preview node
//! animations, move a character's marker, and a read-only view of the app's
//! internal state.

use crate::app::TelescopeApp;
use crate::app::map_alerts::{AlertSummary, IntelAlert};
use crate::app::messages::CharacterSync;
use crate::app::messages::MapSync;
use crate::app::messages::Message;
use crate::app::messages::NodeEffect;
use crate::app::messages::Target;
use crate::app::messages::Type;
use eframe::egui;
use eframe::egui::RichText;
use egui_extras::Column;
use egui_extras::TableBuilder;
use sde::SdeManager;
use std::sync::Arc;
use webb::esi::{SCHEMA_VERSION, SchemaStatus};

/// State of the Debug window's "Advanced" section.
pub(crate) struct DebugState {
    show_advanced: bool,
    intel_channel: String,
    intel_author: String,
    intel_text: String,
    /// `intel_text` is already a full chat-log line (`[ ts ] Author > text`).
    intel_raw: bool,
    intel_result: Option<String>,
    effect: NodeEffect,
    move_character: Option<i32>,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            show_advanced: false,
            intel_channel: String::new(),
            intel_author: String::from("Debug"),
            intel_text: String::new(),
            intel_raw: false,
            intel_result: None,
            effect: NodeEffect::default(),
            move_character: None,
        }
    }
}

impl TelescopeApp {
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_debug_menu(&mut self, ctx: &egui::Context) {
        // Copied out and written back (like the Settings window) so the
        // closure can borrow `self` as a whole.
        let mut open = self.open[1];
        egui::Window::new("Debug Menu")
            .default_width(420.0)
            .default_height(600.0)
            .resizable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Search");

                    ui.horizontal(|ui| {
                        ui.label("Name: ");
                        let response = ui.text_edit_singleline(&mut self.search_text);
                        if response.changed() {
                            if self.search_text.len() >= 3 {
                                let sde = SdeManager::new(
                                    self.settings.get_sde(),
                                    self.settings.get_factor(),
                                );
                                match sde.and_then(|s| {
                                    s.get_system_id(self.search_text.clone().to_lowercase())
                                }) {
                                    Ok(system_results) => {
                                        // Preselect the first result, so the
                                        // Advanced tools (animations, move
                                        // character) have a target right away.
                                        self.search_selected_row =
                                            (!system_results.is_empty()).then_some(0);
                                        self.search_results = system_results;
                                    }
                                    Err(t_error) => {
                                        self.task_msg.spawn(Message::GenericNotification((
                                            Type::Error,
                                            String::from("sde"),
                                            String::from("get_system_id"),
                                            t_error.to_string(),
                                        )));
                                    }
                                }
                            }
                            if self.search_text.is_empty() {
                                self.search_results.clear();
                                self.search_selected_row = None;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.emit_notification, "Notify");
                        if ui.button("Clear").clicked() {
                            self.search_text.clear();
                            self.search_results.clear();
                            self.search_selected_row = None;
                        }
                        let advanced = if self.debug.show_advanced {
                            "Advanced <<<"
                        } else {
                            "Advanced >>>"
                        };
                        if ui.button(advanced).clicked() {
                            self.debug.show_advanced = !self.debug.show_advanced;
                        }
                    });
                    ui.push_id("search_table", |ui| {
                        let mut table = TableBuilder::new(ui)
                            .striped(true)
                            .resizable(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::auto())
                            .column(Column::remainder())
                            .min_scrolled_height(0.0)
                            .max_scroll_height(220.0);

                        table = table.sense(egui::Sense::click());
                        table
                            .header(20.0, |mut header| {
                                header.col(|ui| {
                                    ui.strong("System");
                                });
                                header.col(|ui| {
                                    ui.strong("Region");
                                });
                            })
                            .body(|mut body| {
                                for row_index in 0..self.search_results.len() {
                                    body.row(18.00, |mut row| {
                                        row.set_selected(false);
                                        if let Some(selected_row) = self.search_selected_row
                                            && row_index == selected_row
                                        {
                                            row.set_selected(true);
                                        }
                                        let col_data = row.col(|ui| {
                                            if ui.label(&self.search_results[row_index].1).clicked()
                                            {
                                                self.search_selected_row = Some(row_index);
                                                let tx_map = Arc::clone(&self.map_msg.0);
                                                let system_id = self.search_results[row_index].0;
                                                let _result = tx_map.send(MapSync::CenterOn((
                                                    system_id.try_into().unwrap(),
                                                    Target::System,
                                                )));
                                                if self.emit_notification {
                                                    let _result = tx_map.send(
                                                        MapSync::SystemAlert(debug_alert(
                                                            system_id.try_into().unwrap(),
                                                            self.settings.get_alert_duration(),
                                                        )),
                                                    );
                                                }
                                            }
                                        });
                                        if col_data.1.clicked() {
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let system_id = self.search_results[row_index].0;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                system_id.try_into().unwrap(),
                                                Target::System,
                                            )));
                                            if self.emit_notification {
                                                let _result =
                                                    tx_map.send(MapSync::SystemAlert(debug_alert(
                                                        system_id.try_into().unwrap(),
                                                        self.settings.get_alert_duration(),
                                                    )));
                                            }
                                        }
                                        let col_data = row.col(|ui| {
                                            if ui.label(&self.search_results[row_index].3).clicked()
                                            {
                                                self.search_selected_row = Some(row_index);
                                                let tx_map = Arc::clone(&self.map_msg.0);
                                                let region_id = self.search_results[row_index].2;
                                                let _result = tx_map.send(MapSync::CenterOn((
                                                    region_id.try_into().unwrap(),
                                                    Target::Region,
                                                )));
                                            }
                                        });
                                        if col_data.1.clicked() {
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let region_id = self.search_results[row_index].2;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                region_id.try_into().unwrap(),
                                                Target::Region,
                                            )));
                                        }
                                        if row.response().clicked() {
                                            self.search_selected_row = Some(row_index);
                                        }
                                    });
                                    //self.toggle_row_selection(row_index, &row.response());
                                }
                                if self.search_results.is_empty() {
                                    body.row(18.00, |mut row| {
                                        row.col(|ui| {
                                            ui.label("No result(s)");
                                        });
                                        row.col(|_ui| {});
                                    });
                                }
                            });
                        //
                    });
                    if self.debug.show_advanced {
                        ui.separator();
                        self.debug_advanced_ui(ui);
                    }
                });
            });
        self.open[1] = open;
    }

    /// System picked in the search results, as `(id, name)`.
    fn debug_selected_system(&self) -> Option<(usize, String)> {
        let row = self.search_results.get(self.search_selected_row?)?;
        Some((usize::try_from(row.0).ok()?, row.1.clone()))
    }

    fn debug_advanced_ui(&mut self, ui: &mut egui::Ui) {
        let selected = self.debug_selected_system();
        let target_text = match &selected {
            Some((_, name)) => format!("Target system: {name}"),
            None => String::from("Target system: none (pick one in the search results above)"),
        };
        ui.label(RichText::new(target_text).weak());

        egui::CollapsingHeader::new("Simulate intel line")
            .default_open(true)
            .show(ui, |ui| self.debug_intel_ui(ui));
        egui::CollapsingHeader::new("Test alerts").show(ui, |ui| self.debug_alerts_ui(ui));
        egui::CollapsingHeader::new("Node animations")
            .show(ui, |ui| self.debug_animations_ui(ui, selected.as_ref()));
        egui::CollapsingHeader::new("Move character")
            .show(ui, |ui| self.debug_move_character_ui(ui, selected.as_ref()));
        egui::CollapsingHeader::new("Internal state").show(ui, |ui| self.debug_state_ui(ui));
    }

    /// Runs a chat line through the pattern engine as if it had just been
    /// read from an intel log, so rules and alerts can be tested.
    fn debug_intel_ui(&mut self, ui: &mut egui::Ui) {
        let monitored = self.settings.get_cloned_monitored_channels();
        egui::Grid::new("debug_intel")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Channel:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.debug.intel_channel)
                            .hint_text("any channel")
                            .desired_width(160.0),
                    );
                    egui::ComboBox::from_id_salt("debug_intel_channel")
                        .selected_text("monitored")
                        .show_ui(ui, |ui| {
                            for channel in monitored.iter() {
                                ui.selectable_value(
                                    &mut self.debug.intel_channel,
                                    channel.clone(),
                                    channel,
                                );
                            }
                        });
                });
                ui.end_row();
                if !self.debug.intel_raw {
                    ui.label("Author:");
                    ui.text_edit_singleline(&mut self.debug.intel_author);
                    ui.end_row();
                }
                ui.label("Text:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.debug.intel_text)
                        .hint_text("e.g. Jita clr")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
            });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.debug.intel_raw, "Raw chat-log line")
                .on_hover_text("The text is already `[ YYYY.MM.DD HH:MM:SS ] Author > message`");
            let can_send = !self.debug.intel_text.trim().is_empty();
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked()
            {
                let line = if self.debug.intel_raw {
                    self.debug.intel_text.trim().to_string()
                } else {
                    format!(
                        "[ {} ] {} > {}",
                        chrono::Utc::now().format("%Y.%m.%d %H:%M:%S"),
                        self.debug.intel_author.trim(),
                        self.debug.intel_text.trim()
                    )
                };
                let rules = self.parse_intel_data(self.debug.intel_channel.trim(), &line);
                self.debug.intel_result = Some(if rules.is_empty() {
                    String::from("No rule matched (or the line isn't in chat-log format).")
                } else {
                    format!("Matched: {}", rules.join(", "))
                });
            }
        });
        if let Some(result) = &self.debug.intel_result {
            ui.label(RichText::new(result).weak());
        }
    }

    fn debug_alerts_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("🔔 Play alert sound").clicked() {
                self.audio.play_alarm(&self.settings.get_alert_sound_path());
            }
            if ui.button("Send test notifications").clicked() {
                for (kind, name) in [
                    (Type::Debug, "Debug"),
                    (Type::Info, "Info"),
                    (Type::Warning, "Warning"),
                    (Type::Error, "Error"),
                ] {
                    self.task_msg.spawn(Message::GenericNotification((
                        kind,
                        String::from("Debug"),
                        String::from("test"),
                        format!("{name} test notification"),
                    )));
                }
            }
        });
        ui.label(
            RichText::new(format!(
                "Sound: {}",
                self.settings.get_alert_sound_path().display()
            ))
            .weak(),
        );
    }

    fn debug_animations_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("debug_node_effect")
                .selected_text(self.debug.effect.label())
                .show_ui(ui, |ui| {
                    for effect in NodeEffect::ALL {
                        ui.selectable_value(&mut self.debug.effect, effect, effect.label());
                    }
                });
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(match selected {
                        Some((_, name)) => format!("▶ Play on {name}"),
                        None => String::from("▶ Play"),
                    }),
                )
                .on_disabled_hover_text("Pick a system in the search results first")
                .clicked()
                && let Some((system_id, _)) = selected
            {
                let _ = self
                    .map_msg
                    .0
                    .send(MapSync::NodeEffect((*system_id, self.debug.effect)));
            }
        });
        ui.label(
            RichText::new(
                "Lasting effects run until \"Clear effects\". The map's node template \
                 decides how each effect is drawn.",
            )
            .weak(),
        );
    }

    /// Puts a character's marker on the selected system without it moving in
    /// the game (the real location comes back with "Restore").
    fn debug_move_character_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        if self.esi.characters.is_empty() {
            ui.label("No linked characters.");
            return;
        }
        if self
            .debug
            .move_character
            .is_none_or(|id| !self.esi.characters.iter().any(|c| c.id == id))
        {
            self.debug.move_character = self.esi.characters.first().map(|c| c.id);
        }
        let current = self
            .esi
            .characters
            .iter()
            .find(|c| Some(c.id) == self.debug.move_character)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("debug_move_character")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for character in &self.esi.characters {
                        ui.selectable_value(
                            &mut self.debug.move_character,
                            Some(character.id),
                            &character.name,
                        );
                    }
                });
            let Some(id) = self.debug.move_character else {
                return;
            };
            if ui
                .add_enabled(
                    selected.is_some(),
                    egui::Button::new(match selected {
                        Some((_, name)) => format!("Move to {name}"),
                        None => String::from("Move to…"),
                    }),
                )
                .on_disabled_hover_text("Pick a system in the search results first")
                .clicked()
                && let Some((system_id, _)) = selected
            {
                self.update_player_location(id, *system_id as i32);
            }
            if ui
                .button("Restore")
                .on_hover_text("Ask the location watchdog for the character's real location")
                .clicked()
                && let Some(sender) = &self.char_msg
            {
                let _ = sender.try_send(CharacterSync::Add(id as usize));
            }
        });
    }

    fn debug_state_ui(&self, ui: &mut egui::Ui) {
        let watchdog = match &self.char_msg {
            Some(sender) if !sender.is_closed() => "running",
            _ => "stopped",
        };
        let schema = match self.esi.schema_status {
            Some(SchemaStatus::Created) => String::from("created this run"),
            Some(SchemaStatus::Migrated(from)) => format!("migrated from {from} this run"),
            Some(SchemaStatus::UpToDate) => String::from("up to date"),
            Some(SchemaStatus::Newer(version)) => format!("newer ({version})"),
            None => String::from("could not be opened"),
        };
        egui::Grid::new("debug_state")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, label: &str, value: String| {
                    ui.label(label);
                    ui.label(value);
                    ui.end_row();
                };
                row(ui, "Location watchdog", String::from(watchdog));
                row(
                    ui,
                    "Player DB schema",
                    format!("v{SCHEMA_VERSION}, {schema}"),
                );
                row(
                    ui,
                    "Player DB",
                    self.settings.get_db().display().to_string(),
                );
                row(ui, "SDE", self.settings.get_sde().display().to_string());
                row(
                    ui,
                    "Intel directory",
                    self.settings.get_intel().display().to_string(),
                );
                row(
                    ui,
                    "Monitored channels",
                    self.settings.get_cloned_monitored_channels().join(", "),
                );
                row(ui, "Log entries", self.app_messages.len().to_string());
            });
        ui.add_space(4.0);
        ui.label(RichText::new("Linked characters").strong());
        egui::Grid::new("debug_characters")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Character").weak());
                ui.label(RichText::new("Location").weak());
                ui.label(RichText::new("Token expires").weak());
                ui.end_row();
                for character in &self.esi.characters {
                    ui.label(format!("{} ({})", character.name, character.id));
                    ui.label(character.location.to_string());
                    let token = match self.esi.auth.get(&character.id) {
                        Some(auth) if !auth.refresh_token.is_empty() => auth
                            .expiration
                            .map_or(String::from("unknown"), |expiration| {
                                expiration.format("%Y-%m-%d %H:%M:%S UTC").to_string()
                            }),
                        _ => String::from("no token (link it again)"),
                    };
                    ui.label(token);
                    ui.end_row();
                }
            });
    }
}

/// A made-up intel alert on `system_id`, sent by "Emit notification" to
/// preview the node effect and its tooltip line.
fn debug_alert(system_id: usize, duration: std::time::Duration) -> IntelAlert {
    let text = "Debug alert";
    let summary = AlertSummary {
        leftover: String::from(text),
        ..AlertSummary::default()
    };
    IntelAlert::new(
        system_id,
        std::time::Instant::now(),
        duration,
        text,
        summary,
    )
}
