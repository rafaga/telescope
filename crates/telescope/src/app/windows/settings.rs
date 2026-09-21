use crate::app::TelescopeApp;
use crate::app::messages::CharacterSync;
use crate::app::messages::Message;
use crate::app::messages::SettingsPage;
use crate::app::messages::Type;
use crate::app::messages::send_app_message;
use eframe::egui;
use eframe::egui::Button;
use eframe::egui::Color32;
use eframe::egui::IntoAtoms;
use eframe::egui::FontId;
use eframe::egui::RichText;
use eframe::egui::TextEdit;
use egui_extras::Column;
use egui_extras::TableBuilder;
use notify::RecursiveMode;
use notify::Watcher;
use std::path::Path;
use std::sync::Arc;
use native_tools::dialog::*;

impl TelescopeApp {
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_settings_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Settings")
        .movable(true)
        .resizable(false)
        .fixed_size([700.0,510.0])
        .movable(true)
        .open(&mut self.open[2])
        .show(ctx, |ui| {
            ui.horizontal(|ui|{
                ui.vertical(|ui|{
                    let row_height = 25.0;
                    let labels = ["Intelligence","Data Sources","Characters"];
                    ui.push_id("settings_menu", |ui|{
                        TableBuilder::new(ui)
                        .column(Column::resizable(Column::exact(150.0),false))
                        .striped(false)
                        .vscroll(false)
                        .body(|body| {
                            body.rows(row_height, labels.len(), |mut row| {
                                let label = labels[row.index()];
                                let current_page = match row.index(){
                                    0 => SettingsPage::Intelligence,
                                    1 => SettingsPage::DataSources,
                                    2 => SettingsPage::Characters,
                                    _ => SettingsPage::Characters,
                                };
                                row.col(|ui: &mut egui::Ui|{
                                    let option_selected = || -> bool {
                                        self.selected_settings_page == current_page
                                    };
                                    if ui.selectable_label(option_selected(),label).clicked() {
                                        self.selected_settings_page = current_page;
                                    };
                                });
                            });
                        });
                    });
                    ui.add_space(480.0 - (labels.len() as f32 * row_height));
                });
                ui.separator();
                ui.push_id("settings_config", |ui|{
                    ui.vertical(|ui|{
                        egui::ScrollArea::vertical().show(ui,|ui|{
                            match self.selected_settings_page {
                                // Mapping
                                SettingsPage::Intelligence => {
                                    let mut keys:Vec<usize> = self
                                        .behavior
                                        .tile_data
                                        .keys()
                                        .copied().collect();
                                    keys.sort_unstable();
                                    let num_rows = keys.len().div_ceil(3);
                                    ui.label(RichText::new("Alerts").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        let mut data = self.settings.get_warning_area();
                                        ui.label("Warn me when an enemy is within");
                                        egui::ComboBox::from_label("systems close to me")
                                            .selected_text(data.to_string())
                                            .show_ui(ui, |ui| {
                                                for i in 1u8..8 {
                                                    if ui.selectable_value(&mut data, i, i.to_string()).changed() {
                                                        self.settings.set_warning_area(i);
                                                    }
                                                }
                                            });
                                        ui.end_row();
                                    });
                                    ui.horizontal(|ui|{
                                        let enabled = true;
                                        ui.label("EVE Channel logs:");
                                        let mut str_intel = self.settings.get_intel().to_string_lossy().to_string();
                                        ui.add_enabled(enabled, TextEdit::singleline(&mut str_intel));
                                        let atoms2= ("Select").into_atoms();
                                        if ui.add_enabled(enabled, Button::new(atoms2)).clicked(){
                                            let runtime = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .unwrap();
                                                let app_msg_tx = Arc::clone(&self.app_msg.0);
                                            self.dlg_intel_dir.open_file_dialog(move |result| {
                                                if let DialogResult::Ok(path) = result {
                                                    runtime.block_on(async {
                                                        let _span = tracing::info_span!("spawned intel message data").entered();
                                                        let _ = send_app_message(
                                                            &app_msg_tx,
                                                            Message::UpdateIntelDirectory(path),
                                                        )
                                                        .await;
                                                    });
                                                }
                                            });
                                        }
                                        let atoms = ("Default").into_atoms();
                                        if ui.add_enabled(enabled, Button::new(atoms)).clicked(){
                                            let runtime = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .unwrap();
                                            let app_msg_tx = Arc::clone(&self.app_msg.0);
                                            runtime.block_on(async {
                                                let _ = send_app_message(
                                                    &app_msg_tx,
                                                    Message::DefaultIntelDirectory,
                                                )
                                                .await;
                                            });
                                        }
                                    });
                                    let row_height = 18.0;
                                    let mut available_channels = self.settings.get_available_channels();
                                    // clone keys to avoid borrowing available_channels while we later mutably borrow it
                                    let mut channels: Vec<String> = available_channels.keys().cloned().collect();
                                    channels.sort_unstable();
                                    ui.label(RichText::new("Monitored channels").font(FontId::proportional(20.0)));
                                    ui.label("Select all the Intel Channels to monitor.");
                                    ui.push_id("chan_tbl",|ui|{
                                        TableBuilder::new(ui)
                                        .columns(Column::resizable(Column::exact(230.0), true), 2)
                                        .striped(true)
                                        .vscroll(true)
                                        .body(|mut body|{
                                            if !channels.is_empty() {
                                                body.rows(row_height, channels.len().div_ceil(2), |mut row| {
                                                    let index = row.index() * 2;
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let key = &channels[index];
                                                        let chan = available_channels.get_mut(key).unwrap();
                                                        ui.checkbox(chan, key);
                                                    });
                                                    if index < channels.len()-1 {
                                                        row.col(|ui: &mut egui::Ui| {
                                                            let key = &channels[index + 1];
                                                            let chan = available_channels.get_mut(key).unwrap();
                                                            ui.checkbox(chan, key);
                                                        });
                                                    }
                                                });
                                            } else {
                                                body.row(row_height,|mut row|{
                                                    row.col(|ui|{
                                                        ui.label("No intel channels detected");
                                                    });
                                                });
                                            }
                                        });
                                    });
                                    self.settings.set_available_channels(available_channels);
                                    ui.label(RichText::new("Start-up maps").font(FontId::proportional(20.0)));
                                    ui.label("By default the universe map its shown, and the regional maps where do you have linked characters, but you can override this setting marking the default regional maps to show on startup.").with_new_rect(ui.available_rect_before_wrap());
                                    ui.push_id("rgn_tbl",|ui|{
                                        TableBuilder::new(ui)
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .column(Column::resizable(Column::exact(150.0),false))
                                        .striped(true)
                                        .vscroll(false)
                                        .body(|body| {
                                            body.rows(row_height, num_rows, |mut row| {
                                                let key_index = row.index() * 3;
                                                row.col(|ui: &mut egui::Ui| {
                                                    let region = self.behavior.tile_data.get_mut(&keys[key_index]).unwrap();
                                                    let name = region.get_name();
                                                    //let checked = &mut self.behavior.tile_data.get_mut(&region.get_id()).unwrap().show_on_startup;
                                                    ui.checkbox(&mut region.show_on_startup, name);
                                                });
                                                let mut t_key_index = key_index + 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        ui.checkbox(&mut region.show_on_startup, name);
                                                    });
                                                }
                                                t_key_index += 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        ui.checkbox(&mut region.show_on_startup, name);
                                                    });
                                                }
                                            });
                                        });
                                    });
                                },
                                // Linked Characters
                                SettingsPage::DataSources => {
                                    ui.label(RichText::new("Data Paths").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        ui.label("SDE database:");
                                        let mut str_sde = self.settings.get_sde().to_string_lossy().to_string();
                                        if ui.text_edit_singleline(&mut str_sde).changed() {
                                            let _ = self.settings.set_sde(Path::new(&str_sde));
                                        }
                                    });
                                    ui.horizontal(|ui|{
                                        ui.label("private database:");
                                        let mut str_db = self.settings.get_db().to_string_lossy().to_string();
                                        if ui.text_edit_singleline(&mut str_db).changed() {
                                            let _ = self.settings.set_db(Path::new(&str_db));
                                        }
                                    });
                                    ui.horizontal(|ui|{
                                        if ui.button("🔄 Check for SDE updates").clicked() {
                                            let sde_cache_dir = Self::sde_build_cache_dir(&self.settings);
                                            crate::app::database_updater::DatabaseUpdater::spawn(
                                                self.settings.get_sde().to_path_buf(),
                                                sde_cache_dir.join("data"),
                                                sde_cache_dir.join("sde"),
                                                Arc::clone(&self.app_msg.0),
                                                false,
                                            );
                                        }
                                    });
                                },
                                SettingsPage::Characters => {
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
                                },
                            }
                        });
                    });
                });
            });
            ui.horizontal(|ui|{
                ui.add_space(650.00);
            });
            ui.horizontal(|ui|{
                if ui.add(Button::new("Save")).clicked() {
                    //self.settings.mapping.startup_regions.clear();
                    let mut startup_regions = vec![];
                    for region in self.behavior.tile_data.iter() {
                        if region.1.show_on_startup {
                            startup_regions.push(*region.0);
                        }
                    }
                    if !startup_regions.is_empty() {
                        self.settings.set_startup_regions(startup_regions);
                    }
                    if self.settings.get_intel().exists() {
                        let mut monitored_channels = Vec::new();
                        let channels = self.settings.get_available_channels();
                        for channel_data in channels.iter() {
                            if *channel_data.1 {
                                monitored_channels.push(channel_data.0.to_string());
                            }
                        }
                        monitored_channels.sort_unstable();
                        // Push the freshly-saved selection into the live
                        // handle the running watcher's event handler reads
                        // from, so newly checked/unchecked channels take
                        // effect immediately instead of only after a
                        // restart (the watcher's `IntelEventHandler` is
                        // constructed once and can't be swapped out).
                        if let Ok(mut guard) = self.intel_channels.write() {
                            *guard = monitored_channels.clone();
                        }
                        if monitored_channels.is_empty() && self.settings.get_intel().exists() {
                            let _ = self.watcher.unwatch(self.settings.get_intel());
                        } else {
                            // `watch` doesn't dedupe: calling it again on a
                            // path that's already watched stacks a second
                            // OS-level registration instead of replacing the
                            // first one, so every real filesystem event then
                            // gets delivered once per accumulated
                            // registration -- e.g. clicking "Save" three
                            // times with a channel checked makes every log
                            // line for that channel repeat three times.
                            // `unwatch` first (ignoring the "wasn't watched
                            // yet" error, e.g. on the very first Save) keeps
                            // re-saving idempotent.
                            let _ = self.watcher.unwatch(self.settings.get_intel());
                            let _ = self.watcher.watch(self.settings.get_intel(),RecursiveMode::NonRecursive);
                        }
                        self.settings.set_monitored_channels(monitored_channels);
                    }
                    if let Err(e) = self.settings.save() {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        let app_msg_tx = Arc::clone(&self.app_msg.0);
                        runtime.block_on(async {
                            let _span = tracing::info_span!("spawned intel message data").entered();
                            let _ = send_app_message(
                                &app_msg_tx,
                                Message::GenericNotification((
                                    Type::Error,
                                    String::from("TelescopeApp"),
                                    String::from("open_settings_window"),
                                    e.to_string(),
                                )),
                            )
                            .await;
                        });
                    }
                }
                if !self.settings.its_saved() {
                    ui.colored_label(Color32::YELLOW, "⚠ unsaved changes");
                }
            });
        });
    }
}
