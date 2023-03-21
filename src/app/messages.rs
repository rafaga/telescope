use egui_map::map::objects::MapPoint;

pub enum Message{
    Processed2dMatrix(Vec<MapPoint>),
}

