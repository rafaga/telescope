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
use std::thread::ThreadId;

static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

/// The UI thread's id, recorded alongside the context. See [`request`] for
/// why `request` needs to tell that thread apart from every other one.
static UI_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Stores the UI context. Called on the first frame; later calls are ignored.
pub fn set_context(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    let _ = UI_THREAD.set(std::thread::current().id());
}

/// Asks for a new frame. Cheap and callable from any thread; several calls
/// before the next frame still produce a single frame.
///
/// Does nothing when called from the UI thread itself (the one that ran
/// [`set_context`]). That thread either already has a frame in flight -- in
/// which case a repaint is redundant -- or will drain the queue on its next
/// frame regardless. Forcing one here is actively unsafe: `egui::Context`'s
/// internal `RwLock` isn't reentrant, and `request_repaint` takes a read
/// lock on it. If the UI thread is already inside a `Context::write`/`read`
/// section -- e.g. `Context::tessellate`, held for the whole tessellation
/// pass -- and something logged from deep inside that section reaches here
/// through `log_bridge` (as `epaint`'s tessellator does on some text/font
/// paths), calling `request_repaint` re-enters the same lock on the same
/// thread and deadlocks forever, surfacing as epaint's own "Failed to
/// acquire RwLock read after 10s" debug panic. Background threads (the
/// intel detection thread, the file watcher, log records from dependencies)
/// are exactly what this function exists for, and still go through as before.
pub fn request() {
    if UI_THREAD.get().copied() == Some(std::thread::current().id()) {
        return;
    }
    if let Some(ctx) = CONTEXT.get() {
        ctx.request_repaint();
    }
}
