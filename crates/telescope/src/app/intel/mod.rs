//! Intel (EVE chat log) handling.
//!
//! The input half lives in [`input`] (chatlog name parsing, reading and
//! decoding) and the shared resolvers in [`resolve`]; this module keeps the
//! `TelescopeApp` methods that configure the pipeline. The pipeline itself
//! runs on its own threads, independent of the UI loop: [`reader`] (log
//! bytes -> lines), [`detection`] (rules) and [`dispatch`] (map messages,
//! alarm sound, status log).

use self::dispatch::AlarmConfig;
use self::input::monitored_channel_names;
use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use notify::{RecursiveMode, Watcher};
use webb::graph::{Executor, RuleGraph};

pub(crate) mod detection;
pub(crate) mod dispatch;
pub(crate) mod input;
pub(crate) mod reader;
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
        // `watch` doesn't dedupe: calling it again on a path that's already
        // watched stacks a second OS-level registration instead of replacing
        // the first one, so every real filesystem event then gets delivered
        // once per accumulated registration -- e.g. clicking "Save" three
        // times with a channel checked makes every log line for that channel
        // repeat three times. The folder watched until now (which is not the
        // one in the settings when the user picked another) is unwatched
        // first, so re-applying is idempotent.
        let previous = self
            .intel_watched
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(previous) = previous {
            let _ = self.watcher.unwatch(&previous);
        }
        if !monitored_channels.is_empty() {
            let dir = self.settings.get_intel().to_path_buf();
            match self.watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    *self
                        .intel_watched
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dir)
                }
                Err(error) => self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    String::from("Intel"),
                    String::from("watch intel folder"),
                    error.to_string(),
                ))),
            }
        }
        self.settings.set_monitored_channels(monitored_channels);
        // Newly monitored channels start at the end of their current logs.
        self.intel_offsets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .sync(
                self.settings.get_intel(),
                &self.settings.get_cloned_monitored_channels(),
            );
    }

    /// Rescans the chat logs: the channel list shown in Settings (from the
    /// folder in the settings, possibly a draft) and the read offsets of the
    /// monitored channels' logs in the watched folder (see
    /// [`input::IntelOffsets::sync`]). Offsets never follow a draft folder:
    /// the lines written meanwhile in the watched one would be skipped if the
    /// draft were cancelled.
    pub(crate) fn scan_intel_files(&mut self) -> Result<(), crate::app::settings::SettingsError> {
        let result = self.settings.scan_channels_logs();
        let watched = self
            .intel_watched
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(dir) = watched {
            self.intel_offsets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .sync(&dir, &self.settings.get_cloned_monitored_channels());
        }
        result
    }

    /// Runs the detection engine over `data` (chat-log lines from `channel`)
    /// without dispatching anything, returning the ids of the matched rules.
    /// Used by the Debug window's line tester.
    #[cfg(debug_assertions)]
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
    /// Refreshes what the dispatch thread reads (see [`dispatch::AlarmShared`]):
    /// the alarm settings and the linked characters' locations. Cheap and
    /// idempotent -- writes only what changed -- so it runs on every pass of
    /// `event_manager`.
    pub(crate) fn sync_alarm_shared(&self) {
        self.alarm_shared.update(
            AlarmConfig::from_settings(&self.settings),
            dispatch::character_locations(&self.esi.characters),
        );
    }

    /// Rebuilds the stargate graph the alarm measures distances on from the
    /// loaded universe (at startup and whenever the SDE is reloaded).
    pub(crate) fn sync_alarm_jumps(&self) {
        let graph = resolve::jump_graph(&self.universe);
        tracing::debug!(systems = graph.len(), "alarm jump graph rebuilt");
        self.alarm_shared.set_jumps(graph);
    }
}
