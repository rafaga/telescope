#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

// Tracy memory profiling: reports every allocation/deallocation made through
// the global allocator to Tracy's memory pane (live usage, alloc/free
// timeline, and allocation-to-zone correlation). `tracing_tracy::client` is
// a re-export of `tracy_client`, so this needs no extra dependency beyond
// `tracing-tracy` itself. The `0` callstack-depth argument means allocations
// are NOT tied to a call stack (cheap); passing a non-zero depth would also
// capture the allocation site, at a real runtime cost. Native-only and
// feature-gated to match the target-gated `tracing-tracy` dependency in
// Cargo.toml.
#[cfg(all(feature = "profile-with-tracy", not(target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: tracing_tracy::client::ProfiledAllocator<std::alloc::System> =
    tracing_tracy::client::ProfiledAllocator::new(std::alloc::System, 0);

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    // Bridge `log`-based diagnostics from dependencies (hyper, notify, wgpu,
    // rfesi, ...) into `tracing`, so they reach the subscriber(s) set up
    // below instead of being silently dropped now that `env_logger` --
    // which used to own the `log` sink -- is gone.
    tracing_log::LogTracer::init().expect("installing the log-to-tracing bridge");

    // Wire up `tracing` before any span/event/log-bridged-record runs
    // anywhere in the process (sde, egui-map, webb, native_tools and
    // telescope itself all share this same instrumentation):
    //   - `fmt` prints to stderr, filtered by `RUST_LOG` -- same role and
    //     same env var `env_logger::init()` used to have.
    //   - Tracy (only wired up under `profile-with-tracy`) is deliberately
    //     NOT filtered by `RUST_LOG`, so quieting stderr never hides
    //     anything from the profiler. `TracyLayer::default()` starts the
    //     shared `tracy_client::Client` itself (`Client::start()` is
    //     idempotent); open the Tracy desktop app to connect, it
    //     auto-discovers the running process.
    use tracing_subscriber::Layer as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_filter(tracing_subscriber::EnvFilter::from_default_env());

    #[cfg(feature = "profile-with-tracy")]
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(tracing_tracy::TracyLayer::default()),
    )
    .expect("setting the global tracing subscriber");
    #[cfg(not(feature = "profile-with-tracy"))]
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(fmt_layer))
        .expect("setting the global tracing subscriber");

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([400.0, 300.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(&include_bytes!("../../../assets/icon.png")[..])
                    .unwrap(),
            ),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "Telescope",
        native_options,
        Box::new(|cc| Ok(Box::new(telescope::TelescopeApp::new(cc)))),
    )
}

// when compiling to web using trunk.
#[cfg(target_arch = "wasm32")]
fn main() {
    // Bridge `log`-based diagnostics from dependencies into `tracing`, same
    // role as on native.
    tracing_log::LogTracer::init().expect("installing the log-to-tracing bridge");

    // Redirect tracing spans/events (including the ones just bridged from
    // `log`) to the browser console (`console.log` and friends), replacing
    // `eframe::WebLogger`. `DEBUG` matches the level `WebLogger::init` used
    // before.
    tracing_wasm::set_as_global_default_with_config(
        tracing_wasm::WASMLayerConfigBuilder::new()
            .set_max_level(tracing::Level::DEBUG)
            .build(),
    );

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let start_result = eframe::WebRunner::new()
            .start(
                "T3l3SC0P3",
                web_options,
                Box::new(|cc| Ok(Box::new(eframe_template::TemplateApp::new(cc)))),
            )
            .await;

        // Remove the loading text and spinner:
        let loading_text = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("loading_text"));
        if let Some(loading_text) = loading_text {
            match start_result {
                Ok(_) => {
                    loading_text.remove();
                }
                Err(e) => {
                    loading_text.set_inner_html(
                        "<p> The app has crashed. See the developer console for details. </p>",
                    );
                    panic!("Failed to start eframe: {e:?}");
                }
            }
        }
    });
}
