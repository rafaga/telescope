//! The browser entry point: starts `eframe` on the page's canvas, downloads
//! `universe.json` and shows the universe map once it arrives.

use crate::universe::build_map;
use eframe::egui;
use egui_map::map::Map;
use std::cell::RefCell;
use std::rc::Rc;
use universe_snapshot::UniverseSnapshot;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Id of the `<canvas>` in `index.html`.
const CANVAS_ID: &str = "the_canvas_id";
/// The map data, served next to the page (copied there by `index.html`).
const UNIVERSE_URL: &str = "universe.json";

/// State of the universe download, shared with the task that fetches it.
enum Download {
    Pending,
    Ready(UniverseSnapshot),
    Failed(String),
}

/// Starts the app; called from `main`.
pub fn start() {
    tracing_wasm::set_as_global_default();

    wasm_bindgen_futures::spawn_local(async {
        let result = run().await;
        let loading_text = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("loading_text"));
        if let Some(loading_text) = loading_text {
            match result {
                Ok(()) => loading_text.remove(),
                Err(error) => {
                    tracing::error!("failed to start: {error}");
                    loading_text.set_inner_html(
                        "<p> The app has crashed. See the developer console for details. </p>",
                    );
                }
            }
        }
    });
}

async fn run() -> Result<(), String> {
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(CANVAS_ID))
        .ok_or("the canvas element was not found")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "the canvas element is not a <canvas>".to_string())?;

    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(WebApp::new(cc)))),
        )
        .await
        .map_err(|error| format!("{error:?}"))
}

/// The web version of Telescope.
struct WebApp {
    download: Rc<RefCell<Download>>,
    map: Option<Map>,
}

impl WebApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let download = Rc::new(RefCell::new(Download::Pending));
        let slot = Rc::clone(&download);
        let ctx = cc.egui_ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch_bytes(UNIVERSE_URL)
                .await
                .and_then(|bytes| UniverseSnapshot::from_json(&bytes));
            *slot.borrow_mut() = match result {
                Ok(snapshot) => Download::Ready(snapshot),
                Err(error) => Download::Failed(error),
            };
            // Nothing else wakes the UI up when the data arrives.
            ctx.request_repaint();
        });
        Self {
            download,
            map: None,
        }
    }

    /// Builds the map once the download finished.
    fn poll_download(&mut self) {
        let mut download = self.download.borrow_mut();
        if !matches!(*download, Download::Ready(_)) {
            return;
        }
        if let Download::Ready(snapshot) = std::mem::replace(&mut *download, Download::Pending) {
            match build_map(&snapshot) {
                Some(map) => self.map = Some(map),
                None => {
                    *download = Download::Failed("the universe file has no systems".to_string());
                }
            }
        }
    }
}

impl eframe::App for WebApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.map.is_none() {
            self.poll_download();
        }
        let failure = match &*self.download.borrow() {
            Download::Failed(error) => Some(error.clone()),
            _ => None,
        };
        egui::CentralPanel::default().show(ui, |ui| match (&mut self.map, failure) {
            (Some(map), _) => {
                ui.add(map);
            }
            (None, Some(error)) => {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("Could not load the universe: {error}"),
                );
            }
            (None, None) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.spinner();
                    ui.label("Loading the universe...");
                });
            }
        });
    }
}

/// Downloads `url` with the browser's `fetch`.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("there is no window")?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(js_error)?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "fetch did not return a Response".to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {} fetching {url}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(js_error)?)
        .await
        .map_err(js_error)?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

fn js_error(error: JsValue) -> String {
    format!("{error:?}")
}
