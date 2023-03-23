use egui_map::map::objects::MapPoint;

pub enum Message{
    Processed2dMatrix(Vec<MapPoint>),
    EsiAuthSuccess([String;2]),
}

pub enum TelescopeError{
    EsiAuthError,
    DatabaseNotFound,
}