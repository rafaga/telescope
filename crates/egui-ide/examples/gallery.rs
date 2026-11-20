//! Every component of egui-ide in one window, to try them out:
//!
//! ```text
//! cargo run -p egui-ide --example gallery
//! ```
//!
//! * the toolbars at the top (drag a group by its grip to move it);
//! * the toolbox at the left (click an entry or drag it onto the canvas);
//! * the property grid at the right, with a property of every kind;
//! * the canvas in the middle (drag the background to pan, the wheel to zoom,
//!   a node to move it, shift and drag the background to select with a
//!   rectangle, a port onto another one to join them (let go over nothing it
//!   offers a node that fits), double click an edge to remove it; the minimap at the corner
//!   moves the view);
//! * a log at the bottom of what the components report.

use eframe::egui::{self, Color32, RichText};
use egui_ide::{
    PropertyDescriptor, PropertyGrid, PropertyIssue, PropertyKind, PropertyLabels, PropertyModel,
    PropertyValue, ToolbarIcon, ToolbarPanel, Toolbox, ToolboxEntry,
};
use egui_weave::{
    Canvas, CanvasEvent, CanvasModel, CanvasStyle, CanvasView, EdgeState, History, NoCycles,
    NodeControl, NodeDescriptor, NodeId, PortDescriptor, PortDirection, PortRef,
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
    mode: usize,
    canvas: CanvasModel,
    view: CanvasView,
    history: History,
    viewport: egui::Rect,
    fit_pending: bool,
    over_canvas: bool,
    runs: usize,
    counter: usize,
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
                    PropertyDescriptor::new("alert", PropertyKind::Bool)
                        .label("Alert")
                        .default_to(PropertyValue::Bool(true)),
                )
                .property(PropertyDescriptor::new("sound", PropertyKind::Bool).label("Sound"))
                .property(
                    PropertyDescriptor::new("notification", PropertyKind::Bool)
                        .label("Notification")
                        .default_to(PropertyValue::Bool(true)),
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
            mode: 0,
            canvas: Self::seed(),
            view: CanvasView::new(),
            history: History::new(),
            viewport: egui::Rect::NOTHING,
            fit_pending: true,
            over_canvas: false,
            runs: 0,
            counter: 0,
        }
    }

    /// A node for an entry of the toolbox, at `pos`; `None` for an entry that
    /// is not a node.
    fn node_for(kind: &str, id: String, pos: egui::Pos2) -> Option<NodeDescriptor> {
        let node = match kind {
            "Input" => NodeDescriptor::new(id, "Input")
                .summary("Intel channel")
                .accent(Color32::from_rgb(0x4A, 0xA3, 0xDF))
                .output(PortDescriptor::new("out", "Lines").kind("text")),
            "Detection" => NodeDescriptor::new(id, "Detection")
                .summary("Contains text")
                .accent(Color32::from_rgb(0xE0, 0xA0, 0x30))
                .input(PortDescriptor::new("in", "Lines").kind("text").max_edges(1))
                .output(PortDescriptor::new("true", "True").kind("signal"))
                .output(PortDescriptor::new("false", "False").kind("signal"))
                .control(NodeControl::check("case", "Ignore case", true))
                .control(NodeControl::radio(
                    "match",
                    "Match",
                    ["Contains", "Whole word"],
                    0,
                )),
            "Aggregator" => NodeDescriptor::new(id, "Aggregator")
                .summary("Any of")
                .accent(Color32::from_rgb(0x9A, 0x7F, 0xD1))
                .input(PortDescriptor::new("in", "Signals").kind("signal"))
                .output(PortDescriptor::new("out", "Signal").kind("signal")),
            "Formatter" => NodeDescriptor::new(id, "Formatter")
                .summary("Message template")
                .accent(Color32::from_rgb(0x5F, 0xB8, 0x78))
                .input(PortDescriptor::new("in", "Signal").kind("signal"))
                .output(PortDescriptor::new("out", "Text").kind("text"))
                .control(NodeControl::switch("short", "Short form", false)),
            "Output" => NodeDescriptor::new(id, "Output")
                .summary("Alert and sound")
                .accent(Color32::from_rgb(0xD9, 0x6B, 0x6B))
                .input(PortDescriptor::new("in", "Signal").kind("signal"))
                .control(NodeControl::switch("sound", "Sound", true))
                .control(NodeControl::button("preview", "Preview")),
            _ => return None,
        };
        Some(node.at(pos))
    }

    /// A small graph to start with: an input, a detection and an output.
    fn seed() -> CanvasModel {
        let mut model = CanvasModel::new();
        model.rule(NoCycles);
        for (kind, id, pos) in [
            ("Input", "input", egui::pos2(0.0, 0.0)),
            ("Detection", "detection", egui::pos2(260.0, 0.0)),
            ("Output", "output", egui::pos2(540.0, 20.0)),
        ] {
            if let Some(node) = Self::node_for(kind, id.to_owned(), pos) {
                model.add_node(node).expect("the seed nodes are different");
            }
        }
        model
            .connect(
                PortRef::new("input", "out"),
                PortRef::new("detection", "in"),
            )
            .expect("the seed ports fit");
        let edge = model
            .connect(
                PortRef::new("detection", "true"),
                PortRef::new("output", "in"),
            )
            .expect("the seed ports fit");
        model.set_edge_state(edge, EdgeState::True);
        model.take_events();
        model
    }

    /// Adds the node of a toolbox entry at `pos` (in the canvas).
    fn add_node(&mut self, kind: &str, pos: egui::Pos2) -> Option<NodeId> {
        Self::add_node_to(
            &mut self.canvas,
            &mut self.counter,
            &mut self.log,
            kind,
            pos,
        )
    }

    /// Adds the node of a toolbox entry to `model`; what goes wrong is logged.
    fn add_node_to(
        model: &mut CanvasModel,
        counter: &mut usize,
        log: &mut Vec<String>,
        kind: &str,
        pos: egui::Pos2,
    ) -> Option<NodeId> {
        *counter += 1;
        let id = format!("{}-{}", kind.to_lowercase(), counter);
        let node = Self::node_for(kind, id, pos)?;
        let id = node.id().clone();
        match model.add_node(node) {
            Ok(()) => Some(id),
            Err(error) => {
                log.push(format!("Not added: {error}"));
                None
            }
        }
    }

    /// Whether two ports can be joined as far as their kinds of data go: a
    /// port without kind takes any.
    fn kinds_fit(a: Option<&str>, b: Option<&str>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }

    /// The content of the menu of an edge that was let go over nothing: the
    /// nodes with a port that fits, and the one that is chosen is added and
    /// joined. Returns whether something was chosen.
    fn offer_nodes(
        ui: &mut egui::Ui,
        model: &mut CanvasModel,
        from: &PortRef,
        pos: egui::Pos2,
        counter: &mut usize,
        log: &mut Vec<String>,
    ) -> bool {
        let Some((from_output, kind)) = model
            .node(&from.node)
            .and_then(|node| node.port(&from.port))
            .map(|(side, port)| {
                (
                    side == PortDirection::Output,
                    port.kind_name().map(str::to_owned),
                )
            })
        else {
            return true;
        };
        ui.label(RichText::new("Create a node here").weak());
        let mut chosen = None;
        for name in ["Input", "Detection", "Aggregator", "Formatter", "Output"] {
            let Some(node) = Self::node_for(name, String::new(), egui::Pos2::ZERO) else {
                continue;
            };
            let ports = if from_output {
                node.inputs()
            } else {
                node.outputs()
            };
            let fits = ports
                .iter()
                .any(|port| Self::kinds_fit(port.kind_name(), kind.as_deref()));
            if fits && ui.button(name).clicked() {
                chosen = Some(name);
            }
        }
        let Some(name) = chosen else {
            return false;
        };
        let Some(id) = Self::add_node_to(model, counter, log, name, pos) else {
            return true;
        };
        let joined = model.node(&id).and_then(|node| {
            let ports = if from_output {
                node.inputs()
            } else {
                node.outputs()
            };
            let port = ports
                .iter()
                .find(|port| Self::kinds_fit(port.kind_name(), kind.as_deref()))?;
            Some(PortRef {
                node: id.clone(),
                port: port.id().clone(),
            })
        });
        if let Some(new_port) = joined {
            let (from, to) = if from_output {
                (from.clone(), new_port)
            } else {
                (new_port, from.clone())
            };
            if let Err(error) = model.connect(from, to) {
                log.push(format!("Not joined: {error}"));
            }
        }
        true
    }

    /// A made-up run of the graph, to see the states of the edges: it starts
    /// at the nodes without inputs and follows the edges; a detection takes
    /// its true branch when `yes` is set and its false branch otherwise, and
    /// the other branch is left absent, as are the edges that are not reached.
    fn simulate(model: &mut CanvasModel, yes: bool) {
        let edges: Vec<_> = model
            .edges()
            .iter()
            .map(|edge| (edge.id(), edge.from().clone(), edge.to().clone()))
            .collect();
        let mut reached: Vec<NodeId> = model
            .nodes()
            .iter()
            .filter(|node| node.inputs().is_empty())
            .map(|node| node.id().clone())
            .collect();
        let mut states = Vec::new();
        for _ in 0..=model.nodes().len() {
            for (id, from, to) in &edges {
                if states.iter().any(|(done, _)| done == id) || !reached.contains(&from.node) {
                    continue;
                }
                let is_detection = model
                    .node(&from.node)
                    .is_some_and(|node| node.title_text() == "Detection");
                let (taken, state) = if is_detection {
                    let is_true = from.port.as_str() == "true";
                    let taken = is_true == yes;
                    let state = match (taken, is_true) {
                        (false, _) => EdgeState::Absent,
                        (true, true) => EdgeState::True,
                        (true, false) => EdgeState::False,
                    };
                    (taken, state)
                } else {
                    (true, EdgeState::Neutral)
                };
                states.push((*id, state));
                if taken && !reached.contains(&to.node) {
                    reached.push(to.node.clone());
                }
            }
        }
        for (id, _, _) in &edges {
            let state = states
                .iter()
                .find(|(done, _)| done == id)
                .map_or(EdgeState::Absent, |(_, state)| *state);
            model.set_edge_state(*id, state);
        }
    }

    /// Fits the canvas to its nodes.
    fn fit(&mut self) {
        if let Some(content) = CanvasStyle::default().content_rect(&self.canvas) {
            self.view.fit(content, self.viewport, 40.0);
        }
    }

    /// What a toolbar button asked for, once the toolbars are drawn.
    fn act(&mut self, action: &str) {
        let anchor = self.viewport.center();
        match action {
            "Zoom in" => {
                self.view.zoom_in(anchor);
            }
            "Zoom out" => {
                self.view.zoom_out(anchor);
            }
            "Fit view" => self.fit(),
            "Test" | "Test rule" | "Test all" => {
                // the detections take one branch and then the other
                self.runs += 1;
                let yes = self.runs % 2 == 1;
                Self::simulate(&mut self.canvas, yes);
                let branch = if yes { "true" } else { "false" };
                self.log
                    .push(format!("Test run {}: {branch} branch", self.runs));
            }
            "Clear" => {
                let ids: Vec<_> = self.canvas.edges().iter().map(|edge| edge.id()).collect();
                for id in ids {
                    self.canvas.set_edge_state(id, EdgeState::Neutral);
                }
            }
            "Undo" => {
                self.history.undo(&mut self.canvas);
            }
            "Redo" => {
                self.history.redo(&mut self.canvas);
            }
            "Delete" => {
                for id in self.canvas.selection().to_vec() {
                    self.canvas.remove_node(&id);
                }
            }
            _ => {}
        }
    }

    /// A line of the log for something the canvas reported; `None` for what
    /// is too frequent to log (a node that is being moved).
    fn describe(event: &CanvasEvent) -> Option<String> {
        match event {
            CanvasEvent::NodeAdded(node) => Some(format!("Node added: {}", node.id())),
            CanvasEvent::NodeRemoved { node, edges } => Some(format!(
                "Node removed: {} ({} edges)",
                node.id(),
                edges.len()
            )),
            CanvasEvent::Connected(edge) => {
                Some(format!("Connected {} -> {}", edge.from(), edge.to()))
            }
            CanvasEvent::Disconnected(edge) => {
                Some(format!("Disconnected {} -> {}", edge.from(), edge.to()))
            }
            CanvasEvent::ControlChanged {
                node, control, to, ..
            } => Some(format!("{node}.{control} = {to:?}")),
            CanvasEvent::ButtonPressed { node, control } => {
                Some(format!("Button pressed: {node}.{control}"))
            }
            _ => None,
        }
    }

    /// The groups of toolbars, which the user moves by their grip.
    fn toolbars(&mut self, ui: &mut egui::Ui) {
        // every group reports into the same log
        let events = RefCell::new(Vec::<String>::new());
        let say = |text: &str| events.borrow_mut().push(text.to_owned());
        let modes = ["Rule", "All rules", "Both"];
        let ctx = ui.ctx().clone();
        let dark_theme = ui.visuals().dark_mode;
        let (can_undo, can_redo) = (self.history.can_undo(), self.history.can_redo());
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
                bar.enabled(can_undo, |bar| {
                    if bar.icon_button(ToolbarIcon::Undo, "Undo").clicked() {
                        say("Undo");
                    }
                });
                bar.enabled(can_redo, |bar| {
                    if bar.icon_button(ToolbarIcon::Redo, "Redo").clicked() {
                        say("Redo");
                    }
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
                bar.separator();
                let light = if dark_theme {
                    "Light"
                } else {
                    "\u{2713} Light"
                };
                if bar.button(light).clicked() {
                    ctx.set_theme(egui::ThemePreference::Light);
                    say("Light theme");
                }
                let dark = if dark_theme { "\u{2713} Dark" } else { "Dark" };
                if bar.button(dark).clicked() {
                    ctx.set_theme(egui::ThemePreference::Dark);
                    say("Dark theme");
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
        let events = events.into_inner();
        for action in &events {
            self.act(action);
        }
        self.log.extend(events);
    }

    /// The toolbox: a click on an entry is reported.
    fn toolbox(&mut self, ui: &mut egui::Ui) {
        let picked = Toolbox::new("gallery_toolbox")
            // over the canvas the node itself is previewed
            .ghost(!self.over_canvas)
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
            let center = self.view.screen_to_canvas(self.viewport.center());
            self.add_node(item, center - egui::vec2(90.0, 30.0));
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
        // ctrl+z undoes and ctrl+shift+z or ctrl+y redo, unless a text is
        // being typed
        if !ui.ctx().egui_wants_keyboard_input() {
            let undo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
            let redo = egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Z,
            );
            let redo_y = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
            // the one with more modifiers is asked first
            if ui
                .input_mut(|input| input.consume_shortcut(&redo) || input.consume_shortcut(&redo_y))
            {
                self.act("Redo");
            } else if ui.input_mut(|input| input.consume_shortcut(&undo)) {
                self.act("Undo");
            }
        }
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
            self.viewport = ui.available_rect_before_wrap();
            if self.fit_pending {
                self.fit();
                self.fit_pending = false;
            }
            let (counter, log) = (&mut self.counter, &mut self.log);
            // the entry of the toolbox that is being dragged, as a node
            let dragged = egui::DragAndDrop::payload::<&str>(ui.ctx())
                .and_then(|kind| Self::node_for(&kind, String::new(), egui::Pos2::ZERO));
            let response = Canvas::new("gallery_canvas")
                .drop_preview(dragged)
                .minimap(egui::vec2(180.0, 110.0))
                .zoom_slider(true)
                .drop_menu(|ui, model, from, pos| {
                    Self::offer_nodes(ui, model, from, pos, counter, log)
                })
                .show(ui, &mut self.canvas, &mut self.view);
            self.over_canvas = Toolbox::<&str>::hovered(&response);
            if let Some(item) = Toolbox::<&str>::dropped(&response) {
                self.log.push(format!("Dropped on the canvas: {item}"));
                let pos = ui
                    .ctx()
                    .pointer_latest_pos()
                    .unwrap_or_else(|| self.viewport.center());
                self.add_node(item, self.view.screen_to_canvas(pos));
            }
            let events = self.canvas.take_events();
            for event in &events {
                if let Some(line) = Self::describe(event) {
                    self.log.push(line);
                }
            }
            // a drag is one step: it lasts while the pointer is down
            let dragging = ui.input(|input| input.pointer.any_down());
            self.history.record(events, dragging);
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
