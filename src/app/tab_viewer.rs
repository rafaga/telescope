use eframe::egui;
use egui_map::map::Map;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use super::messages::Message;

pub enum Type{
    Undefined,
    Universal,
    Regional,
}

pub struct Tab {
    title: egui::WidgetText,
    pub tab_type: Type,
}

impl Tab{
    pub fn new(title:String, tab_type: Type) -> Self {
        Self {
            title: title.into(),
            tab_type,
        }
    }
}

pub struct TabViewer {
    pub universe_map: Map,
    pub tx: Option<Arc<Sender<Message>>>,
}

impl egui_dock::TabViewer for TabViewer {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title.clone()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab.tab_type {
            Type::Universal => {
                ui.add(&mut self.universe_map);
            },
            Type::Regional => {

            },
            Type::Undefined => todo!(),
        };
    }
}

impl TabViewer{
    pub fn new() -> Self {
        TabViewer {
            universe_map: Map::new(),
            tx: None
        }
    }
}