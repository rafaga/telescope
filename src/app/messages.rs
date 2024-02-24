use egui_map::map::objects::{MapPoint,MapLine};
use sde::objects::EveRegionArea;
use std::collections::HashMap;

pub enum Message {
    ProcessedMapCoordinates(HashMap<usize, MapPoint>),
    RegionAreasLabels(Vec<EveRegionArea>),
    EsiAuthSuccess((String, String)),
    ProcessedRegionalConnections(Vec<MapLine>),
    EsiAuthError(String),
    GenericError(String),
    GenericWarning(String),
    CenterOnSystem(usize),
    CenterOnRegion(usize),
    SystemNotification(usize),
}
