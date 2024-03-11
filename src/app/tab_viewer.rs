use eframe::egui;
use egui_map::map::{objects::*, Map};

pub struct TabViewer {
}

impl egui_dock::TabViewer for TabViewer {
    type Tab = String;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        (&*tab).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab.as_str() {
            "tab1" => {

            },
            "tab2" => {

            },
            &_ => todo!()
        };
    }
}