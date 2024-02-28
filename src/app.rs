use crate::app::messages::{Message, Target, Type};
use data::AppData;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
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
pub struct TelescopeApp<'a> {
    #[serde(skip)]
    initialized: bool,

    // 2d point to paint map
    #[serde(skip)]
    points: Vec<MapPoint>,

    #[serde(skip)]
    map: Map,

    #[serde(skip)]
    tx: Arc<Sender<Message>>,

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

    search_text: String,
    emit_notification: bool,

    search_selected_row: Option<usize>,

    #[serde(skip)]
    search_results: Vec<(usize, String, usize, String)>,

    factor: u64,

    #[serde(skip)]
    path: String,
}

impl<'a> Default for TelescopeApp<'a> {
    fn default() -> Self {
        let (ntx, rx) = channel::<messages::Message>(10);
        let app_data = AppData::new();

        let tx = Arc::new(ntx);

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
            search_text: String::new(),
            search_selected_row: None,
            emit_notification: false,
            factor: 50000000000000,
            path: String::from("assets/sde.db"),
            search_results: Vec::new(),
        }
    }
}

impl<'a> eframe::App for TelescopeApp<'a> {
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
            search_text: _,
            emit_notification: _,
            factor: _,
            path: _,
            search_selected_row: _,
            search_results: _,
        } = self;

        if !self.initialized {
            #[cfg(feature = "puffin")]
            puffin::profile_scope!("telescope_init");

            egui_extras::install_image_loaders(ctx);

            let txs = Arc::clone(&self.tx);
            let str_path = self.path.clone();
            let factor_k = self.factor as i64;
            let future = async move {
                let t_sde = SdeManager::new(Path::new(str_path.as_str()), factor_k);
                if let Ok(points) = t_sde.get_systempoints(2) {
                    if let Ok(hash_map) = t_sde.get_connections(points, 2) {
                        let _result = txs.send(Message::ProcessedMapCoordinates(hash_map)).await;
                    }
                    //we add persistent connections
                    if let Ok(vec_lines) = t_sde.get_regional_connections() {
                        let _result = txs
                            .send(Message::ProcessedRegionalConnections(vec_lines))
                            .await;
                    }
                }
                if let Ok(region_areas) = t_sde.get_region_coordinates() {
                    let _result = txs.send(Message::RegionAreasLabels(region_areas)).await;
                }
            };
            self.tpool.spawn_ok(future);

            let mut vec_chars = Vec::new();
            for pchar in self.esi.characters.iter() {
                vec_chars.push((pchar.id, pchar.photo.as_ref().unwrap().clone()));
            }
            self.map.settings = MapSettings::default();
            self.map.settings.node_text_visibility = VisibilitySetting::Hover;
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
                            let sde = SdeManager::new(
                                Path::new(&self.path),
                                self.factor.try_into().unwrap(),
                            );
                            match sde.get_system_id(self.search_text.clone().to_lowercase()) {
                                Ok(system_results) => self.search_results = system_results,
                                Err(t_error) => {
                                    let txs = Arc::clone(&self.tx);
                                    let future = async move {
                                        let _ = txs
                                            .send(Message::GenericNotification((
                                                Type::Error,
                                                String::from("sde"),
                                                String::from("get_system_id"),
                                                t_error.to_string(),
                                            )))
                                            .await;
                                    };
                                    self.tpool.spawn_ok(future);
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
                                        ui.label(&self.search_results[row_index].1);
                                    });
                                    if col_data.1.clicked() {
                                        let txs = Arc::clone(&self.tx);
                                        let system_id = self.search_results[row_index].0;
                                        let emit_notification = self.emit_notification;
                                        let future = async move {
                                            let _result = txs
                                                .send(Message::CenterOn((
                                                    system_id,
                                                    Target::System,
                                                )))
                                                .await;
                                            if emit_notification {
                                                let _ = txs
                                                    .send(Message::SystemNotification(system_id))
                                                    .await;
                                            }
                                        };
                                        self.tpool.spawn_ok(future);
                                    }
                                    let col_data = row.col(|ui| {
                                        ui.label(&self.search_results[row_index].3);
                                    });
                                    if col_data.1.clicked() {
                                        let txs = Arc::clone(&self.tx);
                                        let region_id = self.search_results[row_index].2;
                                        let future = async move {
                                            let _result = txs
                                                .send(Message::CenterOn((
                                                    region_id,
                                                    Target::Region,
                                                )))
                                                .await;
                                        };
                                        self.tpool.spawn_ok(future);
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
    }
}

impl<'a> TelescopeApp<'a> {
    async fn event_manager(&mut self) {
        let received_data = self.rx.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                Message::ProcessedMapCoordinates(points) => self.map.add_hashmap_points(points),
                Message::ProcessedRegionalConnections(vec_lines) => self.map.add_lines(vec_lines),
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character).await
                }
                Message::GenericNotification(message) => self.update_status_with_error(message),
                Message::RegionAreasLabels(region_areas) => {
                    self.paint_map_region_labels(region_areas).await
                }
                Message::SystemNotification(message) => self.notification_on_map(message).await,
                Message::CenterOn(message) => self.center_on_target(message).await,
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
                    ui.allocate_ui(eframe::egui::Vec2::new(500.00, 150.00), |ui| {
                        eframe::egui::ScrollArea::new([false, true])
                            //.auto_shrink([true,false])
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    if !self.esi.characters.is_empty() {
                                        for char in &self.esi.characters {
                                            #[cfg(feature = "puffin")]
                                            puffin::profile_scope!("displaying character");

                                            ui.allocate_ui(eframe::egui::Vec2::new(300.00, 50.00), |ui| {
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
                                                                        Some(eframe::egui::Color32::YELLOW);
                                                                    //ui.style_mut().visuals.selection.bg_fill = Color32::LIGHT_GRAY;
                                                                    //ui.style_mut().visuals.fade_out_to_color();
                                                                }
                                                            }
                                                            if let Some(player_photo) = &char.photo
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
                                        ui.allocate_ui(eframe::egui::Vec2::new(300.00, 50.00), |ui| {
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
                    ui.allocate_ui(eframe::egui::Vec2::new(500.00, 150.00), |ui| {
                        #[cfg(feature = "puffin")]
                        puffin::profile_scope!("displaying character link buttons");

                        ui.vertical(|ui| {
                            if ui.button("Link new").clicked() {
                                let auth_info = self.esi.esi.get_authorize_url().unwrap();
                                match open::that(auth_info.authorization_url) {
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
                                                        .send(Message::GenericNotification(
                                                            (Type::Error,
                                                            String::from("EsiManager"),
                                                            String::from("launch_auth_server"),
                                                            t_error.to_string())
                                                        ))
                                                        .await;
                                                }
                                            };
                                        };
                                        self.tpool.spawn_ok(future);
                                    }
                                    Err(err) => {
                                        let tx = Arc::clone(&self.tx);
                                        let future = async move {
                                            let _ =
                                                tx.send(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("get_authorize_url"),err.to_string()))).await;
                                        };
                                        self.tpool.spawn_ok(future);
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
                                    let tx = Arc::clone(&self.tx);
                                    let future = async move {
                                        let _ =
                                            tx.send(Message::GenericNotification((Type::Error,String::from("EsiManager"),String::from("remove_characters"),t_error.to_string()))).await;
                                    };
                                    self.tpool.spawn_ok(future);
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

    async fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                let t_sde = SdeManager::new(Path::new(&self.path), self.factor as i64);
                if let Ok(Some(coords)) = t_sde.get_system_coords(message.0) {
                    self.map.set_pos(coords.0 as f32, coords.1 as f32);
                } else {
                    let stx = Arc::clone(&self.tx);
                    let mut msg = String::from("System with Id ");
                    msg += (message.0.to_string() + " could not be located").as_str();
                    let _ = stx
                        .send(Message::GenericNotification((
                            Type::Warning,
                            String::from("self.points Hashmap"),
                            String::from("get"),
                            msg,
                        )))
                        .await;
                }
            }
            Target::Region => {
                todo!();
            }
        }
    }

    async fn update_character_into_database(&mut self, response_data: (String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("update_character_into_database");

        let tx = Arc::clone(&self.tx);
        let auth_info = self.esi.esi.get_authorize_url().unwrap();
        match self.esi.auth_user(auth_info, response_data).await {
            Ok(Some(player)) => {
                self.esi.characters.push(player);
            }
            Ok(None) => {
                let _ = tx
                    .send(Message::GenericNotification((
                        Type::Info,
                        String::from("EsiManager"),
                        String::from("auth_user"),
                        String::from(
                            "Apparently thre was some kind of trouble authenticating the player.",
                        ),
                    )))
                    .await;
            }
            Err(t_error) => {
                let _ = tx
                    .send(Message::GenericNotification((
                        Type::Error,
                        String::from("EsiManager"),
                        String::from("auth_user"),
                        t_error.to_string(),
                    )))
                    .await;
            }
        };
    }

    fn update_status_with_error(&mut self, message: (Type, String, String, String)) {
        match message.0 {
            Type::Error => {
                self.last_message =
                    "Error on ".to_string() + &message.1 + " - " + &message.2 + " > " + &message.3;
            }
            Type::Warning => {
                self.last_message = "Warning on ".to_string()
                    + &message.1
                    + " - "
                    + &message.2
                    + " > "
                    + &message.3;
            }
            Type::Info => {}
        }
    }

    async fn notification_on_map(&mut self, message: usize) {
        let _result = self.map.notify(message);
    }

    async fn paint_map_region_labels(&mut self, region_areas: Vec<EveRegionArea>) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("paint_map_region_labels");

        let labels = Vec::new();
        for region in region_areas {
            let mut label = MapLabel::new();
            label.text = region.name;
            label.center = egui::Pos2::new(
                (region.min.x / self.factor as i64) as f32,
                (region.min.y / self.factor as i64) as f32,
            );
        }
        self.map.add_labels(labels)
    }

    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        cc.egui_ctx.set_fonts(fonts);

        let app: TelescopeApp<'_> = Default::default();
        app
    }
}
