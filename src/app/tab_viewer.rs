use eframe::egui;
use egui_map::map::Map;

pub enum Type{
    Universal,
    Regional,
}

pub struct Tab {
    title: egui::WidgetText,
    tab_type: Option<Type>,
}

impl Tab{
    pub fn new(title:String) -> Self {
        Self {
            title: title.into(),
            tab_type: None
        }
    }
}

pub struct TabViewer {
    pub universe_map:  Map
}

impl egui_dock::TabViewer for TabViewer {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title.clone()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab.tab_type {
            Some(Type::Universal) => {
                ui.add(&mut self.universe_map);
            },
            Some(Type::Regional) => {

            },
            None => todo!()
        };
    }
}