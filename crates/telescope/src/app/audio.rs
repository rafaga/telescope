//! Alarm sound for the intel rules' Sound output.
//!
//! [`AlarmPlayer`] opens the default audio output device at startup and
//! keeps it open for the app's lifetime -- rodio's output handle has to
//! stay alive for as long as anything should be audible, so it lives as a
//! field on [`TelescopeApp`](super::TelescopeApp) rather than being opened
//! per alert (which would also mean a slow device-open on the UI thread for
//! every single match). Playing a sound is then just decoding the alarm
//! clip ([`open_alarm_sound`]) and handing it to the mixer
//! (`AlarmPlayer::play_alarm`): the mixer takes ownership from there and
//! plays it to completion on its own, so the call site never blocks and
//! never has to hold on to anything.
//!
//! An open output is bound to one physical device, and rodio never rebinds
//! it: once that device is unplugged, or Windows drops it on suspend/resume,
//! the stream stays open but silent, and playing into it "succeeds" without
//! a sound. So the stream is opened with an error callback (see
//! [`on_stream_error`]) that flags the loss and tells the user, and the next
//! alarm reopens the output on whatever the default device is by then --
//! the same happens when the system default is switched to another device.
//! Attempts are rate limited ([`REOPEN_COOLDOWN`]) and each outcome is
//! logged with the device name.
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
//! Settings -> Alerts page (`windows::settings::alerts`),
//! resolved against `Settings::alerts_dir`. That's also where
//! the "relative to wherever Telescope is run from" convention lives
//! (`settings::FilePaths::default`'s doc comment explains why: same as
//! `sde.db` and `telescope.toml`, it's meant to sit
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
use rodio::cpal::StreamError;
use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// Shortest time between two attempts to (re)open the output device. A
/// device that is gone stays gone for a while (unplugged, machine asleep),
/// and every attempt enumerates the audio devices on the UI thread, so a
/// burst of alerts must not turn into a burst of attempts.
const REOPEN_COOLDOWN: Duration = Duration::from_secs(5);

/// A freshly opened output: the rodio handle plus the name of the device it
/// is bound to (kept to log which device was lost and to notice when the
/// system default has moved somewhere else).
struct OpenedOutput {
    sink: MixerDeviceSink,
    device: String,
}

/// Opens the system's default output. `lost` is raised by the stream's
/// error callback, from the audio thread, when the device goes away.
type OpenOutput = fn(Arc<AtomicBool>, Arc<MessageSpawner>) -> Result<OpenedOutput, String>;

/// The two calls that touch real audio hardware, behind function pointers so
/// the reopen logic can be tested on a machine (or CI runner) without any.
struct Backend {
    open: OpenOutput,
    /// Name of the device that is the system default right now, if any.
    default_device: fn() -> Option<String>,
}

impl Backend {
    fn system() -> Self {
        Self {
            open: open_default_output,
            default_device: default_device_name,
        }
    }
}

/// The current output and what is known about its health.
struct Output {
    sink: Option<MixerDeviceSink>,
    /// Name of the device `sink` was opened on.
    device: Option<String>,
    /// Raised by the stream's error callback when the device is lost. Each
    /// opened stream gets its own flag, so a late callback from a stream
    /// that was already replaced cannot mark the new one as lost.
    lost: Arc<AtomicBool>,
    last_attempt: Option<Instant>,
    /// Whether the last failed attempt was already shown to the user, so a
    /// device that stays missing is reported once, not on every retry.
    failure_reported: bool,
    /// Whether an output was ever opened, to tell a recovery (worth telling
    /// the user about) from the first open at startup.
    ever_opened: bool,
}

impl Output {
    fn closed() -> Self {
        Self {
            sink: None,
            device: None,
            lost: Arc::new(AtomicBool::new(false)),
            last_attempt: None,
            failure_reported: false,
            ever_opened: false,
        }
    }

    fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// The sink, only while it can actually be heard.
    fn usable_sink(&self) -> Option<&MixerDeviceSink> {
        if self.is_lost() {
            None
        } else {
            self.sink.as_ref()
        }
    }

    /// Reopens the output when there is none, when it was lost, or when the
    /// system default device is no longer the one it is bound to. Rate
    /// limited by [`REOPEN_COOLDOWN`]; reports every outcome that matters
    /// to the user, once.
    fn refresh(&mut self, backend: &Backend, task_msg: &Arc<MessageSpawner>) {
        let reason = if self.sink.is_none() {
            "no output is open"
        } else if self.is_lost() {
            "the output device was lost"
        } else if (backend.default_device)()
            .zip(self.device.as_deref())
            .is_some_and(|(default, open)| default != open)
        {
            "the system default output device changed"
        } else {
            return;
        };
        if self
            .last_attempt
            .is_some_and(|last| last.elapsed() < REOPEN_COOLDOWN)
        {
            return;
        }
        self.last_attempt = Some(Instant::now());

        let lost = Arc::new(AtomicBool::new(false));
        match (backend.open)(Arc::clone(&lost), Arc::clone(task_msg)) {
            Ok(opened) => {
                tracing::info!("audio output opened on '{}' ({reason})", opened.device);
                if self.ever_opened {
                    notify(
                        task_msg,
                        Type::Info,
                        format!("Audio output restored on '{}'.", opened.device),
                    );
                }
                self.sink = Some(opened.sink);
                self.device = Some(opened.device);
                self.lost = lost;
                self.failure_reported = false;
                self.ever_opened = true;
            }
            Err(error) => {
                tracing::warn!("could not open the audio output ({reason}): {error}");
                if !self.failure_reported {
                    self.failure_reported = true;
                    notify(
                        task_msg,
                        Type::Warning,
                        format!(
                            "Audio output unavailable ({error}); alarm sounds are muted. \
                             It is retried on the next alert."
                        ),
                    );
                }
            }
        }
    }
}

fn notify(task_msg: &MessageSpawner, kind: Type, text: String) {
    task_msg.spawn(Message::GenericNotification((
        kind,
        String::from("AlarmPlayer"),
        String::from("audio_output"),
        text,
    )));
}

/// Whether a stream error means the output can no longer be heard and has to
/// be reopened, as opposed to a glitch the stream survives (an underrun).
fn is_device_loss(error: &StreamError) -> bool {
    !matches!(error, StreamError::BufferUnderrun)
}

/// The stream's error callback, running on the audio thread. rodio's own
/// default only logs the error and leaves the (now silent) sink in place, so
/// this is what lets [`AlarmPlayer`] notice the loss: it raises `lost` and
/// tells the user, once per stream however many errors follow.
fn on_stream_error(
    error: &StreamError,
    device: &str,
    lost: &AtomicBool,
    task_msg: &MessageSpawner,
) {
    if !is_device_loss(error) {
        tracing::debug!("audio stream glitch on '{device}': {error}");
        return;
    }
    if !lost.swap(true, Ordering::SeqCst) {
        tracing::error!("audio output '{device}' lost: {error}");
        notify(
            task_msg,
            Type::Warning,
            format!(
                "Audio output '{device}' was lost ({error}). Alarm sounds resume on the \
                 next alert if an output device is available."
            ),
        );
    }
}

fn default_output_device() -> Option<rodio::Device> {
    use rodio::cpal::traits::HostTrait;
    rodio::cpal::default_host().default_output_device()
}

fn describe(device: &rodio::Device) -> String {
    use rodio::DeviceTrait;
    device
        .description()
        .map_or_else(|_| String::from("unknown device"), |d| d.to_string())
}

fn default_device_name() -> Option<String> {
    default_output_device().map(|device| describe(&device))
}

/// Opens the system default output with an error callback that reports a
/// lost device (see [`on_stream_error`]). Like rodio's own
/// `open_default_sink`, it falls back to the device's other supported
/// configurations when the default one is refused.
fn open_default_output(
    lost: Arc<AtomicBool>,
    task_msg: Arc<MessageSpawner>,
) -> Result<OpenedOutput, String> {
    let device = default_output_device().ok_or("no default audio output device")?;
    let name = describe(&device);
    let callback_name = name.clone();
    let sink = rodio::DeviceSinkBuilder::from_device(device)
        .map_err(|error| error.to_string())?
        .with_error_callback(move |error| {
            on_stream_error(&error, &callback_name, &lost, &task_msg);
        })
        .open_sink_or_fallback()
        .map_err(|error| error.to_string())?;
    Ok(OpenedOutput { sink, device: name })
}

/// Owns the audio output and the message spawner used to surface playback
/// failures. The output is opened on the system default device and reopened
/// by [`Self::play_alarm`] when it is missing, lost (unplugged, machine
/// resumed from sleep) or no longer the default, so alarms keep sounding
/// after the device changes instead of going silently to a dead stream.
pub(crate) struct AlarmPlayer {
    output: RefCell<Output>,
    backend: Backend,
    task_msg: Arc<MessageSpawner>,
}

/// Shortest time between two intel alarms: a report is usually repeated by
/// several pilots within seconds, and each repeat would start the sound
/// again on top of the one still playing.
const ALERT_COOLDOWN: Duration = Duration::from_secs(3);

/// Thread-safe front of the [`AlarmPlayer`]. The player (and the output
/// stream it owns, which is not `Send` on every platform) lives on its own
/// thread; this handle sends it the sounds to play, so the alarm can be
/// triggered from any thread -- in particular from the intel dispatch
/// thread, which does not depend on the UI loop running.
pub(crate) struct AudioHandle {
    tx: Sender<PathBuf>,
    /// When an intel alarm last started (see [`Self::play_alert`]).
    last_alert: Mutex<Option<Instant>>,
}

impl AudioHandle {
    /// Starts the audio thread, which opens the output device itself.
    pub(crate) fn spawn(task_msg: Arc<MessageSpawner>) -> Self {
        let (tx, rx) = channel::<PathBuf>();
        let spawned = std::thread::Builder::new()
            .name(String::from("telescope-audio"))
            .spawn(move || {
                let player = AlarmPlayer::new(task_msg);
                while let Ok(path) = rx.recv() {
                    let _span = tracing::info_span!("audio_play", path = %path.display()).entered();
                    let started = Instant::now();
                    player.play_alarm(&path);
                    tracing::debug!(
                        elapsed_us = started.elapsed().as_micros() as u64,
                        "alarm queued"
                    );
                }
                tracing::debug!("audio thread stopped");
            });
        if let Err(error) = spawned {
            tracing::error!("could not start the audio thread: {error}");
        }
        Self {
            tx,
            last_alert: Mutex::new(None),
        }
    }

    /// A handle wired to a channel the caller reads instead of an audio
    /// thread: what would be played arrives as a path on the receiver.
    #[cfg(test)]
    pub(crate) fn for_test() -> (Self, std::sync::mpsc::Receiver<PathBuf>) {
        let (tx, rx) = channel::<PathBuf>();
        (
            Self {
                tx,
                last_alert: Mutex::new(None),
            },
            rx,
        )
    }

    /// Plays `sound_path` on the audio thread. Non-blocking.
    pub(crate) fn play_alarm(&self, sound_path: &Path) {
        if self.tx.send(sound_path.to_path_buf()).is_err() {
            tracing::warn!("the audio thread is not running; alarm skipped");
        }
    }

    /// Plays the alarm of an intel alert, unless one started less than
    /// [`ALERT_COOLDOWN`] ago. Returns whether it played.
    pub(crate) fn play_alert(&self, sound_path: &Path) -> bool {
        if !self.claim_cooldown(Instant::now()) {
            tracing::debug!("alarm suppressed by the cooldown");
            return false;
        }
        self.play_alarm(sound_path);
        true
    }

    /// Whether an alarm may start at `now`; if so, records it as the last one.
    fn claim_cooldown(&self, now: Instant) -> bool {
        let mut last = self
            .last_alert
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|previous| now.duration_since(previous) < ALERT_COOLDOWN) {
            return false;
        }
        *last = Some(now);
        true
    }
}

impl AlarmPlayer {
    /// Opens the default output device. Never panics: a machine with no
    /// usable audio device, or whose driver fails to open, gets a silent
    /// app instead of a crash at startup (and a retry on later alerts).
    #[tracing::instrument(skip(task_msg))]
    pub(crate) fn new(task_msg: Arc<MessageSpawner>) -> Self {
        Self::with_backend(task_msg, Backend::system())
    }

    fn with_backend(task_msg: Arc<MessageSpawner>, backend: Backend) -> Self {
        let mut output = Output::closed();
        output.refresh(&backend, &task_msg);
        Self {
            output: RefCell::new(output),
            backend,
            task_msg,
        }
    }

    /// Plays `sound_path` (see `Settings::get_alert_sound_path`) on the
    /// mixer. Non-blocking: the mixer queues and plays it on its own, this
    /// call never waits for playback to finish. First makes sure the output
    /// is still the right one (see [`Output::refresh`]). Does nothing if
    /// there is no usable output -- without even trying to open
    /// `sound_path`, so a headless machine with no audio device doesn't
    /// also get a spurious "file not found" if `assets/alerts/` happens to
    /// be missing too -- and reports if [`open_alarm_sound`] fails for any
    /// reason.
    #[tracing::instrument(skip(self))]
    pub(crate) fn play_alarm(&self, sound_path: &Path) {
        let mut output = self.output.borrow_mut();
        output.refresh(&self.backend, &self.task_msg);
        let Some(sink) = output.usable_sink() else {
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
        // (`crates/telescope`), not the workspace root `Settings::alerts_dir`
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

    /// A backend with no audio hardware at all: every open fails and there
    /// is no default device. The counter lets a test see how many attempts
    /// were made.
    fn backend_without_device() -> Backend {
        Backend {
            open: |_, _| Err(String::from("no default audio output device")),
            default_device: || None,
        }
    }

    fn drain(rx: &mut mpsc::Receiver<Message>) -> usize {
        std::iter::from_fn(|| rx.try_recv().ok()).count()
    }

    #[test]
    fn a_missing_device_is_reported_once_and_playing_stays_silent() {
        let (task_msg, mut rx) = task_msg_with_receiver();
        let player = AlarmPlayer::with_backend(task_msg, backend_without_device());
        // The failed open at startup is what the user is told about.
        assert_eq!(drain(&mut rx), 1);

        // Must not panic, and must not report again -- whether or not
        // `sound_path` itself is real.
        player.play_alarm(Path::new(
            "/nonexistent-telescope-alerts-directory/whatever.wav",
        ));
        assert_eq!(drain(&mut rx), 0);
    }

    #[test]
    fn reopening_is_retried_only_after_the_cooldown_and_reported_once() {
        use std::sync::atomic::AtomicUsize;
        static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
        let (task_msg, mut rx) = task_msg_with_receiver();
        let player = AlarmPlayer::with_backend(
            task_msg,
            Backend {
                open: |_, _| {
                    ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                    Err(String::from("still gone"))
                },
                default_device: || None,
            },
        );
        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 1);
        assert_eq!(drain(&mut rx), 1);

        // Inside the cooldown: no new attempt.
        player.play_alarm(Path::new("alarm.wav"));
        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 1);

        // After it: one more attempt, and the failure is not reported again.
        player.output.borrow_mut().last_attempt =
            Some(Instant::now() - REOPEN_COOLDOWN - Duration::from_millis(1));
        player.play_alarm(Path::new("alarm.wav"));
        assert_eq!(ATTEMPTS.load(Ordering::SeqCst), 2);
        assert_eq!(drain(&mut rx), 0);
    }

    #[test]
    fn only_glitches_are_not_treated_as_a_lost_device() {
        assert!(is_device_loss(&StreamError::DeviceNotAvailable));
        assert!(is_device_loss(&StreamError::StreamInvalidated));
        assert!(!is_device_loss(&StreamError::BufferUnderrun));
    }

    #[test]
    fn a_lost_device_is_flagged_and_reported_once() {
        let (task_msg, mut rx) = task_msg_with_receiver();
        let lost = AtomicBool::new(false);

        on_stream_error(&StreamError::BufferUnderrun, "Speakers", &lost, &task_msg);
        assert!(!lost.load(Ordering::SeqCst));
        assert_eq!(drain(&mut rx), 0);

        // The callback keeps firing while the device stays gone.
        for _ in 0..3 {
            on_stream_error(
                &StreamError::DeviceNotAvailable,
                "Speakers",
                &lost,
                &task_msg,
            );
        }
        assert!(lost.load(Ordering::SeqCst));
        assert_eq!(drain(&mut rx), 1);
    }

    #[test]
    fn an_alert_repeated_within_the_cooldown_does_not_play_again() {
        let (tx, _rx) = channel::<PathBuf>();
        let handle = AudioHandle {
            tx,
            last_alert: Mutex::new(None),
        };
        let now = Instant::now();
        assert!(handle.claim_cooldown(now));
        assert!(!handle.claim_cooldown(now + Duration::from_millis(500)));
        // Once the cooldown is over it plays again.
        assert!(handle.claim_cooldown(now + ALERT_COOLDOWN + Duration::from_millis(1)));
    }
}
