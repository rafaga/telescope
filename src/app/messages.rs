use egui_extras::RetainedImage;
use egui_map::map::objects::MapPoint;
use sde::objects::EveRegionArea;

pub enum Message{
    Processed2dMatrix(Vec<MapPoint>),
    RegionAreasLabels(Vec<EveRegionArea>),
    EsiAuthSuccess((String,String)),
    EsiAuthError(String),
    GenericError(String),
    GenericWarning(String),
    LoadCharacterPhoto(Vec<(u64,String)>),
    SaveCharacterPhoto(Vec<RetainedImage>),
}
