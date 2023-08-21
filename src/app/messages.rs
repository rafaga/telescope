use egui_extras::RetainedImage;
use egui_map::map::objects::MapPoint;

pub enum Message{
    Processed2dMatrix(Vec<MapPoint>),
    EsiAuthSuccess((String,String)),
    EsiAuthError(String),
    GenericError(String),
    GenericWarning(String),
    LoadCharacterPhoto(Vec<(u64,String)>),
    SaveCharacterPhoto(Vec<RetainedImage>),
}
