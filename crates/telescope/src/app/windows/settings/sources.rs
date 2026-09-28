//! Settings page "Sources": the EVE chat log folder and the channels whose
//! lines reach the rules, each with when its log last changed.

use super::time_ago;
use crate::app::TelescopeApp;
use crate::app::messages::{Message, SettingsPage};
use eframe::egui;
use egui_panels::{PathPicker, Section, StatusKind};
use native_tools::dialog::DialogType;

impl TelescopeApp {
    pub(super) fn show_sources_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::Sources.title(),
                Some(&t!("settings.sources.description")),
            );
            self.intel_flow_stepper(ui, SettingsPage::Sources);
            self.show_folder_section(ui);
            self.show_channels_section(ui);
        });
    }

    fn show_folder_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.sources.folder_title"))
            .description(&t!("settings.sources.folder_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    let mut path = self.settings.get_intel().to_string_lossy().into_owned();
                    let (browse, default) = (t!("settings.browse"), t!("settings.default"));
                    let picker = form.row(t!("settings.sources.folder"), |ui| {
                        PathPicker::new(&mut path, &browse)
                            .reset(&default)
                            .editable(false)
                            .show(ui)
                    });
                    if picker.browse {
                        self.pick_path(
                            DialogType::Directory,
                            self.settings.get_intel(),
                            Message::UpdateIntelDirectory,
                        );
                    }
                    if picker.reset {
                        self.task_msg.spawn(Message::DefaultIntelDirectory);
                    }
                    let (kind, text) = self.folder_status();
                    form.note(|ui| egui_panels::status(ui, kind, &text));
                });
            });
    }

    /// Whether the folder exists and holds chat logs, and the latest line.
    fn folder_status(&self) -> (StatusKind, String) {
        if !self.settings.get_intel().is_dir() {
            return (
                StatusKind::Error,
                t!("settings.sources.folder_missing").into_owned(),
            );
        }
        let count = self.settings.get_available_channels().len();
        if count == 0 {
            return (
                StatusKind::Warning,
                t!("settings.sources.folder_empty").into_owned(),
            );
        }
        let text = match self.settings.get_channel_activity().values().max() {
            Some(latest) => t!(
                "settings.sources.folder_ok_activity",
                count = count,
                ago = time_ago(*latest)
            ),
            None => t!("settings.sources.folder_ok", count = count),
        };
        (StatusKind::Ok, text.into_owned())
    }

    fn show_channels_section(&mut self, ui: &mut egui::Ui) {
        let mut available = self.settings.get_available_channels();
        let mut channels: Vec<String> = available.keys().cloned().collect();
        channels.sort_unstable_by_key(|channel| channel.to_lowercase());
        let active = available.values().filter(|on| **on).count();
        let activity = self.settings.get_channel_activity().clone();
        Section::new(&t!("settings.sources.channels_title"))
            .description(&t!("settings.sources.channels_description"))
            .show(ui, |ui| {
                if channels.is_empty() {
                    egui_panels::status(ui, StatusKind::Info, &t!("settings.sources.no_channels"));
                    return;
                }
                egui_panels::badge(
                    ui,
                    &t!(
                        "settings.sources.channels_count",
                        on = active,
                        total = channels.len()
                    ),
                );
                egui_panels::tile_grid(ui, 2, channels.len(), |ui, index| {
                    let channel = &channels[index];
                    let subtitle = match activity.get(channel) {
                        Some(when) => t!("settings.sources.last_line", ago = time_ago(*when)),
                        None => t!("settings.sources.no_log"),
                    };
                    if let Some(on) = available.get_mut(channel) {
                        egui_panels::tile(ui, on, channel, Some(&subtitle));
                    }
                });
                egui_panels::status(ui, StatusKind::Info, &t!("settings.sources.channel_rule"));
            });
        self.settings.set_available_channels(available);
    }
}
