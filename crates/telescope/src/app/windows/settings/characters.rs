use crate::app::TelescopeApp;
use crate::app::messages::CharacterSync;
use crate::app::messages::Message;
use crate::app::messages::Type;
use eframe::egui;
use eframe::egui::FontId;
use eframe::egui::RichText;
use egui_extras::Column;
use egui_extras::TableBuilder;

impl TelescopeApp {
    pub(super) fn show_characters_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Linked characters").font(FontId::proportional(20.0)));
        ui.label("These are used to emit notifications when something it is close to your location.");
        ui.horizontal(|ui|{
            if ui.button("➕ Add").clicked() {
                let auth_info = self.esi.get_authorize_url().unwrap();
                match open::that(auth_info.url) {
                    Ok(()) => {
                        self.task_auth.spawn();
                    }
                    Err(err) => {
                        self.task_msg.spawn(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("get_authorize_url"),err.to_string())));
                    }
                }
            }
            let button_state = !self.esi.characters.is_empty();
            if ui.add_enabled(button_state, egui::Button::new("✖ Remove")).clicked() {
                let mut index = 0;
                let mut vec_id = vec![];
                for char in &self.esi.characters {
                    if let Some(active_char) = self.esi.active_character {
                        if active_char == char.id {
                            if let Some(sender) = &self.char_msg {
                                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                                let _result = runtime.block_on(async{sender.send(CharacterSync::Remove(char.id as usize)).await});
                            }
                            vec_id.push(char.id);
                            break;
                        }
                        index += 1;
                    }
                }
                if self.esi.active_character.is_some() {
                    self.esi.characters.remove(index);
                    self.esi.active_character = None;
                    if let Err(t_error) = self.esi.remove_characters(Some(vec_id)) {
                        self.task_msg.spawn(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("remove_characters"),t_error.to_string())));
                    }
                }
            }
        });
        TableBuilder::new(ui)
        .column(Column::exact(470.00))
        .striped(true)
        .vscroll(false)
        .body(|mut body| {
            let characters = self.esi.characters.len();
            if characters > 0 {
                body.rows(100.0, characters, |mut row| {
                    let index = row.index();
                    row.col(|ui|{
                        ui.group(|ui|{
                            ui.push_id(self.esi.characters[index].id, |ui| {
                                let inner = ui.horizontal_centered(|ui| {
                                    if let Some(idc) =
                                        self.esi.active_character
                                        && self.esi.characters[index].id == idc {
                                            ui.style_mut()
                                                .visuals
                                                .override_text_color =
                                                Some(eframe::egui::Color32::YELLOW);
                                            //ui.style_mut().visuals.selection.bg_fill = Color32::LIGHT_GRAY;
                                            //ui.style_mut().visuals.fade_out_to_color();
                                        }
                                    if let Some(player_photo) = &self.esi.characters[index].photo
                                    {
                                        ui.add(
                                            eframe::egui::Image::new(
                                                player_photo.as_str(),
                                            )
                                            .fit_to_exact_size(eframe::egui::Vec2::new(
                                                80.0, 80.0,
                                            )),
                                        );
                                    }
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            //ui.image(char_photo, Vec2::new(16.0,16.0));
                                            ui.label("Name:");
                                            ui.label(&self.esi.characters[index].name);
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("Alliance:");
                                            if let Some(alliance) =
                                            self.esi.characters[index].alliance.as_ref()
                                            {
                                                ui.label(&alliance.name);
                                            } else {
                                                ui.label("No alliance");
                                            }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("Corporation:");
                                            if let Some(corp) =
                                            self.esi.characters[index].corp.as_ref()
                                            {
                                                ui.label(&corp.name);
                                            } else {
                                                ui.label("No corporation");
                                            }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("Last Logon:");
                                            ui.label(
                                                self.esi.characters[index].last_logon.to_string(),
                                            );
                                        });
                                    });
                                });
                                let response = inner
                                    .response
                                    .interact(egui::Sense::click());
                                if response.clicked() {
                                    self.esi.active_character =
                                        Some(self.esi.characters[index].id);
                                }
                            });
                        });
                    });
                });
            } else {
                body.row(200.00,|mut row|{
                    row.col(|ui|{
                        ui.add_sized(ui.available_size(),egui::Label::new("⚠ There is no characters currently linked in Telescope."));
                    });
                });
            }
        });
    }
}
