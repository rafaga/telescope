//! Detection stage: runs the graph [`Executor`] off the UI thread.
//!
//! A dedicated thread (with its own current-thread tokio runtime, the same
//! pattern the rest of the app's background work uses) receives [`InputEvent`]s
//! on a bounded channel, evaluates them through the executor and sends the
//! resulting [`Activation`]s on to the dispatch thread (`super::dispatch`). The executor sits behind an
//! [`RwLock`] so the UI can swap it in place when the rules change, without
//! restarting the thread.

use super::input::InputEvent;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use webb::graph::{Activation, Executor, LineContext};
use webb::rules::IntelLine;

/// Capacity of the reader -> detection channel. Bounded on purpose: if the
/// reader ever produces lines faster than they can be evaluated, the extra
/// ones are dropped (a lost line is preferable to stalling the reader).
pub(crate) const INPUT_CAPACITY: usize = 1024;
/// Capacity of the detection -> dispatch channel. Bounded on purpose: a full
/// channel blocks the detection thread (real backpressure) until the
/// dispatch thread drains it.
pub(crate) const OUTPUT_CAPACITY: usize = 1024;

/// Shared handle to the live executor; the UI replaces it behind the lock
/// when the rules change.
pub(crate) type ExecutorHandle = Arc<RwLock<Executor>>;

/// Shared system resolver injected into the executor (backed by the SDE,
/// rebuilt in place when the SDE is).
pub(crate) type ResolverHandle = Arc<super::resolve::SharedResolver>;

/// One line's output activations, as delivered to the dispatch thread.
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
    let spawned = std::thread::Builder::new()
        .name(String::from("telescope-intel-detection"))
        .spawn(move || {
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
                    let started = std::time::Instant::now();
                    let activations = {
                        let executor = executor
                            .read()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        executor.run(&context, resolver.as_ref())
                    };
                    tracing::trace!(
                        channel = %event.channel,
                        outputs = activations.len(),
                        elapsed_us = started.elapsed().as_micros() as u64,
                        "line evaluated"
                    );
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
                }
                tracing::debug!("intel detection stopped");
            });
        });
    if let Err(error) = spawned {
        tracing::error!("could not start the intel detection thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    //! The whole pipeline on real threads: a chat log written to disk, the
    //! reader, detection with the default rules, and the dispatch, ending on
    //! the messages the maps and the audio thread receive.

    use super::super::dispatch::{self, AlarmConfig, AlarmShared, Dispatcher};
    use super::super::input::IntelOffsets;
    use super::super::reader::{self, ReaderShared};
    use super::super::resolve::SharedResolver;
    use super::*;
    use crate::app::audio::AudioHandle;
    use crate::app::messages::{MapSync, Message, MessageSpawner};
    use sde::objects::{SolarSystem, Universe};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use webb::graph::RuleGraph;

    const LOG: &str = "Intel_20240101_120000.txt";

    struct Pipeline {
        dir: PathBuf,
        files: std::sync::mpsc::Sender<String>,
        executor: ExecutorHandle,
        map: broadcast::Receiver<MapSync>,
        sounds: std::sync::mpsc::Receiver<PathBuf>,
        // Keeps the app channel open (the reader and dispatch send on it).
        _app: mpsc::Receiver<Message>,
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    impl Pipeline {
        fn start(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("telescope-e2e-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::File::create(dir.join(LOG)).unwrap();

            let mut universe = Universe::new(1.0);
            let mut system = SolarSystem::new(1.0);
            system.id = 1;
            system.name = String::from("H-5GUI");
            universe.solar_systems.insert(1, system);
            let resolver: ResolverHandle = Arc::new(SharedResolver::new(&universe));
            let (executor, errors) = Executor::new(RuleGraph::default_graph());
            assert!(errors.is_empty());
            let executor: ExecutorHandle = Arc::new(RwLock::new(executor));

            let (input_tx, input_rx) = mpsc::channel(INPUT_CAPACITY);
            let (output_tx, output_rx) = mpsc::channel(OUTPUT_CAPACITY);
            spawn(Arc::clone(&executor), resolver, input_rx, output_tx);

            let shared = Arc::new(AlarmShared::default());
            *shared.config.write().unwrap() = AlarmConfig {
                warning_area: 0,
                sound_path: PathBuf::from("alarm.wav"),
                center_on_alert: false,
                alert_duration: Duration::from_secs(5),
            };
            // A character sitting in the reported system.
            *shared.locations.write().unwrap() = vec![1];
            shared.jumps.write().unwrap().insert(1, Vec::new());
            let (audio, sounds) = AudioHandle::for_test();
            let (map_tx, map) = broadcast::channel(64);
            let (app_tx, app) = mpsc::channel(64);
            let app_tx = Arc::new(app_tx);
            dispatch::spawn(
                Dispatcher {
                    shared,
                    audio: Arc::new(audio),
                    map_msg: Arc::new(map_tx),
                    task_msg: Arc::new(MessageSpawner::new(Arc::clone(&app_tx))),
                },
                output_rx,
            );

            let (files, files_rx) = std::sync::mpsc::channel();
            reader::spawn(
                ReaderShared {
                    watched: Arc::new(RwLock::new(Some(dir.clone()))),
                    offsets: Arc::new(Mutex::new(IntelOffsets::default())),
                    input: input_tx,
                    app_msg: app_tx,
                },
                files_rx,
            );
            Self {
                dir,
                files,
                executor,
                map,
                sounds,
                _app: app,
            }
        }

        /// Appends a line to the log and tells the reader, as the watcher would.
        fn write_line(&self, text: &str) {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(self.dir.join(LOG))
                .unwrap();
            file.write_all(&utf16(&format!(
                "[ 2024.01.01 12:00:01 ] Pilot > {text}\r\n"
            )))
            .unwrap();
            drop(file);
            self.files.send(String::from(LOG)).unwrap();
        }

        /// Waits for the next map message, or `None` if none comes.
        fn next_map(&mut self, wait: Duration) -> Option<MapSync> {
            let deadline = std::time::Instant::now() + wait;
            while std::time::Instant::now() < deadline {
                if let Ok(message) = self.map.try_recv() {
                    return Some(message);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }
    }

    impl Drop for Pipeline {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_reported_sighting_travels_from_the_log_to_the_map_and_the_audio() {
        let mut pipeline = Pipeline::start("sighting");
        pipeline.write_line("H-5GUI*  Floris Saucus  nv");

        let mut pulsed = false;
        while let Some(message) = pipeline.next_map(Duration::from_secs(3)) {
            if matches!(message, MapSync::SystemAlert(_)) {
                pulsed = true;
                break;
            }
        }
        assert!(pulsed, "no pulse reached the map");
        assert_eq!(
            pipeline
                .sounds
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            PathBuf::from("alarm.wav")
        );
    }

    #[test]
    fn a_line_that_matches_nothing_produces_no_alert() {
        let mut pipeline = Pipeline::start("nomatch");
        pipeline.write_line("just chatting about nothing");
        assert!(pipeline.next_map(Duration::from_millis(500)).is_none());
        assert!(pipeline.sounds.try_recv().is_err());
    }

    #[test]
    fn replacing_the_executor_in_place_changes_what_detects() {
        let mut pipeline = Pipeline::start("swap");
        let (empty, _) = Executor::new(RuleGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
        *pipeline.executor.write().unwrap() = empty;
        pipeline.write_line("H-5GUI*  Floris Saucus  nv");
        assert!(pipeline.next_map(Duration::from_millis(500)).is_none());
        assert!(pipeline.sounds.try_recv().is_err());
    }
}
