#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

#[cfg(feature = "puffin")]
fn start_puffin_server() {
    puffin::set_scopes_on(true); // tell puffin to collect data

    match puffin_http::Server::new("0.0.0.0:8585") {
        Ok(puffin_server) => {
            eprintln!("Run:  cargo install puffin_viewer && puffin_viewer --url 127.0.0.1:8585");

            /*std::process::Command::new("puffin_viewer")
            .arg("--url")
            .arg("127.0.0.1:8585")
            .spawn()
            .ok();*/

            // We can store the server if we want, but in this case we just want
            // it to keep running. Dropping it closes the server, so let's not drop it!
            #[allow(clippy::mem_forget)]
            std::mem::forget(puffin_server);
        }
        Err(err) => {
            eprintln!("Failed to start puffin server: {err}");
        }
    };
}

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    //tracing_subscriber::fmt::init();
    env_logger::init(); // Log to stderr (if you run with `RUST_LOG=debug`).
    #[cfg(feature = "puffin")]
    start_puffin_server(); // NOTE: you may only want to call this if the users specifies some flag or clicks a button!

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([400.0, 300.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(&include_bytes!("../assets/icon01-1024.png")[..])
                    .unwrap(),
            ),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Telescope",
        native_options,
        Box::new(|cc| Box::new(telescope::TelescopeApp::new(cc))),
    )
}

// when compiling to web using trunk.
#[cfg(target_arch = "wasm32")]
fn main() {
    // Make sure panics are logged using `console.error`.
    console_error_panic_hook::set_once();

    // Redirect tracing to console.log and friends:
    tracing_wasm::set_as_global_default();

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        eframe::start_web(
            "T3l3SC0P3", // hardcode it
            web_options,
            Box::new(|cc| Box::new(telescope::TelescopeApp::new(cc))),
        )
        .await
        .expect("failed to start eframe");
    });
}
