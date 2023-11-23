use egui_map::map::objects::MapPoint;
use sde::objects::EveRegionArea;
use std::collections::HashMap;

pub enum Message {
    ProcessedMapCoordinates(HashMap<usize, MapPoint>),
    RegionAreasLabels(Vec<EveRegionArea>),
    EsiAuthSuccess((String, String)),
    EsiAuthError(String),
    GenericError(String),
    GenericWarning(String),
}
