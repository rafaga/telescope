//! Settings page "Application": the interface language and the two local
//! databases (the SDE and the private one), with their status.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, SettingsPage};
use crate::i18n;
use eframe::egui;
use egui_panels::{PathPicker, Section, StatusKind, Variant};
use native_tools::dialog::DialogType;
use std::sync::Arc;

impl TelescopeApp {
    pub(super) fn show_application_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::Application.title(),
                Some(&t!("settings.application.description")),
            );
            self.show_language_section(ui);
            self.show_data_section(ui);
        });
    }

    fn show_language_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.application.language_title")).show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row(t!("settings.application.language"), |ui| {
                    let options: Vec<String> = std::iter::once(i18n::AUTO.to_owned())
                        .chain(i18n::available())
                        .collect();
                    let labels: Vec<String> =
                        options.iter().map(|code| language_label(code)).collect();
                    let current = self.settings.get_ui_state().language;
                    let mut index = options
                        .iter()
                        .position(|code| *code == current)
                        .unwrap_or_default();
                    let choices: Vec<(usize, &str)> =
                        labels.iter().map(String::as_str).enumerate().collect();
                    if egui_panels::segmented(ui, &mut index, &choices).changed() {
                        // Previewed at once; saved (or reverted) with the
                        // other settings.
                        i18n::apply_language(&options[index]);
                        self.settings.set_language(&options[index]);
                    }
                });
                form.note(|ui| {
                    ui.weak(t!("settings.application.language_hint"));
                });
            });
        });
    }

    fn show_data_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.application.data_title")).show(ui, |ui| {
            egui_panels::form(ui, |form| {
                let browse = t!("settings.browse");
                let mut sde = self.settings.get_sde().to_string_lossy().into_owned();
                let picker = form.row(t!("settings.application.sde"), |ui| {
                    PathPicker::new(&mut sde, &browse).editable(false).show(ui)
                });
                if picker.browse {
                    self.pick_path(
                        DialogType::File,
                        self.settings.get_sde(),
                        Message::SdePathPicked,
                    );
                }
                form.note(|ui| {
                    let (kind, text) = self.sde_status();
                    egui_panels::status(ui, kind, &text);
                    if egui_panels::button(
                        ui,
                        t!("settings.application.check_updates"),
                        Variant::Ghost,
                    )
                    .clicked()
                    {
                        self.check_sde_updates();
                    }
                });

                let mut db = self.settings.get_db().to_string_lossy().into_owned();
                let picker = form.row(t!("settings.application.player_db"), |ui| {
                    PathPicker::new(&mut db, &browse).editable(false).show(ui)
                });
                if picker.browse {
                    self.pick_path(
                        DialogType::File,
                        self.settings.get_db(),
                        Message::DbPathPicked,
                    );
                }
                form.note(|ui| {
                    ui.weak(t!("settings.application.player_db_hint"));
                });
                let restart = self.settings_snapshot.as_ref().is_some_and(|saved| {
                    saved.get_sde() != self.settings.get_sde()
                        || saved.get_db() != self.settings.get_db()
                });
                if restart {
                    form.note(|ui| {
                        egui_panels::status(
                            ui,
                            StatusKind::Info,
                            &t!("settings.application.restart"),
                        )
                    });
                }
            });
        });
    }

    /// Whether the SDE file exists and how many systems were loaded from it.
    fn sde_status(&self) -> (StatusKind, String) {
        if !self.settings.get_sde().is_file() {
            return (
                StatusKind::Error,
                t!("settings.application.sde_missing").into_owned(),
            );
        }
        let count = self.universe.solar_systems.len();
        if count == 0 {
            return (
                StatusKind::Warning,
                t!("settings.application.sde_empty").into_owned(),
            );
        }
        (
            StatusKind::Ok,
            t!("settings.application.sde_ok", count = count).into_owned(),
        )
    }

    /// Checks for a newer SDE and rebuilds the database when there is one
    /// (see `database_updater`).
    fn check_sde_updates(&self) {
        let sde_cache_dir = Self::sde_build_cache_dir(&self.settings);
        crate::app::database_updater::DatabaseUpdater::spawn(
            self.settings.get_sde().to_path_buf(),
            sde_cache_dir.join("data"),
            sde_cache_dir.join("sde"),
            Arc::clone(&self.app_msg.0),
            false,
            self.settings.get_data_source_urls().clone(),
        );
    }
}

/// How a language setting is offered: "Automatic (English)" for `auto`, the
/// language's own name otherwise (its code when the file has no name).
fn language_label(setting: &str) -> String {
    if setting == i18n::AUTO {
        return t!(
            "settings.application.auto",
            language = i18n::display_name(&i18n::resolve(i18n::AUTO))
        )
        .into_owned();
    }
    let name = i18n::display_name(setting);
    if name.trim().is_empty() {
        setting.to_owned()
    } else {
        name
    }
}
