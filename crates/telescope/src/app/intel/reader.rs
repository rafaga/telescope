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
#[cfg(target_os = "windows")]
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, RwLock};
#[cfg(target_os = "windows")]
use std::time::Duration;
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
    /// Names of the monitored channels, sorted (shared with the watcher's
    /// event handler). The sweep (Windows only) only looks at the logs of these
    /// channels.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub channels: Arc<RwLock<Vec<String>>>,
}

#[cfg(not(target_os = "windows"))]
impl ReaderShared {
    /// Runs the reader loop: reads the files the watcher names. Returns when
    /// every sender of `files` is dropped.
    fn run(self, files: Receiver<String>) {
        while let Ok(file_name) = files.recv() {
            read_file(&self, &file_name);
        }
        tracing::debug!("intel reader stopped");
    }
}

/// Polling fallback, on Windows only: the watcher alone is enough on the other
/// systems.
#[cfg(target_os = "windows")]
impl ReaderShared {
    /// How often the reader looks at the monitored logs by itself, whether
    /// or not the watcher reported anything.
    ///
    /// The OS watcher is not enough on its own: on Windows
    /// (`ReadDirectoryChangesW`) the size/last-write notifications of a file
    /// another process keeps open and appends to are deferred by NTFS, and
    /// can arrive seconds late, batched, or only when the writer closes the
    /// file. Without this sweep a log written by EVE could look unchanged.
    const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

    /// How many days back a log can have started (by the date in its file
    /// name) or been written to (by its last-write time in the folder) and
    /// still be swept. A channel can have years of old logs; only the ones
    /// of recent sessions can still be written to. The last-write time
    /// covers logs whose name carries an old date but that are being written
    /// now, e.g. a replayed corpus of old logs.
    const SWEEP_DAYS: u64 = 2;

    /// Runs the reader loop: reads the files the watcher names, and sweeps
    /// the monitored logs every [`Self::SWEEP_INTERVAL`] so a change the
    /// watcher missed or delayed is still read. Returns when every sender
    /// of `files` is dropped.
    fn run(self, files: Receiver<String>) {
        let mut last_sweep = Instant::now();
        loop {
            let wait = Self::SWEEP_INTERVAL.saturating_sub(last_sweep.elapsed());
            match files.recv_timeout(wait) {
                Ok(file_name) => read_file(&self, &file_name),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            // Also checked after an event: a steady stream of events for one
            // log must not starve the sweep of the others.
            if last_sweep.elapsed() >= Self::SWEEP_INTERVAL {
                self.sweep();
                last_sweep = Instant::now();
            }
        }
        tracing::debug!("intel reader stopped");
    }

    /// Reads whatever was appended to the recent logs of the monitored
    /// channels, with no watcher event. The reads are the ones an event
    /// triggers (so a log with nothing new costs one `stat`), and they run
    /// on the reader thread: a sweep never reads a file at the same time as
    /// an event, and can't duplicate a line.
    fn sweep(&self) {
        let dir = self
            .watched
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(dir) = dir else {
            return;
        };
        let channels = self
            .channels
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if channels.is_empty() {
            return;
        }
        let Ok(entries) = dir.read_dir() else {
            return;
        };
        let oldest = chrono::Utc::now()
            .checked_sub_days(chrono::Days::new(Self::SWEEP_DAYS))
            .map(|day| day.format("%Y%m%d").to_string())
            .unwrap_or_default();
        let oldest_write =
            SystemTime::now().checked_sub(Duration::from_secs(Self::SWEEP_DAYS * 24 * 3600));
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(log) = IntelLogName::parse(&name) else {
                continue;
            };
            // `suffix` starts with the `YYYYMMDD` the session started on.
            let started = log.suffix.get(..8).unwrap_or_default();
            let recent_name = started >= oldest.as_str();
            // The folder's last-write time can lag behind the real one while
            // another process holds the log open, but it is never older than
            // the file's creation, which is enough to tell a live log apart
            // from the years of old ones.
            let recent_write = || {
                entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .zip(oldest_write)
                    .is_some_and(|(modified, oldest)| modified >= oldest)
            };
            if channels.binary_search(&log.channel.to_string()).is_err()
                || !(recent_name || recent_write())
            {
                continue;
            }
            read_file(self, &name);
        }
    }
}

/// Spawns the reader thread and returns immediately. It ends when every
/// sender of `files` is dropped.
pub(crate) fn spawn(shared: ReaderShared, files: Receiver<String>) {
    let spawned = std::thread::Builder::new()
        .name(String::from("telescope-intel-reader"))
        .spawn(move || shared.run(files));
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
            channels: Arc::new(RwLock::new(Vec::new())),
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
                channels: Arc::new(RwLock::new(Vec::new())),
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
                channels: h.shared.channels.clone(),
            },
            queue,
        );
        files.send(String::from(NAME)).unwrap();
        let event = h.lines.blocking_recv().expect("the thread queued the line");
        assert_eq!(event.line.text, "threaded");
        drop(files);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Name of a monitored log that started today, so the sweep considers it.
    #[cfg(target_os = "windows")]
    fn recent_name(channel: &str) -> String {
        format!(
            "{channel}_{}_120000_1.txt",
            chrono::Utc::now().format("%Y%m%d")
        )
    }

    #[cfg(target_os = "windows")]
    fn monitor(h: &Harness, channels: &[&str]) {
        *h.shared.channels.write().unwrap() =
            channels.iter().map(|name| name.to_string()).collect();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_sweep_reads_what_was_appended_without_any_watcher_event() {
        let dir = temp_dir("sweep-reads");
        let name = recent_name("Local");
        append(&dir, &name, &line("first"));
        let mut h = harness(Some(dir.clone()), 8);
        monitor(&h, &["Local"]);

        h.shared.sweep();

        let event = h.lines.try_recv().expect("the sweep read the line");
        assert_eq!(event.line.text, "first");
        assert_eq!(event.channel, "Local");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_sweep_with_nothing_new_queues_nothing() {
        let dir = temp_dir("sweep-idle");
        let name = recent_name("Local");
        append(&dir, &name, &line("only"));
        let mut h = harness(Some(dir.clone()), 8);
        monitor(&h, &["Local"]);

        h.shared.sweep();
        assert!(h.lines.try_recv().is_ok());
        h.shared.sweep();
        assert!(h.lines.try_recv().is_err(), "the line was read twice");

        append(&dir, &name, &line("second"));
        h.shared.sweep();
        assert_eq!(h.lines.try_recv().unwrap().line.text, "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_sweep_skips_unmonitored_channels_and_logs_that_are_old_by_name_and_write_time() {
        let dir = temp_dir("sweep-skips");
        append(&dir, &recent_name("Other"), &line("not monitored"));
        let old = "Local_20200101_120000_1.txt";
        append(&dir, old, &line("too old"));
        let file = std::fs::File::options()
            .write(true)
            .open(dir.join(old))
            .unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(10 * 24 * 3600))
            .unwrap();
        drop(file);
        let mut h = harness(Some(dir.clone()), 8);
        monitor(&h, &["Local"]);

        h.shared.sweep();

        assert!(h.lines.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A log with an old date in its name that is being written now (a replayed
    // corpus of old logs) is still read.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_sweep_reads_a_log_with_an_old_name_that_was_written_recently() {
        let dir = temp_dir("sweep-replayed");
        let name = "Local_20230327_021615_95103254.txt";
        append(&dir, name, &line("replayed"));
        let mut h = harness(Some(dir.clone()), 8);
        monitor(&h, &["Local"]);

        h.shared.sweep();

        assert_eq!(h.lines.try_recv().unwrap().line.text, "replayed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_sweep_without_a_watched_folder_or_channels_does_nothing() {
        let dir = temp_dir("sweep-nothing");
        append(&dir, &recent_name("Local"), &line("ignored"));
        let mut h = harness(None, 8);
        monitor(&h, &["Local"]);
        h.shared.sweep();
        assert!(h.lines.try_recv().is_err());

        let mut h = harness(Some(dir.clone()), 8);
        h.shared.sweep();
        assert!(h.lines.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn the_thread_sweeps_a_change_the_watcher_never_reported() {
        let dir = temp_dir("sweep-thread");
        let name = recent_name("Local");
        append(&dir, &name, &line("silent"));
        let mut h = harness(Some(dir.clone()), 8);
        monitor(&h, &["Local"]);
        let (files, files_rx) = std::sync::mpsc::channel::<String>();
        spawn(
            ReaderShared {
                watched: h.shared.watched.clone(),
                offsets: h.shared.offsets.clone(),
                input: h.shared.input.clone(),
                app_msg: h.shared.app_msg.clone(),
                channels: h.shared.channels.clone(),
            },
            files_rx,
        );

        // No name is ever sent through `files`.
        let event = h.lines.blocking_recv().expect("the sweep queued the line");
        assert_eq!(event.line.text, "silent");
        drop(files);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
