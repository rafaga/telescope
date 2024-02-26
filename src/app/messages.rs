use egui_map::map::objects::{MapLine, MapPoint};
use sde::objects::EveRegionArea;
use std::collections::HashMap;

pub enum Message {
    ProcessedMapCoordinates(HashMap<usize, MapPoint>),
    RegionAreasLabels(Vec<EveRegionArea>),
    EsiAuthSuccess((String, String)),
    ProcessedRegionalConnections(Vec<MapLine>),
    GenericNotification((Type, String, String, String)),
    CenterOn((usize, Target)),
    SystemNotification(usize),
}

pub enum Type {
    Info,
    Error,
    Warning,
}

pub enum Target {
    System,
    Region,
}
