
use egui::{containers::*, widgets::*, *};
use sde::objects::SystemPoint;
use kdtree::{KdTree,ErrorKind};
use kdtree::distance::squared_euclidean;
use egui::util::cache::{ComputerMut, FrameCache};
use std::collections::HashMap;
use std::sync::{Arc,Mutex};
use rand::thread_rng;
use rand::distributions::{Alphanumeric,Distribution};


// This can by any object or point with its associated metadata
/// Struct that contains coordinates to help calculate nearest point in space

pub struct Map {
    pub zoom: f32,
    pub pos: Pos2,
    inner_rect: Option<Rect>,
    points: Option<HashMap<usize,SystemPoint>>,
    tree: Option<KdTree<f64,usize,[f64;2]>>,
    visible_points: Vec<usize>,
    pub min: Pos2,
    pub max: Pos2,
    recalculate: bool,
    initialized: bool,
}

impl Default for Map {
    fn default() -> Self {
        Map::new()
    }
}

impl WidgetWithState for &mut Map {
    type State = MapState;
}

impl Widget for &mut Map {
    fn ui(self, ui_obj: &mut egui::Ui) -> Response {
        let id_map;
        if self.initialized == false {
            let mut rng = thread_rng();
            let component_id: String = Alphanumeric
                .sample_iter(&mut rng)
                .take(15)
                .map(char::from)
                .collect();
            let idx = egui::Id::new(component_id);
            id_map = idx;
            ui_obj.make_persistent_id(idx);
            self.initialized = true;
        }
        else {
            id_map = ui_obj.id();
        }
        let mut state = MapState::load(ui_obj.ctx(), id_map).unwrap_or_default();
        if state.max.x > 0.0 && state.max.x < f32::INFINITY{
            self.max = state.max;
            self.min = state.min;
            self.pos = state.pos;
            self.inner_rect = Some(state.get_inner_rect());
        }

        //self.inner_rect = Some(state.get_inner_rect().clone());
        let style = egui::style::Style::default();
        let canvas = egui::Frame::canvas(&style)
            .stroke(egui::Stroke{width:2.0f32, color:Color32::DARK_GRAY});
        let inner_response = canvas.show(ui_obj, |ui_obj| {
            let scroll = ScrollArea::new([true;2])
                .drag_to_scroll(true)
                .auto_shrink([true;2])
                .max_height(f32::INFINITY)
                .max_width(f32::INFINITY)
                .always_show_scroll(false);
            
            let scroll_area = scroll.show(ui_obj, |ui_obj| { 
                let (resp,paint) = ui_obj.allocate_painter(ui_obj.max_rect().size(),egui::Sense::click_and_drag());
                //self.inner_rect = Some(ui_obj.clip_rect());
                //ui_obj.scroll_to_cursor(Some(Align::Center));
                ui_obj.memory_mut( |mem| {
                    let tree_id = id_map.with("-tree");
                    if let Some(tree) = mem.data.get_temp(tree_id) {
                        self.tree = tree;
                    }
                    let points_id = id_map.with("-points");
                    if let Some(points) = mem.data.get_temp(points_id) {
                        self.points = points;
                    }
                    let vispos_id = id_map.with("-vispos");
                    if let Some(vis_points) = mem.data.get_temp(vispos_id){
                        self.visible_points = vis_points;
                    }
                });

                let system_stroke = egui::Stroke{ width: 2f32, color: Color32::DARK_RED};
                let system_color = Color32::YELLOW;
                let radius = 4f32;

                while let Some(idx) = self.visible_points.pop() {
                    if let Some(hashm) = self.points.as_mut(){
                        if let Some(system) = hashm.get(&idx){
                            let center = Pos2::new(system.coords[0] as f32, system.coords[1] as f32);
                            paint.circle(center, radius, system_color, system_stroke);
                            for line in &system.lines {
                                paint.line_segment([center,Pos2::new(line[0] as f32,line[1] as f32)], system_stroke)
                            }
                        }
                    }
                }
                //if !self.painted {
                /*for point in &self.vec_points {
                    paint.circle(*point, radius, system_color, system_stroke);
                }*/
                    //self.painted = true;
                //}
                
                /*for point in &self.vec_points{
                    paint.circle(*point, radius, system_color, system_stroke);
                }*/
                //let respz = resp.interact(egui::Sense::click_and_drag());
                if cfg!(debug_assertions) {
                    let mut init_pos = Pos2::new(180.0, 50.0);
                    let mut msg = String::from("MIN:".to_string() + self.min.x.to_string().as_str() + "," + self.min.y.to_string().as_str());
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    init_pos.y += 15.0;
                    msg = "MAX:".to_string() + self.max.x.to_string().as_str() + "," + self.max.y.to_string().as_str();
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    init_pos.y += 15.0;
                    msg = "CNT:".to_string() + self.pos.x.to_string().as_str() + "," + self.pos.y.to_string().as_str();
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    init_pos.y += 15.0;
                    msg = "VIS:".to_string() + self.visible_points.len().to_string().as_str();
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    if let Some(rect) = self.inner_rect{
                        init_pos.y += 15.0;
                        msg = "REC:(".to_string() + rect.left_top().x.to_string().as_str() + "," + rect.left_top().y.to_string().as_str() + "),(" + rect.right_bottom().x.to_string().as_str() + "," + rect.right_bottom().y.to_string().as_str() + ")";
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                    if let Some(pointer_pos) = resp.hover_pos() {
                        init_pos.y += 15.0;
                        msg = "HVR:".to_string() + pointer_pos.x.to_string().as_str() + "," + pointer_pos.y.to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                    if let Some(points) = &self.points {
                        init_pos.y += 15.0;
                        msg = "NUM:".to_string() + points.len().to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                }
            });
            self.inner_rect = Some(scroll_area.inner_rect);
        });
        
        //ui_obj.allocate_ui(desired_size, add_contents)
        /*let (response, painterz) = ui_obj.allocate_painter(self.rects.map.unwrap().size(), egui::Sense::click_and_drag());
        painterz.circle(self.rects.map.unwrap().center(), 30f32, Color32::RED, Stroke{ width: 10f32, color: Color32::DARK_RED });*/
       
        state.max = self.max;
        state.min = self.min;
        state.pos = self.pos;
        state.zoom = self.zoom;
        state.set_inner_rect(self.inner_rect.unwrap());
        state.store(ui_obj.ctx(), id_map);

        //self.calculate_visible_points();
        
        ui_obj.memory_mut( |mem| {
            let tree_id = id_map.with("-tree");
            mem.data.insert_temp(tree_id, Arc::new(Mutex::new(self.tree.clone())));
            let points_id = id_map.with("-points");
            mem.data.insert_temp(points_id, Arc::new(Mutex::new(self.points.clone())));
            if self.recalculate == true {
                self.calculate_visible_points();
                self.recalculate = false;
            }
            let vispos_id = id_map.with("-vispos");
            mem.data.insert_temp(vispos_id, Arc::new(Mutex::new(self.visible_points.clone())));
        });
        
        inner_response.response
    }

}

impl Map {
    pub fn new() -> Self {
        Map {
            zoom: 1.0,
            pos: Pos2 {x:0.0f32, y:0.0f32},
            min: Pos2 {x:f32::INFINITY , y:f32::INFINITY},
            max: Pos2 {x:f32::NEG_INFINITY , y:f32::NEG_INFINITY },
            tree: None,
            points: None,
            inner_rect: None,
            visible_points: Vec::new(),
            recalculate: false,
            initialized: false,
        }
    }

    fn calculate_visible_points(&mut self) -> () {
        if let Some(visual_rect) = self.inner_rect {
            let dist = (((visual_rect.right_bottom().x - visual_rect.left_top().x)/2.0) as f64,((visual_rect.right_bottom().y - visual_rect.left_top().y)/2.0) as f64);
            let hipotenuse = dist.0.powi(2) + dist.1.powi(2);
            let mut center = [self.pos[0] as f64,self.pos[1] as f64];
            if let Some(tree) = &self.tree{
                let vis_pos = tree.within(center.as_mut(), hipotenuse, &squared_euclidean);
                self.visible_points.clear();
                if let Ok(vector) = vis_pos {
                    for point in vector{
                        self.visible_points.push(*point.1);
                    }
                }
            }
        }
    }

    pub fn add_points(&mut self, points: Vec<SystemPoint>) -> (){
        let mut hmap = HashMap::new();
        self.tree = Some(KdTree::<f64,usize,[f64;2]>::new(2));
        for point in points{
            if point.coords[0] < self.min.x as f64 {
                self.min.x = point.coords[0] as f32;
            }
            if point.coords[1] < self.min.y as f64 {
                self.min.y = point.coords[1] as f32;
            }
            if point.coords[0] > self.max.x as f64  {
                self.max.x = point.coords[0] as f32;
            }
            if point.coords[1] > self.max.y as f64  {
                self.max.y = point.coords[1] as f32;
            }
            if let Some(tree) = &mut self.tree {
                let _result = tree.add([point.coords[0],point.coords[1]],point.id);
            }
            hmap.entry(point.id).or_insert(point);
        }
        self.points = Some(hmap);
        let mut _new_x = 0.0f32;
        let mut _new_y = 0.0f32;
        if self.min.x < 0.0 {
            _new_x = (self.max.x + (self.min.x * -1.0))/2.0 + self.min.x;
        }
        else {
            _new_x = (self.max.x + self.min.x)/2.0 + self.min.x;
        }
        if self.min.y < 0.0 {
            _new_y = (self.max.y + (self.min.y * -1.0))/2.0 + self.min.y;
        }
        else {
            _new_y = (self.max.y + self.min.y)/2.0 + self.min.y;
        }
        self.pos = Pos2::new(_new_x, _new_y);
        self.recalculate=true;
    } 

    pub fn load_state(ctx: &Context, id: Id) -> Option<MapState> {
        MapState::load(ctx, id)
    }

    pub fn store_state(ctx: &Context, id: Id, state: MapState) {
        state.store(ctx, id);
    }
    
}

#[derive(Clone, Copy, Default)]
#[derive(serde::Deserialize, serde::Serialize)]
pub struct MapState{
    pub zoom: f32,
    pub pos: Pos2,
    pub min: Pos2,
    pub max: Pos2,
    pub rect_a: Pos2,
    pub rect_b: Pos2,
}

impl MapState {

    pub fn load(ctx: &Context, id: Id) -> Option<Self> {
        ctx.data_mut(|d| d.get_persisted(id))
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| d.insert_persisted(id, self));
    }

    pub fn get_inner_rect(self) -> Rect {
        egui::Rect::from_two_pos(self.rect_a,self.rect_b)
    }

    pub fn set_inner_rect(&mut self, rect:Rect) -> () {
        self.rect_a = rect.left_top();
        self.rect_b = rect.right_bottom();
    }

}
