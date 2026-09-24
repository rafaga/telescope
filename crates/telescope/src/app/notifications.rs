//! The on-screen status log: turns incoming `GenericNotification` messages into
//! colored log entries (capped at `MAX_APP_MESSAGES`) and reports errors changing
//! the intel directory.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use crate::app::settings::{SettingsError, UiState};
use eframe::egui::{
    self, Color32, FontFamily, FontId, Margin, TextFormat, epaint::text::LayoutJob,
};

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
/// `message` would show exactly the same log entry as `last` and arrived
/// within [`NOTIFICATION_DEDUP_WINDOW`] of it, and should be collapsed.
///
/// Source and context only count for errors, the only type whose entry
/// prints them. For the rest they are invisible, and comparing them let
/// through the most common duplicate: one intel line matched by two
/// `notify` rules (e.g. `intel_line` plus a `ship_report_*` dictionary)
/// arrives twice with the same text but a different rule id as context.
fn is_duplicate_notification(
    last: Option<&(Type, String, String, String, std::time::Instant)>,
    message: &(Type, String, String, String),
    now: std::time::Instant,
) -> bool {
    match last {
        Some((last_type, last_source, last_context, last_text, last_time)) => {
            let same_origin = message.0 != Type::Error
                || (*last_source == message.1 && *last_context == message.2);
            *last_type == message.0
                && same_origin
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

/// Id of the expanded log panel (its size is persisted under it by egui).
const LOG_PANEL_ID: &str = "log_panel";
/// Smallest height the expanded log panel can be dragged to, in points.
const LOG_PANEL_MIN_HEIGHT: f32 = 60.0;

impl TelescopeApp {
    /// The status log at the bottom of the window. Collapsed, it is a single
    /// bar with the entry count and the latest entry; expanded, a resizable
    /// panel with the whole (scrollable) log. Clicking the header toggles it;
    /// the state and height are remembered in `telescope.toml` (`[ui]`).
    pub(crate) fn show_log_panel(&mut self, ui: &mut egui::Ui) {
        let saved = self.settings.get_ui_state();
        let mut expanded = saved.log_expanded;
        let mut toggle = false;

        let collapsed_panel = egui::Panel::bottom("log_panel_collapsed").resizable(false);
        let expanded_panel = egui::Panel::bottom(LOG_PANEL_ID)
            .resizable(true)
            .min_size(LOG_PANEL_MIN_HEIGHT)
            .default_size(saved.log_height);
        let messages = &self.app_messages;
        egui::Panel::show_switched(
            ui,
            &mut expanded,
            collapsed_panel,
            expanded_panel,
            |ui, is_expanded| {
                ui.horizontal(|ui| {
                    let arrow = if is_expanded { "⏷" } else { "⏵" };
                    let title = t!("log.title", count = messages.len());
                    let header = egui::Button::new(format!("{arrow} {title}")).frame(false);
                    let hint = if is_expanded {
                        t!("log.collapse")
                    } else {
                        t!("log.expand")
                    };
                    if ui.add(header).on_hover_text(hint).clicked() {
                        toggle = true;
                    }
                    if !is_expanded && let Some(last) = messages.last() {
                        ui.separator();
                        ui.add(egui::Label::new(last.clone()).truncate());
                    }
                });
                if is_expanded {
                    egui::Frame::canvas(ui.style())
                        .inner_margin(Margin::symmetric(2, 5))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .auto_shrink(false)
                                .show_rows(
                                    ui,
                                    ui.text_style_height(&egui::TextStyle::Body),
                                    messages.len(),
                                    |ui, row_range| {
                                        for index in row_range {
                                            ui.label(messages[index].clone());
                                        }
                                    },
                                );
                        });
                }
            },
        );
        if toggle {
            expanded = !expanded;
        }

        // Remember the layout once the user is done changing it (not on every
        // frame of a resize drag).
        let height =
            egui::containers::panel::PanelState::load(ui.ctx(), egui::Id::new(LOG_PANEL_ID))
                .filter(|_| expanded)
                .map_or(saved.log_height, |state| state.size().y.round());
        let state = UiState {
            log_expanded: expanded,
            log_height: height,
            ..saved.clone()
        };
        if state != saved
            && !ui.input(|input| input.pointer.any_down())
            && let Err(t_error) = self.settings.save_ui_state(state)
        {
            tracing::warn!("could not save the log panel layout: {t_error}");
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
    fn the_same_line_from_another_rule_is_a_duplicate() {
        let now = Instant::now();
        let last = (
            Type::Info,
            String::from("PatternEngine"),
            String::from("intel_line"),
            String::from("hello"),
            now,
        );
        let other_rule = (
            Type::Info,
            String::from("PatternEngine"),
            String::from("ship_report_en"),
            String::from("hello"),
        );
        assert!(is_duplicate_notification(Some(&last), &other_rule, now));
    }

    #[test]
    fn errors_from_different_places_are_not_duplicates() {
        let now = Instant::now();
        let last = (
            Type::Error,
            String::from("a"),
            String::from("x"),
            String::from("failed"),
            now,
        );
        let other = (
            Type::Error,
            String::from("b"),
            String::from("x"),
            String::from("failed"),
        );
        assert!(!is_duplicate_notification(Some(&last), &other, now));
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
