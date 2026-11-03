//! One copy of Telescope at a time.
//!
//! zKillboard bans an address that goes over 15 requests a second, and each
//! copy of Telescope asks on its own, so two copies running together can earn
//! the ban between them. A [`SingleInstance`] takes an exclusive lock on a
//! small file in the program's working directory and keeps it for as long as
//! the process lives; a second copy cannot take it and ends. The operating
//! system releases the lock when the process ends, however it ends (a crash
//! included), so a stale lock file never keeps Telescope from starting.
//!
//! The lock is the standard library's own (`flock` on Unix, `LockFileEx` on
//! Windows), the same on every platform.
//!
//! A copy that restarts itself (see the GPU diagnostics) starts the new
//! process before the old one is gone, so [`SingleInstance::acquire`] waits a
//! few seconds for the lock before it gives up.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long [`SingleInstance::acquire`] waits for the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceConfig {
    /// The longest it waits for the copy that holds the lock to let it go.
    pub wait: Duration,
    /// How often it tries again meanwhile.
    pub poll: Duration,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(10),
            poll: Duration::from_millis(100),
        }
    }
}

/// Why the lock could not be taken.
#[derive(Debug)]
pub enum InstanceError {
    /// Another copy holds the lock.
    AlreadyRunning,
    /// The lock file could not be opened or locked. It says nothing about
    /// other copies.
    Io(io::Error),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("another copy is already running"),
            Self::Io(error) => write!(f, "the lock file could not be used: {error}"),
        }
    }
}

impl std::error::Error for InstanceError {}

/// The lock of the running copy. It is released when this is dropped or the
/// process ends.
#[derive(Debug)]
pub struct SingleInstance {
    /// Held open: the lock goes away with the handle.
    _file: File,
}

impl SingleInstance {
    /// Takes the lock kept in the file `path` (created if it is not there),
    /// waiting up to [`InstanceConfig::wait`] for another copy to release it.
    pub fn acquire(path: &Path, config: InstanceConfig) -> Result<Self, InstanceError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(InstanceError::Io)?;
        let deadline = Instant::now() + config.wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(InstanceError::AlreadyRunning);
                    }
                    std::thread::sleep(config.poll);
                }
                Err(TryLockError::Error(error)) => return Err(InstanceError::Io(error)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock file of its own for each test, in the temporary folder.
    struct TempLock(std::path::PathBuf);

    impl TempLock {
        fn new(name: &str) -> Self {
            Self(std::env::temp_dir().join(format!(
                "telescope-instance-{}-{name}.lock",
                std::process::id()
            )))
        }
    }

    impl Drop for TempLock {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn no_wait() -> InstanceConfig {
        InstanceConfig {
            wait: Duration::ZERO,
            ..InstanceConfig::default()
        }
    }

    #[test]
    fn a_second_copy_cannot_take_the_lock() {
        let lock = TempLock::new("second");
        let first = SingleInstance::acquire(&lock.0, no_wait()).expect("the first copy");
        assert!(matches!(
            SingleInstance::acquire(&lock.0, no_wait()),
            Err(InstanceError::AlreadyRunning)
        ));
        drop(first);
    }

    #[test]
    fn the_lock_can_be_taken_again_once_it_is_released() {
        let lock = TempLock::new("again");
        drop(SingleInstance::acquire(&lock.0, no_wait()).expect("the first copy"));
        assert!(SingleInstance::acquire(&lock.0, no_wait()).is_ok());
    }

    #[test]
    fn a_copy_waits_for_the_one_that_is_leaving() {
        let lock = TempLock::new("waits");
        let first = SingleInstance::acquire(&lock.0, no_wait()).expect("the first copy");
        let leaving = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(first);
        });
        let config = InstanceConfig {
            wait: Duration::from_secs(5),
            poll: Duration::from_millis(20),
        };
        assert!(SingleInstance::acquire(&lock.0, config).is_ok());
        leaving.join().expect("the leaving thread");
    }

    #[test]
    fn a_lock_file_that_cannot_be_opened_is_an_io_error() {
        let missing = std::env::temp_dir()
            .join("telescope-instance-no-such-folder")
            .join("telescope.lock");
        assert!(matches!(
            SingleInstance::acquire(&missing, no_wait()),
            Err(InstanceError::Io(_))
        ));
    }
}
