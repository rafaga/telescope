use egui_tiles::TileId;

pub enum Message {
    EsiAuthSuccess((String, String)),
    GenericNotification((Type, String, String, String)),
    ToggleRegionMap(),
    MapClosed(TileId),
}

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    SystemNotification(usize),
}

pub enum Type {
    Info,
    Error,
    Warning,
}

#[derive(Clone)]
pub enum Target {
    System,
    Region,
}

pub enum SettingsPage {
    Mapping,
    LinkedCharacters
}