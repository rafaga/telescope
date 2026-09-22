//! The on-screen status log: turns incoming `GenericNotification` messages into
//! colored log entries (capped at `MAX_APP_MESSAGES`) and reports errors changing
//! the intel directory.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use crate::app::settings::SettingsError;
use eframe::egui::{Color32, FontFamily, FontId, TextFormat, epaint::text::LayoutJob};

/// Maximum number of entries kept in [`TelescopeApp::app_messages`], the
/// notification log shown at the bottom of the window.
///
/// Every `GenericNotification` the app ever emits -- intel matches, ESI
/// errors, debug traces -- ends up here via `update_status_with_error`,
/// which only ever pushes, never trims. Across a long play session that is
/// an unbounded `Vec<LayoutJob>` growing for as long as the app stays open;
/// the on-screen list is already virtualized (`show_rows` in `update()`
/// only lays out the visible rows), so the cost is pure memory growth, not
/// rendering time, but it still never comes back down. Oldest entries are
/// dropped once this cap is reached -- see `update_status_with_error`.
const MAX_APP_MESSAGES: usize = 500;

/// How long an incoming `GenericNotification` must differ from the one
/// immediately before it (in the on-screen log, not necessarily the one
/// most recently *received*, since a dedup'd notification never updates
/// this) to be shown, rather than silently collapsed.
///
/// Two independent upstream sources can each emit back-to-back duplicates
/// of what is, to the user, the exact same event: the file watcher can
/// report more than one filesystem event for a single write (see
/// `file.rs`'s combined `EventKind` handling), and the pattern engine can
/// match more than one rule against the same intel line, each producing
/// its own `Message::GenericNotification` (see
/// `patterns::PatternEngine::evaluate`). Both paths funnel through
/// [`TelescopeApp::update_status_with_error`], so a single exact-match
/// check here -- same [`Type`], source, context and text, within this
/// window -- covers both without either upstream needing to know about
/// the other or de-duplicate itself. `ActionConfig::MapAlert` matches
/// never reach this function on their success path, so they are
/// unaffected.
///
/// A short window rather than an unconditional "collapse repeats of the
/// last entry" keeps this a duplicate-*burst* filter, not a general
/// throttle: the same text reappearing minutes apart (e.g. the same
/// pattern matching again on a later intel line) is still shown.
const NOTIFICATION_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(1);

/// The pure comparison behind the dedup check in
/// [`TelescopeApp::update_status_with_error`], split out so it can be unit
/// tested without spinning up a whole `TelescopeApp`. `true` means
/// `message` is an exact repeat of `last` and arrived within
/// [`NOTIFICATION_DEDUP_WINDOW`] of it, and should be collapsed.
fn is_duplicate_notification(
    last: Option<&(Type, String, String, String, std::time::Instant)>,
    message: &(Type, String, String, String),
    now: std::time::Instant,
) -> bool {
    match last {
        Some((last_type, last_source, last_context, last_text, last_time)) => {
            *last_type == message.0
                && *last_source == message.1
                && *last_context == message.2
                && *last_text == message.3
                && now.duration_since(*last_time) < NOTIFICATION_DEDUP_WINDOW
        }
        None => false,
    }
}

impl TelescopeApp {
    /// Surfaces a `SettingsError` from an intel-directory change as an
    /// on-screen `GenericNotification` instead of letting it disappear
    /// silently. `set_intel`/`scan_channels_logs` failures in the
    /// `UpdateIntelDirectory`/`DefaultIntelDirectory` handlers used to be
    /// discarded with `let _ = ...`, which left the Settings UI just
    /// showing "No intel channels detected" with no indication of why --
    /// e.g. a stale/rejected path (`set_intel` only accepts paths that
    /// already exist) or a permissions/read error on the directory
    /// (`scan_channels_logs`).
    #[tracing::instrument(skip(self, error))]
    pub(crate) fn notify_intel_error(&self, context: &'static str, error: SettingsError) {
        self.task_msg.spawn(Message::GenericNotification((
            Type::Error,
            String::from("TelescopeApp"),
            String::from(context),
            error.to_string(),
        )));
    }

    #[tracing::instrument(skip(self, message))]
    pub(crate) fn update_status_with_error(&mut self, message: (Type, String, String, String)) {
        let now = std::time::Instant::now();
        if is_duplicate_notification(self.last_notification.as_ref(), &message, now) {
            // Exact repeat of the immediately-preceding notification,
            // arrived within the dedup window -- collapse it. See
            // `NOTIFICATION_DEDUP_WINDOW`'s doc comment for why this one
            // check covers both the watcher-duplication and the
            // multi-rule-match cases.
            return;
        }
        self.last_notification = Some((
            message.0,
            message.1.clone(),
            message.2.clone(),
            message.3.clone(),
            now,
        ));

        let full_time = chrono::Local::now().time().to_string();
        let time = full_time.split_at(12);
        let mut job = LayoutJob::default();
        let normal_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::LIGHT_GRAY,
            ..Default::default()
        };
        let time_text = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DARK_GRAY,
            ..Default::default()
        };
        let warn = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::KHAKI,
            ..Default::default()
        };
        let info = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::BLUE,
            ..Default::default()
        };
        let debug = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::DEBUG_COLOR,
            ..Default::default()
        };
        let error = TextFormat {
            font_id: FontId::new(12.0, FontFamily::Proportional),
            color: Color32::RED,
            ..Default::default()
        };
        job.append("[", 0.0, normal_text.clone());
        job.append(time.0, 0.0, time_text.clone());
        job.append("] ", 0.0, normal_text.clone());
        match message.0 {
            Type::Error => {
                job.append("ERROR: ", 0.0, error.clone());
                job.append(
                    (message.1 + " - " + &message.2 + " - ").as_str(),
                    0.0,
                    normal_text.clone(),
                );
            }
            Type::Warning => {
                job.append("WARN: ", 0.0, warn.clone());
            }
            Type::Info => {
                job.append("INFO: ", 0.0, info.clone());
            }
            Type::Debug => {
                job.append("DEBUG: ", 0.0, debug.clone());
            }
        }
        job.append(&message.3, 0.0, normal_text.clone());
        self.app_messages.push(job);
        // Drop the oldest entry once we're over the cap, so this log stays
        // bounded no matter how long the app runs. `remove(0)` shifts at
        // most `MAX_APP_MESSAGES` elements -- bounded by the cap itself, not
        // by session length -- so this stays cheap even though it's O(n).
        if self.app_messages.len() > MAX_APP_MESSAGES {
            self.app_messages.remove(0);
        }
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn message(kind: Type, text: &str) -> (Type, String, String, String) {
        (
            kind,
            String::from("source"),
            String::from("context"),
            String::from(text),
        )
    }

    #[test]
    fn nothing_is_a_duplicate_when_there_is_no_previous_notification() {
        assert!(!is_duplicate_notification(
            None,
            &message(Type::Info, "hello"),
            Instant::now()
        ));
    }

    #[test]
    fn an_exact_repeat_within_the_window_is_a_duplicate() {
        let now = Instant::now();
        let last = (
            Type::Info,
            String::from("source"),
            String::from("context"),
            String::from("hello"),
            now,
        );
        assert!(is_duplicate_notification(
            Some(&last),
            &message(Type::Info, "hello"),
            now
        ));
    }

    #[test]
    fn different_text_is_not_a_duplicate() {
        let now = Instant::now();
        let last = (
            Type::Info,
            String::from("source"),
            String::from("context"),
            String::from("hello"),
            now,
        );
        assert!(!is_duplicate_notification(
            Some(&last),
            &message(Type::Info, "goodbye"),
            now
        ));
    }

    #[test]
    fn different_type_is_not_a_duplicate() {
        let now = Instant::now();
        let last = (
            Type::Warning,
            String::from("source"),
            String::from("context"),
            String::from("hello"),
            now,
        );
        assert!(!is_duplicate_notification(
            Some(&last),
            &message(Type::Info, "hello"),
            now
        ));
    }

    #[test]
    fn a_repeat_outside_the_window_is_not_a_duplicate() {
        let now = Instant::now();
        let last = (
            Type::Info,
            String::from("source"),
            String::from("context"),
            String::from("hello"),
            now,
        );
        let later = now + NOTIFICATION_DEDUP_WINDOW + Duration::from_millis(1);
        assert!(!is_duplicate_notification(
            Some(&last),
            &message(Type::Info, "hello"),
            later
        ));
    }
}
