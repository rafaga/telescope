use egui_map::map::objects::{MapPoint,MapLine};
use sde::objects::EveRegionArea;
use std::collections::HashMap;

pub enum Message {
    ProcessedMapCoordinates(HashMap<usize, MapPoint>),
    RegionAreasLabels(Vec<EveRegionArea>),
    EsiAuthSuccess((String, String)),
    ProcessedRegionalConnections(Vec<MapLine>),
    GenericMessage((MessageType,String,String,String)),
    CenterOnSystem(usize),
    SystemNotification(usize),
}

pub enum MessageType {
    Info,
    Error,
    Warning,
}


pub enum TargetType{
    System,
}