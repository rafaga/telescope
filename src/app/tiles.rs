use crate::app::messages::{MapSync, Message, Target, Type};
use eframe::egui::{
    self, epaint::CircleShape, vec2, Align2, Color32, FontId, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Style, TextStyle, Ui, Vec2, WidgetText
};
use egui_extras::{Column, TableBuilder};
use egui_map::map::{
    objects::{
        ContextMenuManager, MapLabel, MapPoint, MapSettings, NodeTemplate, VisibilitySetting,
    },
    Map,
};
use egui_tiles::{Behavior, SimplificationOptions, TileId, Tiles, UiResponse};
//use futures::executor::ThreadPool;
use sde::SdeManager;
use std::collections::HashMap;
use std::time::Instant;
use std::{path::Path, rc::Rc, sync::Arc};
use tokio::sync::broadcast::Receiver;

use super::messages::MessageSpawner;

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
    //generic_sender: Arc<Sender<Message>>,
    path: String,
    factor: i64,
    task_msg: Arc<MessageSpawner>,
    //tpool: Rc<ThreadPool>,
}

impl UniversePane {
    pub fn new(
        receiver: Receiver<MapSync>,
        path: String,
        factor: i64,
        task_msg: Arc<MessageSpawner>,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor: factor as i64,
            task_msg,
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
        self.event_manager();
        ui.add(&mut self.map);
        UiResponse::None
    }

    fn get_title(&self) -> WidgetText {
        "Universe".into()
    }

    fn event_manager(&mut self) {
        let received_data = self.mapsync_reciever.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                MapSync::SystemNotification((system_id,time)) => {
                    let _result = self.map.notify(system_id, time.into());
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
                MapSync::PlayerMoved((player_id, location)) => {
                    self.map.update_marker(player_id, location)
                }
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
                        self.task_msg.spawn(Message::GenericNotification((
                            Type::Warning,
                            String::from("SdeManager"),
                            String::from("get_system_coords"),
                            msg,
                        )));
                    }
                    Err(t_error) => {
                        self.task_msg.spawn(Message::GenericNotification((
                            Type::Error,
                            String::from("SdeManager"),
                            String::from("get_system_coords"),
                            t_error.to_string(),
                        )));
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
    path: String,
    factor: i64,
    region_id: usize,
    tab_name: String,
    task_msg: Arc<MessageSpawner>,
}

impl RegionPane {
    pub fn new(
        receiver: Receiver<MapSync>,
        path: String,
        factor: i64,
        region_id: usize,
        task_msg: Arc<MessageSpawner>,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor: factor as i64,
            region_id,
            tab_name: String::from("Region"),
            task_msg,
        };
        object.generate_data(object.path.clone(), object.factor, object.region_id);
        object.map.settings = MapSettings::default();
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.set_context_manager(Rc::new(ContextMenu::new()));
        object.map.set_node_template(Rc::new(Template::new()));
        object
    }

    fn generate_data(&mut self, path: String, factor: i64, region_id: usize) {
        let t_sde = SdeManager::new(Path::new(path.as_str()), factor.try_into().unwrap());

        match t_sde.get_abstract_systems(vec![region_id as u32]) {
            Ok(points) => {
                if let Ok(points) =
                    t_sde.get_abstract_system_connections(points, vec![region_id as u32])
                {
                    self.map.add_hashmap_points(points);
                }
                if let Ok(lines) = t_sde.get_abstract_connections(vec![region_id as u32]) {
                    self.map.add_lines(lines);
                }
            }
            Err(t_err) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    "RegionPane".to_string(),
                    "generate_data".to_string(),
                    t_err.to_string(),
                )));
                return;
            }
        }
        let t_region_id = self.region_id;
        let region = t_sde.get_region(vec![t_region_id as u32], None).unwrap();
        let keys: Vec<u32> = region.keys().copied().collect();
        self.tab_name
            .clone_from(&region.get(&keys[0]).unwrap().name);
    }
}

impl TabPane for RegionPane {
    fn event_manager(&mut self) {
        let received_data = self.mapsync_reciever.try_recv();
        if let Ok(msg) = received_data {
            match msg {
                MapSync::SystemNotification((system_id,time)) => {
                    let _result = self.map.notify(system_id, time.into());
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
                MapSync::PlayerMoved((player_id, location)) => {
                    self.map.update_marker(player_id, location)
                }
            };
        }
    }

    fn get_title(&self) -> WidgetText {
        self.tab_name.clone().into()
    }

    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.event_manager();
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

pub struct TileData {
    tile_id: Option<TileId>,
    name: String,
    visible: bool,
    pub(crate) show_on_startup: bool,
}

impl TileData {
    pub fn new(name: String, show_on_startup: bool) -> Self {
        Self {
            tile_id: None,
            name,
            visible: false,
            show_on_startup,
        }
    }

    pub fn set_visible(&mut self, value: bool) {
        self.visible = value;
    }

    pub fn get_visible(&self) -> bool {
        self.visible
    }

    pub fn set_tile_id(&mut self, value: Option<TileId>) {
        self.tile_id = value;
    }

    pub fn get_tile_id(&self) -> Option<TileId> {
        self.tile_id
    }

    pub fn get_name(&self) -> String {
        self.name.clone()
    }
}

pub struct TreeBehavior {
    simplification_options: SimplificationOptions,
    tab_bar_height: f32,
    gap_width: f32,
    task_msg: Arc<MessageSpawner>,
    search_text: String,
    factor: i64,
    path: String,
    search_regions: Vec<usize>,
    pub tile_data: HashMap<usize, TileData>,
}

impl TreeBehavior {
    pub fn new(task_msg: Arc<MessageSpawner>, factor: i64, path: String) -> Self {
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
            task_msg,
            factor,
            path,
            search_text: String::new(),
            tile_data: HashMap::new(),
            search_regions: Vec::new(),
        }
    }

    fn toggle_regions(&mut self, region_id: usize) {
        #[cfg(feature = "puffin")]
        puffin::profile_scope!("toggle_regions");
        let visible = self.tile_data.get_mut(&region_id).unwrap().get_visible();
        let tile_id = self.tile_data.get_mut(&region_id).unwrap().get_tile_id();
        //let ttx = Arc::clone(&self.generic_sender);
        if visible {
            if tile_id.is_some() {
                self.task_msg.spawn(Message::MapShown(region_id));
            } else {
                self.task_msg.spawn(Message::NewRegionalPane(region_id));
            }
        } else {
            self.task_msg.spawn(Message::MapHidden(region_id));
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

    fn on_close_tab(&self, tile_id: TileId, button_response: Response) {
        if button_response.clicked() {
            for tile in self.tile_data.iter() {
                if tile.1.get_tile_id() == Some(tile_id) {
                    //let ttx = Arc::clone(&self.generic_sender);
                    let region_id = *tile.0;
                    /*let future = async move {
                        let _x = ttx.send(Message::MapHidden(region_id)).await;
                    };
                    self.tpool.spawn_ok(future);*/
                    self.task_msg.spawn(Message::MapHidden(region_id));
                }
            }
        }
    }
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

    fn tab_ui(
        &mut self,
        tiles: &Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        id: eframe::egui::Id,
        tile_id: TileId,
        active: bool,
        is_being_dragged: bool,
    ) -> eframe::egui::Response {
        let text = self.tab_title_for_tile(tiles, tile_id);
        let str_text = text.text().to_string().clone();
        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = text.into_galley(ui, Some(false), f32::INFINITY, font_id);

        // this is for close button
        let nid = egui::Id::new(str_text.clone());

        let x_margin = self.tab_title_spacing(ui.visuals());
        let mut rect_close = None;
        let rect = ui
            .allocate_space(vec2(
                galley.size().x + 2.0 * x_margin,
                ui.available_height(),
            ))
            .1;
        if str_text != "Universe" && !is_being_dragged {
            rect_close = Some(
                ui.allocate_space(vec2(2.0 * x_margin, ui.available_height()))
                    .1,
            );
        }
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let mut close_response = None;
        if let Some(rect2) = rect_close {
            close_response = Some(ui.interact(rect2, nid, Sense::click()));
        }

        let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, active);
        // Show a gap when dragged
        if ui.is_rect_visible(rect) && !is_being_dragged {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, active);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, active);
            ui.painter().rect(rect.shrink(0.5), 0.0, bg_color, stroke);

            if active {
                // Make the tab name area connect with the tab ui area:
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom(),
                    Stroke::new(stroke.width + 1.0, bg_color),
                );
            }
            ui.painter().galley(
                egui::Align2::CENTER_CENTER
                    .align_size_within_rect(galley.size(), rect)
                    .min,
                galley,
                text_color,
            );
        }

        if rect_close.is_some() {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, active);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, active);
            ui.painter()
                .rect(rect_close.unwrap().shrink(0.5), 0.0, bg_color, stroke);
            if ui.is_rect_visible(rect_close.unwrap()) {
                let a = WidgetText::from(String::from("×")).into_galley(
                    ui,
                    Some(false),
                    f32::INFINITY,
                    TextStyle::Button.resolve(ui.style()),
                );
                let pos = egui::Align2::CENTER_CENTER
                    .align_size_within_rect(a.size(), rect_close.unwrap())
                    .min;
                ui.painter().galley(pos, a, text_color);
                self.on_close_tab(tile_id, close_response.unwrap());
            }
        }

        self.on_tab_button(tiles, tile_id, response)
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
        ui.menu_button("➕", |ui| {
            let mut _data: Vec<usize> = Vec::new();
            ui.label("Search region:");
            if ui.text_edit_singleline(&mut self.search_text).changed() {
                self.search_regions.clear();
                if self.search_text.len() > 3 {
                    let t_sde = SdeManager::new(Path::new(&self.path), self.factor);
                    let regions = t_sde
                        .get_region(vec![], Some(self.search_text.clone()))
                        .unwrap();
                    self.search_regions = regions.keys().copied().map(|x| x as usize).collect();
                }
            }

            if self.search_regions.is_empty() {
                _data = self.tile_data.keys().copied().collect();
            } else {
                _data.clone_from(&self.search_regions);
            }
            ui.add_space(7.0);
            TableBuilder::new(ui)
                .column(Column::remainder())
                .striped(true)
                .vscroll(true)
                .max_scroll_height(100.00)
                .body(|body| {
                    body.rows(25.0, _data.len(), |mut row| {
                        let key_index = row.index();
                        let name = self
                            .tile_data
                            .get_mut(&(_data[key_index]))
                            .unwrap()
                            .get_name();
                        row.col(|ui: &mut egui::Ui| {
                            if ui
                                .checkbox(
                                    &mut self
                                        .tile_data
                                        .get_mut(&(_data[key_index]))
                                        .unwrap()
                                        .visible,
                                    name,
                                )
                                .changed()
                            {
                                self.toggle_regions(_data[key_index]);
                            };
                        });
                    });
                });
        });
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

struct Template {
}

impl Template {
    fn new() -> Self {
        Self {
        }
    }
}

impl NodeTemplate for Template {
    fn node_ui(&self, ui: &mut Ui, viewport_point: Pos2, zoom: f32, system: &MapPoint) {
        let mut shapes = Vec::new();
        let mut colors: (Color32, Color32) = (ui.visuals().extreme_bg_color, Color32::TRANSPARENT);
        let rect = Rect::from_center_size(viewport_point, Vec2::new(90.0 * zoom, 35.0 * zoom));
        colors.1 = if ui.visuals().dark_mode {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        shapes.push(Shape::rect_stroke(
            rect,
            Rounding::same(10.0 * zoom),
            Stroke::new(4.0 * zoom, colors.1),
        ));
        shapes.push(Shape::rect_filled(rect, Rounding::same(10.0 * zoom), colors.0));
        ui.ctx().fonts(|fonts|{
            shapes.push(Shape::text(
                fonts,
                viewport_point,
                Align2::CENTER_CENTER,
                system.get_name(),
                FontId::proportional(12.0 * zoom),
                colors.1,
            ));
        });
        ui.painter().extend(shapes);
    }

    fn selection_ui(&self, ui: &mut Ui, viewport_point: Pos2, zoom:f32) {
        let mut shapes = Vec::new();
        let rect = Rect::from_center_size(viewport_point, Vec2::new(94.0 * zoom, 39.0 * zoom));
        let color = if ui.visuals().dark_mode {
            Color32::YELLOW
        } else {
            Color32::KHAKI
        };
        shapes.push(Shape::rect_stroke(
            rect,
            Rounding::same(10.0 * zoom),
            Stroke::new(3.0 * zoom, color)
        ));
        ui.painter().extend(shapes);
    }

    fn marker_ui(&self, ui: &mut Ui, viewport_point: Pos2, zoom:f32) {
        let mut shapes = Vec::new();
        let led_position = Pos2::new(viewport_point.x + (45.0 * zoom), viewport_point.y - (17.0 * zoom));
        let color = if ui.visuals().dark_mode {
            Color32::LIGHT_GREEN
        } else {
            Color32::GREEN
        };
        let mut transparency = (chrono::Local::now().timestamp_millis() % 2550) / 5;
        if transparency > 255 {
            transparency = 255 - (transparency - 255)
        }
        let corrected_color = Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            transparency as u8,
        );
        let border_color = if ui.visuals().dark_mode {
            Color32::WHITE
        } else {
            Color32::BLACK
        };
        shapes.push(Shape::Circle(CircleShape::stroke(led_position, 6.5 * zoom, Stroke::new(4.0 * zoom, border_color))));
        shapes.push(Shape::Circle(CircleShape::filled(led_position, 6.0 * zoom, ui.visuals().extreme_bg_color)));
        shapes.push(Shape::Circle(CircleShape::filled(led_position, 6.0 * zoom, corrected_color)));
        ui.ctx().request_repaint();
        ui.painter().extend(shapes);
    }

    fn notification_ui(&self, ui: &mut Ui, viewport_point: Pos2, zoom:f32, initial_time: Instant, color: Color32) -> bool {
        let mut shapes = Vec::new();
        let current_instant = Instant::now();
        let time_diff = current_instant.duration_since(initial_time);
        let secs_played = time_diff.as_secs_f32();
        let mut transparency:f32 = 1.00 - (secs_played / 2.00).abs();
        if transparency < 0.00 {
            transparency = 0.00;
        }
        let corrected_color = Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (255.00 * transparency).round() as u8,
        );
        let rect = Rect::from_center_size(viewport_point, Vec2::new(90.0 * zoom, 35.0 * zoom));
        shapes.push(Shape::rect_stroke(
            rect,
            Rounding::same(10.0 * zoom),
            Stroke::new((4.00 + (25.00 * secs_played)) * zoom, corrected_color)
        ));
        ui.painter().extend(shapes);
        ui.ctx().request_repaint();
        if secs_played < 2.00 {
            return true;
        }
        false
    }
}
