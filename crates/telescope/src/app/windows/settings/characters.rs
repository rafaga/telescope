//! Settings page "Characters": the linked EVE characters used to emit notifications
//! when something is close to their location, and the buttons to link/unlink one.
//!
//! This file only renders; the linking logic lives in `app/character_link.rs`.

use crate::app::TelescopeApp;
use eframe::egui;
use eframe::egui::FontId;
use eframe::egui::RichText;
use webb::objects::Character;

/// Side of the square character portrait, in points.
const PORTRAIT_SIZE: f32 = 80.0;
/// Height of the placeholder shown when no character is linked.
const EMPTY_STATE_HEIGHT: f32 = 200.0;
/// Vertical gap between character cards.
const CARD_SPACING: f32 = 4.0;

impl TelescopeApp {
    pub(super) fn show_characters_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Linked characters").font(FontId::proportional(20.0)));
        ui.label("These are used to emit notifications when something is close to your location.");
        self.character_toolbar(ui);
        ui.add_space(CARD_SPACING);

        if self.esi.characters.is_empty() {
            empty_state(ui);
            return;
        }

        // The page is already inside the Settings window's ScrollArea.
        let mut clicked = None;
        for character in &self.esi.characters {
            let selected = self.esi.active_character == Some(character.id);
            if character_card(ui, character, selected).clicked() {
                clicked = Some(character.id);
            }
            ui.add_space(CARD_SPACING);
        }
        if clicked.is_some() {
            self.esi.active_character = clicked;
        }
    }

    fn character_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("➕ Add")
                .on_hover_text("Log in with EVE SSO in your browser to link a character")
                .clicked()
            {
                self.start_character_link();
            }

            let selected = self.esi.active_character.and_then(|id| {
                self.esi
                    .characters
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| (id, c.name.clone()))
            });
            let remove = ui.add_enabled(selected.is_some(), egui::Button::new("✖ Remove"));
            let remove = match &selected {
                Some((_, name)) => remove.on_hover_text(format!("Unlink {name}")),
                None => remove.on_disabled_hover_text("Select a character to unlink it"),
            };
            if remove.clicked()
                && let Some((id, _)) = selected
            {
                self.unlink_character(id);
            }
        });
    }
}

/// One linked character: portrait, name, alliance, corporation and last logon.
/// Returns a clickable response covering the whole card.
fn character_card(ui: &mut egui::Ui, character: &Character, selected: bool) -> egui::Response {
    let visuals = ui.visuals();
    let (fill, stroke) = if selected {
        (
            visuals.selection.bg_fill.gamma_multiply(0.35),
            visuals.selection.stroke,
        )
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    let frame = egui::Frame::group(ui.style()).fill(fill).stroke(stroke);

    let response = ui
        .push_id(character.id, |ui| {
            frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let portrait_size = egui::Vec2::splat(PORTRAIT_SIZE);
                        match &character.photo {
                            Some(photo) => {
                                ui.add(
                                    egui::Image::new(photo.as_str())
                                        .fit_to_exact_size(portrait_size),
                                );
                            }
                            None => {
                                ui.allocate_exact_size(portrait_size, egui::Sense::hover());
                            }
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&character.name).strong().size(16.0));
                            egui::Grid::new("character_details")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    detail_row(
                                        ui,
                                        "Alliance:",
                                        character
                                            .alliance
                                            .as_ref()
                                            .map_or("No alliance", |a| a.name.as_str()),
                                    );
                                    detail_row(
                                        ui,
                                        "Corporation:",
                                        character
                                            .corp
                                            .as_ref()
                                            .map_or("No corporation", |c| c.name.as_str()),
                                    );
                                    detail_row(
                                        ui,
                                        "Last logon:",
                                        &character
                                            .last_logon
                                            .format("%Y-%m-%d %H:%M UTC")
                                            .to_string(),
                                    );
                                });
                        });
                    });
                })
                .response
        })
        .inner;

    response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.weak(label);
    ui.label(value);
    ui.end_row();
}

fn empty_state(ui: &mut egui::Ui) {
    ui.add_sized(
        [ui.available_width(), EMPTY_STATE_HEIGHT],
        egui::Label::new(
            "⚠ There are no characters linked in Telescope yet. Use ➕ Add to link one.",
        ),
    );
}
