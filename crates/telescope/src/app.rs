use crate::app::file::IntelEventHandler;
use crate::app::messages::{CharacterSync, MapSync, Message, SettingsPage, Target, Type};
use crate::app::tiles::{TabPane, TileData, TreeBehavior, UniversePane};
use chrono::Utc;
use data::AppData;
use eframe::egui::{
    self, Button, Color32, FontFamily, FontId, Margin, RichText, TextFormat, Vec2,
    epaint::text::LayoutJob,
};
use eframe::egui::{IntoAtoms, TextEdit};
use egui_extras::{Column, TableBuilder};
use egui_map::map::objects::*;
use egui_tiles::{Tiles, Tree};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use regex::RegexBuilder;
use sde::{SdeManager, objects::Universe};
use settings::Manager;
use std::thread;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};
use tokio::time::{Duration, sleep};
use webb::esi::EsiManager;
use egui_file_dialog::FileDialog;
//use std::path::PathBuf;

use self::messages::{AuthSpawner, MessageSpawner};
use self::tiles::RegionPane;

mod data;
mod file;
mod messages;
mod settings;
mod tiles;

pub struct TelescopeApp {
    initialized: bool,

    // 2d point to paint map
    points: Vec<MapPoint>,
    // generic messages
    app_msg: (Arc<Sender<Message>>, Receiver<Message>),
    // map synchronization Messages
    map_msg: (Arc<BCSender<MapSync>>, BCReceiver<MapSync>),
    char_msg: Option<Arc<Sender<CharacterSync>>>,

    // these are the flags to open the windows
    // 0 - About Window
    // 1 - Character Window
    // 2 - Preferences Window
    open: [bool; 3],

    // the ESI Manager
    esi: EsiManager,
    app_messages: Vec<LayoutJob>,
    search_text: String,
    emit_notification: bool,
    search_selected_row: Option<usize>,
    search_results: Vec<(usize, String, usize, String)>,
    universe: Universe,
    selected_settings_page: SettingsPage,
    tree: Option<Tree<Box<dyn TabPane>>>,

    behavior: TreeBehavior,
    task_msg: Arc<MessageSpawner>,
    task_auth: AuthSpawner,
    settings: Manager,
    watcher: RecommendedWatcher,
    open_dialog:FileDialog,
}

impl Default for TelescopeApp {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let settings = Manager::new();
        // generic message handler
        let (gtx, grx) = mpsc::channel::<messages::Message>(40);
        // map synchronization handler
        let (mtx, mrx) = broadcast::channel::<messages::MapSync>(30);

        let app_data = AppData::new();
        let esi = webb::esi::EsiManager::new(
            app_data.user_agent.as_str(),
            app_data.client_id,
            app_data.secret_key,
            app_data.url.as_str(),
            app_data.scope,
            settings.paths.local_db.clone(),
        );

        let mut sde = SdeManager::new(Path::new(&settings.paths.sde_db), settings.factor);
        let _ = sde.get_universe();

        let arc_map_sender = Arc::new(mtx);
        let arc_msg_sender = Arc::new(gtx);
        let msgmon = Arc::new(MessageSpawner::new(Arc::clone(&arc_msg_sender)));
        let authmon = AuthSpawner::new(Arc::clone(&arc_msg_sender));

        let intel_event_handler = IntelEventHandler::new(
            settings.channels.monitored.clone(),
            Arc::clone(&arc_msg_sender),
        );
        let mut watcher = RecommendedWatcher::new(intel_event_handler, Config::default()).unwrap();
        let open_dialog = FileDialog::new();

        if let Some(intel_path) = &settings.paths.intel {
            watcher
                .watch(intel_path, RecursiveMode::NonRecursive)
                .expect("Error monitoring intel file path");
        }

        Self {
            // Example stuff:
            initialized: false,
            points: Vec::new(),
            app_msg: (arc_msg_sender, grx),
            map_msg: (arc_map_sender, mrx),
            char_msg: None,
            open: [false; 3],
            esi,
            app_messages: Vec::new(),
            search_text: String::new(),
            search_selected_row: None,
            emit_notification: false,
            behavior: TreeBehavior::new(
                Arc::clone(&msgmon),
                settings.factor,
                settings.paths.sde_db.clone(),
            ),
            search_results: Vec::new(),
            tree: None,
            universe: sde.universe,
            selected_settings_page: SettingsPage::Intelligence,
            task_msg: msgmon,
            task_auth: authmon,
            settings,
            watcher,
            open_dialog,
        }
    }
}

impl eframe::App for TelescopeApp {
    /// Called by the frame work to save state before shutdown.
    /*fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }*/
    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        //let mut rt = tokio::runtime::Runtime::new().unwrap();
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let Self {
            initialized: _,
            points: _points,
            app_msg: _,
            map_msg: _,
            char_msg: _,
            open: _,
            esi: _,
            app_messages: _,
            search_text: _,
            emit_notification: _,
            search_selected_row: _,
            search_results: _,
            tree: _,
            universe: _,
            selected_settings_page: _,
            behavior: _,
            task_msg: _,
            task_auth: _,
            settings: _,
            watcher: _,
            open_dialog: _,
        } = self;

        if !self.initialized {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("telescope_init");

            egui_extras::install_image_loaders(ctx);

            self.tree = Some(self.create_tree());

            let mut vec_chars = Vec::new();
            for pchar in self.esi.characters.iter() {
                vec_chars.push((pchar.id, pchar.photo.as_ref().unwrap().clone()));
            }

            let regions: Vec<u32> = self
                .universe
                .regions
                .keys()
                .copied()
                .filter(|val| val < &11000000)
                .collect();

            for key in &regions {
                let region = self.universe.regions.get(key).unwrap();
                self.behavior.tile_data.insert(
                    region.id as usize,
                    TileData::new(region.name.clone(), false),
                );
            }

            for counter in 0..self.settings.mapping.startup_regions.len() {
                if regions.contains(&(self.settings.mapping.startup_regions[counter] as u32)) {
                    self.behavior
                        .tile_data
                        .entry(self.settings.mapping.startup_regions[counter])
                        .and_modify(|z_region| {
                            z_region.show_on_startup = true;
                        });
                    self.create_new_regional_pane(self.settings.mapping.startup_regions[counter]);
                }
            }

            if !self.esi.characters.is_empty() {
                let mut ids = vec![];
                for char in &self.esi.characters {
                    ids.push(char.id as usize);
                }
                self.start_watchdog(ids);
            }

            self.initialized = true;
        }

        self.event_manager();
        // Examples of how to create different panels and windows.
        // Pick whichever suits you.
        // Tip: a good default choice is to just keep the `CentralPanel`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        #[cfg(not(target_arch = "wasm32"))] // no File->Quit on web pages!
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Preferences").clicked() {
                        self.open[2] = true;
                    }
                    if ui.button("Debug").clicked() {
                        self.open[1] = true;
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About Telescope").clicked() {
                        self.open[0] = true;
                    }
                });
            });
        });

        // Bottom menu
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.separator();
            });
        });

        if self.open[0] {
            self.open_about_window(ctx);
        }

        // Debug menu
        if self.open[1] {
            self.open_debug_menu(ctx);
        }

        if self.open[2] {
            self.open_settings_window(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("inserting map");
            if let Some(tree) = &mut self.tree {
                let mut rect = ui.available_size_before_wrap();
                rect.y -= 100.0;
                tree.set_height(rect.y);
                tree.ui(&mut self.behavior, ui);
            }
            let _ = egui::Frame::canvas(ui.style())
                .inner_margin(Margin::symmetric(2, 5))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .max_height(100.0)
                        .max_width(f32::INFINITY)
                        .auto_shrink(false)
                        .show_rows(
                            ui,
                            ui.text_style_height(&egui::TextStyle::Body),
                            self.app_messages.len(),
                            |ui, row_range| {
                                ui.vertical(|ui| {
                                    for index in row_range {
                                        ui.label(self.app_messages[index].clone());
                                    }
                                });
                            },
                        );
                });
        });

        //ui.add(&mut self.map);
        /*if let Some(points) = self.universe.points {

        }*/
        //ui.label("鑑於對人類家庭所有成員的固有尊嚴及其平等的和不移的權利的承認，乃是世界自由、正義與和平的基礎");
    }
}

impl TelescopeApp {
    fn event_manager(&mut self) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        while let Ok(message) = self.app_msg.1.try_recv() {
            match message {
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character)
                }
                Message::GenericNotification(message) => self.update_status_with_error(message),
                Message::MapHidden(region_id) => self.hide_abstract_map(region_id),
                Message::NewRegionalPane(region_id) => self.create_new_regional_pane(region_id),
                Message::MapShown(region_id) => self.show_abstract_map(region_id),
                Message::PlayerNewLocation((player_id, solar_system_id)) => {
                    self.update_player_location(player_id, solar_system_id)
                }
                Message::IntelFileChanged(file_name) => {
                    self.load_intel_file(file_name);
                },
                Message::ShowSelectDirDialog() => {
                    self.open_directory_selector();
                }
            };
        }
    }

    fn open_about_window(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        egui::Window::new("About Telescope")
            .fixed_size((400.0, 200.0))
            .open(&mut self.open[0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../../../assets/icon.png"))
                            .fit_to_exact_size(Vec2 { x: 200.0, y: 200.0 }),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading("Telescope");
                        ui.strong("v ".to_owned() + env!("CARGO_PKG_VERSION"));
                        ui.label("Author: Rafael Amador");
                        ui.label("Licensed under MIT");
                        if ui.link("https://github.com/rafaga/telescope").clicked() {
                            let _a = open::that("https://github.com/rafaga/telescope");
                        }
                        egui::warn_if_debug_build(ui);
                    });
                });
            });
    }

    fn open_settings_window(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        egui::Window::new("Settings")
        .movable(true)
        .resizable(false)
        .fixed_size([650.0,510.0])
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
                                        ui.label("Warn me when an enemy is within");
                                        egui::ComboBox::from_label("systems close to me")
                                            .selected_text(self.settings.mapping.warning_area.clone())
                                            .show_ui(ui, |ui| {
                                                for i in 1u8..8 {
                                                    if ui.selectable_value(&mut self.settings.mapping.warning_area, i.to_string(), i.to_string()).changed() {
                                                        self.settings.saved = false;
                                                    }
                                                }
                                            });
                                        ui.end_row();
                                    });
                                    ui.horizontal(|ui|{
                                        let atoms= ().into_atoms();
                                        ui.checkbox(&mut self.settings.paths.enable_custom_intel, atoms);
                                        ui.label("EVE Channel logs:");
                                        if ui.add_enabled(self.settings.paths.enable_custom_intel, TextEdit::singleline(&mut self.settings.paths.custom_intel)).changed() {
                                            self.settings.saved = false;
                                        }
                                        let atoms2= ("Select").into_atoms();
                                        if ui.add_enabled(self.settings.paths.enable_custom_intel, Button::new(atoms2)).clicked(){
                                            //let dir = Path::new(self.settings.paths.custom_intel.as_str()).to_path_buf();
                                            let runtime = tokio::runtime::Builder::new_current_thread()
                                                .enable_all()
                                                .build()
                                                .unwrap();
                                            let app_msg_tx = Arc::clone(&self.app_msg.0);
                                            thread::spawn(move || {
                                                runtime.block_on(async {
                                                    #[cfg(feature = "puffin")]
                                                    puffin::profile_scope!("spawned intel message data");
                                                    let _ = app_msg_tx
                                                        .send(Message::ShowSelectDirDialog())
                                                        .await;
                                                });
                                            });
                                        }
                                    });
                                    let row_height = 18.0;
                                    let mut channels:Vec<String> = self
                                        .settings
                                        .channels
                                        .available
                                        .keys().cloned().collect();
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
                                                        let chan = self.settings.channels.available.get_mut(&channels[index]).unwrap();
                                                        if ui.checkbox(chan, &channels[index]).changed() {
                                                            self.settings.saved = false;
                                                        }
                                                    });
                                                    if index < channels.len()-1 {
                                                        row.col(|ui: &mut egui::Ui| {
                                                            let chan = self.settings.channels.available.get_mut(&channels[index + 1]).unwrap();
                                                            if ui.checkbox(chan, &channels[index + 1]).changed() {
                                                                self.settings.saved = false;
                                                            }
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
                                                    if ui.checkbox(&mut region.show_on_startup, name).changed() {
                                                        self.settings.saved = false;
                                                    }
                                                });
                                                let mut t_key_index = key_index + 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        if ui.checkbox(&mut region.show_on_startup, name).changed() {
                                                            self.settings.saved = false;
                                                        }
                                                    });
                                                }
                                                t_key_index += 1;
                                                if t_key_index < keys.len() {
                                                    row.col(|ui: &mut egui::Ui| {
                                                        let region = self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                                        let name = region.get_name();
                                                        if ui.checkbox(&mut region.show_on_startup, name).changed() {
                                                            self.settings.saved = false;
                                                        }
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
                                        if ui.text_edit_singleline(&mut self.settings.paths.sde_db).changed() {
                                            self.settings.saved = false;
                                        }
                                    });
                                    ui.horizontal(|ui|{
                                        ui.label("private database:");
                                        if ui.text_edit_singleline(&mut self.settings.paths.local_db).changed() {
                                            self.settings.saved = false;
                                        }
                                    });
                                },
                                SettingsPage::Characters => {
                                    ui.label(RichText::new("Linked characters").font(FontId::proportional(20.0)));
                                    ui.label("These are used to emit notifications when something it is close to your location.");
                                    ui.horizontal(|ui|{
                                        if ui.button("➕ Add").clicked() {
                                            let auth_info = self.esi.esi.get_authorize_url().unwrap();
                                            match open::that(auth_info.authorization_url) {
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
                    self.settings.mapping.startup_regions.clear();
                    for region in self.behavior.tile_data.iter() {
                        if region.1.show_on_startup {
                            self.settings.mapping.startup_regions.push(*region.0);
                        }
                    }
                    if let Some(intel_path) = &self.settings.paths.intel {
                        let _ = self.watcher.unwatch(intel_path);
                    }
                    let mut monitored_channels = Vec::new();
                    for channel_data in self.settings.channels.available.iter() {
                        if *channel_data.1 {
                            monitored_channels.push(channel_data.0.to_string());
                        }
                    }
                    monitored_channels.sort_unstable();
                    if let Some(intel_path) = &self.settings.paths.intel {
                        let _ = self.watcher.watch(intel_path, RecursiveMode::NonRecursive);
                    }
                    self.settings.channels.monitored = Arc::new(monitored_channels);
                    self.settings.save();
                }
                if !self.settings.saved {
                    ui.colored_label(Color32::YELLOW, "⚠ unsaved changes");
                }
            });
        });
    }

    fn load_intel_file(&mut self, file_name: String) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut path = self.settings.paths.intel.as_ref().unwrap().clone();
        path = path.join(file_name.as_str());

        //getting the first byte to read from ythe last recorded file lenght
        let mut start = 0;
        if let Some(log_entry) = self.settings.channels.log_files.get(&file_name) {
            start = log_entry.0;
        }
        let mut _length = 0;

        if let Ok(mut intel_file) = File::open(path) {
            // Seek to the start position
            _length = start - intel_file.metadata().unwrap().len();
            if intel_file.seek(SeekFrom::Start(start)).is_ok() {
                // Create a reader with a fixed length
                let mut chunk = intel_file.take(_length);
                let mut new_data = String::new();
                if let Ok(bytes_read) = chunk.read_to_string(&mut new_data) {
                    let _ = self.parse_intel_data(new_data);
                    self.settings
                        .channels
                        .log_files
                        .entry(file_name)
                        .and_modify(|hash_entry| {
                            hash_entry.0 = start + bytes_read as u64;
                            hash_entry.1 = Utc::now();
                        });
                }
            }
        }
    }

    fn parse_intel_data(&mut self, data: String) -> Result<(), String> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if let Ok(set) =
            RegexBuilder::new(r"(\[\s\d{4}\.\d{2}\.\d{2}\s\d{2}:\d{2}:\d{2}\s\]){1}(.+>)(.+)")
                .case_insensitive(true)
                .build()
        {
            data.lines()
                .filter(|line| set.is_match(line))
                .for_each(|intel_line| {
                    let data_x = intel_line.to_string().clone();
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let app_msg_tx = Arc::clone(&self.app_msg.0);
                    thread::spawn(move || {
                        runtime.block_on(async {
                            #[cfg(feature = "puffin")]
                            puffin::profile_scope!("spawned intel message data");

                            let _ = app_msg_tx
                                .send(Message::GenericNotification((
                                    Type::Info,
                                    String::from("TelescopeApp"),
                                    String::from("parse_intel_data"),
                                    data_x,
                                )))
                                .await;
                        });
                    });
                });
            Ok(())
        } else {
            Err(String::from("Error Building Regex"))
        }
    }

    fn open_directory_selector(&mut self){
        self.open_dialog
            .pick_directory();
    }

    fn update_character_into_database(&mut self, response_data: (String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let auth_info = self.esi.esi.get_authorize_url().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match rt
            .as_ref()
            .expect("Esi character authentication failure")
            .block_on(self.esi.auth_user(auth_info, response_data))
        {
            Ok(Some(player)) => {
                let id = player.id as usize;
                self.esi.characters.push(player);
                if self.esi.characters.len() == 1 {
                    self.start_watchdog(vec![id]);
                } else if let Some(sender) = &self.char_msg {
                    let _result = rt
                        .as_ref()
                        .expect("Esi character authentication failure")
                        .block_on(async { sender.send(CharacterSync::Add(id)).await });
                }
            }
            Ok(None) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Info,
                    String::from("EsiManager"),
                    String::from("auth_user"),
                    String::from(
                        "Apparently there was some kind of trouble authenticating the player.",
                    ),
                )));
            }
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("EsiManager"),
                    String::from("auth_user"),
                    t_error.to_string(),
                )));
            }
        };
    }

    fn update_status_with_error(&mut self, message: (Type, String, String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let full_time = chrono::Local::now().time().to_string();
        let time = full_time.split_at(12);
        let mut job = LayoutJob::default();
        let normal_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::LIGHT_GRAY,
            ..Default::default()
        };
        let time_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DARK_GRAY,
            ..Default::default()
        };
        let warn = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::KHAKI,
            ..Default::default()
        };
        let info = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::BLUE,
            ..Default::default()
        };
        let debug = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DEBUG_COLOR,
            ..Default::default()
        };
        let error = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::RED,
            ..Default::default()
        };
        job.append("[", 0.0, normal_text.clone());
        job.append(time.0, 0.0, time_text.clone());
        job.append("] ", 0.0, normal_text.clone());
        match message.0 {
            Type::Error => {
                job.append("ERROR: ", 0.0, error.clone());
                job.append(
                    (message.1 + " - " + &message.2 + " - ").as_str(),
                    0.0,
                    normal_text.clone(),
                );
            }
            Type::Warning => {
                job.append("WARN: ", 0.0, warn.clone());
            }
            Type::Info => {
                job.append("INFO: ", 0.0, info.clone());
            }
            Type::Debug => {
                job.append("DEBUG: ", 0.0, debug.clone());
            }
        }
        job.append(&message.3, 0.0, normal_text.clone());
        self.app_messages.push(job);
    }

    fn create_new_regional_pane(&mut self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let pane = Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.paths.sde_db.clone(),
            self.settings.region_factor,
            Some(region_id),
            Arc::clone(&self.task_msg),
        );
        let tile_id = self.tree.as_mut().unwrap().tiles.insert_pane(pane);
        let root = self.tree.as_ref().unwrap().root.unwrap();
        let counter = self.tree.as_ref().unwrap().tiles.len();
        self.tree
            .as_mut()
            .unwrap()
            .move_tile_to_container(tile_id, root, counter, false);
        self.behavior.tile_data.entry(region_id).and_modify(|data| {
            data.set_visible(true);
            data.set_tile_id(Some(tile_id));
        });
    }

    fn show_abstract_map(&mut self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.behavior
            .tile_data
            .entry(region_id)
            .and_modify(|region| {
                self.tree
                    .as_mut()
                    .unwrap()
                    .set_visible(region.get_tile_id().unwrap(), true);
                region.set_visible(true);
            });
    }

    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut fonts = eframe::egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Noto Sans Regular".to_owned(),
            Arc::new(eframe::egui::FontData::from_static(include_bytes!(
                "../../../assets/NotoSansTC-Regular.otf"
            ))),
        );
        fonts
            .families
            .get_mut(&eframe::egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "Noto Sans Regular".to_owned());

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        /*if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }*/
        cc.egui_ctx.set_fonts(fonts);

        let app: TelescopeApp = Default::default();
        app
    }

    fn generate_pane(
        receiver: BCReceiver<MapSync>,
        path: String,
        factor: i64,
        region_id: Option<usize>,
        task_msg: Arc<MessageSpawner>,
    ) -> Box<dyn TabPane> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let pane: Box<dyn TabPane> = if let Some(region) = region_id {
            Box::new(RegionPane::new(receiver, path, factor, region, task_msg))
        } else {
            Box::new(UniversePane::new(receiver, path, factor, task_msg))
        };
        pane
    }

    fn hide_abstract_map(&mut self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if let Some(tile_id) = self
            .behavior
            .tile_data
            .get(&region_id)
            .unwrap()
            .get_tile_id()
        {
            self.tree.as_mut().unwrap().tiles.toggle_visibility(tile_id);
            self.behavior
                .tile_data
                .entry(region_id)
                .and_modify(|entry| {
                    entry.set_visible(false);
                });
        }
    }

    fn create_tree(&self) -> Tree<Box<dyn TabPane>> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let mut tiles = Tiles::default();
        let id = tiles.insert_pane(Self::generate_pane(
            self.map_msg.0.subscribe(),
            self.settings.paths.sde_db.clone(),
            self.settings.factor,
            None,
            Arc::clone(&self.task_msg),
        ));
        let tile_ids = vec![id];
        let root = tiles.insert_tab_tile(tile_ids);
        egui_tiles::Tree::new("maps", root, tiles)
    }

    pub fn start_watchdog(&mut self, character_id: Vec<usize>) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (sender, mut receiver) = mpsc::channel::<CharacterSync>(10);
        let app_sender = Arc::clone(&self.app_msg.0);
        let map_sender = Arc::clone(&self.map_msg.0);
        let mut t_esi = self.esi.clone();
        thread::spawn(move || {
            runtime.block_on(async {
                #[cfg(feature = "puffin")]
                puffin::profile_scope!("spawned watchdog");

                let mut character_ids = vec![];
                if let Err(t_error) = t_esi.esi.update_spec().await {
                    let _ = app_sender
                        .send(Message::GenericNotification((
                            Type::Error,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            t_error.to_string(),
                        )))
                        .await;
                } else {
                    let _ = app_sender
                        .send(Message::GenericNotification((
                            Type::Info,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            String::from("Starting watchdog"),
                        )))
                        .await;
                }
                for char_id in character_id {
                    character_ids.push((char_id, 0))
                }
                while !character_ids.is_empty() {
                    if !t_esi.valid_token().await {
                        if let Err(t_error) = t_esi.refresh_token().await {
                            let _ = app_sender
                                .send(Message::GenericNotification((
                                    Type::Error,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    t_error.to_string(),
                                )))
                                .await;
                            return;
                        } else {
                            let _ = app_sender
                                .send(Message::GenericNotification((
                                    Type::Debug,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    String::from("token refreshed successfully"),
                                )))
                                .await;
                        }
                    }
                    for item in &mut character_ids {
                        //PlayerDatabase
                        match t_esi.get_location(item.0.try_into().unwrap()).await {
                            Ok(new_location) => {
                                if item.1 != (new_location as usize) {
                                    item.1 = new_location as usize;
                                    let _ = app_sender
                                        .send(Message::PlayerNewLocation((
                                            item.0.try_into().unwrap(),
                                            new_location,
                                        )))
                                        .await;
                                    if let Err(t_error) =
                                        map_sender.send(MapSync::PlayerMoved((item.0, item.1)))
                                    {
                                        let _ = app_sender
                                            .send(Message::GenericNotification((
                                                Type::Error,
                                                String::from("Telescope App"),
                                                String::from("start_watchdog"),
                                                t_error.to_string(),
                                            )))
                                            .await;
                                    }
                                }
                            }
                            Err(t_error) => {
                                let _ = app_sender
                                    .send(Message::GenericNotification((
                                        Type::Error,
                                        String::from("Telescope App"),
                                        String::from("start_watchdog - get_location - ")
                                            + item.0.to_string().as_str(),
                                        t_error.to_string(),
                                    )))
                                    .await;
                                break;
                            }
                        }
                    }
                    sleep(Duration::new(5, 0)).await;
                    while let Ok(message) = receiver.try_recv() {
                        match message {
                            CharacterSync::Add(char_data) => character_ids.push((char_data, 0)),
                            CharacterSync::Remove(char_id) => {
                                for index in 0..character_ids.len() {
                                    if character_ids[index].0 == char_id {
                                        character_ids.remove(index);
                                        break;
                                    }
                                }
                                if character_ids.is_empty() {
                                    let _ = app_sender
                                        .send(Message::GenericNotification((
                                            Type::Info,
                                            String::from("Telescope App"),
                                            String::from("start_watchdog"),
                                            String::from("Watchdog ended"),
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }
                    }
                    if let Err(TryRecvError::Disconnected) = receiver.try_recv() {
                        character_ids.clear();
                        let _ = app_sender
                            .send(Message::GenericNotification((
                                Type::Info,
                                String::from("Telescope App"),
                                String::from("start_watchdog"),
                                String::from("Watchdog ended"),
                            )))
                            .await;
                        break;
                    }
                    sleep(Duration::new(25, 0)).await;
                }
            });
        });

        self.char_msg = Some(Arc::new(sender));
    }

    fn open_debug_menu(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        egui::Window::new("Debug Menu")
            .fixed_size((400.0, 600.0))
            .open(&mut self.open[1])
            .show(ctx, |ui| {
                ui.heading("Search");

                ui.horizontal(|ui| {
                    ui.label("Name: ");
                    let response = ui.text_edit_singleline(&mut self.search_text);
                    if response.changed() {
                        if self.search_text.len() >= 3 {
                            let sde = SdeManager::new(
                                Path::new(&self.settings.paths.sde_db),
                                self.settings.factor,
                            );
                            match sde.get_system_id(self.search_text.clone().to_lowercase()) {
                                Ok(system_results) => self.search_results = system_results,
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
                    if ui.button("Advanced >>>").clicked() {}
                });
                ui.push_id("search_table", |ui| {
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto())
                        .column(Column::remainder())
                        .min_scrolled_height(0.0);

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
                                        if ui.label(&self.search_results[row_index].1).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let system_id = self.search_results[row_index].0;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                system_id,
                                                Target::System,
                                            )));
                                            if self.emit_notification {
                                                let _result =
                                                    tx_map.send(MapSync::SystemNotification((
                                                        system_id,
                                                        tokio::time::Instant::now(),
                                                    )));
                                            }
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let system_id = self.search_results[row_index].0;
                                        let _result = tx_map
                                            .send(MapSync::CenterOn((system_id, Target::System)));
                                        if self.emit_notification {
                                            let _result = tx_map.send(MapSync::SystemNotification(
                                                (system_id, tokio::time::Instant::now()),
                                            ));
                                        }
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].3).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let region_id = self.search_results[row_index].2;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                region_id,
                                                Target::Region,
                                            )));
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let region_id = self.search_results[row_index].2;
                                        let _result = tx_map
                                            .send(MapSync::CenterOn((region_id, Target::Region)));
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
            });
    }

    fn update_player_location(&mut self, player_id: i32, solar_system_id: i32) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        for index in 0..self.esi.characters.len() {
            if self.esi.characters[index].id == player_id {
                self.esi.characters[index].location = solar_system_id;
                let char = &mut self.esi.characters[index].clone();
                if let Ok(_a) = self.esi.write_character(char) {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Debug,
                        String::from("Telescope App"),
                        String::from("update_player_location"),
                        String::from("Player location updated"),
                    )));
                }
            }
        }
    }
}
