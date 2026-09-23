//! The "About Telescope" window.

use crate::app::TelescopeApp;
use eframe::egui;
use eframe::egui::Vec2;

impl TelescopeApp {
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_about_window(&mut self, ctx: &egui::Context) {
        egui::Window::new(t!("about.title"))
            .id(egui::Id::new("about_window"))
            .fixed_size((400.0, 200.0))
            .open(&mut self.open[0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../../../../../assets/icon.png"))
                            .fit_to_exact_size(Vec2 { x: 200.0, y: 200.0 }),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.heading("Telescope");
                        ui.strong("v ".to_owned() + env!("CARGO_PKG_VERSION"));
                        ui.label(t!("about.author", name = "Rafael Amador"));
                        ui.label(t!("about.license", license = "MIT"));
                        if ui.link("https://github.com/rafaga/telescope").clicked() {
                            let _a = open::that("https://github.com/rafaga/telescope");
                        }
                        egui::warn_if_debug_build(ui);
                    });
                });
            });
    }
}
