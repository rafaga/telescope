//! The on-screen status log: turns incoming `GenericNotification` messages into
//! colored log entries (capped at `MAX_APP_MESSAGES`) and reports errors changing
//! the intel directory.

use crate::app::TelescopeApp;
use crate::app::messages::Message;
use crate::app::messages::Type;
use crate::app::settings::SettingsError;
use eframe::egui::Color32;
use eframe::egui::FontFamily;
use eframe::egui::FontId;
use eframe::egui::TextFormat;
use eframe::egui::epaint::text::LayoutJob;

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
