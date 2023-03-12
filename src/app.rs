use egui::FontData;
use egui::FontDefinitions;
use egui::FontFamily;
use sde::objects::SystemPoint;
use crate::app::map::Map;
use sde::SdeManager;
use std::path::Path;
use std::sync::mpsc::{self,Sender,Receiver};
use std::thread;
use crate::app::messages::Message;

pub mod messages;
pub mod map;


/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)] // if we add new fields, give them default values when deserializing old state
pub struct TemplateApp {
    // Example stuff:
    label: String,

    #[serde(skip)]
    initialized: bool,

    // 2d point to paint map
    #[serde(skip)]
    points: Vec<SystemPoint>,

    #[serde(skip)]
    map: Map,

    #[serde(skip)]
    tx: Sender<Message>,

    #[serde(skip)]
    rx: Receiver<Message>,

}

impl Default for TemplateApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel::<messages::Message>();
        Self {
            // Example stuff:
            label: "Hello World!".to_owned(),
            initialized: false,
            points: Vec::new(),
            map: Map::new(),
            tx,
            rx,
        }
    }
}

impl eframe::App for TemplateApp {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.event_manager();
        let Self {
            label,
            initialized: _,
            points: _points,
            map: _map,
            tx: _tx,
            rx: _rx,
        } = self;

        if self.initialized == false {
            let txs = self.tx.clone();
            let factor = 10000000000000;
            thread::spawn(move ||{
                let path = Path::new("assets/sde-isometric.db");
                let manager = SdeManager::new(path, factor); 
                if let Ok(points) = manager.get_systempoints(2) {
                    let mut obj_vec = Vec::new();
                    for point in points{
                        let vec_cords = vec![point.coords[0], point.coords[1]];
                        let object = SystemPoint::new(point.id,vec_cords);
                        obj_vec.push(object);
                    }
                    let _result = txs.send(Message::Processed2dMatrix(obj_vec));
                }
            });
            self.initialized = true;
        }

        
        // Examples of how to create different panels and windows.
        // Pick whichever suits you.
        // Tip: a good default choice is to just keep the `CentralPanel`.
        // For inspiration and more examples, go to https://emilk.github.io/egui

        #[cfg(not(target_arch = "wasm32"))] // no File->Quit on web pages!
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Read SDE").clicked() {

                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        _frame.close();
                    }
                });
            });
        });

        // Bottom menu
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            // The top panel is often a good place for a menu bar:
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.spinner();
                ui.label("Initializing ... ");
                ui.separator();
                egui::warn_if_debug_build(ui);
            });
        });

        egui::SidePanel::left("side_panel")
        .resizable(true)
        .show(ctx, |ui| {
            ui.heading("Side Panel");

            ui.horizontal(|ui| {
                ui.label("Write something: ");
                ui.text_edit_singleline(label);
            });

            /*ui.add(egui::Slider::new(value, 0.0..=10.0).text("value"));
            if ui.button("Increment").clicked() {
                *value += 1.0;
            }*/
        });

        egui::CentralPanel::default()
        .show(ctx, |ui| {
            
            // The central panel the region left after adding TopPanel's and SidePanel's
            /* 
            ui.heading("eframe template");
            ui.hyperlink("https://github.com/emilk/eframe_template");
            ui.add(egui::github_link_file!(
                "https://github.com/emilk/eframe_template/blob/master/",
                "Source code."
            ));
            */
            ui.add(&mut self.map);
            /*if let Some(points) = self.universe.points {

            }*/
            //ui.label("鑑於對人類家庭所有成員的固有尊嚴及其平等的和不移的權利的承認，乃是世界自由、正義與和平的基礎");
            
        });

        if false {
            egui::Window::new("Window").show(ctx, |ui| {
                ui.label("Windows can be moved by dragging them.");
                ui.label("They are automatically sized based on contents.");
                ui.label("You can turn on resizing and scrolling if you like.");
                ui.label("You would normally choose either panels OR windows.");
            });
        }
    }
}

impl TemplateApp {
    fn initialize_application(&mut self) -> () {
    }

    fn event_rcv_generic_update(&self, _message: String) -> () {

    }

    fn event_rcv_error(&self, _message: String) -> () {

    }


    fn event_manager(&mut self) -> () {
        let received_data = self.rx.try_recv(); 
        if let Ok(msg) = received_data{
            match msg{
                Message::GenericUpdate(msg) => self.event_rcv_error(msg),
                Message::Error(msg) => self.event_rcv_generic_update(msg),
                Message::Processed2dMatrix(points) => self.map.add_points(points),
            };
        }
    }

    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.
        // cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "Noto Sans Regular".to_owned(),
            FontData::from_static(include_bytes!("../assets/NotoSansTC-Regular.otf")),
        );
        fonts.families.get_mut(&FontFamily::Proportional).unwrap()
        .insert(0, "Noto Sans Regular".to_owned());
        cc.egui_ctx.set_fonts(fonts);
        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            return eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
        }
        let mut app:TemplateApp = Default::default();
        app.initialize_application();
        app
    }
}
