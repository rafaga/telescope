//! Forwards `WARN`/`ERROR` diagnostics from dependencies to the on-screen log.
//!
//! Crates like `egui_extras` (image loaders), `webb`, `hyper` or `wgpu` report
//! their failures only through `tracing`/`log`, so until now they reached the
//! console (`RUST_LOG`) and Tracy but never the log panel at the bottom of the
//! window -- e.g. a character portrait that fails to download because there is
//! no network.
//!
//! [`PanelLayer`] is a `tracing_subscriber` layer (installed in `main.rs`) that
//! copies each such event into a small shared queue; the UI thread empties it
//! every frame with [`drain`] and shows the entries like any other
//! `GenericNotification`. The layer can fire on any thread (tokio workers,
//! the file watcher, the loader threads), so the queue is a `Mutex`, and
//! [`set_repaint_context`] lets it wake up an idle UI so the entry shows up
//! right away instead of on the next mouse move.
//!
//! Events from `telescope` itself are skipped: every `tracing::warn!` in this
//! crate that matters to the user already sends its own `GenericNotification`,
//! and forwarding them too would show them twice.

use eframe::egui;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Most entries kept waiting for the UI thread. A dependency stuck in a loop
/// (say, a loader retrying every frame) must not grow this without bound
/// while, for example, the window is minimized and no frame drains it; the
/// oldest entries are dropped first.
const MAX_PENDING: usize = 200;

/// A diagnostic waiting to be shown in the log panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// `true` for `ERROR`, `false` for `WARN`.
    pub is_error: bool,
    /// Crate that emitted it (first segment of the target), e.g. `egui_extras`.
    pub source: String,
    /// Rest of the target, e.g. `loaders::http_loader` (may be empty).
    pub context: String,
    /// The event's message, followed by its other fields as `name=value`.
    pub text: String,
}

static PENDING: Mutex<VecDeque<LogRecord>> = Mutex::new(VecDeque::new());
static REPAINT: OnceLock<egui::Context> = OnceLock::new();

/// Lets the layer request a repaint when a new entry arrives. Called once,
/// when the app starts; later calls are ignored.
pub fn set_repaint_context(ctx: &egui::Context) {
    let _ = REPAINT.set(ctx.clone());
}

/// Takes every entry received since the last call, oldest first.
pub fn drain() -> Vec<LogRecord> {
    match PENDING.lock() {
        Ok(mut pending) => pending.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

fn push(record: LogRecord) {
    if let Ok(mut pending) = PENDING.lock() {
        if pending.len() >= MAX_PENDING {
            pending.pop_front();
        }
        pending.push_back(record);
    }
    if let Some(ctx) = REPAINT.get() {
        ctx.request_repaint();
    }
}

/// Splits a target such as `egui_extras::loaders::http_loader` into
/// (`egui_extras`, `loaders::http_loader`).
fn split_target(target: &str) -> (String, String) {
    match target.split_once("::") {
        Some((krate, rest)) => (krate.to_owned(), rest.to_owned()),
        None => (target.to_owned(), String::new()),
    }
}

/// Whether an event from `target` belongs to this crate (see the module docs).
fn is_own_target(target: &str) -> bool {
    target == "telescope" || target.starts_with("telescope::")
}

/// Targets whose warnings are driver/loader chatter rather than something the
/// user can act on (e.g. `wgpu_hal`'s "GENERAL [Loader Message (0x0)]" from the
/// Vulkan loader at every start). Their errors are still shown.
fn is_noisy_target(target: &str) -> bool {
    target == "wgpu_hal" || target.starts_with("wgpu_hal::")
}

#[cfg(not(target_arch = "wasm32"))]
pub use layer::PanelLayer;

#[cfg(not(target_arch = "wasm32"))]
mod layer {
    use super::{LogRecord, is_noisy_target, is_own_target, push, split_target};
    use std::fmt::Write as _;
    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_log::NormalizeEvent;
    use tracing_subscriber::layer::{Context, Layer};

    /// The `tracing_subscriber` layer; see the module docs.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct PanelLayer;

    impl<S: Subscriber> Layer<S> for PanelLayer {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            // Records bridged from the `log` crate (egui_extras, wgpu, hyper,
            // ...) arrive with the target `log`; the original one is only
            // recovered through `normalized_metadata`.
            let normalized = event.normalized_metadata();
            let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
            let level = *metadata.level();
            if level > Level::WARN
                || is_own_target(metadata.target())
                || (level == Level::WARN && is_noisy_target(metadata.target()))
            {
                return;
            }

            let mut visitor = FieldVisitor::default();
            event.record(&mut visitor);
            let (source, context) = split_target(metadata.target());
            push(LogRecord {
                is_error: level == Level::ERROR,
                source,
                context,
                text: visitor.finish(),
            });
        }
    }

    #[derive(Default)]
    struct FieldVisitor {
        message: String,
        fields: String,
    }

    impl FieldVisitor {
        fn finish(self) -> String {
            match (self.message.is_empty(), self.fields.is_empty()) {
                (_, true) => self.message,
                (true, false) => self.fields,
                (false, false) => format!("{} ({})", self.message, self.fields),
            }
        }
    }

    impl Visit for FieldVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "message" {
                self.message = value.to_owned();
            } else {
                self.record_debug(field, &value);
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            let name = field.name();
            if name == "message" {
                self.message = format!("{value:?}");
            } else if !name.starts_with("log.") {
                // `log.target`, `log.file`, ... are bookkeeping added by
                // the `log` bridge, not part of the message.
                if !self.fields.is_empty() {
                    self.fields.push_str(", ");
                }
                let _ = write!(self.fields, "{name}={value:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_is_split_into_crate_and_module() {
        assert_eq!(
            split_target("egui_extras::loaders::http_loader"),
            ("egui_extras".to_owned(), "loaders::http_loader".to_owned())
        );
        assert_eq!(split_target("hyper"), ("hyper".to_owned(), String::new()));
    }

    #[test]
    fn own_events_are_recognized() {
        assert!(is_own_target("telescope"));
        assert!(is_own_target("telescope::app::audio"));
        assert!(!is_own_target("telescope_extra"));
        assert!(!is_own_target("webb::esi"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn warnings_and_errors_from_dependencies_are_queued() {
        use tracing_subscriber::layer::SubscriberExt;

        let subscriber = tracing_subscriber::registry().with(PanelLayer);
        tracing::subscriber::with_default(subscriber, || {
            drain();
            tracing::error!(target: "egui_extras::loaders::http_loader", "Failed to load {}", "x.png");
            tracing::warn!(target: "webb::esi", status = 404, "player database schema");
            tracing::info!(target: "webb::esi", "ignored: below WARN");
            tracing::error!(target: "telescope::app", "ignored: own crate");
            tracing::warn!(target: "wgpu_hal::vulkan::instance", "ignored: loader chatter");
        });
        assert_eq!(
            drain(),
            vec![
                LogRecord {
                    is_error: true,
                    source: "egui_extras".to_owned(),
                    context: "loaders::http_loader".to_owned(),
                    text: "Failed to load x.png".to_owned(),
                },
                LogRecord {
                    is_error: false,
                    source: "webb".to_owned(),
                    context: "esi".to_owned(),
                    text: "player database schema (status=404)".to_owned(),
                },
            ]
        );
    }
}
