//! Intel (EVE chat log) handling.
//!
//! The input half lives in [`input`] (chatlog name parsing, reading and
//! decoding) and the shared resolvers in [`resolve`]; this module keeps the
//! `TelescopeApp` methods that read a log file and dispatch its matches.

use self::detection::DetectedLine;
use self::input::{ChatLogSource, monitored_channel_names};
use self::resolve::nearest_origin_within;
use crate::app::TelescopeApp;
use crate::app::messages::{MapSync, Message, Target, Type};
use chrono::Utc;
use notify::{RecursiveMode, Watcher};
use std::time::Instant;
use webb::graph::{Activation, Data, Executor, Mensaje, RuleGraph};
use webb::map_alerts::{AlertSummary, IntelAlert};
use webb::rules::{IntelLine, OutputKind};

pub(crate) mod detection;
pub(crate) mod input;
pub(crate) mod resolve;

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
        let executor = self
            .intel_executor
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut tags = Vec::new();
        for raw in data.lines() {
            let Some(line) = webb::rules::parse_line(raw) else {
                continue;
            };
            let context = webb::graph::LineContext {
                line,
                channel: channel.to_string(),
            };
            for activation in executor.run(&context, self.intel_resolver.as_ref()) {
                for message in activation.messages {
                    if !tags.contains(&message.tag) {
                        tags.push(message.tag);
                    }
                }
            }
        }
        tags
    }

    /// Dispatches the Output nodes that fired for one input line.
    #[tracing::instrument(skip(self, detected))]
    pub(crate) fn process_detected_line(&mut self, detected: DetectedLine) {
        let DetectedLine {
            channel,
            line,
            activations,
        } = detected;
        if activations.is_empty() {
            return;
        }
        let suppress = activations
            .iter()
            .any(|activation| activation.kind == OutputKind::Suppress);
        let mut systems: Vec<usize> = Vec::new();
        for activation in &activations {
            for message in &activation.messages {
                if let Data::Systems(ids) = &message.data {
                    for id in ids {
                        if !systems.contains(id) {
                            systems.push(*id);
                        }
                    }
                }
            }
        }
        let visual = activations
            .iter()
            .any(|activation| activation.kind == OutputKind::Visual);
        let sound = activations
            .iter()
            .any(|activation| activation.kind == OutputKind::Sound);
        if visual && !suppress {
            self.dispatch_pulse(&line, &systems);
        }
        if sound && !suppress {
            self.dispatch_sound(&systems);
        }
        for activation in activations
            .iter()
            .filter(|activation| activation.kind == OutputKind::Tooltip)
        {
            self.dispatch_tooltip(&line, &systems, activation);
        }
        if let Some(activation) = activations
            .iter()
            .find(|activation| activation.kind == OutputKind::Log)
        {
            self.dispatch_log(&channel, &line, activation.log.use_current_time);
        }
    }

    /// Pulses the map node of every reported system (no tooltip entry). A
    /// line may name several candidates; only those that resolved count.
    #[tracing::instrument(skip(self, line, systems))]
    fn dispatch_pulse(&self, line: &IntelLine, systems: &[usize]) {
        if systems.is_empty() {
            return;
        }
        let received = Instant::now();
        for system_id in systems {
            let alert = IntelAlert::new(
                *system_id,
                received,
                self.settings.get_alert_duration(),
                &line.text,
                AlertSummary::default(),
                false,
            );
            let _ = self.map_msg.0.send(MapSync::SystemAlert(alert));
        }
    }

    /// Lists `line` in the tooltip of every reported system (no visual alert),
    /// as configured by the Tooltip output: the structured summary when there
    /// is one, the processed text as fallback.
    #[tracing::instrument(skip(self, line, systems, activation))]
    fn dispatch_tooltip(&self, line: &IntelLine, systems: &[usize], activation: &Activation) {
        if systems.is_empty() {
            return;
        }
        let messages: Vec<&Mensaje> = activation.messages.iter().collect();
        let mut summary = AlertSummary::from_messages(&messages);
        // The fallback is the processed text; the reported system only anchors
        // the entry, it is not shown.
        summary.leftover = activation
            .messages
            .iter()
            .filter(|message| !matches!(message.data, Data::Systems(_)))
            .map(|message| message.text.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        let received = Instant::now();
        for system_id in systems {
            let alert = IntelAlert::new(
                *system_id,
                received,
                self.settings.get_alert_duration(),
                &line.text,
                summary.clone(),
                activation.tooltip.emojis,
            );
            let _ = self.map_msg.0.send(MapSync::SystemTooltip(alert));
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

    /// Appends the line to the status log, optionally stamped with the current
    /// time instead of the line's own timestamp.
    #[tracing::instrument(skip(self, line))]
    fn dispatch_log(&self, channel: &str, line: &IntelLine, use_current_time: bool) {
        let text = if use_current_time {
            let mut stamped = line.clone();
            stamped.timestamp = Utc::now();
            stamped.to_string()
        } else {
            line.to_string()
        };
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("Intel"),
            channel.to_string(),
            text,
        )));
    }

    /// Persists `graph` to the player database and rebuilds the live executor
    /// from it. Used at startup and whenever the rules change.
    #[tracing::instrument(skip(self, graph))]
    pub(crate) fn apply_graph(&mut self, graph: RuleGraph) {
        if let Err(error) = self.esi.save_graph(&graph) {
            self.task_msg.spawn(Message::GenericNotification((
                Type::Error,
                String::from("Intel"),
                String::from("save_graph"),
                error.to_string(),
            )));
        }
        let (executor, errors) = Executor::new(graph.clone());
        for error in errors {
            self.task_msg.spawn(Message::GenericNotification((
                Type::Error,
                String::from("Intel"),
                String::from("load_graph"),
                error.to_string(),
            )));
        }
        *self
            .intel_executor
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = executor;
        self.intel_graph = graph;
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

