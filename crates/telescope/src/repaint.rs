//! Wakes the UI from other threads.
//!
//! egui only runs `update()` when something asks for a frame: input, an
//! animation, or `Context::request_repaint`. Work finished off the UI thread
//! -- a chat log line read by the watcher, a line evaluated by the intel
//! detection thread, a log record from a dependency -- reaches the UI through
//! a queue that is only drained inside `update()`, so whoever fills the queue
//! must also ask for a frame, or the result waits for the next mouse move.
//!
//! The context is only known once the first frame runs ([`set_context`]);
//! wake-ups requested before that are dropped, and that first frame drains
//! whatever was queued anyway.

use eframe::egui;
use std::sync::OnceLock;

static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

/// Stores the UI context. Called on the first frame; later calls are ignored.
pub fn set_context(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
}

/// Asks for a new frame. Cheap and callable from any thread; several calls
/// before the next frame still produce a single frame.
pub fn request() {
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}
