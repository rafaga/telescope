//! Settings page "Maps": the regions whose map opens at start-up and the
//! intensity of the glow that marks a linked character's system.

use crate::app::TelescopeApp;
use crate::app::messages::SettingsPage;
use crate::app::tiles::CHARACTER_ICON;
use eframe::egui::{self, CornerRadius, Stroke, StrokeKind};
use egui_panels::{Section, Slider};
use std::collections::HashMap;

impl TelescopeApp {
    pub(super) fn show_maps_page(&mut self, ui: &mut egui::Ui) {
        egui_panels::page(ui, |ui| {
            egui_panels::page_header(
                ui,
                &SettingsPage::Maps.title(),
                Some(&t!("settings.maps.description")),
            );
            self.show_startup_regions_section(ui);
            self.show_marker_section(ui);
        });
    }

    fn show_startup_regions_section(&mut self, ui: &mut egui::Ui) {
        let characters_here = self.characters_per_region();
        Section::new(&t!("settings.maps.startup_title"))
            .description(&t!("settings.maps.startup_description"))
            .show(ui, |ui| {
                let selected = self
                    .behavior
                    .tile_data
                    .values()
                    .filter(|data| data.show_on_startup)
                    .count();
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_ui.region_filter)
                            .hint_text(t!("settings.maps.filter_hint"))
                            .desired_width(220.0_f32.min(ui.available_width())),
                    );
                    egui_panels::badge(ui, &t!("settings.maps.selected", count = selected));
                });
                // Where the linked characters are, since the rows themselves
                // are only check boxes: the rows say it on hover.
                let mut here: Vec<(String, usize)> = characters_here
                    .iter()
                    .filter_map(|(region, count)| {
                        let data = self.behavior.tile_data.get(region)?;
                        Some((data.get_name(), *count))
                    })
                    .collect();
                here.sort_unstable();
                if !here.is_empty() {
                    let regions: Vec<String> = here
                        .iter()
                        .map(|(name, count)| format!("{name} ({count})"))
                        .collect();
                    ui.weak(t!(
                        "settings.maps.characters_summary",
                        regions = regions.join(", ")
                    ));
                }
                let filter = self.settings_ui.region_filter.trim().to_lowercase();
                let mut regions: Vec<(usize, String)> = self
                    .behavior
                    .tile_data
                    .iter()
                    .map(|(id, data)| (*id, data.get_name()))
                    .filter(|(_, name)| filter.is_empty() || name.to_lowercase().contains(&filter))
                    .collect();
                regions.sort_unstable_by(|a, b| a.1.cmp(&b.1));
                // a list of check boxes, like the watched channels and the
                // overlay characters
                let tile_data = &mut self.behavior.tile_data;
                egui_panels::tile_grid(ui, 2, regions.len(), |ui, index| {
                    let (region, name) = &regions[index];
                    let count = characters_here.get(region);
                    if let Some(data) = tile_data.get_mut(region) {
                        // The icon after the name, so the names stay aligned. It is
                        // a color emoji: it keeps its own colors.
                        let label: egui::WidgetText = match count {
                            Some(_) => format!("{name} {CHARACTER_ICON}").into(),
                            None => name.as_str().into(),
                        };
                        let response = ui.checkbox(&mut data.show_on_startup, label);
                        if let Some(count) = count {
                            response
                                .on_hover_text(t!("settings.maps.characters_here", count = count));
                        }
                    }
                });
            });
    }

    fn show_marker_section(&mut self, ui: &mut egui::Ui) {
        Section::new(&t!("settings.maps.marker_title")).show(ui, |ui| {
            egui_panels::form(ui, |form| {
                form.row(t!("settings.maps.glow"), |ui| {
                    let mut percent = (self.settings.get_glow_intensity() * 100.0).round() as u32;
                    if Slider::new(&mut percent, 0..=100)
                        .step(5.0)
                        .width(220.0)
                        .show(ui)
                        .changed()
                    {
                        self.settings.set_glow_intensity(percent as f32 / 100.0);
                    }
                    ui.label(format!("{percent} %"));
                    glow_preview(ui, self.settings.get_glow_intensity());
                });
                form.note(|ui| {
                    ui.weak(t!("settings.maps.glow_hint"));
                });
            });
        });
    }

    /// How many linked characters are in each region (by region id).
    fn characters_per_region(&self) -> HashMap<usize, usize> {
        let mut regions = HashMap::new();
        for character in &self.esi.characters {
            let region = u32::try_from(character.location)
                .ok()
                .and_then(|system| self.universe.solar_systems.get(&system))
                .map(|system| system.region as usize);
            if let Some(region) = region {
                *regions.entry(region).or_insert(0) += 1;
            }
        }
        regions
    }
}

/// A map node with a character's glow at `intensity` (0 to 1), next to the
/// slider.
fn glow_preview(ui: &mut egui::Ui, intensity: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(78.0, 24.0), egui::Sense::hover());
    let node = rect.shrink2(egui::vec2(8.0, 4.0));
    let visuals = ui.visuals();
    let glow = visuals.selection.bg_fill;
    let painter = ui.painter();
    if intensity > 0.0 {
        for step in 1..=4u8 {
            let spread = f32::from(step) * 1.5;
            let alpha = intensity * (1.0 - f32::from(step) / 5.0) * 0.5;
            painter.rect_stroke(
                node.expand(spread),
                CornerRadius::same(3 + step),
                Stroke::new(1.5, glow.gamma_multiply(alpha)),
                StrokeKind::Outside,
            );
        }
    }
    painter.rect(
        node,
        CornerRadius::same(3),
        visuals.extreme_bg_color,
        Stroke::new(1.0, visuals.widgets.noninteractive.fg_stroke.color),
        StrokeKind::Outside,
    );
    if intensity > 0.0 {
        painter.rect_filled(
            node,
            CornerRadius::same(3),
            glow.gamma_multiply(intensity * 0.6),
        );
    }
    painter.text(
        node.center(),
        egui::Align2::CENTER_CENTER,
        "H-5GUI",
        egui::FontId::proportional(11.0),
        if intensity > 0.0 {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        },
    );
}
