//! Every component of egui-ide in one window, to try them out:
//!
//! ```text
//! cargo run -p egui-ide --example gallery
//! ```
//!
//! * the toolbars at the top (drag a group by its grip to move it);
//! * the toolbox at the left (click an entry or drag it onto the canvas);
//! * the property grid at the right, with a property of every kind;
//! * a log at the bottom of what the components report.

use eframe::egui::{self, Color32, RichText, Sense};
use egui_ide::{
    PropertyDescriptor, PropertyGrid, PropertyIssue, PropertyKind, PropertyLabels,
    PropertyModel,
    PropertyValue, ToolbarIcon, ToolbarPanel, Toolbox, ToolboxEntry,
};
use std::cell::RefCell;

/// The state of the window.
struct Gallery {
    model: PropertyModel,
    log: Vec<String>,
    show_toolbox: bool,
    show_properties: bool,
    bold: bool,
    snap: bool,
    zoom: usize,
    mode: usize,
}

impl Gallery {
    /// The window with a model that has a property of every kind.
    fn new() -> Self {
        let mut model = PropertyModel::new().labels(PropertyLabels::default());
        model.group("Text", |group| {
            group
                .property(
                    PropertyDescriptor::new("name", PropertyKind::Text)
                        .label("Single line")
                        .description("A line of text.")
                        .default_to(PropertyValue::Text("Mining detection".to_owned())),
                )
                .property(
                    PropertyDescriptor::new("template", PropertyKind::Multiline)
                        .label("Multiline")
                        .description("Several lines of text."),
                )
                .property(
                    PropertyDescriptor::new("file", PropertyKind::File)
                        .label("File")
                        .default_to(PropertyValue::Text("/logs/rules.toml".to_owned())),
                )
                .property(
                    PropertyDescriptor::new("folder", PropertyKind::Folder)
                        .label("Folder")
                        .default_to(PropertyValue::Text("/logs/Gamelogs".to_owned())),
                )
                .property(
                    PropertyDescriptor::new("since", PropertyKind::Date)
                        .label("Date")
                        .default_to(PropertyValue::Text("2026-11-16".to_owned())),
                )
                .property(
                    PropertyDescriptor::new("serial", PropertyKind::ReadOnly)
                        .label("Read-only")
                        .default_to(PropertyValue::Text("a1f3-7c2e".to_owned())),
                );
        });
        model.group("Numbers", |group| {
            group
                .property(
                    PropertyDescriptor::new("count", PropertyKind::Int)
                        .label("Integer")
                        .default_to(PropertyValue::Int(42)),
                )
                .property(
                    PropertyDescriptor::new("delay", PropertyKind::Int)
                        .label("With unit")
                        .unit("ms")
                        .range(0.0, 5000.0)
                        .default_to(PropertyValue::Int(250)),
                )
                .property(
                    PropertyDescriptor::new("min", PropertyKind::Int)
                        .label("Min")
                        .default_to(PropertyValue::Int(10)),
                )
                .property(
                    PropertyDescriptor::new("max", PropertyKind::Int)
                        .label("Max")
                        .default_to(PropertyValue::Int(120)),
                )
                .property(
                    PropertyDescriptor::new("ratio", PropertyKind::Float)
                        .label("Decimal")
                        .range(0.0, 1.0)
                        .default_to(PropertyValue::Float(0.5)),
                );
        });
        model.group("Selection", |group| {
            group
                .property(
                    PropertyDescriptor::new("enabled", PropertyKind::Bool)
                        .label("Boolean")
                        .default_to(PropertyValue::Bool(true)),
                )
                .property(
                    PropertyDescriptor::new(
                        "mode",
                        PropertyKind::Choice(vec![
                            "Contains".to_owned(),
                            "Starts with".to_owned(),
                            "Matches".to_owned(),
                        ]),
                    )
                    .label("Enum"),
                )
                .property(
                    PropertyDescriptor::new(
                        "flags",
                        PropertyKind::Flags(vec![
                            "Alert".to_owned(),
                            "Sound".to_owned(),
                            "Notification".to_owned(),
                        ]),
                    )
                    .label("Flags")
                    .default_to(PropertyValue::Flags(vec![true, false, true])),
                )
                .property(
                    PropertyDescriptor::new("color", PropertyKind::Color)
                        .label("Color")
                        .default_to(PropertyValue::Color([0xE0, 0xA0, 0x30])),
                );
        });
        model.group("Collections", |group| {
            group
                .property(
                    PropertyDescriptor::new("words", PropertyKind::TextList)
                        .label("String list")
                        .default_to(PropertyValue::TextList(vec![
                            "alpha".to_owned(),
                            "beta".to_owned(),
                        ])),
                )
                .property(
                    PropertyDescriptor::new("pairs", PropertyKind::KeyValue)
                        .label("Key / value")
                        .default_to(PropertyValue::KeyValue(vec![
                            ("ore".to_owned(), "veldspar".to_owned()),
                            ("ice".to_owned(), "white glaze".to_owned()),
                        ])),
                )
                .property(
                    PropertyDescriptor::new("target", PropertyKind::Reference)
                        .label("Reference")
                        .default_to(PropertyValue::Text("Formatter".to_owned())),
                );
        });
        model.group("States and actions", |group| {
            group
                .property(
                    PropertyDescriptor::new("positive", PropertyKind::Int)
                        .label("Error")
                        .default_to(PropertyValue::Int(-5))
                        .rule(|value| match value.as_int() {
                            Some(number) if number <= 0 => {
                                Some(PropertyIssue::error("It has to be greater than 0."))
                            }
                            _ => None,
                        }),
                )
                .property(
                    PropertyDescriptor::new("pattern", PropertyKind::Text)
                        .label("Warning")
                        .default_to(PropertyValue::Text("*".to_owned()))
                        .rule(|value| match value.as_text() {
                            Some("*") => Some(PropertyIssue::warning("It matches every line.")),
                            _ => None,
                        }),
                )
                .property(
                    PropertyDescriptor::new("locked", PropertyKind::Text)
                        .label("Disabled")
                        .enabled(false)
                        .default_to(PropertyValue::Text("Local".to_owned())),
                )
                .property(
                    PropertyDescriptor::new("run", PropertyKind::Action("Run".to_owned()))
                        .label("Test rule"),
                )
                .property(
                    PropertyDescriptor::new(
                        "help",
                        PropertyKind::Link("Open help \u{2197}".to_owned()),
                    )
                    .label("Documentation"),
                );
        });
        Self {
            model,
            log: Vec::new(),
            show_toolbox: true,
            show_properties: true,
            bold: false,
            snap: true,
            zoom: 1,
            mode: 0,
        }
    }

    /// The groups of toolbars, which the user moves by their grip.
    fn toolbars(&mut self, ui: &mut egui::Ui) {
        // every group reports into the same log
        let events = RefCell::new(Vec::<String>::new());
        let say = |text: &str| events.borrow_mut().push(text.to_owned());
        let zooms = ["50%", "100%", "200%"];
        let modes = ["Rule", "All rules", "Both"];
        ToolbarPanel::new("gallery_toolbars")
            .group("file", "File", |bar| {
                if bar.icon_button(ToolbarIcon::Done, "Done").clicked() {
                    say("Done");
                }
                if bar.icon_button(ToolbarIcon::Clear, "Clear").clicked() {
                    say("Clear");
                }
            })
            .group("edit", "Edit", |bar| {
                if bar.icon_button(ToolbarIcon::Undo, "Undo").clicked() {
                    say("Undo");
                }
                bar.enabled(false, |bar| {
                    bar.icon_button(ToolbarIcon::Redo, "Redo");
                });
                bar.separator();
                if bar
                    .icon_button(ToolbarIcon::Duplicate, "Duplicate")
                    .clicked()
                {
                    say("Duplicate");
                }
                if bar.icon_button(ToolbarIcon::Delete, "Delete").clicked() {
                    say("Delete");
                }
            })
            .group("view", "View", |bar| {
                bar.icon_toggle(&mut self.show_toolbox, ToolbarIcon::Toolbox, "Toolbox");
                bar.icon_toggle(
                    &mut self.show_properties,
                    ToolbarIcon::Properties,
                    "Properties",
                );
                bar.separator();
                if bar.icon_button(ToolbarIcon::ZoomOut, "Zoom out").clicked() {
                    say("Zoom out");
                }
                if bar.icon_button(ToolbarIcon::ZoomIn, "Zoom in").clicked() {
                    say("Zoom in");
                }
                if bar.icon_button(ToolbarIcon::FitView, "Fit view").clicked() {
                    say("Fit view");
                }
            })
            .group("rules", "Rules", |bar| {
                let main = bar.split_button(Some(ToolbarIcon::Play), "Test", |ui| {
                    for variant in ["Test rule", "Test all", "Test with file\u{2026}"] {
                        if ui.button(variant).clicked() {
                            say(variant);
                            ui.close();
                        }
                    }
                });
                if main.clicked() {
                    say("Test");
                }
                bar.separator();
                if bar.combo("zoom", &mut self.zoom, &zooms, 70.0, "Zoom") {
                    say("Zoom changed");
                }
                if bar.combo("mode", &mut self.mode, &modes, 90.0, "Mode") {
                    say("Mode changed");
                }
            })
            .group("buttons", "Buttons", |bar| {
                if bar.button("Text").clicked() {
                    say("Text button");
                }
                bar.toggle(&mut self.bold, "Toggle");
                bar.separator();
                if bar
                    .icon_text_button(ToolbarIcon::Done, "Accept", "Accept")
                    .clicked()
                {
                    say("Accept");
                }
                bar.icon_text_toggle(&mut self.snap, ToolbarIcon::FitView, "Snap", "Snap to grid");
            })
            .show(ui);
        self.log.extend(events.into_inner());
    }

    /// The toolbox: a click on an entry is reported.
    fn toolbox(&mut self, ui: &mut egui::Ui) {
        let picked = Toolbox::new("gallery_toolbox")
            .header("Toolbox", "Drag onto the canvas or click to add.")
            .group(
                "Detections",
                [
                    ToolboxEntry::new("Input", "Input").color(Color32::from_rgb(0x4A, 0xA3, 0xDF)),
                    ToolboxEntry::new("Detection", "Detection")
                        .color(Color32::from_rgb(0xE0, 0xA0, 0x30)),
                ],
            )
            .group(
                "Logic",
                [
                    ToolboxEntry::new("Aggregator", "Aggregator")
                        .color(Color32::from_rgb(0x9A, 0x7F, 0xD1)),
                    ToolboxEntry::new("Formatter", "Formatter")
                        .color(Color32::from_rgb(0x5F, 0xB8, 0x78)),
                ],
            )
            .group(
                "Outputs",
                [
                    ToolboxEntry::new("Output", "Output")
                        .color(Color32::from_rgb(0xD9, 0x6B, 0x6B)),
                    ToolboxEntry::new("Present", "Already there")
                        .enabled(false)
                        .disabled_hint("There is one already."),
                ],
            )
            .show(ui);
        if let Some(item) = picked {
            self.log.push(format!("Toolbox click: {item}"));
        }
    }

    /// The property grid, and what its buttons and links report.
    fn properties(&mut self, ui: &mut egui::Ui) {
        let changed = PropertyGrid::new("gallery_properties")
            .name_width(120.0)
            .empty_hint("Nothing to show.")
            .show_model(ui, &mut self.model);
        for id in changed {
            let shown = self
                .model
                .get(&id)
                .map_or_else(String::new, ToString::to_string);
            self.log.push(format!("Changed {id}: {shown}"));
        }
        for id in self.model.take_activated() {
            self.log.push(format!("Activated {id}"));
        }
    }
}

impl eframe::App for Gallery {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbars")
            .frame(egui::Frame::side_top_panel(ui.style()))
            .resizable(false)
            .show(ui, |ui| self.toolbars(ui));
        egui::Panel::bottom("log").resizable(false).show(ui, |ui| {
            let from = self.log.len().saturating_sub(4);
            if self.log.is_empty() {
                ui.label(RichText::new("Nothing reported yet.").weak());
            }
            for line in &self.log[from..] {
                ui.label(line);
            }
        });
        if self.show_toolbox {
            egui::Panel::left("toolbox_panel")
                .resizable(true)
                .default_size(190.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.toolbox(ui));
                });
        }
        if self.show_properties {
            egui::Panel::right("properties_panel")
                .resizable(true)
                .default_size(340.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.properties(ui));
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let canvas = ui.allocate_response(ui.available_size(), Sense::hover());
            ui.painter().text(
                canvas.rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop an entry of the toolbox here",
                egui::FontId::proportional(16.0),
                ui.visuals().weak_text_color(),
            );
            if let Some(item) = Toolbox::<&str>::dropped(&canvas) {
                self.log.push(format!("Dropped on the canvas: {item}"));
            }
        });
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "egui-ide gallery",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(Gallery::new()))),
    )
}
