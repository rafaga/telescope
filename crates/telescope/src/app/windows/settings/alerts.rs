//! The alert pages of the Settings screen, one for each way a reported
//! system is alerted: the visual alert (how close it has to be for the
//! sound, how long the map alert lasts), the sound (with a button that plays
//! the whole alert with the values being edited) and the overlay.

use crate::app::TelescopeApp;
use crate::app::messages::{MapSync, SettingsPage, Target};
use crate::app::settings::{Mapping, OverlayOrder, Settings};
use eframe::egui;
use egui_panels::{Section, Slider, StatusKind, Variant};
use std::time::Instant;
use webb::map_alerts::{AlertSummary, IntelAlert};

/// What the overlay shows, out of the two settings behind it (`overlay_enabled`
/// and `overlay_all_characters`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OverlayMode {
    /// No overlay.
    Off,
    /// One overlay for all the characters.
    All,
    /// One overlay for each character turned on in the list.
    Each,
}

impl OverlayMode {
    fn of(settings: &Settings) -> Self {
        match (
            settings.get_overlay_enabled(),
            settings.get_overlay_all_characters(),
        ) {
            (false, _) => Self::Off,
            (true, true) => Self::All,
            (true, false) => Self::Each,
        }
    }

    /// Writes the mode into the settings. Off keeps the choice between one
    /// overlay and one per character for when it is turned on again.
    fn apply(self, settings: &mut Settings) {
        settings.set_overlay_enabled(self != Self::Off);
        match self {
            Self::Off => {}
            Self::All => settings.set_overlay_all_characters(true),
            Self::Each => settings.set_overlay_all_characters(false),
        }
    }
}

/// The warning radii offered, in jumps.
const RADII: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

impl TelescopeApp {
    /// The distance and the alert on the maps.
    pub(crate) fn show_alerts_visual_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::AlertsVisual.title(),
                Some(&t!("settings.alerts.visual_description")),
            );
            self.show_distance_section(ui);
            self.show_map_alert_section(ui);
        });
    }

    /// The sound, and the button that plays the whole alert.
    pub(crate) fn show_alerts_sound_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::AlertsSound.title(),
                Some(&t!("settings.alerts.sound_description")),
            );
            self.show_sound_section(ui);
        });
    }

    /// The overlay over the game.
    pub(crate) fn show_alerts_overlay_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::AlertsOverlay.title(),
                Some(&t!("settings.alerts.overlay_page_description")),
            );
            self.show_overlay_section(ui);
        });
    }

    fn show_distance_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.alerts.distance_title"))
            .description(&t!("settings.alerts.distance_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    form.row(t!("settings.alerts.radius"), |ui| {
                        let labels: Vec<String> = RADII.iter().map(u8::to_string).collect();
                        let options: Vec<(u8, &str)> = RADII
                            .iter()
                            .zip(&labels)
                            .map(|(radius, label)| (*radius, label.as_str()))
                            .collect();
                        let mut radius = self.settings.get_warning_area();
                        if egui_panels::segmented(ui, &mut radius, &options).changed() {
                            self.settings.set_warning_area(radius);
                        }
                        ui.label(t!("settings.alerts.jumps"));
                    });
                });
            });
    }

    fn show_map_alert_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.alerts.map_title")).show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row(t!("settings.alerts.duration"), |ui| {
                    let mut secs = self.settings.get_alert_duration_secs();
                    let slider = Slider::new(
                        &mut secs,
                        Mapping::MIN_ALERT_DURATION_SECS..=Mapping::MAX_ALERT_DURATION_SECS,
                    )
                    .step(10.0)
                    .width(220.0)
                    .show(ui);
                    if slider.changed() {
                        self.settings.set_alert_duration_secs(secs);
                    }
                    ui.label(format!("{}:{:02}", secs / 60, secs % 60));
                });
                form.note(|ui| {
                    ui.weak(t!("settings.alerts.duration_hint"));
                });
                form.row_hint(
                    t!("settings.alerts.center"),
                    &t!("settings.alerts.center_hint"),
                    |ui| {
                        let mut center = self.settings.get_center_on_alert();
                        if egui_panels::switch(ui, &mut center).changed() {
                            self.settings.set_center_on_alert(center);
                        }
                        ui.weak(t!("settings.alerts.center_hint"));
                    },
                );
            });
        });
    }

    fn show_overlay_section(&mut self, ui: &mut egui::Ui) {
        // The overlay of each character lives in the player database, so a
        // change there is stored right away, not with Apply.
        let mut toggled: Option<(i64, bool)> = None;
        Section::new(&t!("settings.alerts.overlay_title"))
            .description(&t!("settings.alerts.overlay_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    let mode = OverlayMode::of(&self.settings);
                    form.row(t!("settings.alerts.overlay_mode"), |ui| {
                        let off = t!("settings.alerts.overlay_mode_off");
                        let all = t!("settings.alerts.overlay_mode_all");
                        let each = t!("settings.alerts.overlay_mode_each");
                        let options = [
                            (OverlayMode::Off, off.as_ref()),
                            (OverlayMode::All, all.as_ref()),
                            (OverlayMode::Each, each.as_ref()),
                        ];
                        let mut chosen = mode;
                        if egui_panels::segmented(ui, &mut chosen, &options).changed() {
                            chosen.apply(&mut self.settings);
                        }
                    });
                    form.row(t!("settings.alerts.overlay_characters"), |ui| {
                        let picking = mode == OverlayMode::Each;
                        ui.vertical(|ui| {
                            // the list can only be edited with one overlay per character
                            ui.add_enabled_ui(picking, |ui| {
                                if self.esi.characters.is_empty() {
                                    ui.weak(t!("settings.alerts.overlay_no_characters"));
                                }
                                for character in &self.esi.characters {
                                    let mut enabled = character.overlay.enabled;
                                    if ui.checkbox(&mut enabled, &character.name).changed() {
                                        toggled = Some((character.id, enabled));
                                    }
                                }
                            });
                            if !picking {
                                ui.weak(t!("settings.alerts.overlay_characters_hint"));
                            }
                        });
                    });
                    form.row(t!("settings.alerts.overlay_order"), |ui| {
                        let mut order = self.settings.get_overlay_order();
                        let age = t!("settings.alerts.overlay_order_age");
                        let jumps = t!("settings.alerts.overlay_order_jumps");
                        let options = [
                            (OverlayOrder::Age, age.as_ref()),
                            (OverlayOrder::Jumps, jumps.as_ref()),
                        ];
                        if egui_panels::segmented(ui, &mut order, &options).changed() {
                            self.settings.set_overlay_order(order);
                        }
                    });
                    form.row(t!("settings.alerts.overlay_max_rows"), |ui| {
                        let mut rows = f64::from(self.settings.get_overlay_max_rows());
                        let slider =
                            Slider::new(&mut rows, 1.0..=f64::from(Mapping::MAX_OVERLAY_MAX_ROWS))
                                .step(1.0)
                                .width(220.0)
                                .show(ui);
                        if slider.changed() {
                            self.settings.set_overlay_max_rows(rows as u8);
                        }
                        ui.label(format!("{rows:.0}"));
                    });
                    form.row(t!("settings.alerts.overlay_opacity"), |ui| {
                        let mut percent = (self.settings.get_overlay_opacity() * 100.0).round();
                        let slider = Slider::new(&mut percent, 30.0..=100.0)
                            .step(5.0)
                            .width(220.0)
                            .show(ui);
                        if slider.changed() {
                            self.settings.set_overlay_opacity(percent / 100.0);
                        }
                        ui.label(format!("{percent:.0}%"));
                    });
                    form.note(|ui| {
                        ui.weak(t!("settings.alerts.overlay_hint"));
                    });
                });
            });
        if let Some((id, enabled)) = toggled
            && let Some(character) = self.esi.characters.iter().find(|c| c.id == id)
        {
            let overlay = webb::objects::CharacterOverlay {
                enabled,
                ..character.overlay
            };
            self.write_character_overlay(id, overlay);
        }
    }

    fn show_sound_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.alerts.sound_title")).show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row(t!("settings.alerts.sound"), |ui| {
                    let mut current = self
                        .settings
                        .get_alert_sound()
                        .to_string_lossy()
                        .into_owned();
                    let combo = egui::ComboBox::from_id_salt("alert_sound")
                        .selected_text(current.clone())
                        .width(220.0_f32.min(ui.available_width()))
                        .show_ui(ui, |ui| {
                            let mut sounds: Vec<String> = self
                                .settings
                                .alerts_dir()
                                .read_dir()
                                .into_iter()
                                .flatten()
                                .flatten()
                                .filter_map(|file| file.file_name().to_str().map(str::to_owned))
                                .collect();
                            sounds.sort_unstable();
                            for name in sounds {
                                if ui
                                    .selectable_value(&mut current, name.clone(), &name)
                                    .changed()
                                {
                                    let _ = self.settings.set_alert_sound(&name);
                                }
                            }
                        });
                    egui_panels::label_with_row(ui, &combo.response);
                    // Plays the selected sound (even before saving).
                    if egui_panels::button(ui, "▶", Variant::Secondary)
                        .on_hover_text(t!("settings.alerts.play"))
                        .clicked()
                    {
                        self.audio.play_alarm(&self.settings.get_alert_sound_path());
                    }
                });
                form.note(|ui| {
                    if egui_panels::button(ui, t!("settings.alerts.test"), Variant::Primary)
                        .on_hover_text(t!("settings.alerts.test_hint"))
                        .clicked()
                    {
                        self.settings_ui.test_result = Some(self.test_full_alert());
                    }
                    match &self.settings_ui.test_result {
                        Some((kind, text)) => {
                            egui_panels::status(ui, *kind, text);
                        }
                        None => {
                            ui.weak(t!("settings.alerts.test_hint"));
                        }
                    }
                });
            });
        });
    }

    /// Plays a whole alert with the values being edited, without saving
    /// anything: the system of a linked character (the selected one first)
    /// pulses and gets a tooltip entry, the sound plays and, when enabled,
    /// the maps center on it.
    fn test_full_alert(&self) -> (StatusKind, String) {
        let located = |id: Option<i64>| {
            self.esi
                .characters
                .iter()
                .filter(|character| id.is_none_or(|id| character.id == id))
                .find_map(|character| {
                    let system = u32::try_from(character.location).ok()?;
                    let data = self.universe.solar_systems.get(&system)?;
                    Some((character.name.clone(), system, data.name.clone()))
                })
        };
        let Some((name, system, system_name)) =
            located(self.esi.active_character).or_else(|| located(None))
        else {
            return (
                StatusKind::Warning,
                t!("settings.alerts.test_no_character").into_owned(),
            );
        };
        let text = t!("settings.alerts.test_line", system = system_name).into_owned();
        let summary = AlertSummary {
            leftover: text.clone(),
            ..AlertSummary::default()
        };
        let alert = |summary| {
            IntelAlert::new(
                system as usize,
                Instant::now(),
                self.settings.get_alert_duration(),
                &text,
                summary,
                false,
            )
        };
        let _ = self
            .map_msg
            .0
            .send(MapSync::SystemAlert(alert(AlertSummary::default())));
        let _ = self.map_msg.0.send(MapSync::SystemTooltip(alert(summary)));
        self.audio.play_alarm(&self.settings.get_alert_sound_path());
        if self.settings.get_center_on_alert() {
            let _ = self
                .map_msg
                .0
                .send(MapSync::CenterOn((system as usize, Target::System)));
        }
        (
            StatusKind::Ok,
            t!(
                "settings.alerts.test_done",
                system = system_name,
                name = name
            )
            .into_owned(),
        )
    }
}
