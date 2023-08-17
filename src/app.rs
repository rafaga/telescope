use egui::{FontData,FontDefinitions,FontFamily,Vec2};
use egui_extras::RetainedImage;
use sde::SdeManager;
use webb::objects::Character;
use std::path::Path;
use std::sync::mpsc::{self,Sender,Receiver};
use std::thread;
use egui_map::map::{Map,objects::*};
use crate::app::messages::Message;
use data::AppData;
use std::collections::HashMap;
use futures::executor::ThreadPool;

pub mod messages;
pub mod data;

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct TemplateApp<'a> {

    #[serde(skip)]
    initialized: bool,

    // 2d point to paint map
    #[serde(skip)]
    points: Vec<MapPoint>,

    #[serde(skip)]
    map: Map,

    #[serde(skip)]
    tx: Sender<Message>,

    #[serde(skip)]
    rx: Receiver<Message>,

    // these are the flags to open the windows
    // 0 - About Window
    // 1 - Character Window
    open: [bool;2],

    // the ESI Manager
    #[serde(skip)]
    esi: webb::esi::EsiManager<'a>,

    #[serde(skip)]
    portraits: HashMap<u64,RetainedImage>,

    #[serde(skip)]
    tpool: ThreadPool,
}

impl<'a> Default for TemplateApp<'a> {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel::<messages::Message>();
        let app_data = AppData::new();

        let esi = webb::esi::EsiManager::new(app_data.user_agent.as_str(),app_data.client_id.as_str(),app_data.secret_key.as_str(),app_data.url.as_str(), app_data.scope,Some("telescope.db"));
        
        let mut tp_builder = ThreadPool::builder();
        tp_builder.name_prefix("telescope-tp-");
        let tpool = tp_builder.create().unwrap();
       
        Self {
            // Example stuff:
            initialized: false,
            points: Vec::new(),
            map: Map::new(),
            tx,
            rx,
            open: [false;2],
            esi,
            portraits: HashMap::new(),
            tpool,
        }
    }
}

impl<'a> eframe::App for TemplateApp<'a> {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.event_manager();
        let Self {
            initialized: _,
            points: _points,
            map: _map,
            tx: _tx,
            rx: _rx,
            open: _,
            esi: _,
            portraits: _,
            tpool: _,
        } = self;

        if !self.initialized {
            let txs = self.tx.clone();
            let factor = 50000000000000;
            thread::spawn(move ||{
                let path = Path::new("assets/sde.db");
                let manager = SdeManager::new(path, factor); 
                if let Ok(points) = manager.get_systempoints(2) {
                    let _result = txs.send(Message::Processed2dMatrix(points));
                }
            });
            let vec_chars = self.esi.characters.clone();
            for charz in vec_chars.into_iter() {
                if let Some(image) = self.get_photo(charz.id, charz.photo.unwrap()) {
                    self.portraits.entry(charz.id).or_insert(image);
                } else {
                    let _ = self.tx.send(Message::GenericError("Error retrieving player Image - Invalid format or file not found".to_string()));
                }
            }
            
            self.initialized = true;
        }

        
        // Examples of how to create different panels and windows.
        // Pick whichever suits you.
        // Tip: a good default choice is to just keep the `CentralPanel`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        #[cfg(not(target_arch = "wasm32"))] // no File->Quit on web pages!
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.menu_button("Options", |ui| {
                        if ui.button("Characters").clicked() {
                            self.open[1] = true;
                        }
                    });
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        _frame.close();
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
                ui.spinner();
                ui.label("Initializing ... ");
                ui.separator();
                egui::warn_if_debug_build(ui);
            });
        });

        /*egui::SidePanel::left("side_panel")
        .resizable(true)
        .show(ctx, |ui| {
            ui.heading("Side Panel");

            ui.horizontal(|ui| {
                ui.label("Write something: ");
                ui.text_edit_singleline(label);
            });

            /*ui.add(egui::Slider::new(value, 0.0..=10.0).text("value"));
            if ui.button("Increment").clicked() {
                *value += 1.0;
            }*/
        });*/
        
        if self.open[0] {
            self.open_about_window(ctx);
        }

        if self.open[1] {
            self.open_character_window(ctx);
        }

        egui::CentralPanel::default()
        .show(ctx, |ui| {
            
            // The central panel the region left after adding TopPanel's and SidePanel's
            /* 
            ui.heading("eframe template");
            ui.hyperlink("https://github.com/emilk/eframe_template");
            ui.add(egui::github_link_file!(
                "https://github.com/emilk/eframe_template/blob/master/",
                "Source code."
            ));
            */
            ui.add(&mut self.map);
            /*if let Some(points) = self.universe.points {

            }*/
            //ui.label("鑑於對人類家庭所有成員的固有尊嚴及其平等的和不移的權利的承認，乃是世界自由、正義與和平的基礎");
            
        });

        if false {
            egui::Window::new("Window").show(ctx, |ui| {
                ui.label("Windows can be moved by dragging them.");
                ui.label("They are automatically sized based on contents.");
                ui.label("You can turn on resizing and scrolling if you like.");
                ui.label("You would normally choose either panels OR windows.");
            });
        }
    }
}

impl<'a> TemplateApp<'a> {
    fn initialize_application(&mut self) {
    }

    fn event_manager(&mut self)  {
        let received_data = self.rx.try_recv(); 
        if let Ok(msg) = received_data{
            match msg{
                Message::Processed2dMatrix(points) => self.map.add_points(points),
                Message::EsiAuthSuccess(character) => self.update_character_into_database(character),
                Message::EsiAuthError(message) => self.update_status_with_error(message),
                Message::GenericError(message) => self.update_status_with_error(message),
                Message::GenericWarning(message) => self.update_status_with_error(message),
            };
        }
    }

    fn open_character_window(&mut self, ctx: &egui::Context) {     
        egui::Window::new("Linked Characters")
        .open(&mut self.open[1])
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui|{
                ui.allocate_ui(Vec2::new(500.00,150.00), |ui|{
                    egui::ScrollArea::new([false,true])
                    //.auto_shrink([true,false])
                    .show(ui,|ui|{
                        ui.vertical(|ui|{
                            if &self.esi.characters.len() > &0 {
                                for char in  &self.esi.characters {
                                    ui.allocate_ui(Vec2::new(300.00,50.00), |ui|{
                                        ui.group(|ui|{                   
                                            ui.horizontal_centered(|ui|{
                                                ui.checkbox(&mut false, "");
                                                let player_portrait = self.portraits.get(&char.id);
                                                if let Some(image) = player_portrait {
                                                    ui.image(image.texture_id(ctx),Vec2::new(75.0,75.0));
                                                }
                                                ui.vertical(|ui|{
                                                    ui.horizontal(|ui|{
                                                        //ui.image(char_photo, Vec2::new(16.0,16.0));
                                                        ui.label("Name:");
                                                        ui.label(&char.name);
                                                    });
                                                    ui.horizontal(|ui|{
                                                        ui.label("Aliance:");
                                                        if let Some(alliance) = char.alliance.as_ref() {
                                                            ui.label(&alliance.name);
                                                        }
                                                        else{
                                                            ui.label("No alliance");
                                                        }
                                                    });
                                                    ui.horizontal(|ui|{
                                                        ui.label("Corporation:");
                                                        if let Some(corp) = char.corp.as_ref() {
                                                            ui.label(&corp.name);
                                                        }
                                                        else{
                                                            ui.label("No corporation");
                                                        }
                                                    });
                                                    ui.horizontal(|ui|{
                                                        ui.label("Last Logon:");
                                                        ui.label(char.last_logon.to_string());
                                                    });
                                                });
                                            });
                                        });
                                    });
                                }                                        
                            } else {
                                ui.allocate_ui(Vec2::new(300.00,50.00), |ui|{
                                    ui.group(|ui|{
                                        ui.vertical_centered(|ui|{
                                            ui.label("No character has been linked, please");
                                            ui.label("link a new Character to proceed.");
                                        });
                                    });
                                });
                            }
                        });
                    });
                });
                ui.separator();
                ui.allocate_ui(Vec2::new(500.00,150.00), |ui|{
                    ui.vertical(|ui|{
                        if ui.button("Link new").clicked() {
                            let (url,_rand) = self.esi.esi.get_authorize_url().unwrap();
                            match open::that(&url){
                                Ok(()) => {
                                    let _result = match self.esi.auth_user(56123) {
                                        Ok(Some(char)) => self.tx.send(Message::EsiAuthSuccess(char)),
                                        Ok(None) => self.tx.send(Message::EsiAuthError("Error de autenticacion".to_string())),
                                        Err(error_z) => panic!("ESI Error: '{}'", error_z),
                                    };
                                },
                                Err(err) => panic!("An error occurred when opening '{}': {}", url, err),
                            }
                        } 
                        if ui.button("Unlink").clicked() {

                        }
                    });
                });
            });
        });
    }

    fn open_about_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("About Telescope")
        .open(&mut self.open[0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Telescope");
                ui.label("Author: Rafael Amador Galván");
                ui.label("©2023");
                if ui.link("https://github.com/rafaga/telescope").clicked() {
                    let _a = open::that("https://github.com/rafaga/telescope");
                }
            });
        });
    }

    fn update_character_into_database(&mut self, player:Character) {
        if let Some(photo) = self.get_photo(player.id, player.photo.clone().unwrap()){
            self.portraits.insert(player.id,photo);
        }else {
            let _ = self.tx.send(Message::GenericError("Error retrieving player Image - Invalid format or file not found".to_string()));
        }
        self.esi.characters.push(player);
    }

    fn update_status_with_error(&mut self, message: String) {
        let _message_x = message;
    }

    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "Noto Sans Regular".to_owned(),
            FontData::from_static(include_bytes!("../assets/NotoSansTC-Regular.otf")),
        );
        fonts.families.get_mut(&FontFamily::Proportional).unwrap()
        .insert(0, "Noto Sans Regular".to_owned());
        cc.egui_ctx.set_fonts(fonts);
        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        let mut app:TemplateApp<'_> = Default::default();
        app.initialize_application();
        app
    }

    #[tokio::main]
    async fn get_photo(&mut self,player_id: u64, url: String) -> Option<RetainedImage> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let future = async move {
            if let Ok(Some(image)) = webb::esi::EsiManager::get_player_photo(url.as_str()).await {
                let _a = tx.send(image);
            };
        };
        self.tpool.spawn_ok(future);
        match rx.recv() {
            Ok(img) => {
                let resulting_image = RetainedImage::from_image_bytes(player_id.to_string(), img.as_slice());
                if let Err(t_error) = resulting_image {
                    let _rm = self.tx.send(Message::GenericError(t_error.to_string()));
                    return None;
                }
                return Some(resulting_image.unwrap());
            },
            Err(t_error) => {
                let _rm = self.tx.send(Message::GenericError(t_error.to_string()));
                return None;
            }
        }
    }
}
