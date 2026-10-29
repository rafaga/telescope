//! Settings page "zKillboard": what the killmail feed is doing, a switch to
//! pause it, and the filters applied before the rules.

use super::time_ago;
use crate::app::TelescopeApp;
use crate::app::intel::zkillboard::{FEED_URL, FeedState};
use crate::app::messages::SettingsPage;
use eframe::egui::{self, RichText};
use egui_panels::{Section, StatusKind};
use std::time::Duration;

/// The minimum values the filter offers, in ISK, with their labels.
const MIN_ISK_STEPS: [(u64, &str); 4] = [
    (10_000_000, "10M"),
    (100_000_000, "100M"),
    (1_000_000_000, "1B"),
    (10_000_000_000, "10B"),
];

impl TelescopeApp {
    pub(super) fn show_zkillboard_page(&mut self, ui: &mut egui::Ui) {
        // The status changes by itself, with nobody touching the screen.
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::Zkillboard.title(),
                Some(&t!("settings.zkillboard.description")),
            );
            let (kind, text) = self.zkillboard_status();
            egui_panels::status(ui, kind, &text);
            self.show_zkillboard_feed_section(ui);
            self.show_zkillboard_filters_section(ui);
        });
    }

    /// What the source is doing, from the settings and the source itself.
    fn zkillboard_status(&self) -> (StatusKind, String) {
        if !self.settings.get_zkillboard().enabled {
            return (
                StatusKind::Warning,
                t!("settings.zkillboard.status_paused").into_owned(),
            );
        }
        if !self.intel_zkillboard.is_wanted() {
            return (
                StatusKind::Info,
                t!("settings.zkillboard.status_no_rule").into_owned(),
            );
        }
        let status = self.intel_zkillboard.status();
        match status.state {
            FeedState::Off | FeedState::Starting => (
                StatusKind::Info,
                t!("settings.zkillboard.status_starting").into_owned(),
            ),
            FeedState::Live => {
                let text = match status.last_killmail {
                    Some(when) => t!(
                        "settings.zkillboard.status_live_last",
                        announced = status.announced,
                        ago = time_ago(when.into()),
                        filtered = status.filtered
                    ),
                    None => t!(
                        "settings.zkillboard.status_live",
                        announced = status.announced,
                        filtered = status.filtered
                    ),
                };
                (StatusKind::Ok, text.into_owned())
            }
            FeedState::Failing => (
                StatusKind::Warning,
                t!(
                    "settings.zkillboard.status_failing",
                    error = status.last_error.unwrap_or_default()
                )
                .into_owned(),
            ),
        }
    }

    fn show_zkillboard_feed_section(&mut self, ui: &mut egui::Ui) {
        let mut options = self.settings.get_zkillboard().clone();
        Section::new(&t!("settings.zkillboard.feed_title"))
            .description(&t!("settings.zkillboard.feed_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    form.row_hint(
                        t!("settings.zkillboard.enabled"),
                        &t!("settings.zkillboard.enabled_hint"),
                        |ui| {
                            egui_panels::switch(ui, &mut options.enabled);
                        },
                    );
                    // For reference only: the addresses are not editable.
                    form.row(t!("settings.zkillboard.endpoint_pointer"), |ui| {
                        ui.label(RichText::new(format!("{FEED_URL}/sequence.json")).monospace());
                    });
                    form.row(t!("settings.zkillboard.endpoint_killmail"), |ui| {
                        ui.label(
                            RichText::new(format!("{FEED_URL}/{{sequence}}.json")).monospace(),
                        );
                    });
                });
            });
        self.settings.set_zkillboard(options);
    }

    fn show_zkillboard_filters_section(&mut self, ui: &mut egui::Ui) {
        let mut options = self.settings.get_zkillboard().clone();
        Section::new(&t!("settings.zkillboard.filters_title"))
            .description(&t!("settings.zkillboard.filters_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    form.row_hint(
                        t!("settings.zkillboard.min_isk"),
                        &t!("settings.zkillboard.min_isk_hint"),
                        |ui| {
                            let any = t!("settings.zkillboard.min_isk_any");
                            let mut steps = vec![(0, any.as_ref())];
                            steps.extend(MIN_ISK_STEPS);
                            egui_panels::segmented(ui, &mut options.min_isk, &steps);
                        },
                    );
                    form.row_hint(
                        t!("settings.zkillboard.ignore_npc"),
                        &t!("settings.zkillboard.ignore_npc_hint"),
                        |ui| {
                            egui_panels::switch(ui, &mut options.ignore_npc_only);
                        },
                    );
                    form.note(|ui| {
                        ui.weak(t!("settings.zkillboard.filters_note"));
                    });
                });
            });
        self.settings.set_zkillboard(options);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::ZKillboardOptions;

    #[test]
    fn the_steps_of_the_minimum_value_go_up() {
        assert!(MIN_ISK_STEPS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(
            MIN_ISK_STEPS
                .iter()
                .all(|(isk, _)| *isk <= ZKillboardOptions::MAX_MIN_ISK)
        );
    }
}
