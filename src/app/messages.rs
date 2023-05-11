use egui_map::map::objects::MapPoint;
use webb::objects::Character;

pub enum Message{
    Processed2dMatrix(Vec<MapPoint>),
    EsiAuthSuccess(Character),
    EsiAuthError(String),
}
