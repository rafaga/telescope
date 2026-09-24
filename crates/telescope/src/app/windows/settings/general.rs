//! Settings page "General": the interface language, the data paths (see
//! `data_sources`) and the maps shown at start-up.

use crate::app::TelescopeApp;
use crate::i18n;
use eframe::egui;
use eframe::egui::FontId;
use eframe::egui::RichText;
use egui_extras::{Column, TableBuilder};

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
        self.show_data_sources_section(ui);
        self.show_startup_maps_section(ui);
    }

    /// Regions whose map opens at start-up, in three columns.
    fn show_startup_maps_section(&mut self, ui: &mut egui::Ui) {
        let mut keys: Vec<usize> = self.behavior.tile_data.keys().copied().collect();
        keys.sort_unstable();
        let num_rows = keys.len().div_ceil(3);
        let row_height = 18.0;
        ui.add_space(12.00);
        ui.label(
            RichText::new(t!("settings.general.startup_maps")).font(FontId::proportional(20.0)),
        );
        ui.label(t!("settings.general.startup_help"))
            .with_new_rect(ui.available_rect_before_wrap());
        ui.push_id("rgn_tbl", |ui| {
            TableBuilder::new(ui)
                .column(Column::resizable(Column::exact(150.0), false))
                .column(Column::resizable(Column::exact(150.0), false))
                .column(Column::resizable(Column::exact(150.0), false))
                .striped(true)
                .vscroll(false)
                .body(|body| {
                    body.rows(row_height, num_rows, |mut row| {
                        let key_index = row.index() * 3;
                        row.col(|ui: &mut egui::Ui| {
                            let region = self.behavior.tile_data.get_mut(&keys[key_index]).unwrap();
                            let name = region.get_name();
                            //let checked = &mut self.behavior.tile_data.get_mut(&region.get_id()).unwrap().show_on_startup;
                            ui.checkbox(&mut region.show_on_startup, name);
                        });
                        let mut t_key_index = key_index + 1;
                        if t_key_index < keys.len() {
                            row.col(|ui: &mut egui::Ui| {
                                let region =
                                    self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                let name = region.get_name();
                                ui.checkbox(&mut region.show_on_startup, name);
                            });
                        }
                        t_key_index += 1;
                        if t_key_index < keys.len() {
                            row.col(|ui: &mut egui::Ui| {
                                let region =
                                    self.behavior.tile_data.get_mut(&keys[t_key_index]).unwrap();
                                let name = region.get_name();
                                ui.checkbox(&mut region.show_on_startup, name);
                            });
                        }
                    });
                });
        });
    }
}
