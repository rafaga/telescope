//! Telescope for the browser: a lightweight build that shows the universe map.
//!
//! Build and serve it from this crate's folder with `trunk serve` (or
//! `trunk build --release`). The map data is `assets/universe.json`, written from
//! `sde.db` by `cargo run --release -p telescope --example export_universe`.
//!
//! Natively this binary only prints that hint, so `cargo check --workspace`
//! keeps working on every platform.

// `universe` is only used by the web entry point.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod universe;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "telescope-web is the browser build: run `trunk serve` (or `trunk build`) \
         inside crates/telescope-web."
    );
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}
