//! Says *why* the GPU stopped working, when it does.
//!
//! On Windows (DX12) the driver can drop the GPU device while the computer
//! sleeps, a display is unplugged or the driver resets. wgpu then reports
//! "device lost" without raising an error of its own, and on the next frame
//! `egui-wgpu` panics with "Failed to create staging buffer for index data":
//! `Queue::write_buffer_with` returns `None` because the device is gone.
//! That panic alone says nothing about the cause, and eframe 0.36 cannot
//! rebuild its renderer on a new device, so the process cannot go on.
//!
//! This module cannot recover in place, so when wgpu reported the device lost
//! it relaunches the program (a new process gets a new device), a limited
//! number of times. It also records what surrounds the failure, in the log
//! and on stderr:
//! - the adapter in use, once at startup;
//! - the reason and message wgpu gives when the device is lost;
//! - gaps between frames (a long one right before a failure points to sleep
//!   or a stalled driver rather than to the application);
//! - a report when the staging-buffer panic happens: whether the device was
//!   reported lost, how long ago the last frame was drawn, the longest recent
//!   gap and how long the process had been running.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A gap between two frames at least this long is logged.
const FRAME_GAP_LOG_THRESHOLD: Duration = Duration::from_secs(5);

/// What `egui-wgpu` panics with when it can no longer write to the device.
const STAGING_BUFFER_PANIC: &str = "Failed to create staging buffer";

/// When the process started (the first call to [`install`] or [`frame_tick`]).
fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// The adapter in use, as one line of text.
static ADAPTER: OnceLock<String> = OnceLock::new();

/// Set by the device-lost callback.
static DEVICE_LOST: AtomicBool = AtomicBool::new(false);

/// Milliseconds since [`start`] at the last frame (0 before the first).
static LAST_FRAME_MS: AtomicU64 = AtomicU64::new(0);

/// The longest gap between frames seen so far and when it ended.
static LONGEST_GAP: Mutex<Option<(Duration, Duration)>> = Mutex::new(None);

fn since_start() -> Duration {
    start().elapsed()
}

fn adapter() -> &'static str {
    ADAPTER.get().map_or("unknown adapter", String::as_str)
}

/// Logs the adapter, registers the device-lost callback and installs a panic
/// hook that adds a report when the GPU write fails. Called once, from
/// `TelescopeApp::new`.
pub(crate) fn install(cc: &eframe::CreationContext<'_>) {
    let _ = start();
    if let Some(render_state) = cc.wgpu_render_state.as_ref() {
        let info = render_state.adapter.get_info();
        let summary = format!(
            "{} ({:?}, {:?}, driver {} {}, vendor {:#06x}, device {:#06x})",
            info.name,
            info.backend,
            info.device_type,
            info.driver,
            info.driver_info,
            info.vendor,
            info.device
        );
        tracing::info!(adapter = %summary, "GPU adapter");
        let _ = ADAPTER.set(summary);
        render_state
            .device
            .set_device_lost_callback(move |reason, message| {
                DEVICE_LOST.store(true, Ordering::SeqCst);
                tracing::error!(
                    ?reason,
                    %message,
                    adapter = adapter(),
                    running_for = ?since_start(),
                    since_last_frame = ?since_last_frame(),
                    "GPU device lost"
                );
            });
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or_default();
        if payload.contains(STAGING_BUFFER_PANIC) {
            report(payload);
            if should_restart(
                DEVICE_LOST.load(Ordering::SeqCst),
                since_start(),
                restarts_so_far(),
            ) {
                restart();
            }
        }
        default_hook(info);
    }));
}

/// Environment variable carrying how many automatic restarts led to this
/// process, so a GPU that fails again and again cannot cause a restart loop.
const RESTART_COUNT_VAR: &str = "TELESCOPE_GPU_RESTARTS";

/// Restarts allowed in a row.
const MAX_RESTARTS: u32 = 3;

/// A process that failed sooner than this is not restarted: it would just
/// fail again.
const MIN_UPTIME_FOR_RESTART: Duration = Duration::from_secs(10);

fn restarts_so_far() -> u32 {
    std::env::var(RESTART_COUNT_VAR)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// Whether to relaunch after the GPU write failed. Only when wgpu reported
/// the device lost (otherwise it is a bug in this application, and a restart
/// would hide it), the process had been running a while, and the restart
/// budget is not spent.
fn should_restart(device_lost: bool, uptime: Duration, restarts: u32) -> bool {
    device_lost && uptime >= MIN_UPTIME_FOR_RESTART && restarts < MAX_RESTARTS
}

/// Starts a new copy of this program (a new process gets a new GPU device)
/// and ends this one. On failure to spawn, returns so the panic goes on.
fn restart() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let message = "GPU device lost: restarting Telescope";
    tracing::error!("{message}");
    eprintln!("{message}");
    let spawned = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(RESTART_COUNT_VAR, (restarts_so_far() + 1).to_string())
        .spawn();
    match spawned {
        Ok(_) => std::process::exit(0),
        Err(error) => eprintln!("could not restart: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_only_after_a_reported_loss() {
        let long = Duration::from_secs(60);
        assert!(should_restart(true, long, 0));
        assert!(!should_restart(false, long, 0));
    }

    #[test]
    fn does_not_restart_in_a_loop() {
        let long = Duration::from_secs(60);
        assert!(should_restart(true, long, MAX_RESTARTS - 1));
        assert!(!should_restart(true, long, MAX_RESTARTS));
        assert!(!should_restart(true, Duration::from_secs(2), 0));
    }
}

/// Called at the start of every frame: notes when it happened and logs a
/// long gap since the previous one.
pub(crate) fn frame_tick() {
    let now = since_start();
    let previous = LAST_FRAME_MS.swap(now.as_millis() as u64, Ordering::Relaxed);
    if previous == 0 {
        return;
    }
    let gap = now.saturating_sub(Duration::from_millis(previous));
    if gap < FRAME_GAP_LOG_THRESHOLD {
        return;
    }
    tracing::warn!(
        gap = ?gap,
        running_for = ?now,
        device_lost = DEVICE_LOST.load(Ordering::SeqCst),
        "no frame was drawn for a while (sleep, a hidden window or a stalled driver)"
    );
    if let Ok(mut longest) = LONGEST_GAP.lock()
        && longest.is_none_or(|(known, _)| gap > known)
    {
        *longest = Some((gap, now));
    }
}

/// Time since the last frame began, `None` before the first.
fn since_last_frame() -> Option<Duration> {
    let last = LAST_FRAME_MS.load(Ordering::Relaxed);
    (last != 0).then(|| since_start().saturating_sub(Duration::from_millis(last)))
}

/// The report written when `egui-wgpu` panics on a failed GPU write.
fn report(panic_message: &str) {
    let longest = LONGEST_GAP
        .lock()
        .ok()
        .and_then(|longest| *longest)
        .map(|(gap, at)| format!("{gap:?} (ended {at:?} after start)"))
        .unwrap_or_else(|| String::from("none over 5 s"));
    let text = format!(
        "GPU write failed: {panic_message}\n\
         \x20 device reported lost by wgpu: {}\n\
         \x20 adapter: {}\n\
         \x20 running for: {:?}\n\
         \x20 since the last frame began: {:?}\n\
         \x20 longest gap between frames: {longest}\n\
         \x20 A size that fits with the device reported lost, or a long gap right \
         before, points to sleep or a driver reset; a device not reported lost \
         points to a wgpu validation problem in this application.",
        DEVICE_LOST.load(Ordering::SeqCst),
        adapter(),
        since_start(),
        since_last_frame(),
    );
    tracing::error!("{text}");
    eprintln!("{text}");
}
