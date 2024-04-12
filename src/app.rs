use crate::app::messages::{MapSync, Message, Target, Type};
use crate::app::tiles::{TabPane, TreeBehavior, UniversePane};
use data::AppData;
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use egui_map::map::objects::*;
use egui_tiles::{TileId, Tiles, Tree};
use futures::executor::ThreadPool;
use sde::{objects::Universe, SdeManager};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Builder;
use tokio::sync::broadcast::{self, Receiver as BCReceiver, Sender as BCSender};
use tokio::sync::mpsc::{self, Receiver, Sender};

use self::tiles::RegionPane;

pub mod data;
pub mod messages;
pub mod tiles;

pub struct TelescopeApp<'a> {
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
    esi: webb::esi::EsiManager<'a>,
    last_message: String,
    search_text: String,
    emit_notification: bool,
    search_selected_row: Option<usize>,
    search_results: Vec<(usize, String, usize, String)>,
    factor: u64,
    path: String,
    universe: Universe,
    selected_settings_option: usize,
    tpool: Rc<ThreadPool>,

    //tree: DockState<Tab>,
    tree: Option<Tree<Box<dyn TabPane>>>,
    tile_ids: HashMap<usize, (bool, Option<TileId>)>,
}

impl<'a> Default for TelescopeApp<'a> {
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
            Some("telescope.db"),
        );

        let mut tp_builder = ThreadPool::builder();
        tp_builder.name_prefix("telescope-");
        let tpool = Rc::new(tp_builder.create().unwrap());

        let factor = 50000000000000;
        let string_path = String::from("assets/sde.db");
        let path = string_path.clone();

        let mut sde = SdeManager::new(Path::new(&string_path), factor);
        let _ = sde.get_universe();

        Self {
            // Example stuff:
            initialized: false,
            points: Vec::new(),
            app_msg: (Arc::new(gtx), grx),
            map_msg: (Arc::new(mtx), mrx),
            open: [false; 3],
            esi,
            last_message: String::from("Starting..."),
            search_text: String::new(),
            search_selected_row: None,
            emit_notification: false,
            factor,
            path,
            tpool,
            search_results: Vec::new(),
            tree: None,
            tile_ids: HashMap::new(),
            universe: sde.universe,
            selected_settings_option: 0,
        }
    }
}

impl<'a> eframe::App for TelescopeApp<'a> {
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
        let runtime = Builder::new_multi_thread()
            .thread_name("tp-")
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
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
                tile_ids: _,
                universe: _,
                selected_settings_option: _,
                tpool: _,
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

                let regions: Vec<u32> = self.universe.regions.keys().copied().collect();
                for key in regions {
                    if key < 11000000 {
                        self.tile_ids.insert(key as usize, (false, None));
                    }
                }

                self.initialized = true;
            }

            self.event_manager().await;
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
                            if ui.button("Preferences").clicked() {
                                self.open[2] = true;
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
                                let sde = SdeManager::new(Path::new(&self.path), self.factor);
                                match sde.get_system_id(self.search_text.clone().to_lowercase()) {
                                    Ok(system_results) => self.search_results = system_results,
                                    Err(t_error) => {
                                        let txs = Arc::clone(&self.app_msg.0);
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

            if self.open[1] {
                self.open_character_window(ctx);
            }

            if self.open[2] {
                self.open_settings_window(ctx);
            }

            /*DockArea::new(&mut self.tree)
            .style(Style::from_egui(ctx.style().as_ref()))
            .show(ctx, &mut self.tab_viewer);*/

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
                    tree.ui(&mut TreeBehavior::default(), ui);
                }
            })

            //ui.add(&mut self.map);
            /*if let Some(points) = self.universe.points {

            }*/
            //ui.label("鑑於對人類家庭所有成員的固有尊嚴及其平等的和不移的權利的承認，乃是世界自由、正義與和平的基礎");
        });
    }
}

impl<'a> TelescopeApp<'a> {
    async fn event_manager(&mut self) {
        let received_data = self.app_msg.1.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                Message::EsiAuthSuccess(character) => {
                    self.update_character_into_database(character).await
                }
                Message::GenericNotification(message) => self.update_status_with_error(message),
                Message::RequestRegionName(region_id) => self.get_region_name(region_id),
                Message::ToggleRegionMap() => self.toggle_regions(),
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
                                        let tx = Arc::clone(&self.app_msg.0);
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
                                        let tx = Arc::clone(&self.app_msg.0);
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
                                    let tx = Arc::clone(&self.app_msg.0);
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

    fn open_settings_window(&mut self, ctx: &egui::Context) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("open_preferences_window");

        let t_univ = &self.universe;
        let filtered_keys: Vec<&u32> = t_univ
            .regions
            .keys()
            .filter(|key| key < &&11000000)
            .collect();
        egui::Window::new("Settings")
        .resizable(false)
        .fixed_size([600.0,500.0])
        .movable(true)
        .open(&mut self.open[2])
        .show(ctx, |ui| {
            ui.horizontal(|ui|{
                ui.vertical(|ui|{
                    let row_height = 25.0;
                    let labels = ["Maps","Linked Characters"];
                    ui.push_id("settings_menu", |ui|{
                        TableBuilder::new(ui)
                        .column(Column::resizable(Column::exact(150.0),false))
                        .striped(false)
                        .vscroll(false)
                        .body(|body| {
                            body.rows(row_height, labels.len(), |mut row| {
                                let label = labels[row.index()];
                                let current_index = row.index();
                                row.col(|ui: &mut egui::Ui|{
                                    let option_selected = || -> bool {
                                        self.selected_settings_option == current_index
                                    };
                                    if ui.selectable_label(option_selected(),label).clicked() {
                                        self.selected_settings_option = current_index;
                                    };
                                });
                            });
                        });
                    });
                    ui.add_space(300.0 - (labels.len() as f32 * row_height));
                });
                ui.push_id("settings_config", |ui|{
                    ui.vertical(|ui|{
                        egui::ScrollArea::vertical().show(ui,|ui|{
                            ui.label("By default the universe map its shown, and the regional maps where do you have linked characters, but you can override this setting marking the default regional maps to show on startup.").with_new_rect(ui.available_rect_before_wrap());
                            TableBuilder::new(ui)
                            .column(Column::resizable(Column::exact(150.0),false))
                            .column(Column::resizable(Column::exact(150.0),false))
                            .column(Column::resizable(Column::exact(150.0),false))
                            .striped(true)
                            .vscroll(false)
                            .body(|body| {
                                let row_height = 18.0;
                                let num_rows = filtered_keys.len().div_ceil(3);
                                body.rows(row_height, num_rows, |mut row| {
                                    let key_index = row.index() * 3;
                                    row.col(|ui: &mut egui::Ui| {
                                        let region = t_univ.regions.get(filtered_keys[key_index]).unwrap();
                                        if ui.checkbox(&mut self.tile_ids.get_mut(&(region.id as usize)).unwrap().0, region.name.clone()).changed() {
                                            let txs = Arc::clone(&self.app_msg.0);
                                            let future = async move {
                                                let _ = txs
                                                    .send(Message::ToggleRegionMap())
                                                    .await;
                                            };
                                            self.tpool.spawn_ok(future);
                                        };
                                    });
                                    let mut t_key_index = key_index + 1;
                                    if t_key_index < filtered_keys.len() {
                                        row.col(|ui: &mut egui::Ui| {
                                            let region = t_univ.regions.get(filtered_keys[t_key_index]).unwrap();
                                            if ui.checkbox(&mut self.tile_ids.get_mut(&(region.id as usize)).unwrap().0, region.name.clone()).changed() {
                                                let txs = Arc::clone(&self.app_msg.0);
                                                let future = async move {
                                                    let _ = txs
                                                        .send(Message::ToggleRegionMap())
                                                        .await;
                                                };
                                                self.tpool.spawn_ok(future);
                                            };
                                        });
                                    }
                                    t_key_index += 1;
                                    if t_key_index < filtered_keys.len() {
                                        row.col(|ui: &mut egui::Ui| {
                                            let region = t_univ.regions.get(filtered_keys[t_key_index]).unwrap();
                                            if ui.checkbox(&mut self.tile_ids.get_mut(&(region.id as usize)).unwrap().0, region.name.clone()).changed() {
                                                let txs = Arc::clone(&self.app_msg.0);
                                                let future = async move {
                                                    let _ = txs
                                                        .send(Message::ToggleRegionMap())
                                                        .await;
                                                };
                                                self.tpool.spawn_ok(future);
                                            };
                                        });
                                    }
                                });
                            });
                        });
                    });
                });
            });
            ui.horizontal(|ui|{
                ui.button("Save").clicked();
                if ui.button("Cancel").clicked(){
                }
            });
        });
    }

    async fn update_character_into_database(&mut self, response_data: (String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("update_character_into_database");
        let auth_info = self.esi.esi.get_authorize_url().unwrap();
        match self.esi.auth_user(auth_info, response_data).await {
            Ok(Some(player)) => {
                self.esi.characters.push(player);
            }
            Ok(None) => {
                let t_tx = Arc::clone(&self.app_msg.0);
                let future = async move {
                    let _ = t_tx
                        .send(Message::GenericNotification((
                            Type::Info,
                            String::from("EsiManager"),
                            String::from("auth_user"),
                            String::from(
                                "Apparently thre was some kind of trouble authenticating the player.",
                            ),
                        ))).await;
                };
                self.tpool.spawn_ok(future);
            }
            Err(t_error) => {
                let t_tx = Arc::clone(&self.app_msg.0);
                let future = async move {
                    let _ = t_tx
                        .send(Message::GenericNotification((
                            Type::Error,
                            String::from("EsiManager"),
                            String::from("auth_user"),
                            t_error.to_string(),
                        )))
                        .await;
                };
                self.tpool.spawn_ok(future);
            }
        };
    }

    fn update_status_with_error(&mut self, message: (Type, String, String, String)) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("update_status_with_error");
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

    fn toggle_regions(&mut self) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("toggle_regions");
        let mut new_panes = vec![];
        let mut show_panes = vec![];
        // tile.0 - has the region ID
        // tile.1.0 - has visible state
        // tile.1.1 - has the TileID assosiated with the shown Tile/tab (if exists)
        for tile in self.tile_ids.iter_mut() {
            if tile.1 .0 {
                if tile.1 .1.is_none() {
                    new_panes.push(*tile.0);
                }
            } else {
                show_panes.push(*tile.0);
            }
        }
        let mut tile_ids = self.tile_ids.clone();
        for region_id in new_panes {
            let pane = Self::generate_pane(
                self.map_msg.0.subscribe(),
                Arc::clone(&self.app_msg.0),
                self.path.clone(),
                self.factor,
                Some(region_id),
                Rc::clone(&self.tpool),
            );
            let tile_id = self.tree.as_mut().unwrap().tiles.insert_pane(pane);
            tile_ids.entry(region_id).and_modify(|data| {
                data.0 = true;
                data.1 = Some(tile_id);
            });
            //self.tree.as_mut().unwrap().set_visible(tile_id, true);
            //self.tree.as_mut().unwrap().tiles.insert_tab_tile(tile_id);
        }
        self.tile_ids = tile_ids;
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

        let app: TelescopeApp<'_> = Default::default();
        app
    }

    fn generate_pane(
        receiver: BCReceiver<MapSync>,
        generic_sender: Arc<Sender<Message>>,
        path: String,
        factor: u64,
        region_id: Option<usize>,
        t_pool: Rc<ThreadPool>,
    ) -> Box<dyn TabPane> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("generate_pane");
        let pane: Box<dyn TabPane> = if let Some(region) = region_id {
            Box::new(RegionPane::new(
                receiver,
                generic_sender,
                path,
                factor,
                region,
                t_pool,
            ))
        } else {
            Box::new(UniversePane::new(
                receiver,
                generic_sender,
                path,
                factor,
                t_pool,
            ))
        };
        pane
    }

    fn create_tree(&self) -> Tree<Box<dyn TabPane>> {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("create_tree");
        let mut tiles = Tiles::default();
        let id = tiles.insert_pane(Self::generate_pane(
            self.map_msg.0.subscribe(),
            Arc::clone(&self.app_msg.0),
            self.path.clone(),
            self.factor,
            None,
            Rc::clone(&self.tpool),
        ));
        let tile_ids = vec![id];
        let root = tiles.insert_tab_tile(tile_ids);
        egui_tiles::Tree::new("maps", root, tiles)
    }

    fn get_region_name(&self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("get_region_name");
        if let Some(region) = self.universe.regions.get(&(region_id as u32)) {
            let tx_map = Arc::clone(&self.map_msg.0);
            let _result = tx_map.send(MapSync::GetRegionName(region.name.clone()));
        }
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
