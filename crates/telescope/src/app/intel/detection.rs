//! Detection stage: runs the [`DetectionEngine`] off the UI thread.
//!
//! A dedicated thread (with its own current-thread tokio runtime, the same
//! pattern the rest of the app's background work uses) receives [`InputEvent`]s
//! on a bounded channel, evaluates them against the engine and sends the
//! resulting [`DetectionBatch`]es back to the UI thread. The engine sits
//! behind an [`RwLock`] so the UI can swap it in place when the rules change,
//! without restarting the thread.

use super::input::InputEvent;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use webb::rules::{DetectionBatch, DetectionEngine};

/// Capacity of the UI -> detection channel. Bounded on purpose: if the UI
/// ever produces lines faster than they can be evaluated, the extra ones are
/// dropped (a lost line is preferable to blocking the UI thread).
pub(crate) const INPUT_CAPACITY: usize = 1024;
/// Capacity of the detection -> UI channel. Bounded on purpose: a full
/// channel blocks the detection thread (real backpressure) until the UI
/// drains it.
pub(crate) const OUTPUT_CAPACITY: usize = 1024;

/// Shared handle to the live engine; the UI replaces the engine behind it
/// when the rules change.
pub(crate) type EngineHandle = Arc<RwLock<DetectionEngine>>;

/// One line's detections, as delivered to the UI thread.
pub(crate) struct DetectedLine {
    /// Channel the line came from.
    pub channel: String,
    /// The parsed line and its detections.
    pub batch: DetectionBatch,
}

/// Spawns the detection thread and returns immediately.
pub(crate) fn spawn(
    engine: EngineHandle,
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
                let detections = {
                    let engine = engine.read().unwrap_or_else(|poisoned| poisoned.into_inner());
                    engine.evaluate_line(&event.channel, &event.line)
                };
                if detections.is_empty() {
                    continue;
                }
                let batch = DetectionBatch {
                    line: event.line,
                    detections,
                };
                if output
                    .send(DetectedLine {
                        channel: event.channel,
                        batch,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    });
}
