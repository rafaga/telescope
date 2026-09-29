//! Settings page "Characters": the EVE characters linked through EVE SSO,
//! whose locations set the alert distance. Linking and unlinking apply at
//! once (they don't wait for Apply).
//!
//! This file only renders; the linking logic lives in `app/character_link.rs`.

use crate::app::TelescopeApp;
use crate::app::messages::SettingsPage;
use eframe::egui;
use egui_panels::{Avatar, EntityCard, StatusKind, Variant};
use webb::objects::Character;

impl TelescopeApp {
    pub(super) fn show_characters_page(&mut self, ui: &mut egui::Ui) {
        let style = self.settings.get_character_card_style();
        egui_panels::page(ui, |ui| {
            let link = egui_panels::page_header_with(
                ui,
                &SettingsPage::Characters.title(),
                Some(&t!("settings.characters.description")),
                |ui| {
                    egui_panels::button(ui, t!("settings.characters.add"), Variant::Primary)
                        .on_hover_text(t!("settings.characters.add_hint"))
                        .clicked()
                },
            )
            .inner;
            if link {
                self.start_character_link();
            }

            if self.esi.characters.is_empty() {
                ui.add_sized(
                    [ui.available_width(), style.empty_state_height],
                    egui::Label::new(t!("settings.characters.empty")),
                );
            }
            let mut select = None;
            let mut unlink = None;
            for character in &self.esi.characters {
                let selected = self.esi.active_character == Some(character.id);
                let location = self.location_name(character);
                let card = character_card(character, location, selected, style.portrait_size).show(
                    ui,
                    |ui| {
                        egui_panels::button(ui, t!("settings.characters.remove"), Variant::Danger)
                            .on_hover_text(t!(
                                "settings.characters.remove_hint",
                                name = character.name
                            ))
                            .clicked()
                    },
                );
                if card.inner {
                    unlink = Some(character.id);
                } else if card
                    .response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    select = Some(character.id);
                }
            }
            if let Some(id) = select {
                self.esi.active_character = Some(id);
            }
            if let Some(id) = unlink {
                self.unlink_character(id);
            }
            egui_panels::status(ui, StatusKind::Info, &t!("settings.characters.immediate"));
        });
    }

    /// The name of `character`'s solar system, when it is known.
    fn location_name(&self, character: &Character) -> Option<String> {
        let system = u32::try_from(character.location).ok()?;
        Some(self.universe.solar_systems.get(&system)?.name.clone())
    }
}

/// The card of a linked character: portrait, name, corporation and
/// alliance, location and last login.
fn character_card(
    character: &Character,
    location: Option<String>,
    selected: bool,
    portrait_size: f32,
) -> EntityCard<'_> {
    let corporation = character.corp.as_ref().map_or_else(
        || t!("settings.characters.no_corporation").into_owned(),
        |corp| corp.name.clone(),
    );
    let alliance = character.alliance.as_ref().map_or_else(
        || t!("settings.characters.no_alliance").into_owned(),
        |alliance| alliance.name.clone(),
    );
    let location = location.map_or_else(
        || t!("settings.characters.unknown_location").into_owned(),
        |system| t!("settings.characters.location", system = system).into_owned(),
    );
    let last_logon = t!(
        "settings.characters.last_logon",
        when = character
            .last_logon
            .format("%Y-%m-%d %H:%M UTC")
            .to_string()
    );
    let avatar = match &character.photo {
        Some(photo) => Avatar::Image(photo.as_str().into()),
        None => Avatar::Initials(initials(&character.name)),
    };
    EntityCard::new(&character.name)
        .avatar(avatar)
        .avatar_size(portrait_size)
        .line(format!("{corporation} · {alliance}"))
        .line(format!("{location} · {last_logon}"))
        .selected(selected)
}

/// "Kara Voss" -> "KV".
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}
