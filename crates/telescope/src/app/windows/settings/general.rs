//! Settings page "General": the interface language.

use crate::app::TelescopeApp;
use crate::i18n;
use eframe::egui;
use eframe::egui::FontId;
use eframe::egui::RichText;

impl TelescopeApp {
    pub(super) fn show_general_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t!("settings.general.heading")).font(FontId::proportional(20.0)));
        let mut state = self.settings.get_ui_state();
        let label_for = |setting: &str| {
            if setting == i18n::AUTO {
                t!(
                    "settings.general.auto",
                    language = i18n::display_name(&i18n::resolve(i18n::AUTO))
                )
                .into_owned()
            } else {
                // A language saved in `[ui]` whose file has no name (yet):
                // show its code rather than an empty box.
                let name = i18n::display_name(setting);
                if name.trim().is_empty() {
                    setting.to_owned()
                } else {
                    name
                }
            }
        };
        let mut chosen = state.language.clone();
        ui.horizontal(|ui| {
            ui.label(t!("settings.general.language"));
            egui::ComboBox::from_id_salt("interface_language")
                .selected_text(label_for(&chosen))
                .show_ui(ui, |ui| {
                    let options = std::iter::once(i18n::AUTO.to_owned()).chain(i18n::available());
                    for option in options {
                        let label = label_for(&option);
                        ui.selectable_value(&mut chosen, option, label);
                    }
                })
                .response
                .on_hover_text(t!("settings.general.language_hint"));
        });
        if chosen != state.language {
            i18n::apply_language(&chosen);
            state.language = chosen;
            if let Err(t_error) = self.settings.save_ui_state(state) {
                tracing::warn!("could not save the interface language: {t_error}");
            }
        }
    }
}
