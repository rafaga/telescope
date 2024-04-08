pub enum Message {
    EsiAuthSuccess((String, String)),
    GenericNotification((Type, String, String, String)),
    RequestRegionName(usize),
    ToggleRegionMap(Vec<usize>),
}

#[derive(Clone)]
pub enum MapSync {
    CenterOn((usize, Target)),
    SystemNotification(usize),
    GetRegionName(String),
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
