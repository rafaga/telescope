
use egui::{containers::*, widgets::*, *};
use sde::objects::SystemPoint;
use kdtree::KdTree;
use kdtree::distance::squared_euclidean;
use std::collections::HashMap;
use rand::thread_rng;
use rand::distributions::{Alphanumeric,Distribution};


// This can by any object or point with its associated metadata
/// Struct that contains coordinates to help calculate nearest point in space

pub struct Map {
    pub zoom: f32,
    pub pos: Pos2,
    points: Option<HashMap<usize,SystemPoint>>,
    tree: Option<KdTree<f64,usize,[f64;2]>>,
    visible_points: Option<Vec<usize>>,
    pub min: Pos2,
    pub max: Pos2,
    pub dist: f64,
    pub min_vrect:  Pos2,
    pub max_vrect:  Pos2,
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
            let state = MapState::load(ui_obj.ctx(), id_map).unwrap_or_default();
            if self.recalculate == false {
                if self.max.x != state.max.x && state.max.x > 0.0 && self.max.y != state.max.y && state.max.y > 0.0{
                    self.max = state.max;
                }
                if self.min.x != state.min.x && state.min.x < 0.0 && self.min.y != state.min.y && state.min.y > 0.0{
                    self.min = state.min;
                }
                if self.pos.x != state.pos.x && state.pos.x > 0.0 && self.pos.y != state.pos.y && state.pos.y > 0.0{
                    self.pos = state.pos;
                }
            } 
            if self.min_vrect.x != state.min_vrect.x && state.min_vrect.x > 0.0 && self.min_vrect.y != state.min_vrect.y && state.min_vrect.y > 0.0{
                self.min_vrect = state.min_vrect;
            }
            if self.max_vrect.x != state.max_vrect.x && state.max_vrect.x > 0.0 && self.max_vrect.y != state.max_vrect.y && state.max_vrect.y > 0.0{
                self.max_vrect = state.max_vrect;
            }
            if let None = self.tree{
                self.tree = state.tree;
            }
            if let None = self.visible_points{
                self.visible_points = state.visible_points;
            }
            if let None = self.points{
                self.points = state.points.clone();
            }
            
        }

        let style = egui::style::Style::default();
        let canvas = egui::Frame::canvas(&style)
            .stroke(egui::Stroke{width:2.0f32, color:Color32::DARK_GRAY});
        let inner_response = canvas.show(ui_obj, |ui_obj| {
            let scroll = ScrollArea::new([true;2])
                .drag_to_scroll(true)
                //.auto_shrink([true;2])
                .max_height(f32::INFINITY)
                .max_width(f32::INFINITY)
                .always_show_scroll(true);
            
            let scroll_area = scroll.show(ui_obj, |ui_obj| { 
                
                let (resp,paint) = ui_obj.allocate_painter(ui_obj.max_rect().size(),egui::Sense::click_and_drag());
                let gate_stroke = egui::Stroke{ width: 2f32, color: Color32::DARK_RED};
                let debug_stroke = egui::Stroke{ width: 2f32, color: Color32::GOLD};
                let system_color = Color32::YELLOW;
                let system_stroke = egui::Stroke{ width: 2f32, color: system_color};
                let radius = 4f32;

                for temp_vec_point in &self.visible_points {
                    if let Some(hashm) = self.points.as_mut() {
                        if cfg!(debug_assertions) {
                            paint.circle(self.pos, self.dist as f32, Color32::TRANSPARENT, debug_stroke);
                        } 
                        for temp_point in temp_vec_point{
                            if let Some(system) = hashm.get(&temp_point) {
                                let center = Pos2::new(system.coords[0] as f32, system.coords[1] as f32);
                                for line in &system.lines {
                                    paint.line_segment([center,Pos2::new(line[0] as f32,line[1] as f32)], gate_stroke);
                                }
                                if cfg!(debug_assertions) {
                                    let text_pos = egui::Pos2::new(center.x + 3.0,center.y - 3.0);
                                    //let system_name = self.points.unwrap().get(&system.id).unwrap().name;
                                    paint.debug_text(text_pos,Align2::LEFT_BOTTOM, Color32::LIGHT_GREEN, system.id.to_string());
                                }
                                paint.circle(center, radius, system_color, system_stroke);
                            }
                        } 
                    }
                    let rect = egui::Rect::from_two_pos(self.min_vrect,self.max_vrect);
                    ui_obj.scroll_to_rect(rect, Some(Align::Center));
                }
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
                    msg = "DST:".to_string() + self.dist.to_string().as_str();
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    init_pos.y += 15.0;
                    msg = "REC:(".to_string() + self.min_vrect.x.to_string().as_str() + "," + self.min_vrect.y.to_string().as_str() + "),(" + self.max_vrect.x.to_string().as_str() + "," + self.max_vrect.y.to_string().as_str() + ")";
                    paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    if let Some(tree) = &self.tree {
                        init_pos.y += 15.0;
                        msg = "TSZ:".to_string() + tree.size().to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                    if let Some(points) = &self.points {
                        init_pos.y += 15.0;
                        msg = "NUM:".to_string() + points.len().to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                    if let Some(vec_k) = self.visible_points.as_ref(){
                        init_pos.y += 15.0;
                        msg = "VIS:".to_string() + vec_k.len().to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                    if let Some(pointer_pos) = resp.hover_pos() {
                        init_pos.y += 15.0;
                        msg = "HVR:".to_string() + pointer_pos.x.to_string().as_str() + "," + pointer_pos.y.to_string().as_str();
                        paint.debug_text(init_pos, Align2::LEFT_TOP, Color32::LIGHT_GREEN, msg);
                    }
                }
            });
            // scroll_area.content_size.x = self.max.x - self.min.x;
            // scroll_area.content_size.y = self.max.y - self.min.y;
            self.calculate_bounds(&scroll_area.inner_rect);
        });

        let mut state = MapState::load(ui_obj.ctx(), id_map).unwrap_or_default();
        state.max = self.max;
        state.min = self.min;
        state.max_vrect = self.max_vrect;
        state.min_vrect = self.min_vrect;
        state.pos = self.pos;
        state.zoom = self.zoom;
        state.dist = self.dist;
        if self.recalculate == true{
            self.calculate_visible_points();
            self.recalculate = false;
        }
        state.tree = self.tree.clone();
        state.visible_points = self.visible_points.clone();
        state.points = self.points.clone();
        state.store(ui_obj.ctx(), id_map);        
        inner_response.response
    }

}

impl Map {
    pub fn new() -> Self {
        Map {
            zoom: 1.0,
            pos: Pos2 {x:0.0f32, y:0.0f32},
            min: Pos2 {x:0.0f32, y:0.0f32},
            max: Pos2 {x:0.0f32, y:0.0f32},
            min_vrect: Pos2 {x:0.0f32,y:0.0f32},
            max_vrect: Pos2 {x:0.0f32,y:0.0f32},
            tree: None,
            points: None,
            visible_points: None,
            dist: 0.0,
            recalculate: false,
            initialized: false,
        }
    }

    fn calculate_visible_points(&mut self) -> () {
        if self.dist > 0.0 {
            if let Some(tree) = &self.tree{
                let center = [self.pos[0] as f64,self.pos[1] as f64];
                let radius = self.dist.powi(2);
                let vis_pos = tree.within(&center, radius, &squared_euclidean).unwrap();
                let mut visible_points = vec![];
                for point in vis_pos {
                    visible_points.push(*point.1);
                }
                self.visible_points = Some(visible_points);
            }
        }
    }

    fn calculate_bounds(&mut self, inner_rect:&Rect) -> () {
        let dist_x = (inner_rect.right_bottom().x as f64 - inner_rect.left_top().x as f64)/2.0;
        let dist_y = (inner_rect.right_bottom().y as f64 - inner_rect.left_top().y as f64)/2.0;
        self.dist = (dist_x.powi(2) + dist_y.powi(2)/2.0).sqrt();
        let pos_1 = egui::Pos2::new(self.pos.x - dist_x as f32, self.pos.y - dist_y as f32);
        let pos_2 = egui::Pos2::new(self.pos.x + dist_x as f32, self.pos.y + dist_y as f32);
        self.min_vrect = pos_1;
        self.max_vrect = pos_2;
    }

    pub fn add_points(&mut self, points: Vec<SystemPoint>) -> (){
        let mut hmap = HashMap::new();
        let mut min = (f64::INFINITY,f64::INFINITY);
        let mut max = (f64::NEG_INFINITY,f64::NEG_INFINITY);
        let mut tree = KdTree::<f64,usize,[f64;2]>::new(2);
        for point in points{
            if point.coords[0] < min.0 {
                min.0 = point.coords[0];
            }
            if point.coords[1] < min.1 {
                min.1 = point.coords[1];
            }
            if point.coords[0] > max.0 {
                max.0 = point.coords[0];
            }
            if point.coords[1] > max.1 {
                max.1 = point.coords[1];
            }
            let _result = tree.add([point.coords[0],point.coords[1]],point.id);
            hmap.entry(point.id).or_insert(point);
        }
        self.min = Pos2::new(min.0 as f32,min.1 as f32);
        self.max = Pos2::new(max.0 as f32,max.1 as f32);
        self.points = Some(hmap);
        self.tree = Some(tree);
        /*let new_x;
        let new_y;
        if self.min.x < 0.0 {
            new_x = (self.max.x + (self.min.x * -1.0))/2.0 + self.min.x;
        }
        else {
            new_x = (self.max.x + self.min.x)/2.0 + self.min.x;
        }
        if self.min.y < 0.0 {
            new_y = (self.max.y + (self.min.y * -1.0))/2.0 + self.min.y;
        }
        else {
            new_y = (self.max.y + self.min.y)/2.0 + self.min.y;
        }
        self.pos = Pos2::new(new_x.to_owned(), new_y.to_owned());*/
        	
        let cx = 95415018110898720.00 / 100000000000000.00;	
        let cy = 62620060063386120.00 / 100000000000000.00;
        self.pos = Pos2::new(cx.to_owned(), cy.to_owned());

        self.recalculate=true;
    } 

    pub fn set_pos(mut self, x: f32, y:f32) -> () {
        if x <= self.max.x && x >= self.min.x && y <= self.max.y && y >= self.min.y{
            self.pos = Pos2::new(x,y);
            self.recalculate=true;
        }
    }

    pub fn load_state(ctx: &Context, id: Id) -> Option<MapState> {
        MapState::load(ctx, id)
    }

    pub fn store_state(ctx: &Context, id: Id, state: MapState) {
        state.store(ctx, id);
    }
    
}

#[derive(Clone, Default)]
#[derive(serde::Deserialize, serde::Serialize)]
pub struct MapState{
    pub zoom: f32,
    pub pos: Pos2,
    pub min: Pos2,
    pub max: Pos2,
    pub dist: f64,
    pub min_vrect:  Pos2,
    pub max_vrect:  Pos2,
    points: Option<HashMap<usize,SystemPoint>>,
    tree: Option<KdTree<f64,usize,[f64;2]>>,
    visible_points: Option<Vec<usize>>,
}

impl MapState {

    pub fn load(ctx: &Context, id: Id) -> Option<Self> {
        ctx.data_mut(|d| d.get_persisted(id))
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| d.insert_persisted(id, self));
    }

}
