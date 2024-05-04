use crate::app::messages::{MapSync, Message, SettingsPage, Target, Type};
use crate::app::tiles::{TabPane, TileData, TreeBehavior, UniversePane};
use data::AppData;
use eframe::egui::{self,RichText,FontId};
use egui_extras::{Column, TableBuilder};
use egui_map::map::objects::*;
use egui_tiles::{Tiles, Tree};
//use futures::executor::ThreadPool;
use sde::{objects::Universe, SdeManager};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender};
use webb::esi::EsiManager;

use self::messages::{AuthSpawner, MessageSpawner};
use self::tiles::RegionPane;

mod data;
mod messages;
mod tiles;
mod settings;

pub struct TelescopeApp {
    initialized: bool,

    // 2d point to paint map
    points: Vec<MapPoint>,
    // generic messages
    app_msg: (Arc<Sender<Message>>, Receiver<Message>),
    // map Syncronization Messages
    map_msg: (Arc<BCSender<MapSync>>, BCReceiver<MapSync>),

    // these are the flags to open the windows
    // 0 - About Window
    // 1 - Character Window
    // 2 - Preferences Window
    open: [bool; 3],

    // the ESI Manager
    esi: EsiManager,
    last_message: String,
    search_text: String,
    emit_notification: bool,
    search_selected_row: Option<usize>,
    search_results: Vec<(usize, String, usize, String)>,
    factor: u64,
    path: String,
    universe: Universe,
    selected_settings_page: SettingsPage,
    //tpool: Rc<ThreadPool>,

    //tree: DockState<Tab>,
    tree: Option<Tree<Box<dyn TabPane>>>,

    behavior: TreeBehavior,
    task_msg: Arc<MessageSpawner>,
    task_auth: AuthSpawner,
}

impl Default for TelescopeApp {
    fn default() -> Self {
        // generic message handler
        let (gtx, grx) = mpsc::channel::<messages::Message>(40);
        // map syncronization handler
        let (mtx, mrx) = broadcast::channel::<messages::MapSync>(30);

        let app_data = AppData::new();
        let esi = webb::esi::EsiManager::new(
            app_data.user_agent.as_str(),
            app_data.client_id.as_str(),
            app_data.secret_key.as_str(),
            app_data.url.as_str(),
            app_data.scope,
            Some(String::from("telescope.db")),
        );

        //let mut tp_builder = ThreadPool::builder();
        //tp_builder.name_prefix("telescope-");
        //let tpool = Rc::new(tp_builder.create().unwrap());

        let factor = 50000000000000;
        let string_path = String::from("assets/sde.db");
        let path = string_path.clone();

        let mut sde = SdeManager::new(Path::new(&string_path), factor);
        let _ = sde.get_universe();

        let arc_msg_sender = Arc::new(gtx);

        let msgmon = Arc::new(MessageSpawner::new(Arc::clone(&arc_msg_sender)));
        
        let authmon= AuthSpawner::new(Arc::clone(&arc_msg_sender));

        Self {
            // Example stuff:
            initialized: false,
            points: Vec::new(),
            app_msg: (arc_msg_sender, grx),
            map_msg: (Arc::new(mtx), mrx),
            open: [false; 3],
            esi,
            last_message: String::from("Starting..."),
            search_text: String::new(),
            search_selected_row: None,
            emit_notification: false,
            factor,
            path: path.clone(),
            behavior: TreeBehavior::new(Arc::clone(&msgmon),  factor, path),
            //tpool,
            search_results: Vec::new(),
            tree: None,
            universe: sde.universe,
            selected_settings_page: SettingsPage::Intelligence,
            task_msg: msgmon,
            task_auth: authmon,
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
        puffin::profile_scope!("update");
        let Self {
            initialized: _,
            points: _points,
            app_msg: _,
            map_msg: _,
            open: _,
            esi: _,
            last_message: _,
            search_text: _,
            emit_notification: _,
            factor: _,
            path: _,
            search_selected_row: _,
            search_results: _,
            tree: _,
            universe: _,
            selected_settings_page: _,
            //tpool: _,
            behavior: _,
            task_msg: _,
            task_auth: _,
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
            for key in regions {
                let region = self.universe.regions.get(&key).unwrap();
                self.behavior.tile_data.insert(
                    region.id as usize,
                    TileData::new(region.name.clone(), false),
                );
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
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Preferences").clicked() {
                        self.open[2] = true;
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
            // The top panel is often a good place for a menu bar:
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.separator();
                ui.label(&self.last_message);
            });
        });

        egui::SidePanel::left("side_panel")
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Search");

                ui.horizontal(|ui| {
                    ui.label("Name: ");
                    let response = ui.text_edit_singleline(&mut self.search_text);
                    if response.changed() {
                        if self.search_text.len() >= 3 {
                            let sde = SdeManager::new(Path::new(&self.path), self.factor);
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
                                    if let Some(selected_row) = self.search_selected_row {
                                        if row_index == selected_row {
                                            row.set_selected(true);
                                        }
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].1).clicked()
                                        {
                                            self.search_selected_row = Some(row_index);
                                            self.click_on_system_result(row_index);
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        self.click_on_system_result(row_index);
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].3).clicked()
                                        {
                                            self.search_selected_row = Some(row_index);
                                            self.click_on_region_result(row_index);
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        self.click_on_region_result(row_index);
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

        if self.open[0] {
            self.open_about_window(ctx);
        }

        if self.open[2] {
            self.open_settings_window(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("inserting map");
            // The central panel the region left after adding TopPanel's and SidePanel's
            /*
            ui.heading("eframe template");
            ui.hyperlink("https://github.com/emilk/eframe_template");
            ui.add(egui::github_link_file!(
                "https://github.com/emilk/eframe_template/blob/master/",
                "Source code."
            ));
            */
            if let Some(tree) = &mut self.tree {
                tree.ui(&mut self.behavior, ui);
            }
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
        puffin::profile_scope!("event_manager");

        while let Ok(message) = self.app_msg.1.try_recv() {
            match message {
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character)
                }
                Message::GenericNotification(message) => self.update_status_with_error(message),
                Message::MapHidden(region_id) => self.hide_abstract_map(region_id),
                Message::NewRegionalPane(region_id) => self.create_new_regional_pane(region_id),
                Message::MapShown(region_id) => self.show_abstract_map(region_id),
            };
        }

    }

    fn open_about_window(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("open_about_window");

        egui::Window::new("About Telescope")
            .fixed_size((400.0, 200.0))
            .open(&mut self.open[0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../assets/icon01-128.png"))
                            .fit_to_original_size(1.0),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading("Telescope");
                        ui.strong("v 0.0.1");
                        ui.label("Author: Rafael Amador Galván");
                        ui.label("©2023-2024, All rights reserved.");
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
        puffin::profile_scope!("open_preferences_window");
        egui::Window::new("Settings")
        .movable(true)
        .resizable(false)
        .fixed_size([650.0,500.0])
        .movable(true)
        .open(&mut self.open[2])
        .show(ctx, |ui| {
            ui.horizontal(|ui|{
                ui.vertical(|ui|{
                    let row_height = 25.0;
                    let labels = ["Intelligence","Data Sources"];
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
                                    _ => SettingsPage::DataSources,
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
                                    let keys:Vec<usize> = self
                                        .behavior
                                        .tile_data
                                        .keys()
                                        .copied().collect();
                                    let num_rows = keys.len().div_ceil(3);
                                    ui.label(RichText::new("Alerts").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        ui.label("Warn when an enemy is within this number of jumps close to you:");
                                        ui.text_edit_singleline(&mut "0");
                                    });
                                    let row_height = 18.0;
                                    ui.label(RichText::new("Start-up maps").font(FontId::proportional(20.0)));
                                    ui.label("By default the universe map its shown, and the regional maps where do you have linked characters, but you can override this setting marking the default regional maps to show on startup.").with_new_rect(ui.available_rect_before_wrap());
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
                                    
                                },
                                // Linked Characters
                                SettingsPage::DataSources => {
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
                                                if self.esi.active_character.unwrap() == char.id {
                                                    vec_id.push(char.id);
                                                    break;
                                                }
                                                index += 1;
                                            }
                                            self.esi.characters.remove(index);
                                            self.esi.active_character = None;
                                            if let Err(t_error) = self.esi.remove_characters(Some(vec_id)) {
                                                self.task_msg.spawn(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("remove_characters"),t_error.to_string())));
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
                                                                {
                                                                    if self.esi.characters[index].id == idc {
                                                                        ui.style_mut()
                                                                            .visuals
                                                                            .override_text_color =
                                                                            Some(eframe::egui::Color32::YELLOW);
                                                                        //ui.style_mut().visuals.selection.bg_fill = Color32::LIGHT_GRAY;
                                                                        //ui.style_mut().visuals.fade_out_to_color();
                                                                    }
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
                                                                        ui.label("Aliance:");
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
                                    ui.label(RichText::new("Static data").font(FontId::proportional(20.0)));
                                    ui.horizontal(|ui|{
                                        ui.label("SDE database path:");
                                        ui.text_edit_singleline(&mut self.path);
                                    });
                                    ui.horizontal(|ui|{
                                        ui.label("private data:");
                                        ui.text_edit_singleline(&mut self.esi.path);
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
                ui.button("Save").clicked();
                ui.button("Cancel").clicked();
            });
        });
    }

    fn update_character_into_database(&mut self, response_data: (String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("update_character_into_database");
        let auth_info = self.esi.esi.get_authorize_url().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match rt.expect("Esi character authentication failure").block_on(self.esi.auth_user(auth_info, response_data)) {
            Ok(Some(player)) => {
                self.esi.characters.push(player);
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
        puffin::profile_scope!("update_status_with_error");
        match message.0 {
            Type::Error => {
                self.last_message =
                    "ERROR: ".to_string() + &message.1 + " - " + &message.2 + " > " + &message.3;
            }
            Type::Warning => {
                self.last_message = "WARN:  ".to_string()
                    + &message.1
                    + " - "
                    + &message.2
                    + " > "
                    + &message.3;
            }
            Type::Info => {
                self.last_message = "INFO:  ".to_string()
                    + &message.1
                    + " - "
                    + &message.2
                    + " > "
                    + &message.3;
            }
        }
    }

    fn create_new_regional_pane(&mut self, region_id: usize) {
        let pane = Self::generate_pane(
            self.map_msg.0.subscribe(),
            //Arc::clone(&self.app_msg.0),
            self.path.clone(),
            self.factor,
            Some(region_id),
            Arc::clone(&self.task_msg)
            //Rc::clone(&self.tpool),
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
        puffin::profile_scope!("new_eframe");
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut fonts = eframe::egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Noto Sans Regular".to_owned(),
            eframe::egui::FontData::from_static(include_bytes!("../assets/NotoSansTC-Regular.otf")),
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
        //generic_sender: Arc<Sender<Message>>,
        path: String,
        factor: u64,
        region_id: Option<usize>,
        //t_pool: Rc<ThreadPool>,
        task_msg:Arc<MessageSpawner>,
    ) -> Box<dyn TabPane> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("generate_pane");
        let pane: Box<dyn TabPane> = if let Some(region) = region_id {
            Box::new(RegionPane::new(
                receiver,
                //generic_sender,
                path,
                factor,
                region,
                //t_pool,
                task_msg
            ))
        } else {
            Box::new(UniversePane::new(
                receiver,
                //generic_sender,
                path,
                factor,
                //t_pool,
                task_msg,
            ))
        };
        pane
    }

    fn hide_abstract_map(&mut self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("close_abstract_map");
        if let Some(tile_id) = self
            .behavior
            .tile_data
            .get(&region_id)
            .unwrap()
            .get_tile_id()
        {
            self.tree.as_mut().unwrap().tiles.toggle_visibility(tile_id);
            self.behavior.tile_data.entry(region_id).and_modify(|entry| {
                entry.set_visible(false);
            });
        }
    }

    fn create_tree(&self) -> Tree<Box<dyn TabPane>> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("create_tree");
        let mut tiles = Tiles::default();
        let id = tiles.insert_pane(Self::generate_pane(
            self.map_msg.0.subscribe(),
            //Arc::clone(&self.app_msg.0),
            self.path.clone(),
            self.factor,
            None,
            //Rc::clone(&self.tpool),
            Arc::clone(&self.task_msg)
        ));
        let tile_ids = vec![id];
        let root = tiles.insert_tab_tile(tile_ids);
        egui_tiles::Tree::new("maps", root, tiles)
    }

    fn click_on_system_result(&self, row_index: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("click_on_system_result");
        let tx_map = Arc::clone(&self.map_msg.0);
        let system_id = self.search_results[row_index].0;
        let emit_notification = self.emit_notification;
        let _result = tx_map.send(MapSync::CenterOn((system_id, Target::System)));
        if emit_notification {
            let _result = tx_map.send(MapSync::SystemNotification(system_id));
        }
    }

    fn click_on_region_result(&self, row_index: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("click_on_region_result");
        let tx_map = Arc::clone(&self.map_msg.0);
        let region_id = self.search_results[row_index].2;
        let _result = tx_map.send(MapSync::CenterOn((region_id, Target::Region)));
    }
}
