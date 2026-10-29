//! The log file of Telescope.
//!
//! Besides the console (`RUST_LOG`) and the panel at the bottom of the window
//! (warnings and errors only), what `tracing` reports goes to a file in the
//! `logs` folder of the working directory (see [`crate::app_dirs`]): a new
//! file every day, `telescope.<date>.log`, of which the last few are kept.
//! The file is written by a thread of its own, so logging never waits for the
//! disk.
//!
//! What goes into it follows `RUST_LOG` when that is set, and
//! [`FileLogConfig::filter`] when it is not.
//!
//! The log is a convenience: if the folder can't be created or the file can't
//! be opened, Telescope runs without it.

use std::path::PathBuf;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// How the log file behaves. The defaults are what Telescope uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLogConfig {
    /// The folder of the files, relative to the working directory.
    pub dir: PathBuf,
    /// The files are named `<prefix>.<date>.<suffix>`.
    pub prefix: String,
    pub suffix: String,
    /// How many files are kept: the older ones are deleted when a new one is
    /// started.
    pub max_files: usize,
    /// What is written when `RUST_LOG` is not set (an `EnvFilter` string).
    pub filter: String,
}

impl Default for FileLogConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("logs"),
            prefix: String::from("telescope"),
            suffix: String::from("log"),
            max_files: 7,
            filter: String::from("warn,telescope=info,webb=info"),
        }
    }
}

/// An open log file. Keep it alive for as long as the program logs: when it
/// is dropped what is still waiting is written and the file is closed.
pub struct FileLog {
    config: FileLogConfig,
    writer: NonBlocking,
    // Only held, to be dropped last.
    _guard: WorkerGuard,
}

impl FileLog {
    /// Opens the log file of today, creating the folder when needed. `None`
    /// when that is not possible.
    pub fn start(config: FileLogConfig) -> Option<Self> {
        let appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(&config.prefix)
            .filename_suffix(&config.suffix)
            .max_log_files(config.max_files)
            .build(&config.dir)
            .inspect_err(|error| {
                eprintln!(
                    "Telescope: no log file in {}: {error}",
                    config.dir.display()
                );
            })
            .ok()?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        Some(Self {
            config,
            writer,
            _guard: guard,
        })
    }

    /// Where the lines are written (for `tracing_subscriber::fmt::layer`).
    pub fn writer(&self) -> NonBlocking {
        self.writer.clone()
    }

    /// What the file takes: `RUST_LOG` when it is set, else the configured
    /// filter.
    pub fn filter(&self) -> EnvFilter {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&self.config.filter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;

    /// A folder of its own, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("telescope-app-log-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn config_in(folder: &Folder) -> FileLogConfig {
        FileLogConfig {
            dir: folder.0.join("logs"),
            ..FileLogConfig::default()
        }
    }

    #[test]
    fn the_defaults_keep_a_week_of_daily_files() {
        let config = FileLogConfig::default();
        assert_eq!(config.dir, PathBuf::from("logs"));
        assert_eq!(config.max_files, 7);
        assert_eq!(config.prefix, "telescope");
    }

    #[test]
    fn what_is_logged_ends_up_in_the_file_of_the_day() {
        let folder = Folder::new();
        let config = config_in(&folder);
        let dir = config.dir.clone();
        let log = FileLog::start(config).expect("the log file opens");
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(log.writer())
            .with_filter(EnvFilter::new("info"));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::warn!("the feed refused the request");
        });
        // Dropping the log writes what was waiting.
        drop(log);
        let files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("telescope.") && name.ends_with(".log"),
            "{name}"
        );
        let text = std::fs::read_to_string(&files[0]).unwrap();
        assert!(text.contains("the feed refused the request"), "{text}");
        assert!(!text.contains('\u{1b}'), "no colour codes in the file");
    }

    #[test]
    fn an_unusable_folder_gives_no_log() {
        let folder = Folder::new();
        // A file where the folder should be.
        std::fs::create_dir_all(&folder.0).unwrap();
        let blocker = folder.0.join("logs");
        std::fs::write(&blocker, b"not a folder").unwrap();
        let config = FileLogConfig {
            dir: blocker.join("inner"),
            ..FileLogConfig::default()
        };
        assert!(FileLog::start(config).is_none());
    }
}
