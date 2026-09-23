//! Alarm sound for `ActionConfig::MapAlert` matches.
//!
//! [`AlarmPlayer`] opens the default audio output device once, at startup,
//! and keeps it open for the app's lifetime -- rodio's output handle has to
//! stay alive for as long as anything should be audible, so it lives as a
//! field on [`TelescopeApp`](super::TelescopeApp) rather than being opened
//! per alert (which would also mean a slow device-open on the UI thread for
//! every single match). Playing a sound is then just decoding the alarm
//! clip ([`open_alarm_sound`]) and handing it to the mixer
//! (`AlarmPlayer::play_alarm`): the mixer takes ownership from there and
//! plays it to completion on its own, so the call site never blocks and
//! never has to hold on to anything.
//!
//! Unlike the window icon and the two UI fonts (embedded with
//! `include_bytes!`, see `app.rs`/`main.rs`/`windows/about.rs`), the alarm
//! clip is read from disk at the moment it's needed rather than baked into
//! the binary -- so it can be swapped (or picked, see below) without a
//! rebuild, and doesn't add its size to every build whether or not sound is
//! ever used. It's read fresh (opened, decoded, handed off) on every alert
//! instead of cached, same as `load_intel_file` re-reads its chat log
//! chunk each time rather than keeping the file open: a clip is small
//! (well under a second of audio) and alerts are infrequent, so the I/O
//! cost is negligible next to the simplicity of not managing a cache.
//!
//! [`AlarmPlayer`] itself has no opinion on *which* sound plays -- the
//! caller (`TelescopeApp::dispatch_map_alert`, in `intel.rs`) passes
//! [`Self::play_alarm`] the path, which it gets from
//! `Settings::get_alert_sound_path`: the file the user picked on the
//! Settings -> Intelligence page (`windows::settings::intelligence`),
//! resolved against `settings::ALERTS_DIR`. That constant is also where
//! the "relative to wherever Telescope is run from" convention lives
//! (`settings::FilePaths::default`'s doc comment explains why: same as
//! `sde.db`, `patterns.toml` and `telescope.toml`, it's meant to sit
//! alongside the app, not somewhere the user has to go look for it). The
//! packaged installer has to actually ship `assets/alerts/` next to the
//! executable for any of this to find it there -- see the `resources`
//! entry in `packager.json`. Nothing here assumes that succeeded, though:
//! a missing file, or the whole `assets/alerts/` directory never having
//! made it onto this machine, is just another `Err` from
//! [`open_alarm_sound`], reported the same way as a decode failure -- see
//! its tests below for the two ways that can happen.
//!
//! Every failure here (no output device, the sound file missing, a decode
//! error) is non-fatal -- alerts keep working on the map and in the status
//! log either way, only the sound is skipped -- but each one still reaches
//! the user, not just the log file: a `tracing::warn!` for `RUST_LOG`
//! (and, since rodio's own `tracing` feature is on, the same channel rodio's
//! *own* internal warnings already use), plus a `Message::GenericNotification`
//! so it also shows up in the on-screen status log, the same two-tier
//! reporting `TelescopeApp::default`'s `SdeManager::new` failure handling
//! and `notify_intel_error` already use elsewhere in this app.

use super::messages::{Message, MessageSpawner, Type};
use rodio::Decoder;
use rodio::MixerDeviceSink;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

/// Opens and decodes the alarm sound at `path`. Split out of
/// [`AlarmPlayer::play_alarm`] as its own free function specifically so it
/// can be unit tested on its own: unlike the rest of [`AlarmPlayer`], it
/// never touches the audio output device, so -- unlike testing
/// [`AlarmPlayer::new`]'s success path, which depends on a real device
/// being available and can't be relied on in every environment `cargo
/// test` runs in (present on a dev machine, absent on a headless CI
/// runner) -- its behavior on a missing file, a missing directory or a
/// corrupt/unsupported file is exactly as testable here as it is at
/// runtime.
fn open_alarm_sound(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|error| {
        format!(
            "could not open the alarm sound at {}: {error}",
            path.display()
        )
    })?;
    Decoder::try_from(file).map_err(|error| format!("could not decode the alarm sound: {error}"))
}

/// Owns the handle to the default audio output device and the message
/// spawner used to surface playback failures. `sink` is `None` when no
/// output device could be opened, so [`Self::play_alarm`] silently does
/// nothing instead of failing on every single alert.
pub(crate) struct AlarmPlayer {
    sink: Option<MixerDeviceSink>,
    task_msg: Arc<MessageSpawner>,
}

impl AlarmPlayer {
    /// Opens the default output device. Never panics: a machine with no
    /// usable audio device, or whose driver fails to open, gets a silent
    /// app instead of a crash at startup.
    #[tracing::instrument(skip(task_msg))]
    pub(crate) fn new(task_msg: Arc<MessageSpawner>) -> Self {
        let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => Some(sink),
            Err(error) => {
                tracing::warn!(
                    "no audio output device available, alarm sounds are disabled: {error}"
                );
                task_msg.spawn(Message::GenericNotification((
                    Type::Warning,
                    String::from("AlarmPlayer"),
                    String::from("new"),
                    format!(
                        "No audio output device available; alarm sounds are disabled ({error})."
                    ),
                )));
                None
            }
        };
        Self { sink, task_msg }
    }

    /// Plays `sound_path` (see `Settings::get_alert_sound_path`) on the
    /// mixer. Non-blocking: the mixer queues and plays it on its own, this
    /// call never waits for playback to finish. Reports and does nothing
    /// if there is no output device (see [`Self::new`]) -- without even
    /// trying to open `sound_path`, so a headless machine with no audio
    /// device doesn't also get a spurious "file not found" if
    /// `assets/alerts/` happens to be missing too -- or if
    /// [`open_alarm_sound`] fails for any reason.
    #[tracing::instrument(skip(self))]
    pub(crate) fn play_alarm(&self, sound_path: &Path) {
        let Some(sink) = &self.sink else {
            return;
        };
        match open_alarm_sound(sound_path) {
            Ok(source) => sink.mixer().add(source),
            Err(message) => {
                tracing::warn!("{message}");
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Warning,
                    String::from("AlarmPlayer"),
                    String::from("play_alarm"),
                    message,
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    /// A `MessageSpawner` over a channel this test can read back, to check
    /// whether `play_alarm` reported anything -- `MessageSpawner::spawn`
    /// only ever `try_send`s (see its own doc comment) and never blocks
    /// waiting for a receiver, so an unread channel is fine to construct
    /// one with even when a test doesn't care what, if anything, lands in
    /// it.
    fn task_msg_with_receiver() -> (Arc<MessageSpawner>, mpsc::Receiver<Message>) {
        let (tx, rx) = mpsc::channel::<Message>(4);
        (Arc::new(MessageSpawner::new(Arc::new(tx))), rx)
    }

    #[test]
    fn open_alarm_sound_errors_when_the_file_is_missing() {
        // The directory exists (it's this repo's real `assets/alerts/`,
        // see `open_alarm_sound_decodes_a_real_bundled_sound` below) but
        // this file inside it does not.
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/alerts"));
        let path = dir.join("this-sound-does-not-exist.wav");

        assert!(open_alarm_sound(&path).is_err());
    }

    #[test]
    fn open_alarm_sound_errors_when_the_alerts_directory_itself_is_missing() {
        // The other half of the question this module exists to answer: a
        // machine that never got `assets/alerts/` bundled next to it (see
        // this module's docs and `packager.json`'s `resources` entry)
        // shouldn't crash either, just play no sound. `File::open` reports
        // both this and a missing-file-in-an-existing-directory the same
        // way (ENOENT), but it's worth its own test since the user-facing
        // failure mode ("nothing was ever installed here") is different
        // from the one above ("something's missing from what was").
        let path = PathBuf::from("/nonexistent-telescope-alerts-directory/whatever.wav");

        assert!(open_alarm_sound(&path).is_err());
    }

    #[test]
    fn open_alarm_sound_decodes_a_real_bundled_sound() {
        // Positive control: without this, the two tests above could be
        // passing for the wrong reason (`open_alarm_sound` always failing,
        // say). `CARGO_MANIFEST_DIR` is a compile-time constant (this
        // crate's own directory on disk), not the test binary's working
        // directory -- `cargo test` sets that to the package root
        // (`crates/telescope`), not the workspace root `settings::ALERTS_DIR`
        // is actually relative to (see
        // `Settings::set_alert_sound_for_test`'s doc comment for the same
        // issue on the settings side) -- so this resolves correctly
        // regardless of where `cargo test` is invoked from.
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/alerts/1_campana_info.wav"
        ));

        assert!(open_alarm_sound(&path).is_ok());
    }

    #[test]
    fn play_alarm_without_an_output_device_reports_nothing_and_does_not_panic() {
        // `sink: None` is what `AlarmPlayer::new` produces on a machine
        // with no usable audio output (see its doc comment) -- constructed
        // directly here (this test module can reach `AlarmPlayer`'s
        // private fields, being a child of the module that declares them)
        // rather than through `new()`, since opening a real device isn't
        // something a test can rely on either way.
        let (task_msg, mut rx) = task_msg_with_receiver();
        let player = AlarmPlayer {
            sink: None,
            task_msg,
        };

        // Must not panic, and -- since `new()` already reported the
        // missing device once at startup -- must not report anything a
        // second time here, whether or not `sound_path` itself is real.
        player.play_alarm(Path::new(
            "/nonexistent-telescope-alerts-directory/whatever.wav",
        ));

        assert!(rx.try_recv().is_err());
    }
}
