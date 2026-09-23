//! The map area of the main window, built on `egui_tiles`: `UniversePane` (the
//! whole universe) and `RegionPane` (a single region), both implementing
//! [`TabPane`], and `TreeBehavior`, which lays out the tabs and keeps the
//! per-region [`TileData`] (visibility, show on start-up).
//!
//! It also bridges the `sde` map points and connections into the types `egui-map`
//! renders.

use crate::app::messages::{MapSync, Message, Target, Type};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Style,
    TextStyle, TextWrapMode, Ui, Vec2, WidgetText, epaint::CircleShape, text::Galley, vec2,
};
use egui::PopupCloseBehavior;
use egui::containers::menu::{MenuButton, MenuConfig};
use egui_extras::{Column, TableBuilder};
use egui_map::map::{
    Map,
    objects::{
        ContextMenuManager, MapPoint, MapSegment, MapSettings, MarkerContext, NodeContext,
        NodeTemplate, NotificationContext, RegionLabel, SelectionContext, VisibilitySetting,
    },
};
use egui_tiles::{Behavior, SimplificationOptions, TabState, TileId, Tiles, UiResponse};
//use futures::executor::ThreadPool;
use sde::SdeManager;
use sde::objects::{ProjectedAxis, SdePoint, SdeSegment};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use tokio::sync::broadcast::Receiver;

use super::messages::MessageSpawner;

/// Bridges a single `sde` map point (native type, as of the `objects`
/// rewrite `sde` no longer depends on `egui-map` at all) into `egui-map`'s
/// own `MapPoint`, which the widget actually renders. Factored out of
/// [`sde_points_to_map`] so the per-point conversion logic has one home.
fn sde_point_to_map(id: usize, point: SdePoint) -> MapPoint {
    // `get_systems`/`get_abstract_systems` only ever fill the first two
    // components (`coords[2]` stays `0.0`), so dropping Z is the correct
    // 2D projection here.
    let coords = point.to_2d(ProjectedAxis::Z);
    let mut map_point = MapPoint::new(id, coords);
    if let Some(name) = point.name {
        map_point.set_name(name);
    }
    // `MapPoint::connections` is now `Vec<(usize, usize)>`, matching
    // `SdePoint::connections` exactly, so this is a plain move — no more
    // `format!`-based id synthesis needed.
    map_point.connections = point.connections;
    // `SdePoint::color` is the star's raw hex color (`sde` has no
    // rendering dependency, so it can't hand us a `Color32` directly);
    // parse it here, where it's actually needed. A missing star, or a
    // malformed hex value, both just fall back to `MapPoint`'s default
    // (`None`), which keeps the map's single default node color.
    map_point.color = point
        .color
        .as_deref()
        .and_then(|hex| Color32::from_hex(hex).ok());
    map_point
}

/// `HashMap<usize, SdePoint>` -> `HashMap<usize, MapPoint>`, for
/// [`Map::add_hashmap_points`] (used by both `UniversePane` and
/// `RegionPane`). Single pass; `collect()` into a `HashMap` from a
/// `HashMap`'s `into_iter()` already reserves capacity up front from the
/// exact `size_hint`, so there's no repeated reallocation as entries are
/// inserted.
fn sde_points_to_map(points: HashMap<usize, SdePoint>) -> HashMap<usize, MapPoint> {
    points
        .into_iter()
        .map(|(id, point)| (id, sde_point_to_map(id, point)))
        .collect()
}

/// Same idea as [`sde_point_to_map`], for `get_connections`/
/// `get_abstract_connections`. `MapSegment::id` is now `(usize, usize)`,
/// same as `SdeSegment::id` and the `HashMap` key itself, so `id` carries
/// straight across with no `Rc<str>`/`format!` synthesis.
fn sde_segment_to_map(id: (usize, usize), segment: SdeSegment) -> MapSegment {
    let point1 = [segment.point1[0] as f32, segment.point1[1] as f32];
    let point2 = [segment.point2[0] as f32, segment.point2[1] as f32];
    MapSegment::new(id, point1, point2)
}

/// `HashMap<(usize, usize), SdeSegment>` -> `HashMap<(usize, usize),
/// MapSegment>`, for [`Map::add_hashmap_lines`] (used by both
/// `UniversePane` and `RegionPane`).
fn sde_segments_to_map(
    segments: HashMap<(usize, usize), SdeSegment>,
) -> HashMap<(usize, usize), MapSegment> {
    segments
        .into_iter()
        .map(|(id, segment)| (id, sde_segment_to_map(id, segment)))
        .collect()
}

// use eframe::egui::include_image;
pub trait TabPane {
    fn ui(&mut self, ui: &mut Ui) -> UiResponse;
    fn get_title(&self) -> WidgetText;
    fn event_manager(&mut self);
    fn center_on_target(&mut self, message: (usize, Target));
    /// Places (or moves) the marker of a linked character on its solar
    /// system. Called directly by the app for every pane, visible or not --
    /// see `TelescopeApp::update_player_location`.
    fn update_marker(&mut self, player_id: usize, system_id: usize);
    /// Removes the marker of a character that is no longer linked.
    fn remove_marker(&mut self, player_id: usize);
}

pub struct UniversePane {
    map: Map,
    mapsync_reciever: Receiver<MapSync>,
    //generic_sender: Arc<Sender<Message>>,
    path: PathBuf,
    factor: f64,
    task_msg: Arc<MessageSpawner>,
    //tpool: Rc<ThreadPool>,
}

impl UniversePane {
    #[tracing::instrument(skip(receiver, task_msg))]
    pub fn new(
        receiver: Receiver<MapSync>,
        path: PathBuf,
        factor: f64,
        task_msg: Arc<MessageSpawner>,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor,
            task_msg,
        };
        object.generate_data();
        object.map.settings = MapSettings::default();
        object.map.settings.region_label_alpha = 0.30;
        object.map.settings.style.region_label_font =
            FontId::new(96.0, egui::FontFamily::Name("Custom".into()));
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.set_context_manager(Rc::new(ContextMenu::new()));
        object
    }

    #[tracing::instrument(skip(self))]
    fn generate_data(&mut self) {
        if let Ok(t_sde) = SdeManager::new(&self.path, self.factor) {
            if let Ok(points) = t_sde.get_systems() {
                self.map.add_hashmap_points(sde_points_to_map(points));
            }
            if let Ok(hash_conns) = t_sde.get_connections() {
                self.map.add_hashmap_lines(sde_segments_to_map(hash_conns));
            }
        }
        if let Ok(t_sde) = SdeManager::new(&self.path, self.factor)
            && let Ok(region_areas) = t_sde.get_region_coordinates()
        {
            // Region names are backdrop for the whole-galaxy view, not
            // per-node annotations -- `RegionLabel`/`add_region_labels`
            // (egui-map 0.9) instead of the free-floating `MapLabel` this
            // used to piggyback on: it scales with zoom rather than staying
            // a fixed screen size, is always painted behind everything
            // else, and is drawn in a faded color by default, so it reads
            // as the region's name rather than competing with system names.
            let mut labels = Vec::new();
            for region in region_areas {
                let mut label = RegionLabel::new();
                label.text = region.name;
                // Sit the label at the centre of the region's bounding box,
                // in the same scaled map space the nodes use: `get_systems`
                // divides by `factor` and applies the same inversion
                // `get_region_coordinates` does, so `egui-map` can project it
                // with pan/zoom like any node. Using a corner (`region.min`)
                // would pin it to the region's edge instead.

                label.center = Pos2::new(
                    ((region.min.x() + region.max.x()) / 2.0 / self.factor) as f32,
                    ((region.min.y() + region.max.y()) / 2.0 / self.factor) as f32,
                );
                labels.push(label);
            }
            self.map.add_region_labels(labels);
        }
    }
}

impl TabPane for UniversePane {
    fn update_marker(&mut self, player_id: usize, system_id: usize) {
        self.map.update_marker(player_id, system_id);
    }

    fn remove_marker(&mut self, player_id: usize) {
        self.map.remove_marker(player_id);
    }

    #[tracing::instrument(skip(self, ui))]
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.event_manager();
        ui.add(&mut self.map);
        UiResponse::None
    }

    #[tracing::instrument(skip(self))]
    fn get_title(&self) -> WidgetText {
        t!("map.universe").into_owned().into()
    }

    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        // `while let`, not `if let`: `mapsync_reciever` is a broadcast
        // channel that keeps filling up whether or not this pane's `ui()`
        // gets called every frame (e.g. while its tab is hidden behind
        // another one). A single `try_recv()` only ever drains one message
        // per frame, so a backlog built up while hidden -- or just a burst
        // of updates -- would fall permanently behind instead of catching up
        // once this pane is visible again.
        while let Ok(msg) = self.mapsync_reciever.try_recv() {
            match msg {
                MapSync::NodeEffect((system_id, effect)) => {
                    if let Some(node) = self.map.node(system_id) {
                        effect.apply(node);
                    }
                }
                MapSync::SystemNotification((system_id, time)) => {
                    if let Some(node) = self.map.node(system_id) {
                        node.pulse(time.into());
                    }
                    //self.map.notify(system_id, time.into());
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
            };
        }
    }

    #[tracing::instrument(skip(self, message))]
    fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                // Center through the widget's own node index instead of
                // re-reading coordinates from the SDE.
                //
                // This used to call `get_system_coords()` (which reads
                // `centerX/centerY/centerZ`) and project it with
                // `to_2d(ProjectedAxis::Y)` -> `(centerX, centerZ)`. But the
                // nodes on this map come from `get_systems()`, which reads
                // `position2DX/position2DY` -- and those are NOT `(centerX,
                // centerZ)`: the SDE builder computes them with
                // `isometric_projection_2d()`, which for `ProjectedAxis::Y` is
                // `(x - y, z + (x + y) / 2)`. A shear, not an axis drop; the
                // two agree only at the origin. So the view was being moved to
                // a point in a coordinate space no node lives in, and the
                // target system ended up off-screen.
                //
                // `set_pos_from_nodeid` uses the coordinates already loaded
                // into the widget, so it cannot disagree with them. It returns
                // `false` when the id was never loaded -- which is meaningful
                // here, since this map only carries K-space systems that have
                // at least one stargate connection.
                if !self.map.set_pos_from_nodeid(message.0) {
                    let mut msg = String::from("System with Id ");
                    msg += (message.0.to_string() + " is not present on this map").as_str();
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Warning,
                        String::from("UniversePane"),
                        String::from("center_on_target"),
                        msg,
                    )));
                }
            }
            Target::Region => {}
        }
    }
}

pub struct RegionPane {
    map: Map,
    mapsync_reciever: Receiver<MapSync>,
    path: PathBuf,
    factor: f64,
    region_id: usize,
    tab_name: String,
    task_msg: Arc<MessageSpawner>,
}

impl RegionPane {
    #[tracing::instrument(skip(receiver, task_msg))]
    pub fn new(
        receiver: Receiver<MapSync>,
        path: PathBuf,
        factor: f64,
        region_id: usize,
        task_msg: Arc<MessageSpawner>,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor,
            region_id,
            tab_name: String::from("Region"),
            task_msg,
        };
        object.generate_data();
        object.map.settings = MapSettings::default();
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.set_context_manager(Rc::new(ContextMenu::new()));
        object.map.set_node_template(Rc::new(Template::new()));
        object
    }

    #[tracing::instrument(skip(self))]
    fn generate_data(&mut self) {
        let t_sde = match SdeManager::new(&self.path, self.factor) {
            Ok(t_sde) => t_sde,
            Err(t_err) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    "RegionPane".to_string(),
                    "generate_data".to_string(),
                    t_err.to_string(),
                )));
                return;
            }
        };

        match t_sde.get_abstract_systems(vec![self.region_id as u32]) {
            Ok(points) => {
                self.map.add_hashmap_points(sde_points_to_map(points));
                if let Ok(lines) = t_sde.get_abstract_connections(vec![self.region_id as u32]) {
                    self.map.add_hashmap_lines(sde_segments_to_map(lines));
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
    fn update_marker(&mut self, player_id: usize, system_id: usize) {
        self.map.update_marker(player_id, system_id);
    }

    fn remove_marker(&mut self, player_id: usize) {
        self.map.remove_marker(player_id);
    }

    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        // See `UniversePane::event_manager`'s comment: `while let`, not
        // `if let`, so a backlog from being hidden or from a burst of
        // updates gets fully drained instead of leaking one message behind
        // every frame.
        while let Ok(msg) = self.mapsync_reciever.try_recv() {
            match msg {
                MapSync::NodeEffect((system_id, effect)) => {
                    if let Some(node) = self.map.node(system_id) {
                        effect.apply(node);
                    }
                }
                MapSync::SystemNotification((system_id, time)) => {
                    if let Some(node) = self.map.node(system_id) {
                        node.pulse(time.into());
                    }
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
            };
        }
    }

    #[tracing::instrument(skip(self))]
    fn get_title(&self) -> WidgetText {
        self.tab_name.clone().into()
    }

    #[tracing::instrument(skip(self, ui))]
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.event_manager();
        ui.add(&mut self.map);
        UiResponse::None
    }

    #[tracing::instrument(skip(self, message))]
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
    #[tracing::instrument]
    pub fn new(name: String, show_on_startup: bool) -> Self {
        Self {
            tile_id: None,
            name,
            visible: false,
            show_on_startup,
        }
    }

    // Deliberately uninstrumented from here down: these are single-field
    // accessors called many times per frame (the settings window walks every
    // region's `TileData` on every repaint). A Tracy zone costs orders of
    // magnitude more than the field access it would be measuring, so spans
    // here only bury the zones that matter.
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
    factor: f64,
    path: PathBuf,
    search_regions: Vec<usize>,
    pub tile_data: HashMap<usize, TileData>,
}

impl TreeBehavior {
    #[tracing::instrument(skip(task_msg))]
    pub fn new(task_msg: Arc<MessageSpawner>, factor: f64, path: PathBuf) -> Self {
        Self {
            simplification_options: SimplificationOptions {
                prune_empty_containers: true,
                prune_single_child_containers: true,
                prune_empty_tabs: true,
                prune_single_child_tabs: false,
                all_panes_must_have_tabs: false,
                join_nested_linear_containers: true,
                flatten_tabs_in_tabs: true,
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

    #[tracing::instrument(skip(self))]
    fn toggle_regions(&mut self, region_id: usize) {
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
    #[tracing::instrument(skip_all)]
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

    #[tracing::instrument(skip_all)]
    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        id: eframe::egui::Id,
        tile_id: TileId,
        tab_state: &TabState,
    ) -> eframe::egui::Response {
        let text = self.tab_title_for_tile(tiles, tile_id);
        let str_text = text.text().to_string().clone();
        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = text.into_galley(ui, Some(TextWrapMode::Truncate), f32::INFINITY, font_id);

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
        if str_text != t!("map.universe") && !tab_state.is_being_dragged {
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

        let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, tab_state);
        // Show a gap when dragged
        if ui.is_rect_visible(rect) && !tab_state.is_being_dragged {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, tab_state);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, tab_state);
            ui.painter().rect(
                rect.shrink(0.5),
                0.0,
                bg_color,
                stroke,
                egui::StrokeKind::Middle,
            );

            if tab_state.active {
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

        if let Some(tr_close) = rect_close {
            let bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, tab_state);
            let stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, tab_state);
            ui.painter().rect(
                tr_close.shrink(0.5),
                0.0,
                bg_color,
                stroke,
                egui::StrokeKind::Middle,
            );
            if ui.is_rect_visible(tr_close) {
                let a = WidgetText::from(String::from("×")).into_galley(
                    ui,
                    Some(TextWrapMode::Truncate),
                    f32::INFINITY,
                    TextStyle::Button.resolve(ui.style()),
                );
                let pos = egui::Align2::CENTER_CENTER
                    .align_size_within_rect(a.size(), tr_close)
                    .min;
                ui.painter().galley(pos, a, text_color);
                self.on_close_tab(tile_id, close_response.unwrap());
            }
        }

        self.on_tab_button(tiles, tile_id, response)
    }

    #[tracing::instrument(skip_all)]
    fn tab_title_for_pane(&mut self, view: &Box<dyn TabPane>) -> WidgetText {
        view.get_title()
    }

    #[tracing::instrument(skip_all)]
    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        ui.add_space(1.5);
        // `ui.menu_button` defaults to `PopupCloseBehavior::CloseOnClick`, which closes the
        // popup on *any* click once it has been open for a frame -- including the click used
        // to place the caret in the search box below. That made the text field appear to lose
        // focus as soon as it was clicked. Building the menu explicitly lets us ask for
        // `CloseOnClickOutside` instead, the same behavior egui's own `SubMenuButton` uses so
        // that interactive widgets inside a popup keep working normally.
        MenuButton::new("➕")
            .config(MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                let mut _data: Vec<usize> = Vec::new();
                ui.label(t!("map.search_region"));
                if ui.text_edit_singleline(&mut self.search_text).changed() {
                    self.search_regions.clear();
                    if self.search_text.len() > 3 {
                        let t_sde = SdeManager::new(Path::new(&self.path), self.factor);
                        if let Ok(regions) =
                            t_sde.and_then(|s| s.get_region(vec![], Some(self.search_text.clone())))
                        {
                            // The SDE query can match regions (e.g. wormhole space)
                            // that were filtered out of `tile_data` at startup; keep
                            // only ids we can actually look up below.
                            self.search_regions = regions
                                .keys()
                                .copied()
                                .map(|x| x as usize)
                                .filter(|id| self.tile_data.contains_key(id))
                                .collect();
                        }
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

    // Uninstrumented for the same reason as `TileData`'s accessors: egui_tiles
    // queries these repeatedly per tile per frame, and a span costs far more
    // than returning a `f32`.
    fn tab_bar_height(&self, _style: &Style) -> f32 {
        self.tab_bar_height
    }

    fn gap_width(&self, _style: &Style) -> f32 {
        self.gap_width
    }

    fn simplification_options(&self) -> SimplificationOptions {
        self.simplification_options
    }
}

struct ContextMenu {}

impl ContextMenu {
    #[tracing::instrument]
    fn new() -> Self {
        Self {}
    }
}

impl ContextMenuManager for ContextMenu {
    #[tracing::instrument(skip(self, ui))]
    fn ui(&self, ui: &mut Ui) {
        if ui.button(t!("map.set_beacon")).clicked() {
            ui.close();
        }
        ui.separator();
        if ui.button(t!("map.settings")).clicked() {
            ui.close();
        }
    }
}

struct Template {
    // Cached per-node label `Galley`s, keyed by node id. `node_ui` runs once
    // per visible node, every frame; `FontsView::layout_no_wrap` already
    // memoizes by content internally (see its own doc comment), but that
    // memoization still pays for a fresh `String` allocation of the label
    // text plus hashing the whole layout job on *every* call, hit or miss.
    // Keying our own cache on the node's id instead (a `usize`, hashed for
    // free) skips all of that whenever a node's label hasn't actually
    // changed since last frame -- the common case unless the map is being
    // actively zoomed, renamed, or re-themed.
    label_cache: RefCell<HashMap<usize, CachedLabel>>,
}

/// One entry in [`Template::label_cache`]: the inputs that produced
/// `galley`, so a lookup can tell a still-valid entry from a stale one --
/// `name`/`color`/`font_size` are exactly the arguments `node_ui` passes to
/// [`FontsView::layout_no_wrap`](egui::text::Fonts) to build the label, so
/// comparing them against the node's current values is the same test
/// egui's own content-hash cache is already making internally, just without
/// re-deriving that hash (or re-allocating the `String`) on every call.
struct CachedLabel {
    name: Option<String>,
    color: Color32,
    font_size: f32,
    galley: Arc<Galley>,
}

impl Template {
    #[tracing::instrument]
    fn new() -> Self {
        Self {
            label_cache: RefCell::new(HashMap::new()),
        }
    }
}

impl NodeTemplate for Template {
    // `skip_all`, not `skip(self, ui)`: this runs once per visible node per
    // frame, and `tracing-tracy` formats a span's recorded fields into the
    // Tracy *zone name* (`Config::format_fields_in_zone_name` defaults to
    // true). Leaving `system` recorded meant `{:?}`-formatting the whole
    // `MapPoint` -- its `name: Option<String>` and `connections:
    // Vec<(usize, usize)>` included -- into a fresh string on every call, and
    // then allocating a distinct Tracy zone per distinct value. A capture of
    // this app produced 8522 separate zone names from 42349 `node_ui` calls,
    // with this function accounting for 68% of all frame time. The same
    // reasoning applies to the three hooks below.
    #[tracing::instrument(skip_all)]
    fn node_ui(&self, ui: &mut Ui, ctx: NodeContext) {
        let mut shapes = Vec::new();
        let rect =
            Rect::from_center_size(ctx.position, Vec2::new(90.0 * ctx.zoom, 35.0 * ctx.zoom));
        // `ctx.color`: the same per-node color (a system's own star-color
        // override, falling back to the theme's node color) the built-in
        // circle paints with when no template is installed -- see
        // `Map::paint_map_points`'s `node_color` and `NodeContext::color`'s
        // doc comment. Using the theme's flat `theme.node` here instead would
        // silently drop every star-color override reaching this node.
        shapes.push(Shape::rect_stroke(
            rect,
            CornerRadius::same((10.0 * ctx.zoom).round() as u8),
            Stroke::new(4.0 * ctx.zoom, ctx.theme.node),
            egui::StrokeKind::Middle,
        ));
        // `ctx.background_color`: the chip background behind the node,
        // matching the surrounding UI's own faint background -- exactly what
        // `ui.visuals().extreme_bg_color` did before `NodeContext` grew this
        // field. A fixed `Color32::BLACK` here would paint an opaque black
        // chip under a light theme.
        shapes.push(Shape::rect_filled(
            rect,
            CornerRadius::same((10.0 * ctx.zoom).round() as u8),
            ctx.background_color,
        ));
        // Snap to the nearest half-pixel: egui's font atlas caches rasterized
        // glyphs keyed on the exact `FontId` size, and `12.0 * ctx.zoom` is
        // continuous in `ctx.zoom` -- during a zoom gesture it changes on
        // essentially every frame, so the atlas never gets a cache hit and
        // re-rasterizes this label from scratch each time. The 0.5px
        // granularity is not perceptible but turns that continuous stream of
        // atlas misses into a small, reusable set of sizes.
        let font_size = ((12.0 * ctx.zoom) * 2.0).round() / 2.0;
        // `ctx.theme.text`: the same field the built-in label painting uses
        // (`Map`'s own `text_color: self.theme_colors().text`) so this
        // label's color tracks the active theme instead of a hand-rolled
        // dark/light check.
        let text_color = ctx.theme.text;
        // Reuse this node's cached `Galley` if nothing that would change its
        // shape has changed (name, color, quantized size); only fall back to
        // laying it out again -- opening our own narrowly-scoped
        // `fonts_mut`, held only for the layout call itself -- when it's
        // genuinely stale. See `Template::label_cache`'s doc comment for why
        // this is worth doing on top of egui's own font-layout cache.
        //
        // This must NOT be widened to wrap the rest of this function:
        // `ui.painter().extend(shapes)` below also needs to touch
        // `egui::Context`'s single whole-object lock, and `fonts_mut` holds
        // that same lock for as long as its closure runs -- nesting the two
        // is a same-thread reentrant lock attempt, which deadlocks (egui
        // panics after a 10s timeout in debug builds: "Failed to acquire
        // RwLock write ... Deadlock?").
        let mut label_cache = self.label_cache.borrow_mut();
        let galley = match label_cache.get(&ctx.point.id) {
            Some(cached)
                if cached.name == ctx.point.name
                    && cached.color == text_color
                    && cached.font_size == font_size =>
            {
                Arc::clone(&cached.galley)
            }
            _ => {
                let name = ctx.point.name.clone();
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        name.clone().unwrap_or_default(),
                        FontId::proportional(font_size),
                        text_color,
                    )
                });
                label_cache.insert(
                    ctx.point.id,
                    CachedLabel {
                        name,
                        color: text_color,
                        font_size,
                        galley: Arc::clone(&galley),
                    },
                );
                galley
            }
        };
        drop(label_cache);
        // Same anchoring `Shape::text` does internally (`Align2::anchor_size`
        // then `Shape::galley`) -- we can't call `Shape::text` itself here
        // since it always lays the text out fresh; this is the equivalent
        // that accepts an already-built `Galley` instead.
        let text_rect = Align2::CENTER_CENTER.anchor_size(ctx.position, galley.size());
        shapes.push(Shape::galley(text_rect.min, galley, text_color));
        ui.painter().extend(shapes);
    }

    #[tracing::instrument(skip_all)]
    fn selection_ui(&self, ui: &mut Ui, ctx: SelectionContext) {
        /// Gap between the node's border and the selection stroke, in screen points.
        const SELECTION_GAP: f32 = 2.0;

        let zoom = ctx.zoom;
        // Same rect and border as `node_ui` (90x35, 4.0 stroke with `Middle`):
        // the node's outer edge sits 2.0 * zoom outside its rect.
        let node_rect = Rect::from_center_size(ctx.position, Vec2::new(90.0 * zoom, 35.0 * zoom));
        let node_outer = node_rect.expand(2.0 * zoom);
        // The stroke starts exactly SELECTION_GAP points further out and grows outward.
        let rect = node_outer.expand(SELECTION_GAP);
        // Concentric radius: node radius + half its border + the gap.
        let radius = (10.0 * zoom + 2.0 * zoom + SELECTION_GAP).round() as u8;

        ui.painter().add(Shape::rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(2.0 * zoom, ctx.theme.selected),
            egui::StrokeKind::Outside,
        ));
    }

    #[tracing::instrument(skip_all)]
    fn marker_ui(&self, ui: &mut Ui, ctx: MarkerContext) {
        let viewport_point = ctx.position;
        let zoom = ctx.zoom;
        let mut shapes = Vec::new();
        let led_position = Pos2::new(
            viewport_point.x + (45.0 * zoom),
            viewport_point.y - (17.0 * zoom),
        );
        let color = if ui.visuals().dark_mode {
            Color32::LIGHT_GREEN
        } else {
            Color32::GREEN
        };
        let mut transparency = (chrono::Local::now().timestamp_millis() % 2550) / 5;
        if transparency > 255 {
            transparency = 255 - (transparency - 255)
        }
        let corrected_color =
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), transparency as u8);
        shapes.push(Shape::Circle(CircleShape::stroke(
            led_position,
            6.5 * zoom,
            Stroke::new(4.0 * zoom, corrected_color),
        )));
        shapes.push(Shape::Circle(CircleShape::filled(
            led_position,
            6.0 * zoom,
            ui.visuals().extreme_bg_color,
        )));
        shapes.push(Shape::Circle(CircleShape::filled(
            led_position,
            6.0 * zoom,
            corrected_color,
        )));
        ui.ctx().request_repaint();
        ui.painter().extend(shapes);
    }

    #[tracing::instrument(skip_all)]
    fn notification_ui(&self, ui: &mut Ui, ctx: NotificationContext) -> bool {
        let viewport_point = ctx.position;
        let zoom = ctx.zoom;
        let initial_time = ctx.initial_time;
        let color = ctx.color;
        let mut shapes = Vec::new();
        let current_instant = Instant::now();
        let time_diff = current_instant.duration_since(initial_time);
        let secs_played = time_diff.as_secs_f32();
        let mut transparency: f32 = 1.00 - (secs_played / 2.00).abs();
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
            CornerRadius::same((10.0 * zoom).round() as u8),
            Stroke::new((4.00 + (25.00 * secs_played)) * zoom, corrected_color),
            egui::StrokeKind::Middle,
        ));
        ui.painter().extend(shapes);
        ui.ctx().request_repaint();
        if secs_played < 2.00 {
            return true;
        }
        false
    }
}
