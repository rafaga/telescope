//! Intel (EVE chat log) handling.
//!
//! The input half lives in [`input`] (chatlog name parsing, reading and
//! decoding) and the shared resolvers in [`resolve`]; this module keeps the
//! `TelescopeApp` methods that read a log file and dispatch its matches.

use self::detection::DetectedLine;
use self::input::{ChatLogSource, monitored_channel_names};
use self::resolve::{allows_partial_match, exact_system, nearest_origin_within};
use self::router::Router;
use crate::app::TelescopeApp;
use crate::app::messages::{MapSync, Message, Target, Type};
use chrono::Utc;
use notify::{RecursiveMode, Watcher};
use sde::SdeManager;
use std::ops::Range;
use std::time::Instant;
use webb::map_alerts::{AlertSummary, IntelAlert};
use webb::rules::{Detection, DetectionBatch, DetectionEngine, OutputKind, RulesConfig};

pub(crate) mod detection;
pub(crate) mod input;
mod resolve;
pub(crate) mod router;

pub(crate) use self::input::IntelLogName;

impl TelescopeApp {
    /// Applies the channel selection made in the Settings window: pushes it
    /// into the live handle the file watcher reads, (re)registers or drops
    /// the OS-level watch on the intel directory, and stores the selection
    /// in the settings (unsaved until `Settings::save`).
    ///
    /// Does nothing if the configured intel directory doesn't exist.
    pub(crate) fn apply_intel_settings(&mut self) {
        if !self.settings.get_intel().exists() {
            return;
        }
        let monitored_channels = monitored_channel_names(&self.settings.get_available_channels());
        // Push the freshly-saved selection into the live handle the running
        // watcher's event handler reads from, so newly checked/unchecked
        // channels take effect immediately instead of only after a restart
        // (the watcher's `IntelEventHandler` is constructed once and can't be
        // swapped out).
        if let Ok(mut guard) = self.intel_channels.write() {
            *guard = monitored_channels.clone();
        }
        if monitored_channels.is_empty() {
            let _ = self.watcher.unwatch(self.settings.get_intel());
        } else {
            // `watch` doesn't dedupe: calling it again on a path that's
            // already watched stacks a second OS-level registration instead
            // of replacing the first one, so every real filesystem event then
            // gets delivered once per accumulated registration -- e.g.
            // clicking "Save" three times with a channel checked makes every
            // log line for that channel repeat three times. `unwatch` first
            // (ignoring the "wasn't watched yet" error, e.g. on the very
            // first Save) keeps re-saving idempotent.
            let _ = self.watcher.unwatch(self.settings.get_intel());
            let _ = self
                .watcher
                .watch(self.settings.get_intel(), RecursiveMode::NonRecursive);
        }
        self.settings.set_monitored_channels(monitored_channels);
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn load_intel_file(&mut self, file_name: String) {
        let dir = self.settings.get_intel().to_path_buf();
        let mut log_files_map = self.settings.get_log_files_channels();
        // Offset recorded by the last read; 0 for a file seen for the first
        // time.
        let start = log_files_map
            .get(&file_name)
            .map(|log_entry| log_entry.0)
            .unwrap_or(0);
        if let Some(read) = ChatLogSource::read_new(&dir, &file_name, start) {
            // Hand the parsed lines to the detection thread. `try_send`, not
            // `blocking_send`: this runs on the UI thread, and a full queue
            // (detection can't keep up) must drop lines rather than freeze
            // the UI.
            for event in read.events {
                if let Err(error) = self.intel_input.try_send(event) {
                    tracing::warn!("intel input channel rejected a line: {error}");
                }
            }
            log_files_map.entry(file_name).and_modify(|hash_entry| {
                hash_entry.0 = read.end_offset;
                hash_entry.1 = Utc::now();
            });
        }
        self.settings.set_log_files_channels(log_files_map);
    }

    /// Runs the detection engine over `data` (chat-log lines from `channel`)
    /// without dispatching anything, returning the ids of the matched rules.
    /// Used by the Debug window's line tester.
    #[tracing::instrument(skip(self, data))]
    pub(crate) fn parse_intel_data(&self, channel: &str, data: &str) -> Vec<String> {
        let engine = self
            .intel_engine
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        engine
            .evaluate(channel, data)
            .into_iter()
            .map(|detection| detection.rule_id)
            .collect()
    }

    /// Routes one detected line (a batch of detections of a single input
    /// line) through the output rules and runs the resolvers of the kinds
    /// that fire.
    #[tracing::instrument(skip(self, detected))]
    pub(crate) fn process_detected_line(&mut self, detected: DetectedLine) {
        let actions = self
            .intel_router
            .actions(&detected.channel, &detected.batch);
        if actions.is_empty() {
            return;
        }
        let (systems, spans) = self.resolve_batch_systems(&detected.batch);
        if actions.contains(&OutputKind::Visual) {
            self.dispatch_visual(&detected.batch, &systems, spans);
        }
        if actions.contains(&OutputKind::Sound) {
            self.dispatch_sound(&systems);
        }
        if actions.contains(&OutputKind::Log) {
            self.dispatch_log(&detected.channel, &detected.batch);
        }
    }

    /// The solar-system ids named by `batch` and the byte spans of the
    /// candidates that resolved, so the tooltip summary can drop them from the
    /// leftover text.
    fn resolve_batch_systems(&self, batch: &DetectionBatch) -> (Vec<usize>, Vec<Range<usize>>) {
        let mut systems = Vec::new();
        let mut spans = Vec::new();
        for detection in &batch.detections {
            if let Some(system_group) = &detection.system_group
                && let Some(system_id) = self.resolve_reported_system(detection, system_group)
            {
                spans.push(detection.span.clone());
                if !systems.contains(&system_id) {
                    systems.push(system_id);
                }
            }
        }
        (systems, spans)
    }

    /// Sends the map alert of `batch` to every system it reports. A line may
    /// name several candidates (a pilot name also fits the system pattern);
    /// only those that resolve to a real system count.
    #[tracing::instrument(skip(self, batch, systems, spans))]
    fn dispatch_visual(
        &self,
        batch: &DetectionBatch,
        systems: &[usize],
        spans: Vec<Range<usize>>,
    ) {
        if systems.is_empty() {
            return;
        }
        let text = &batch.line.text;
        let detections: Vec<&Detection> = batch.detections.iter().collect();
        let summary = AlertSummary::from_line(text, &detections, spans);
        let received = Instant::now();
        for system_id in systems {
            let alert = IntelAlert::new(
                *system_id,
                received,
                self.settings.get_alert_duration(),
                text,
                summary.clone(),
            );
            let _ = self.map_msg.0.send(MapSync::SystemAlert(alert));
        }
    }

    /// Sounds the alarm when any reported system is within the warning radius
    /// of a linked character, and centers the maps on the closest one when
    /// `center_on_alert` is on.
    #[tracing::instrument(skip(self, systems))]
    fn dispatch_sound(&self, systems: &[usize]) {
        if let Some(character_system) = systems
            .iter()
            .find_map(|system_id| self.nearest_character_in_range(*system_id))
        {
            self.audio.play_alarm(&self.settings.get_alert_sound_path());
            if self.settings.get_center_on_alert() {
                let _ = self.map_msg.0.send(MapSync::CenterOn((
                    character_system as usize,
                    Target::System,
                )));
            }
        }
    }

    /// Appends the line to the status log.
    #[tracing::instrument(skip(self, batch))]
    fn dispatch_log(&self, channel: &str, batch: &DetectionBatch) {
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("Intel"),
            channel.to_string(),
            batch.line.to_string(),
        )));
    }

    /// The solar system named by the `system_group` capture of `detection`, if
    /// the text is a plausible name of a real system.
    ///
    /// An exact name (any case) is looked up in the loaded universe. Only a
    /// code-like text (see [`allows_partial_match`]) may also resolve to a
    /// system whose name merely contains it (SQL `LIKE` against the SDE),
    /// e.g. "H-5GU" or a nickname such as "4-h"; plain words never do, so a
    /// pilot name next to the system can't turn into some unrelated system.
    /// Without a loaded universe everything goes to the SDE as before.
    fn resolve_reported_system(
        &self,
        detection: &Detection,
        system_group: &str,
    ) -> Option<usize> {
        let system_name = detection.captures.get(system_group)?;
        //validate the captured text before using it in any query; real
        //solar system names are at most 17 chars and may contain spaces
        //and dashes (e.g. "Old Man Star", "Tash-Murkon Prime")
        if system_name.is_empty()
            || system_name.len() > 20
            || !system_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ' ')
        {
            return None;
        }
        let known = !self.universe.solar_systems.is_empty();
        if known {
            let exact = exact_system(
                self.universe
                    .solar_systems
                    .values()
                    .map(|system| (system.id, system.name.as_str())),
                system_name,
            );
            if let Some(system_id) = exact {
                return usize::try_from(system_id).ok();
            }
            if !allows_partial_match(system_name) {
                return None;
            }
        }
        let sde = SdeManager::new(self.settings.get_sde(), self.settings.get_factor());
        match sde.and_then(|s| s.get_system_id(system_name.to_lowercase())) {
            Ok(results) => {
                //prefer an exact name match over partial (LIKE) results
                let found = results
                    .iter()
                    .find(|entry| entry.1.eq_ignore_ascii_case(system_name))
                    .or_else(|| {
                        results
                            .first()
                            .filter(|_| !known || allows_partial_match(system_name))
                    });
                found
                    .and_then(|entry| usize::try_from(entry.0).ok())
                    .filter(|system_id| *system_id > 0)
            }
            Err(t_error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("Intel"),
                    String::from("resolve_reported_system"),
                    t_error.to_string(),
                )));
                None
            }
        }
    }

    /// Persists `rules` to the player database and rebuilds the live engine
    /// and router from them. Used at startup and whenever the rules change.
    #[tracing::instrument(skip(self, rules))]
    pub(crate) fn apply_rules(&mut self, rules: RulesConfig) {
        if let Err(error) = self.esi.save_rules(&rules) {
            self.task_msg.spawn(Message::GenericNotification((
                Type::Error,
                String::from("Intel"),
                String::from("save_rules"),
                error.to_string(),
            )));
        }
        match DetectionEngine::from_config(&rules) {
            Ok((engine, errors)) => {
                for error in errors {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Error,
                        String::from("Intel"),
                        String::from("load_rules"),
                        error.to_string(),
                    )));
                }
                *self
                    .intel_engine
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = engine;
            }
            Err(error) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("Intel"),
                    String::from("load_rules"),
                    error.to_string(),
                )));
            }
        }
        self.intel_router = Router::new(&rules);
        self.intel_rules = rules;
    }
}

impl TelescopeApp {
    /// Whether an alert in `system_id` should sound: it is within the
    /// warning radius set in Settings (`warning_area`, in stargate jumps) of
    /// at least one linked character's last known location. Returns the
    /// solar system of the closest such character (where the maps are
    /// centered when `center_on_alert` is on), or `None` for no sound.
    ///
    /// When that can't be judged -- no character has a known location yet,
    /// or the universe (SDE) isn't loaded -- the alert stays silent: the
    /// sound only fires for a distance that was actually computed. The map
    /// notification itself is never filtered, only the sound.
    fn nearest_character_in_range(&self, system_id: usize) -> Option<u32> {
        let origins: Vec<u32> = self
            .esi
            .characters
            .iter()
            .filter_map(|character| u32::try_from(character.location).ok())
            .filter(|location| *location > 0)
            .collect();
        let target = u32::try_from(system_id).ok()?;
        if origins.is_empty() || self.universe.solar_systems.is_empty() {
            return None;
        }
        nearest_origin_within(
            &self.universe.solar_systems,
            &origins,
            target,
            self.settings.get_warning_area(),
        )
    }
}

