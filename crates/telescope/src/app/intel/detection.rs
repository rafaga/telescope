//! Detection stage: runs the graph [`Executor`] off the UI thread.
//!
//! A dedicated thread (with its own current-thread tokio runtime, the same
//! pattern the rest of the app's background work uses) receives [`InputEvent`]s
//! on a bounded channel, evaluates them through the executor and sends the
//! resulting [`Activation`]s back to the UI thread. The executor sits behind an
//! [`RwLock`] so the UI can swap it in place when the rules change, without
//! restarting the thread.

use super::input::InputEvent;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use webb::graph::{Activation, Executor, LineContext};
use webb::rules::IntelLine;

/// Capacity of the UI -> detection channel. Bounded on purpose: if the UI
/// ever produces lines faster than they can be evaluated, the extra ones are
/// dropped (a lost line is preferable to blocking the UI thread).
pub(crate) const INPUT_CAPACITY: usize = 1024;
/// Capacity of the detection -> UI channel. Bounded on purpose: a full
/// channel blocks the detection thread (real backpressure) until the UI
/// drains it.
pub(crate) const OUTPUT_CAPACITY: usize = 1024;

/// Shared handle to the live executor; the UI replaces it behind the lock
/// when the rules change.
pub(crate) type ExecutorHandle = Arc<RwLock<Executor>>;

/// Shared system resolver injected into the executor (backed by the SDE,
/// rebuilt in place when the SDE is).
pub(crate) type ResolverHandle = Arc<super::resolve::SharedResolver>;

/// One line's output activations, as delivered to the UI thread.
pub(crate) struct DetectedLine {
    /// Channel the line came from.
    pub channel: String,
    /// The parsed line.
    pub line: IntelLine,
    /// The Output nodes that fired.
    pub activations: Vec<Activation>,
}

/// Spawns the detection thread and returns immediately.
pub(crate) fn spawn(
    executor: ExecutorHandle,
    resolver: ResolverHandle,
    mut input: mpsc::Receiver<InputEvent>,
    output: mpsc::Sender<DetectedLine>,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("detection thread runtime");
        runtime.block_on(async move {
            while let Some(event) = input.recv().await {
                let context = LineContext {
                    line: event.line,
                    channel: event.channel.clone(),
                };
                let activations = {
                    let executor = executor
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    executor.run(&context, resolver.as_ref())
                };
                if activations.is_empty() {
                    continue;
                }
                let line = context.line;
                if output
                    .send(DetectedLine {
                        channel: event.channel,
                        line,
                        activations,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                // The UI drains detections inside `update()`: ask for a frame.
                crate::repaint::request();
            }
        });
    });
}
