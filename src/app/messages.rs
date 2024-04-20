pub enum Message {
    EsiAuthSuccess((String, String)),
    GenericNotification((Type, String, String, String)),
    NewRegionalPane(usize),
    MapHidden(usize),
    MapShown(usize),
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

#[derive(PartialEq)]
pub enum SettingsPage {
    Mapping,
    LinkedCharacters,
}
