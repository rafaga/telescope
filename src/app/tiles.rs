use crate::app::messages::{MapSync, Message, Target, Type};
use eframe::egui::{Pos2, Style, Ui, WidgetText};
use egui_map::map::{
    objects::{ContextMenuManager, MapLabel, MapSettings, VisibilitySetting},
    Map,
};
use egui_tiles::{Behavior, SimplificationOptions, TileId, Tiles, UiResponse};
use futures::executor::ThreadPool;
use sde::SdeManager;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::Sender;
// use eframe::egui::include_image;

pub trait TabPane {
    fn ui(&mut self, ui: &mut Ui) -> UiResponse;
    fn get_title(&self) -> WidgetText;
    fn event_manager(&mut self);
    fn center_on_target(&mut self, message: (usize, Target));
}

pub struct UniversePane {
    map: Map,
    mapsync_reciever: Receiver<MapSync>,
    generic_sender: Arc<Sender<Message>>,
    path: String,
    factor: i64,
    tpool: Rc<ThreadPool>,
}

impl UniversePane {
    pub fn new(
        receiver: Receiver<MapSync>,
        generic_sender: Arc<Sender<Message>>,
        path: String,
        factor: u64,
        thread_pool: Rc<ThreadPool>,
    ) -> Self {
        /*let mut tp_builder = ThreadPool::builder();
        tp_builder.name_prefix("univ-");
        let tpool = tp_builder.create().unwrap();*/

        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            generic_sender,
            path,
            factor: factor as i64,
            tpool: thread_pool,
        };
        object.generate_data(object.path.clone(), object.factor);
        object.map.settings = MapSettings::default();
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.set_context_manager(Rc::new(ContextMenu::new()));
        object
    }

    fn generate_data(&mut self, path: String, factor: i64) {
        let t_sde = SdeManager::new(Path::new(path.as_str()), factor.try_into().unwrap());
        if let Ok(points) = t_sde.get_systempoints() {
            //we get connections
            if let Ok(hashmap) = t_sde.get_system_connections(points) {
                self.map.add_hashmap_points(hashmap);
            }

            if let Ok(hash_conns) = t_sde.get_connections() {
                self.map.add_lines(hash_conns);
            }
        }
        let t_sde = SdeManager::new(Path::new(path.as_str()), factor.try_into().unwrap());
        if let Ok(region_areas) = t_sde.get_region_coordinates() {
            let mut labels = Vec::new();
            for region in region_areas {
                let mut label = MapLabel::new();
                label.text = region.name;
                label.center = Pos2::new(
                    (region.min.x / factor) as f32,
                    (region.min.y / factor) as f32,
                );
                labels.push(label);
            }
            self.map.add_labels(labels);
        }
    }
}

impl TabPane for UniversePane {
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        ui.add(&mut self.map);
        self.event_manager();
        UiResponse::None
        /*let dragged = ui
            .allocate_rect(ui.max_rect(), Sense::drag())
            .on_hover_cursor(CursorIcon::Grab)
            .dragged();
        if dragged {
            UiResponse::DragStarted
        } else {
            UiResponse::None
        }*/
    }

    fn get_title(&self) -> WidgetText {
        "Universe".into()
    }

    fn event_manager(&mut self) {
        let received_data = self.mapsync_reciever.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                MapSync::SystemNotification(system_id) => {
                    let _result = self.map.notify(system_id);
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
                MapSync::GetRegionName(_) => (),
            };
        }
    }

    fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                let t_sde = SdeManager::new(Path::new(&self.path), self.factor.try_into().unwrap());
                match t_sde.get_system_coords(message.0) {
                    Ok(Some(coords)) => {
                        self.map.set_pos(coords.try_into().unwrap());
                    }
                    Ok(None) => {
                        let mut msg = String::from("System with Id ");
                        msg += (message.0.to_string() + " could not be located").as_str();
                        let gtx = Arc::clone(&self.generic_sender);
                        let future = async move {
                            let _result = gtx
                                .send(Message::GenericNotification((
                                    Type::Warning,
                                    String::from("SdeManager"),
                                    String::from("get_system_coords"),
                                    msg,
                                )))
                                .await;
                        };
                        self.tpool.spawn_ok(future);
                    }
                    Err(t_error) => {
                        let gtx = Arc::clone(&self.generic_sender);
                        let future = async move {
                            let _result = gtx
                                .send(Message::GenericNotification((
                                    Type::Error,
                                    String::from("SdeManager"),
                                    String::from("get_system_coords"),
                                    t_error.to_string(),
                                )))
                                .await;
                        };
                        self.tpool.spawn_ok(future);
                    }
                };
            }
            Target::Region => {}
        }
    }
}

pub struct RegionPane {
    map: Map,
    mapsync_reciever: Receiver<MapSync>,
    generic_sender: Arc<Sender<Message>>,
    path: String,
    factor: i64,
    region_id: usize,
    tab_name: String,
    tpool: Rc<ThreadPool>,
}

impl RegionPane {
    pub fn new(
        receiver: Receiver<MapSync>,
        generic_sender: Arc<Sender<Message>>,
        path: String,
        factor: u64,
        region_id: usize,
        thread_pool: Rc<ThreadPool>,
    ) -> Self {
        //let mut tp_builder = ThreadPool::builder();
        /*let mut thread_prefix = String::from("rg-");
        thread_prefix += &(region_id.to_string().as_str().to_owned() + &"-".to_string());
        tp_builder.name_prefix(thread_prefix);
        let tpool = tp_builder.create().unwrap();*/
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            generic_sender,
            path,
            factor: factor as i64,
            region_id,
            tab_name: String::from("Region"),
            tpool: thread_pool,
        };
        object.generate_data(object.path.clone(), object.factor, object.region_id);
        object.map.settings = MapSettings::default();
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.set_context_manager(Rc::new(ContextMenu::new()));
        object
    }

    fn generate_data(&mut self, path: String, factor: i64, region_id: usize) {
        let t_sde = SdeManager::new(Path::new(path.as_str()), factor.try_into().unwrap());
        if let Ok(points) = t_sde.get_abstract_systems(vec![region_id as u32]) {
            if let Ok(points) =
                t_sde.get_abstract_system_connections(points, vec![region_id as u32])
            {
                self.map.add_hashmap_points(points);
            }
            if let Ok(lines) = t_sde.get_abstract_connections(vec![region_id as u32]) {
                self.map.add_lines(lines);
            }
        }
        let txs = Arc::clone(&self.generic_sender);
        let t_region_id = self.region_id;
        let future = async move {
            let _ = txs.send(Message::RequestRegionName(t_region_id)).await;
        };
        self.tpool.spawn_ok(future);
    }
}

impl TabPane for RegionPane {
    fn event_manager(&mut self) {
        let received_data = self.mapsync_reciever.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                MapSync::SystemNotification(system_id) => {
                    let _result = self.map.notify(system_id);
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
                MapSync::GetRegionName(region_name) => {
                    self.tab_name = region_name;
                }
            };
        }
    }

    fn get_title(&self) -> WidgetText {
        self.tab_name.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        ui.add(&mut self.map);
        UiResponse::None
    }

    fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                self.map.set_pos_from_nodeid(message.0);
            }
            Target::Region => {}
        }
    }
}

pub struct TreeBehavior {
    simplification_options: SimplificationOptions,
    tab_bar_height: f32,
    gap_width: f32,
}

impl Default for TreeBehavior {
    fn default() -> Self {
        Self {
            simplification_options: SimplificationOptions {
                prune_empty_containers: true,
                prune_single_child_containers: true,
                prune_empty_tabs: true,
                prune_single_child_tabs: false,
                all_panes_must_have_tabs: false,
                join_nested_linear_containers: true,
            },
            tab_bar_height: 24.0,
            gap_width: 2.0,
        }
    }
}

impl TreeBehavior {
    /*fn ui(&mut self, ui: &mut Ui) {
        let Self {
            simplification_options,
            tab_bar_height,
            gap_width,
        } = self;

        Grid::new("behavior_ui").num_columns(2).show(ui, |ui| {
            ui.label("All panes must have tabs:");
            ui.checkbox(&mut simplification_options.all_panes_must_have_tabs, "");
            ui.end_row();

            ui.label("Join nested containers:");
            ui.checkbox(
                &mut simplification_options.join_nested_linear_containers,
                "",
            );
            ui.end_row();

            ui.label("Tab bar height:");
            ui.add(
                DragValue::new(tab_bar_height)
                    .clamp_range(0.0..=100.0)
                    .speed(1.0),
            );
            ui.end_row();

            ui.label("Gap width:");
            ui.add(DragValue::new(gap_width).clamp_range(0.0..=20.0).speed(1.0));
            ui.end_row();
        });
    }*/
}

impl Behavior<Box<dyn TabPane>> for TreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        view: &mut Box<dyn TabPane>,
    ) -> UiResponse {
        view.ui(ui)
    }

    fn tab_title_for_pane(&mut self, view: &Box<dyn TabPane>) -> WidgetText {
        view.get_title()
    }

    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        ui.add_space(1.5);
        /* let img = include_image!("../../assets/layout-board.png");
        ui.menu_image_button(img, |ui| {
            ui.menu_button("My sub-menu", |ui| {
                if ui.button("Close the menu").clicked() {
                    ui.close_menu();
                }
            });
        }); */
        /*if ui.button("➕").clicked() {
            self.add_child_to = Some(tile_id);
        }*/
    }

    // ---
    // Settings:

    fn tab_bar_height(&self, _style: &Style) -> f32 {
        self.tab_bar_height
    }

    fn gap_width(&self, _style: &Style) -> f32 {
        self.gap_width
    }

    fn simplification_options(&self) -> SimplificationOptions {
        self.simplification_options
    }
    /*fn tab_bg_color(
            &self,
            visuals: &eframe::egui::Visuals,
            _tiles: &Tiles<Pane>,
            _tile_id: TileId,
            active: bool,
        ) -> Color32 {
        if visuals.dark_mode {
            if active {
                Color32::from_rgba_unmultiplied(12, 14, 16, 100)
            } else {
                Color32::from_rgba_unmultiplied(50, 60, 70, 100)
            }
        } else {
            if active {
                Color32::from_rgba_unmultiplied(12, 14, 16, 100)
            } else {
                Color32::from_rgba_unmultiplied(50, 60, 70, 100)
            }
        }
    }*/
}

struct ContextMenu {}

impl ContextMenu {
    fn new() -> Self {
        Self {}
    }
}

impl ContextMenuManager for ContextMenu {
    fn ui(&self, ui: &mut Ui) {
        if ui.button("set beacon").clicked() {
            ui.close_menu();
        }
        ui.separator();
        if ui.button("⚙ settings").clicked() {
            ui.close_menu();
        }
    }
}
