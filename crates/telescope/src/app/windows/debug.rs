use crate::app::TelescopeApp;
use crate::app::messages::MapSync;
use crate::app::messages::Message;
use crate::app::messages::Target;
use crate::app::messages::Type;
use eframe::egui;
use egui_extras::Column;
use egui_extras::TableBuilder;
use sde::SdeManager;
use std::sync::Arc;

impl TelescopeApp {
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_debug_menu(&mut self, ctx: &egui::Context) {
        egui::Window::new("Debug Menu")
            .fixed_size((400.0, 600.0))
            .open(&mut self.open[1])
            .show(ctx, |ui| {
                ui.heading("Search");

                ui.horizontal(|ui| {
                    ui.label("Name: ");
                    let response = ui.text_edit_singleline(&mut self.search_text);
                    if response.changed() {
                        if self.search_text.len() >= 3 {
                            let sde = SdeManager::new(
                                self.settings.get_sde(),
                                self.settings.get_factor(),
                            );
                            match sde.and_then(|s| {
                                s.get_system_id(self.search_text.clone().to_lowercase())
                            }) {
                                Ok(system_results) => self.search_results = system_results,
                                Err(t_error) => {
                                    self.task_msg.spawn(Message::GenericNotification((
                                        Type::Error,
                                        String::from("sde"),
                                        String::from("get_system_id"),
                                        t_error.to_string(),
                                    )));
                                }
                            }
                        }
                        if self.search_text.is_empty() {
                            self.search_results.clear();
                            self.search_selected_row = None;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.emit_notification, "Notify");
                    if ui.button("Clear").clicked() {
                        self.search_text.clear();
                        self.search_results.clear();
                        self.search_selected_row = None;
                    }
                    if ui.button("Advanced >>>").clicked() {}
                });
                ui.push_id("search_table", |ui| {
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto())
                        .column(Column::remainder())
                        .min_scrolled_height(0.0);

                    table = table.sense(egui::Sense::click());
                    table
                        .header(20.0, |mut header| {
                            header.col(|ui| {
                                ui.strong("System");
                            });
                            header.col(|ui| {
                                ui.strong("Region");
                            });
                        })
                        .body(|mut body| {
                            for row_index in 0..self.search_results.len() {
                                body.row(18.00, |mut row| {
                                    row.set_selected(false);
                                    if let Some(selected_row) = self.search_selected_row
                                        && row_index == selected_row
                                    {
                                        row.set_selected(true);
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].1).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let system_id = self.search_results[row_index].0;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                system_id.try_into().unwrap(),
                                                Target::System,
                                            )));
                                            if self.emit_notification {
                                                let _result =
                                                    tx_map.send(MapSync::SystemNotification((
                                                        system_id.try_into().unwrap(),
                                                        tokio::time::Instant::now(),
                                                    )));
                                            }
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let system_id = self.search_results[row_index].0;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            system_id.try_into().unwrap(),
                                            Target::System,
                                        )));
                                        if self.emit_notification {
                                            let _result =
                                                tx_map.send(MapSync::SystemNotification((
                                                    system_id.try_into().unwrap(),
                                                    tokio::time::Instant::now(),
                                                )));
                                        }
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].3).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let region_id = self.search_results[row_index].2;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                region_id.try_into().unwrap(),
                                                Target::Region,
                                            )));
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let region_id = self.search_results[row_index].2;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            region_id.try_into().unwrap(),
                                            Target::Region,
                                        )));
                                    }
                                    if row.response().clicked() {
                                        self.search_selected_row = Some(row_index);
                                    }
                                });
                                //self.toggle_row_selection(row_index, &row.response());
                            }
                            if self.search_results.is_empty() {
                                body.row(18.00, |mut row| {
                                    row.col(|ui| {
                                        ui.label("No result(s)");
                                    });
                                    row.col(|_ui| {});
                                });
                            }
                        });
                    //
                });
            });
    }
}
