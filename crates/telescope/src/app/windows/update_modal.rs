//! The "new version available" dialog.

use crate::app::TelescopeApp;
use eframe::egui::{self, Margin, RichText, Vec2};

/// Width of the dialog.
const WINDOW_WIDTH: f32 = 380.0;

impl TelescopeApp {
    /// Shows the dialog while `update_prompt` holds a release newer than the
    /// running one. *Go to the release* opens its page; both buttons, Escape
    /// and a click outside close it. It is drawn with the frame and buttons
    /// of the SDE update dialog (`database_updater`), without its title bar.
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn show_update_prompt(&mut self, ctx: &egui::Context) {
        let Some(info) = &self.update_prompt else {
            return;
        };
        let mut open_release = false;
        let mut dismissed = false;
        let modal = egui::Modal::new(egui::Id::new("update_modal"))
            .frame(egui_panels::dialog_frame(ctx).inner_margin(Margin::ZERO))
            .show(ctx, |ui| {
                let theme = egui_panels::Theme::get(ui.ctx());
                let palette = theme.palette(ui.visuals());
                let width = WINDOW_WIDTH
                    .min(ui.ctx().content_rect().width() - 48.0)
                    .max(280.0);
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;

                egui::Frame::NONE
                    .inner_margin(Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(width - 36.0);
                        ui.spacing_mut().item_spacing.y = 14.0;
                        ui.label(
                            RichText::new(t!("update_modal.title"))
                                .size(theme.section_title_size)
                                .color(palette.strong_text),
                        );
                        ui.label(t!(
                            "update_modal.message",
                            latest = info.latest,
                            current = info.current
                        ));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if egui_panels::button(
                                        ui,
                                        t!("update_modal.download").into_owned(),
                                        egui_panels::Variant::Primary,
                                    )
                                    .clicked()
                                    {
                                        open_release = true;
                                        dismissed = true;
                                    }
                                    if egui_panels::button(
                                        ui,
                                        t!("common.cancel").into_owned(),
                                        egui_panels::Variant::Ghost,
                                    )
                                    .clicked()
                                    {
                                        dismissed = true;
                                    }
                                },
                            );
                        });
                    });
            });
        if open_release {
            let _ = open::that(&info.url);
        }
        if dismissed || modal.should_close() {
            self.update_prompt = None;
        }
    }
}
