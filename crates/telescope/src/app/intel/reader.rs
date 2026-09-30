//! Reading stage: reads the appended bytes of a changed chat log off the UI
//! thread.
//!
//! The file watcher (`app::file::IntelEventHandler`) sends the name of every
//! changed monitored log to this thread, which reads and decodes what was
//! appended ([`ChatLogSource::read_new`]) and hands the parsed lines to the
//! detection thread. Nothing here touches the UI, so the intel pipeline
//! (read -> detect -> dispatch) keeps running while the eframe loop is
//! stalled or the window is hidden.

use super::input::{ChatLogSource, InputEvent, IntelLogName, IntelOffsets};
use crate::app::messages::{Message, try_send_app_message};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime};
use tokio::sync::mpsc;

/// The folder the watcher watches, if any (see `TelescopeApp::intel_watched`).
pub(crate) type WatchedDir = Arc<RwLock<Option<PathBuf>>>;
/// Read offsets shared between this thread and the UI, which syncs them when
/// the settings or the files change.
pub(crate) type SharedOffsets = Arc<Mutex<IntelOffsets>>;

/// Everything the reader thread needs besides its queue.
pub(crate) struct ReaderShared {
    pub watched: WatchedDir,
    pub offsets: SharedOffsets,
    /// Reader -> detection thread.
    pub input: mpsc::Sender<InputEvent>,
    /// Back to the UI, for the channel activity shown in Settings -> Sources.
    pub app_msg: Arc<mpsc::Sender<Message>>,
}

/// Spawns the reader thread and returns immediately. It ends when every
/// sender of `files` is dropped.
pub(crate) fn spawn(shared: ReaderShared, files: Receiver<String>) {
    let spawned = std::thread::Builder::new()
        .name(String::from("telescope-intel-reader"))
        .spawn(move || {
            while let Ok(file_name) = files.recv() {
                read_file(&shared, &file_name);
            }
            tracing::debug!("intel reader stopped");
        });
    if let Err(error) = spawned {
        tracing::error!("could not start the intel reader thread: {error}");
    }
}

/// Reads what was appended to `file_name` since the last read.
#[tracing::instrument(skip(shared))]
fn read_file(shared: &ReaderShared, file_name: &str) {
    // The watched folder, not the one in the settings: that one can be a
    // draft the watcher doesn't report on yet.
    let dir = shared
        .watched
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let Some(dir) = dir else {
        tracing::debug!("no watched folder; change ignored");
        return;
    };
    // Offset recorded by the last read (see `IntelOffsets`). The lock is not
    // held during the file I/O.
    let start = lock(&shared.offsets).get(file_name);
    let started = Instant::now();
    let Some(read) = ChatLogSource::read_new(&dir, file_name, start) else {
        tracing::trace!(start, "nothing new to read");
        return;
    };
    let lines = read.events.len();
    let mut dropped = 0usize;
    for event in read.events {
        // `try_send`: a full queue (detection can't keep up) must drop lines
        // rather than stall the reader.
        if let Err(error) = shared.input.try_send(event) {
            dropped += 1;
            tracing::warn!("intel input channel rejected a line: {error}");
        }
    }
    lock(&shared.offsets).set(file_name, read.end_offset);
    tracing::debug!(
        lines,
        dropped,
        bytes = read.end_offset.saturating_sub(start),
        elapsed_us = started.elapsed().as_micros() as u64,
        "chat log read"
    );
    // Last activity of the channel, for Settings -> Sources.
    if let Some(log) = IntelLogName::parse(file_name) {
        let _ = try_send_app_message(
            &shared.app_msg,
            Message::ChannelActivity(log.channel.to_string(), SystemTime::now()),
        );
    }
}

fn lock(offsets: &SharedOffsets) -> std::sync::MutexGuard<'_, IntelOffsets> {
    offsets
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn appended_lines_reach_the_detection_queue_and_the_offset_advances() {
        let dir = std::env::temp_dir().join(format!("telescope-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = "Intel_20240101_120000.txt";
        let mut file = std::fs::File::create(dir.join(name)).unwrap();
        file.write_all(&utf16("[ 2024.01.01 12:00:01 ] Pilot > hello\r\n"))
            .unwrap();
        drop(file);

        let (input, mut lines) = mpsc::channel(8);
        let (app_msg, mut app_rx) = mpsc::channel(8);
        let shared = ReaderShared {
            watched: Arc::new(RwLock::new(Some(dir.clone()))),
            offsets: Arc::new(Mutex::new(IntelOffsets::default())),
            input,
            app_msg: Arc::new(app_msg),
        };

        read_file(&shared, name);
        let event = lines.try_recv().expect("one line queued");
        assert_eq!(event.channel, "Intel");
        assert!(matches!(
            app_rx.try_recv(),
            Ok(Message::ChannelActivity(channel, _)) if channel == "Intel"
        ));
        // Nothing new: the second read queues nothing.
        read_file(&shared, name);
        assert!(lines.try_recv().is_err());
        assert!(lock(&shared.offsets).get(name) > 0);

        let _ = std::fs::remove_dir_all(dir);
    }

    const NAME: &str = "Intel_20240101_120000.txt";

    fn line(text: &str) -> String {
        format!("[ 2024.01.01 12:00:01 ] Pilot > {text}\r\n")
    }

    /// A fresh temp folder per test (tests run in parallel).
    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telescope-reader-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(dir: &std::path::Path, name: &str, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .unwrap();
        // The UTF-16LE BOM only at the start of the file, like the game does.
        if file.metadata().unwrap().len() == 0 {
            file.write_all(&[0xFF, 0xFE]).unwrap();
        }
        file.write_all(&utf16(text)).unwrap();
    }

    struct Harness {
        shared: ReaderShared,
        lines: mpsc::Receiver<InputEvent>,
        app_rx: mpsc::Receiver<Message>,
    }

    fn harness(dir: Option<PathBuf>, capacity: usize) -> Harness {
        let (input, lines) = mpsc::channel(capacity);
        let (app_msg, app_rx) = mpsc::channel(8);
        Harness {
            shared: ReaderShared {
                watched: Arc::new(RwLock::new(dir)),
                offsets: Arc::new(Mutex::new(IntelOffsets::default())),
                input,
                app_msg: Arc::new(app_msg),
            },
            lines,
            app_rx,
        }
    }

    #[test]
    fn a_change_without_a_watched_folder_is_ignored() {
        let dir = temp_dir("unwatched");
        append(&dir, NAME, &line("hello"));
        let mut h = harness(None, 8);

        read_file(&h.shared, NAME);

        assert!(h.lines.try_recv().is_err());
        assert!(h.app_rx.try_recv().is_err());
        assert_eq!(lock(&h.shared.offsets).get(NAME), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_appended_lines_are_read_on_the_next_change() {
        let dir = temp_dir("append");
        append(&dir, NAME, &line("first"));
        let mut h = harness(Some(dir.clone()), 8);

        read_file(&h.shared, NAME);
        assert_eq!(h.lines.try_recv().unwrap().line.text, "first");
        let after_first = lock(&h.shared.offsets).get(NAME);

        append(&dir, NAME, &line("second"));
        read_file(&h.shared, NAME);
        assert_eq!(h.lines.try_recv().unwrap().line.text, "second");
        assert!(h.lines.try_recv().is_err());
        assert!(lock(&h.shared.offsets).get(NAME) > after_first);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_queue_drops_lines_but_the_offset_still_advances() {
        let dir = temp_dir("full");
        append(
            &dir,
            NAME,
            &format!("{}{}{}", line("one"), line("two"), line("three")),
        );
        let mut h = harness(Some(dir.clone()), 1);

        read_file(&h.shared, NAME);

        assert_eq!(h.lines.try_recv().unwrap().line.text, "one");
        assert!(h.lines.try_recv().is_err(), "the rest was dropped");
        // The dropped lines are not read again.
        read_file(&h.shared, NAME);
        assert!(h.lines.try_recv().is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_is_skipped_without_touching_the_offset() {
        let dir = temp_dir("missing");
        let mut h = harness(Some(dir.clone()), 8);

        read_file(&h.shared, NAME);

        assert!(h.lines.try_recv().is_err());
        assert!(h.app_rx.try_recv().is_err());
        assert_eq!(lock(&h.shared.offsets).get(NAME), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_is_not_a_chat_log_is_read_without_channel_or_activity() {
        let dir = temp_dir("notalog");
        append(&dir, "notes.txt", &line("hello"));
        let mut h = harness(Some(dir.clone()), 8);

        read_file(&h.shared, "notes.txt");

        let event = h.lines.try_recv().expect("the line is still read");
        assert_eq!(event.channel, "");
        assert_eq!(event.source, "notes.txt");
        assert!(h.app_rx.try_recv().is_err(), "no channel, no activity");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_log_that_shrank_is_read_again_from_the_start() {
        let dir = temp_dir("rotated");
        append(
            &dir,
            NAME,
            &format!("{}{}", line("old one"), line("old two")),
        );
        let mut h = harness(Some(dir.clone()), 8);
        read_file(&h.shared, NAME);
        while h.lines.try_recv().is_ok() {}

        std::fs::remove_file(dir.join(NAME)).unwrap();
        append(&dir, NAME, &line("new"));
        read_file(&h.shared, NAME);

        assert_eq!(h.lines.try_recv().unwrap().line.text, "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn offsets_are_kept_per_file() {
        let dir = temp_dir("perfile");
        let other = "Delve_20240101_120000.txt";
        append(&dir, NAME, &line("a"));
        append(&dir, other, &line("b"));
        let mut h = harness(Some(dir.clone()), 8);

        read_file(&h.shared, NAME);
        read_file(&h.shared, other);

        let channels: Vec<String> = std::iter::from_fn(|| h.lines.try_recv().ok())
            .map(|event| event.channel)
            .collect();
        assert_eq!(channels, ["Intel", "Delve"]);
        let offsets = lock(&h.shared.offsets);
        assert!(offsets.get(NAME) > 0);
        assert!(offsets.get(other) > 0);
        drop(offsets);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_thread_reads_every_queued_name_and_stops_when_the_queue_closes() {
        let dir = temp_dir("thread");
        append(&dir, NAME, &line("threaded"));
        let mut h = harness(Some(dir.clone()), 8);
        let (files, queue) = std::sync::mpsc::channel();

        spawn(
            ReaderShared {
                watched: h.shared.watched.clone(),
                offsets: h.shared.offsets.clone(),
                input: h.shared.input.clone(),
                app_msg: h.shared.app_msg.clone(),
            },
            queue,
        );
        files.send(String::from(NAME)).unwrap();
        let event = h.lines.blocking_recv().expect("the thread queued the line");
        assert_eq!(event.line.text, "threaded");
        drop(files);
        let _ = std::fs::remove_dir_all(dir);
    }
}
