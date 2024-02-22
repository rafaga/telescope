use eframe::egui;
use egui_extras::{Column, TableBuilder};
use eframe::Error;


#[derive(PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum TableType {
    Manual,
    Homogeneous,
    Heterogenous,
}

/// Shows off a table with dynamic layout
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Table{
    table_type: TableType,
    headers: Vec<String>,
    striped: bool,
    resizable: bool,
    clickable: bool,
    num_rows: usize,
    scroll_to_row_slider: usize,
    scroll_to_row: Option<usize>,
    selection: std::collections::HashSet<usize>,
    checked: bool,
}

impl Table{
    pub fn new() -> Self {
        Self::default()
    }

    fn add_header(mut self, name:String) -> Result<(),Error> {
        if !name.is_empty() {
            self.headers.push(name);
        } 
        Ok(())
    }

    fn table_ui(&mut self, ui: &mut egui::Ui) {

        let text_height = egui::TextStyle::Body
            .resolve(ui.style())
            .size
            .max(ui.spacing().interact_size.y);

        let mut table = TableBuilder::new(ui)
            .striped(self.striped)
            .resizable(self.resizable)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto())
            .column(Column::auto())
            .column(Column::initial(100.0).range(40.0..=300.0))
            .column(Column::initial(100.0).at_least(40.0).clip(true))
            .column(Column::remainder())
            .min_scrolled_height(0.0);

        if self.clickable {
            table = table.sense(egui::Sense::click());
        }

        if let Some(row_index) = self.scroll_to_row.take() {
            table = table.scroll_to_row(row_index, None);
        }

        table
            .header(20.0, |mut header| {
                for header_name in &self.headers {
                    header.col(|ui| {
                        ui.strong(header_name);
                    });
                }
            })
            .body(|mut body| match &self.table_type {
                TableType::Manual => {
                    for row_index in 0..1 {
                        let is_thick = thick_row(row_index);
                        let row_height = if is_thick { 30.0 } else { 18.0 };
                        body.row(row_height, |mut row| {
                            row.set_selected(self.selection.contains(&row_index));

                            row.col(|ui| {
                                ui.label(row_index.to_string());
                            });
                            row.col(|ui| {
                                ui.checkbox(&mut self.checked, "Click me");
                            });
                            row.col(|ui| {
                                expanding_content(ui);
                            });
                            row.col(|ui| {
                                ui.label(long_text(row_index));
                            });
                            row.col(|ui| {
                                ui.style_mut().wrap = Some(false);
                                if is_thick {
                                    ui.heading("Extra thick row");
                                } else {
                                    ui.label("Normal row");
                                }
                            });

                            self.toggle_row_selection(row_index, &row.response());
                        });
                    }
                }
                TableType::Homogeneous => {
                    body.rows(text_height, self.num_rows, |mut row| {
                        let row_index = row.index();
                        row.set_selected(self.selection.contains(&row_index));

                        row.col(|ui| {
                            ui.label(row_index.to_string());
                        });
                        row.col(|ui| {
                            ui.checkbox(&mut self.checked, "Click me");
                        });
                        row.col(|ui| {
                            expanding_content(ui);
                        });
                        row.col(|ui| {
                            ui.label(long_text(row_index));
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::Label::new("Thousands of rows of even height").wrap(false),
                            );
                        });

                        self.toggle_row_selection(row_index, &row.response());
                    });
                }
                TableType::Heterogenous => {
                    let row_height = |i: usize| if thick_row(i) { 30.0 } else { 18.0 };
                    body.heterogeneous_rows((0..self.num_rows).map(row_height), |mut row| {
                        let row_index = row.index();
                        row.set_selected(self.selection.contains(&row_index));

                        row.col(|ui| {
                            ui.label(row_index.to_string());
                        });
                        row.col(|ui| {
                            ui.checkbox(&mut self.checked, "Click me");
                        });
                        row.col(|ui| {
                            expanding_content(ui);
                        });
                        row.col(|ui| {
                            ui.label(long_text(row_index));
                        });
                        row.col(|ui| {
                            ui.style_mut().wrap = Some(false);
                            if thick_row(row_index) {
                                ui.heading("Extra thick row");
                            } else {
                                ui.label("Normal row");
                            }
                        });

                        self.toggle_row_selection(row_index, &row.response());
                    });
                }
            });

    }

    fn toggle_row_selection(&mut self, row_index: usize, row_response: &egui::Response) {
        if row_response.clicked() {
            if self.selection.contains(&row_index) {
                self.selection.remove(&row_index);
            } else {
                self.selection.insert(row_index);
            }
        }
    }
    
}

impl Default for Table {
    fn default() -> Self {
        Self {
            table_type: TableType::Manual,
            striped: true,
            resizable: true,
            clickable: true,
            num_rows: 0,
            scroll_to_row_slider: 0,
            scroll_to_row: None,
            selection: Default::default(),
            checked: false,
            headers: Vec::new(),
        }
    }
}

fn expanding_content(ui: &mut egui::Ui) {
    let width = ui.available_width().clamp(20.0, 200.0);
    let height = ui.available_height();
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        (1.0, ui.visuals().text_color()),
    );
}

fn long_text(row_index: usize) -> String {
    format!("Row {row_index} has some long text that you may want to clip, or it will take up too much horizontal space!")
}

fn thick_row(row_index: usize) -> bool {
    row_index % 6 == 0
}