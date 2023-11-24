use crate::app::messages::Message;
use data::AppData;
use egui::{Color32, FontData, FontDefinitions, FontFamily, Image, Vec2};
use egui_map::map::{objects::*, Map};
use futures::executor::ThreadPool;
use sde::objects::EveRegionArea;
use sde::SdeManager;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};

pub mod data;
pub mod messages;

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
    tx: Arc<Box<Sender<Message>>>,

    #[serde(skip)]
    rx: Receiver<Message>,

    // these are the flags to open the windows
    // 0 - About Window
    // 1 - Character Window
    open: [bool; 2],

    // the ESI Manager
    #[serde(skip)]
    esi: webb::esi::EsiManager<'a>,

    #[serde(skip)]
    tpool: ThreadPool,

    //#[serde(skip)]
    last_message: String,
}

impl<'a> Default for TemplateApp<'a> {
    fn default() -> Self {
        let (ntx, rx) = channel::<messages::Message>(10);
        let app_data = AppData::new();

        let tx = Arc::new(Box::new(ntx));

        let esi = webb::esi::EsiManager::new(
            app_data.user_agent.as_str(),
            app_data.client_id.as_str(),
            app_data.secret_key.as_str(),
            app_data.url.as_str(),
            app_data.scope,
            Some("telescope.db"),
        );

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
            open: [false; 2],
            esi,
            tpool,
            last_message: String::from("Starting..."),
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
    #[tokio::main]
    async fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.event_manager().await;
        let Self {
            initialized: _,
            points: _points,
            map: _map,
            tx: _tx,
            rx: _rx,
            open: _,
            esi: _,
            tpool: _,
            last_message: _,
        } = self;

        if !self.initialized {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("telescope_init");

            egui_extras::install_image_loaders(ctx);
            let txs = self.tx.clone();
            let future = async move {
                let factor = 50000000000000;
                let path = Path::new("assets/sde.db");
                let manager = SdeManager::new(path, factor);
                if let Ok(points) = manager.get_systempoints(2) {
                    if let Ok(hash_map) = manager.get_connections(points, 2) {
                        let _result = txs.send(Message::ProcessedMapCoordinates(hash_map)).await;
                    }
                }
                if let Ok(region_areas) = manager.get_region_coordinates() {
                    let _result = txs.send(Message::RegionAreasLabels(region_areas)).await;
                }
            };
            self.tpool.spawn_ok(future);

            let mut vec_chars = Vec::new();
            for pchar in self.esi.characters.iter() {
                vec_chars.push((pchar.id, pchar.photo.as_ref().unwrap().clone()));
            }
            self.map.settings = MapSettings::default();
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
                ui.separator();
                ui.label(&self.last_message);
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
    fn initialize_application(&mut self) {}

    async fn event_manager(&mut self) {
        let received_data = self.rx.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                Message::ProcessedMapCoordinates(points) => self.map.add_hashmap_points(points),
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character).await
                }
                Message::EsiAuthError(message) => self.update_status_with_error(message),
                Message::GenericError(message) => self.update_status_with_error(message),
                Message::GenericWarning(message) => self.update_status_with_warning(message),
                Message::RegionAreasLabels(region_areas) => {
                    self.paint_map_region_labels(region_areas).await
                }
            };
        }
    }

    fn open_character_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Linked Characters")
            .open(&mut self.open[1])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui(Vec2::new(500.00, 150.00), |ui| {
                        egui::ScrollArea::new([false, true])
                            //.auto_shrink([true,false])
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    if !self.esi.characters.is_empty() {
                                        for char in &self.esi.characters {
                                            #[cfg(feature = "puffin")]
                                            puffin::profile_scope!("displaying character");

                                            ui.allocate_ui(Vec2::new(300.00, 50.00), |ui| {
                                                ui.group(|ui| {
                                                    ui.push_id(char.id, |ui| {
                                                        let inner = ui.horizontal_centered(|ui| {
                                                            //ui.radio_value(&mut self.esi.active_character, Some(char.id),"");
                                                            if let Some(idc) =
                                                                self.esi.active_character
                                                            {
                                                                if char.id == idc {
                                                                    ui.style_mut()
                                                                        .visuals
                                                                        .override_text_color =
                                                                        Some(Color32::YELLOW);
                                                                    //ui.style_mut().visuals.selection.bg_fill = Color32::LIGHT_GRAY;
                                                                    //ui.style_mut().visuals.fade_out_to_color();
                                                                }
                                                            }
                                                            if let Some(player_photo) = &char.photo
                                                            {
                                                                ui.add(
                                                                    Image::new(
                                                                        player_photo.as_str(),
                                                                    )
                                                                    .fit_to_exact_size(Vec2::new(
                                                                        80.0, 80.0,
                                                                    )),
                                                                );
                                                            }
                                                            ui.vertical(|ui| {
                                                                ui.horizontal(|ui| {
                                                                    //ui.image(char_photo, Vec2::new(16.0,16.0));
                                                                    ui.label("Name:");
                                                                    ui.label(&char.name);
                                                                });
                                                                ui.horizontal(|ui| {
                                                                    ui.label("Aliance:");
                                                                    if let Some(alliance) =
                                                                        char.alliance.as_ref()
                                                                    {
                                                                        ui.label(&alliance.name);
                                                                    } else {
                                                                        ui.label("No alliance");
                                                                    }
                                                                });
                                                                ui.horizontal(|ui| {
                                                                    ui.label("Corporation:");
                                                                    if let Some(corp) =
                                                                        char.corp.as_ref()
                                                                    {
                                                                        ui.label(&corp.name);
                                                                    } else {
                                                                        ui.label("No corporation");
                                                                    }
                                                                });
                                                                ui.horizontal(|ui| {
                                                                    ui.label("Last Logon:");
                                                                    ui.label(
                                                                        char.last_logon.to_string(),
                                                                    );
                                                                });
                                                            });
                                                        });
                                                        let response = inner
                                                            .response
                                                            .interact(egui::Sense::click());
                                                        if response.clicked() {
                                                            self.esi.active_character =
                                                                Some(char.id);
                                                        }
                                                    });
                                                });
                                            });
                                        }
                                    } else {
                                        ui.allocate_ui(Vec2::new(300.00, 50.00), |ui| {
                                            ui.group(|ui| {
                                                ui.vertical_centered(|ui| {
                                                    ui.label(
                                                        "No character has been linked, please",
                                                    );
                                                    ui.label("link a new Character to proceed.");
                                                });
                                            });
                                        });
                                    }
                                });
                            });
                    });
                    ui.separator();
                    ui.allocate_ui(Vec2::new(500.00, 150.00), |ui| {
                        #[cfg(feature = "puffin")]
                        puffin::profile_scope!("displaying character link buttons");

                        ui.vertical(|ui| {
                            if ui.button("Link new").clicked() {
                                let auth_info = self.esi.esi.get_authorize_url().unwrap();
                                match open::that(&auth_info.authorization_url) {
                                    Ok(()) => {
                                        let tx = Arc::clone(&self.tx);
                                        let future = async move {
                                            match webb::esi::EsiManager::launch_auth_server(56123) {
                                                Ok(data) => {
                                                    let _ = tx
                                                        .send(Message::EsiAuthSuccess(data))
                                                        .await;
                                                }
                                                Err(t_error) => {
                                                    let _ = tx
                                                        .send(Message::GenericError(
                                                            t_error.to_string(),
                                                        ))
                                                        .await;
                                                }
                                            };
                                        };
                                        self.tpool.spawn_ok(future);
                                    }
                                    Err(err) => {
                                        let _ =
                                            self.tx.send(Message::GenericError(err.to_string()));
                                    }
                                }
                            }
                            if ui.button("Unlink").clicked() {
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
                                    let _ =
                                        self.tx.send(Message::GenericError(t_error.to_string()));
                                }
                            }
                        });
                    });
                });
            });
    }

    fn open_about_window(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("open_about_window");

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
                    egui::warn_if_debug_build(ui);
                });
            });
    }

    async fn update_character_into_database(&mut self, response_data: (String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("update_character_into_database");

        let tx = Arc::clone(&self.tx);
        let auth_info = self.esi.esi.get_authorize_url().unwrap();
        match self.esi.auth_user(auth_info,response_data).await {
            Ok(Some(player)) => {
                self.esi.characters.push(player);
            }
            Ok(None) => {
                let _ = tx
                    .send(Message::GenericWarning(
                        "There was some error authenticating the player.".to_string(),
                    ))
                    .await;
            }
            Err(t_error) => {
                let _ = tx.send(Message::GenericError(t_error.to_string())).await;
            }
        };
    }

    fn update_status_with_error(&mut self, message: String) {
        self.last_message = "Error: ".to_string() + &message;
    }

    fn update_status_with_warning(&mut self, message: String) {
        self.last_message = "Warning: ".to_string() + &message;
    }

    async fn paint_map_region_labels(&mut self, region_areas: Vec<EveRegionArea>) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("paint_map_region_labels");

        let labels = Vec::new();
        for region in region_areas {
            let mut label = MapLabel::new();
            label.text = region.name;
            label.center = egui::Pos2::new(
                (region.min.x / 50000000000000) as f32,
                (region.min.y / 50000000000000) as f32,
            );
        }
        self.map.add_labels(labels)
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
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "Noto Sans Regular".to_owned());
        cc.egui_ctx.set_fonts(fonts);
        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        let mut app: TemplateApp<'_> = Default::default();
        app.initialize_application();
        app
    }
}
