//! Dispatch stage: acts on the Output nodes that fired, off the UI thread.
//!
//! A dedicated thread drains the detection thread's output and performs each
//! output: map pulses and tooltip entries (sent on the `MapSync` broadcast the
//! panes read), the alarm sound (through the [`AudioHandle`]), and the status
//! log line. Everything it needs from the app lives in [`AlarmShared`], a
//! snapshot the UI thread refreshes (`TelescopeApp::sync_alarm_shared`), so the
//! alarm neither reads UI state nor waits for a frame.

use super::detection::DetectedLine;
use super::nearby::{NearbyEvent, NearbyEvents};
use super::resolve::{JumpGraph, jumps_to_nearest_origin, nearest_origin_within};
use crate::app::audio::AudioHandle;
use crate::app::messages::{MapSync, Message, MessageSpawner, Target, Type};
use crate::app::settings::Settings;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use webb::graph::{Activation, Data, FORMATTED_TAG, Mensaje};
use webb::map_alerts::{AlertSummary, IntelAlert, leftover};
use webb::objects::Character;
use webb::rules::{InputKind, IntelLine, OutputKind};

/// The settings the dispatch reads, copied out of `Settings`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AlarmConfig {
    /// Alarm radius, in stargate jumps.
    pub warning_area: u8,
    pub sound_path: PathBuf,
    pub center_on_alert: bool,
    /// How long a pulse / tooltip entry stays on the map.
    pub alert_duration: Duration,
}

impl AlarmConfig {
    /// The alarm settings as they are in `settings`.
    pub(crate) fn from_settings(settings: &Settings) -> Self {
        Self {
            warning_area: settings.get_warning_area(),
            sound_path: settings.get_alert_sound_path(),
            center_on_alert: settings.get_center_on_alert(),
            alert_duration: settings.get_alert_duration(),
        }
    }
}

impl Default for AlarmConfig {
    fn default() -> Self {
        Self {
            warning_area: 0,
            sound_path: PathBuf::new(),
            center_on_alert: false,
            alert_duration: Duration::ZERO,
        }
    }
}

/// State shared between the UI thread (writer) and the dispatch thread.
#[derive(Default)]
pub(crate) struct AlarmShared {
    pub config: RwLock<AlarmConfig>,
    /// Last known solar system of each linked character (only known ones).
    pub locations: RwLock<Vec<u32>>,
    /// Stargate connections of the loaded universe.
    pub jumps: RwLock<JumpGraph>,
    /// Recent events near the linked characters (read by the overlay).
    pub nearby: NearbyEvents,
    /// The systems each overlay measures its events from, by the overlay's
    /// key: a character's id and its location, or 0 and every location when
    /// one overlay joins them. Empty (before the first sync): one overlay
    /// from `locations`.
    pub nearby_origins: RwLock<Vec<(i64, Vec<u32>)>>,
}

/// The known solar systems of `characters` (those with a location).
pub(crate) fn character_locations(characters: &[Character]) -> Vec<u32> {
    characters
        .iter()
        .filter_map(|character| u32::try_from(character.location).ok())
        .filter(|location| *location > 0)
        .collect()
}

impl AlarmShared {
    /// Stores `config` and `locations`, writing (and logging) only what
    /// changed. Returns whether anything did.
    pub(crate) fn update(&self, config: AlarmConfig, locations: Vec<u32>) -> bool {
        let mut changed = false;
        if *self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            != config
        {
            tracing::debug!(?config, "alarm config updated");
            *self
                .config
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
            changed = true;
        }
        if *self
            .locations
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            != locations
        {
            tracing::debug!(?locations, "character locations updated");
            *self
                .locations
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = locations;
            changed = true;
        }
        changed
    }

    /// Sets where each overlay measures from (see [`Self::nearby_origins`]).
    /// The events of an overlay that is gone are dropped.
    pub(crate) fn set_nearby_origins(&self, groups: Vec<(i64, Vec<u32>)>) {
        let mut current = self
            .nearby_origins
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *current != groups {
            let keys: Vec<i64> = groups.iter().map(|(key, _)| *key).collect();
            self.nearby.retain_keys(&keys);
            *current = groups;
        }
    }

    /// Replaces the stargate graph (see `TelescopeApp::sync_alarm_jumps`).
    pub(crate) fn set_jumps(&self, graph: JumpGraph) {
        *self
            .jumps
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = graph;
    }
}

/// What the dispatch thread works with.
pub(crate) struct Dispatcher {
    pub shared: Arc<AlarmShared>,
    pub audio: Arc<AudioHandle>,
    pub map_msg: Arc<broadcast::Sender<MapSync>>,
    pub task_msg: Arc<MessageSpawner>,
}

/// Whether a line written at `timestamp` is still within the grace period
/// (the alert duration) at `now`: only then it is alerted. A timestamp ahead
/// of `now` (clocks that differ a little) counts as fresh.
pub(crate) fn is_fresh(
    timestamp: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    grace: Duration,
) -> bool {
    now.signed_duration_since(timestamp)
        .to_std()
        .is_ok_and(|age| age <= grace)
        || timestamp > now
}

/// Spawns the dispatch thread and returns immediately. It ends when the
/// detection thread drops its sender.
pub(crate) fn spawn(dispatcher: Dispatcher, mut detected: mpsc::Receiver<DetectedLine>) {
    let spawned = std::thread::Builder::new()
        .name(String::from("telescope-intel-dispatch"))
        .spawn(move || {
            while let Some(line) = detected.blocking_recv() {
                dispatcher.process(line);
            }
            tracing::debug!("intel dispatch stopped");
        });
    if let Err(error) = spawned {
        tracing::error!("could not start the intel dispatch thread: {error}");
    }
}

impl Dispatcher {
    /// Dispatches the Output nodes that fired for one input line.
    #[tracing::instrument(skip(self, detected), fields(channel = %detected.channel, outputs = detected.activations.len()))]
    fn process(&self, detected: DetectedLine) {
        let started = Instant::now();
        let DetectedLine {
            channel,
            line,
            activations,
        } = detected;
        if activations.is_empty() {
            return;
        }
        let config = self
            .shared
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
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
        tracing::debug!(
            visual,
            sound,
            suppress,
            systems = systems.len(),
            "line matched"
        );
        // Lines that piled up (Telescope was busy, or started late) are all
        // processed at once. The ones older than the grace period are only
        // recorded below (tooltip, log): they are not alerted, neither with
        // the map pulse and the overlay nor with the sound.
        let fresh = is_fresh(line.timestamp, Utc::now(), config.alert_duration);
        if !fresh {
            tracing::debug!(
                timestamp = %line.timestamp,
                "line outside the grace period: recorded, not alerted"
            );
        }
        let alert = fresh && !suppress;
        // A killmail alerts only within the warning radius: the feed carries
        // every kill of the universe. The reports of the chat are not held back.
        let from_feed = InputKind::ZKillboard.fixed_channel() == Some(channel.as_str());
        let alerting = if alert && from_feed {
            self.killmail_systems(&config, &systems)
        } else {
            systems.clone()
        };
        if alert {
            self.record_nearby(&config, &line, &alerting);
        }
        if visual && alert {
            self.pulse(&config, &line, &alerting, from_feed);
        }
        if sound && alert {
            self.sound(&config, &alerting);
        }
        for activation in activations
            .iter()
            .filter(|activation| activation.kind == OutputKind::Tooltip)
        {
            self.tooltip(&config, &line, &systems, activation, from_feed);
        }
        if let Some(activation) = activations
            .iter()
            .find(|activation| activation.kind == OutputKind::Log)
        {
            self.log(&channel, &line, activation.log.use_current_time);
        }
        tracing::debug!(
            elapsed_us = started.elapsed().as_micros() as u64,
            "line dispatched"
        );
    }

    /// The systems of a killmail that may alert: those within the warning
    /// radius of a linked character.
    fn killmail_systems(&self, config: &AlarmConfig, systems: &[usize]) -> Vec<usize> {
        systems
            .iter()
            .copied()
            .filter(|system_id| {
                self.nearest_character_in_range(config, *system_id)
                    .is_some()
            })
            .collect()
    }

    /// Keeps, for the overlay, every reported system that is within the
    /// warning radius of a linked character. Like the sound, it needs the
    /// characters' locations and the stargate graph to judge the distance.
    fn record_nearby(&self, config: &AlarmConfig, line: &IntelLine, systems: &[usize]) {
        if systems.is_empty() {
            return;
        }
        let mut groups = self
            .shared
            .nearby_origins
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if groups.is_empty() {
            let all = self
                .shared
                .locations
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            groups.push((0, all));
        }
        let jumps = self
            .shared
            .jumps
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if jumps.is_empty() {
            return;
        }
        let received = Instant::now();
        for (key, origins) in &groups {
            if origins.is_empty() {
                continue;
            }
            for system_id in systems {
                let Ok(target) = u32::try_from(*system_id) else {
                    continue;
                };
                if let Some((_, distance)) =
                    jumps_to_nearest_origin(&jumps, origins, target, config.warning_area)
                {
                    self.shared.nearby.push(NearbyEvent {
                        key: *key,
                        system_id: target,
                        jumps: distance,
                        text: line.text.clone(),
                        received,
                    });
                }
            }
        }
        crate::repaint::request();
    }

    /// Sends a map message and wakes the UI so the panes pick it up.
    fn send_map(&self, message: MapSync) {
        let _ = self.map_msg.send(message);
        crate::repaint::request();
    }

    /// Pulses the map node of every reported system (no tooltip entry). A
    /// line may name several candidates; only those that resolved count.
    fn pulse(&self, config: &AlarmConfig, line: &IntelLine, systems: &[usize], killmail: bool) {
        let received = Instant::now();
        for system_id in systems {
            let mut alert = IntelAlert::new(
                *system_id,
                received,
                config.alert_duration,
                &line.text,
                AlertSummary::default(),
                false,
            );
            alert.killmail = killmail;
            self.send_map(MapSync::SystemAlert(alert));
        }
    }

    /// Lists `line` in the tooltip of every reported system (no visual alert),
    /// as configured by the Tooltip output: the structured summary when there
    /// is one, the processed text as fallback.
    fn tooltip(
        &self,
        config: &AlarmConfig,
        line: &IntelLine,
        systems: &[usize],
        activation: &Activation,
        killmail: bool,
    ) {
        if systems.is_empty() {
            return;
        }
        let messages: Vec<&Mensaje> = activation.messages.iter().collect();
        let mut summary = AlertSummary::from_messages(&messages);
        // The fallback text, shown when there are no ships and no count: a
        // Formatter's rendering when one feeds the tooltip, otherwise what is
        // left of the line once the reported system and every other detection
        // are removed (usually the pilot names).
        let formatted: Vec<&str> = messages
            .iter()
            .filter(|message| message.tag == FORMATTED_TAG)
            .map(|message| message.text.as_str())
            .collect();
        summary.leftover = if formatted.is_empty() {
            leftover(&line.text, &messages)
        } else {
            formatted.join(" · ")
        };
        let received = Instant::now();
        for system_id in systems {
            let mut alert = IntelAlert::new(
                *system_id,
                received,
                config.alert_duration,
                &line.text,
                summary.clone(),
                activation.tooltip.emojis,
            );
            alert.killmail = killmail;
            self.send_map(MapSync::SystemTooltip(alert));
        }
    }

    /// Sounds the alarm when any reported system is within the warning radius
    /// of a linked character, and centers the maps on the closest one when
    /// `center_on_alert` is on. A report repeated within the alarm's cooldown
    /// (see `AudioHandle::play_alert`) neither sounds nor moves the maps
    /// again.
    fn sound(&self, config: &AlarmConfig, systems: &[usize]) {
        let Some(character_system) = systems
            .iter()
            .find_map(|system_id| self.nearest_character_in_range(config, *system_id))
        else {
            tracing::debug!("no linked character in range; no sound");
            return;
        };
        let played = self.audio.play_alert(&config.sound_path);
        tracing::debug!(character_system, played, "alarm decision");
        if played && config.center_on_alert {
            self.send_map(MapSync::CenterOn((
                character_system as usize,
                Target::System,
            )));
        }
    }

    /// Whether an alert in `system_id` should sound: it is within the
    /// warning radius (in stargate jumps) of at least one linked character's
    /// last known location. Returns the solar system of the closest such
    /// character (where the maps are centered when `center_on_alert` is on),
    /// or `None` for no sound.
    ///
    /// When that can't be judged -- no character has a known location yet,
    /// or the universe (SDE) isn't loaded -- the alert stays silent: the
    /// sound only fires for a distance that was actually computed. The map
    /// notification itself is never filtered, only the sound.
    fn nearest_character_in_range(&self, config: &AlarmConfig, system_id: usize) -> Option<u32> {
        let origins = self
            .shared
            .locations
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let target = u32::try_from(system_id).ok()?;
        let jumps = self
            .shared
            .jumps
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if origins.is_empty() || jumps.is_empty() {
            return None;
        }
        nearest_origin_within(&jumps, &origins, target, config.warning_area)
    }

    /// Appends the line to the status log, optionally stamped with the current
    /// time instead of the line's own timestamp.
    fn log(&self, channel: &str, line: &IntelLine, use_current_time: bool) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use webb::graph::{LogConfig, TooltipConfig};

    struct Rig {
        dispatcher: Dispatcher,
        map: broadcast::Receiver<MapSync>,
        app: mpsc::Receiver<Message>,
        sounds: std::sync::mpsc::Receiver<PathBuf>,
    }

    /// Stargate chain 1 - 2 - 3 - 4 - 5, one character at system 1, radius 2.
    fn rig(locations: Vec<u32>) -> Rig {
        let shared = Arc::new(AlarmShared::default());
        *shared.config.write().unwrap() = AlarmConfig {
            warning_area: 2,
            sound_path: PathBuf::from("alarm.wav"),
            center_on_alert: true,
            alert_duration: Duration::from_secs(5),
        };
        *shared.locations.write().unwrap() = locations;
        *shared.jumps.write().unwrap() = (1u32..=5)
            .map(|id| {
                let mut links = Vec::new();
                if id > 1 {
                    links.push(id - 1);
                }
                if id < 5 {
                    links.push(id + 1);
                }
                (id, links)
            })
            .collect();
        let (audio, sounds) = AudioHandle::for_test();
        let (map_tx, map) = broadcast::channel(32);
        let (app_tx, app) = mpsc::channel(32);
        Rig {
            dispatcher: Dispatcher {
                shared,
                audio: Arc::new(audio),
                map_msg: Arc::new(map_tx),
                task_msg: Arc::new(MessageSpawner::new(Arc::new(app_tx))),
            },
            map,
            app,
            sounds,
        }
    }

    fn activation(kind: OutputKind, systems: &[usize]) -> Activation {
        Activation {
            output_id: format!("{kind:?}"),
            kind,
            tooltip: TooltipConfig::default(),
            log: LogConfig::default(),
            messages: vec![Mensaje {
                tag: String::from("system_report"),
                text: String::from("system"),
                data: Data::Systems(systems.to_vec()),
                spans: Vec::new(),
                category: None,
            }],
        }
    }

    fn detected(activations: Vec<Activation>) -> DetectedLine {
        DetectedLine {
            channel: String::from("intel"),
            line: IntelLine {
                timestamp: Utc::now(),
                author: String::from("Pilot"),
                text: String::from("reported"),
            },
            activations,
        }
    }

    fn drain_map(rig: &mut Rig) -> Vec<MapSync> {
        std::iter::from_fn(|| rig.map.try_recv().ok()).collect()
    }

    /// A line written `seconds` ago.
    fn detected_ago(seconds: i64, activations: Vec<Activation>) -> DetectedLine {
        let mut line = detected(activations);
        line.line.timestamp = Utc::now() - chrono::Duration::seconds(seconds);
        line
    }

    #[test]
    fn a_line_is_fresh_only_within_the_grace_period() {
        let now = Utc::now();
        let grace = Duration::from_secs(60);
        let ago = |seconds: i64| now - chrono::Duration::seconds(seconds);
        assert!(is_fresh(ago(0), now, grace));
        assert!(is_fresh(ago(60), now, grace));
        assert!(!is_fresh(ago(61), now, grace));
        assert!(!is_fresh(ago(86_400), now, grace));
        // a timestamp a little ahead (clocks that differ) is fresh
        assert!(is_fresh(now + chrono::Duration::seconds(3), now, grace));
        // with no grace only the present counts
        assert!(is_fresh(now, now, Duration::ZERO));
        assert!(!is_fresh(ago(1), now, Duration::ZERO));
    }

    #[test]
    fn lines_older_than_the_grace_period_are_logged_but_not_alerted() {
        // the rig's grace period is 5 seconds; a backlog is processed at once
        let mut rig = rig(vec![1]);
        let old = |activations| detected_ago(3_600, activations);
        rig.dispatcher.process(old(vec![
            activation(OutputKind::Visual, &[2]),
            activation(OutputKind::Sound, &[2]),
            activation(OutputKind::Tooltip, &[2]),
            activation(OutputKind::Log, &[]),
        ]));

        // no sound, no pulse, nothing for the overlay
        assert!(rig.sounds.try_recv().is_err());
        let sent = drain_map(&mut rig);
        assert!(
            sent.iter().all(|m| !matches!(m, MapSync::SystemAlert(_))),
            "no visual alert"
        );
        assert!(
            sent.iter().all(|m| !matches!(m, MapSync::CenterOn(_))),
            "no centering"
        );
        assert!(
            rig.dispatcher
                .shared
                .nearby
                .recent(0, Instant::now(), Duration::from_secs(60))
                .is_empty()
        );
        // but it was recorded: the tooltip and the log keep it
        assert!(sent.iter().any(|m| matches!(m, MapSync::SystemTooltip(_))));
        assert!(matches!(
            rig.app.try_recv(),
            Ok(Message::GenericNotification((Type::Info, _, _, _)))
        ));
    }

    #[test]
    fn in_a_backlog_only_the_lines_within_the_grace_period_alert() {
        let mut rig = rig(vec![1]);
        let outputs = || {
            vec![
                activation(OutputKind::Visual, &[2]),
                activation(OutputKind::Sound, &[2]),
            ]
        };
        // an hour of old lines, then one from a second ago
        for seconds in [3_600, 1_800, 600, 30] {
            rig.dispatcher.process(detected_ago(seconds, outputs()));
        }
        assert!(rig.sounds.try_recv().is_err(), "nothing sounded yet");
        assert!(drain_map(&mut rig).is_empty(), "nothing pulsed yet");

        rig.dispatcher.process(detected_ago(1, outputs()));
        assert_eq!(rig.sounds.try_recv().unwrap(), PathBuf::from("alarm.wav"));
        let sent = drain_map(&mut rig);
        assert!(sent.iter().any(|m| matches!(m, MapSync::SystemAlert(_))));
        let kept = rig
            .dispatcher
            .shared
            .nearby
            .recent(0, Instant::now(), Duration::from_secs(60));
        assert_eq!(kept.len(), 1, "only the fresh line reaches the overlay");
    }

    /// A killmail line: the same as `detected`, from the feed's channel.
    fn killmail(activations: Vec<Activation>) -> DetectedLine {
        let mut line = detected(activations);
        line.channel = String::from("zkillboard");
        line
    }

    #[test]
    fn a_killmail_alerts_only_within_the_radius() {
        let mut rig = rig(vec![1]);
        rig.dispatcher.process(killmail(vec![
            activation(OutputKind::Visual, &[3, 5]),
            activation(OutputKind::Sound, &[3, 5]),
        ]));
        let sent = drain_map(&mut rig);
        let pulsed: Vec<usize> = sent
            .iter()
            .filter_map(|message| match message {
                MapSync::SystemAlert(alert) => Some(alert.system_id),
                _ => None,
            })
            .collect();
        // System 5 is four jumps away; the radius is two.
        assert_eq!(pulsed, vec![3]);
        assert!(
            sent.iter()
                .any(|message| matches!(message, MapSync::SystemAlert(alert) if alert.killmail))
        );
        assert!(rig.sounds.try_recv().is_ok());
    }

    #[test]
    fn a_killmail_and_a_report_of_the_same_system_both_alert() {
        // They are not matched with each other: there is no reliable way to
        // tell they are about the same event.
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .process(killmail(vec![activation(OutputKind::Visual, &[2])]));
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Visual, &[2])]));
        let sent = drain_map(&mut rig);
        assert_eq!(sent.len(), 2);
        assert!(matches!(&sent[0], MapSync::SystemAlert(alert) if alert.killmail));
        assert!(matches!(&sent[1], MapSync::SystemAlert(alert) if !alert.killmail));
    }

    #[test]
    fn a_visual_output_pulses_every_reported_system() {
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Visual, &[2, 4])]));
        let sent = drain_map(&mut rig);
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|m| matches!(m, MapSync::SystemAlert(_))));
        assert!(rig.sounds.try_recv().is_err());
    }

    #[test]
    fn a_tooltip_output_lists_the_line_and_needs_a_system() {
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Tooltip, &[3])]));
        assert!(matches!(
            drain_map(&mut rig).as_slice(),
            [MapSync::SystemTooltip(_)]
        ));
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Tooltip, &[])]));
        assert!(drain_map(&mut rig).is_empty());
    }

    #[test]
    fn a_sound_within_range_plays_and_centers_on_the_character() {
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Sound, &[3])]));
        assert_eq!(rig.sounds.try_recv().unwrap(), PathBuf::from("alarm.wav"));
        assert!(matches!(
            drain_map(&mut rig).as_slice(),
            [MapSync::CenterOn((1, Target::System))]
        ));
    }

    #[test]
    fn events_within_the_radius_are_kept_for_the_overlay() {
        let rig = rig(vec![1]);
        rig.dispatcher.process(detected(vec![
            activation(OutputKind::Visual, &[3]),
            activation(OutputKind::Visual, &[5]),
        ]));
        let kept = rig
            .dispatcher
            .shared
            .nearby
            .recent(0, Instant::now(), Duration::from_secs(60));
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].system_id, kept[0].jumps), (3, 2));
        assert_eq!(kept[0].text, "reported");
    }

    #[test]
    fn suppressed_lines_and_unlocated_characters_leave_nothing_for_the_overlay() {
        let rig = rig(vec![1]);
        rig.dispatcher.process(detected(vec![
            activation(OutputKind::Visual, &[2]),
            activation(OutputKind::Suppress, &[]),
        ]));
        let rig2 = self::rig(Vec::new());
        rig2.dispatcher
            .process(detected(vec![activation(OutputKind::Visual, &[1])]));
        for rig in [&rig, &rig2] {
            assert!(
                rig.dispatcher
                    .shared
                    .nearby
                    .recent(0, Instant::now(), Duration::from_secs(60))
                    .is_empty()
            );
        }
    }

    #[test]
    fn a_repeated_sound_within_the_cooldown_neither_plays_nor_centers() {
        let mut rig = rig(vec![1]);
        for _ in 0..2 {
            rig.dispatcher
                .process(detected(vec![activation(OutputKind::Sound, &[2])]));
        }
        assert!(rig.sounds.try_recv().is_ok());
        assert!(rig.sounds.try_recv().is_err());
        assert_eq!(drain_map(&mut rig).len(), 1);
    }

    #[test]
    fn a_sound_out_of_range_or_without_locations_stays_silent() {
        let rig = rig(vec![1]);
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Sound, &[5])]));
        assert!(rig.sounds.try_recv().is_err());

        let mut rig = self::rig(Vec::new());
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Sound, &[1])]));
        assert!(rig.sounds.try_recv().is_err());
        assert!(drain_map(&mut rig).is_empty());
    }

    #[test]
    fn the_center_on_alert_setting_is_honoured() {
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .shared
            .config
            .write()
            .unwrap()
            .center_on_alert = false;
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Sound, &[2])]));
        assert!(rig.sounds.try_recv().is_ok());
        assert!(drain_map(&mut rig).is_empty());
    }

    #[test]
    fn suppress_cancels_the_visual_and_sound_outputs_only() {
        let mut rig = rig(vec![1]);
        rig.dispatcher.process(detected(vec![
            activation(OutputKind::Visual, &[2]),
            activation(OutputKind::Sound, &[2]),
            activation(OutputKind::Suppress, &[2]),
            activation(OutputKind::Tooltip, &[2]),
        ]));
        assert!(rig.sounds.try_recv().is_err());
        assert!(matches!(
            drain_map(&mut rig).as_slice(),
            [MapSync::SystemTooltip(_)]
        ));
    }

    #[test]
    fn a_log_output_reaches_the_status_log() {
        let mut rig = rig(vec![1]);
        rig.dispatcher
            .process(detected(vec![activation(OutputKind::Log, &[])]));
        match rig.app.try_recv() {
            Ok(Message::GenericNotification((Type::Info, source, channel, text))) => {
                assert_eq!(source, "Intel");
                assert_eq!(channel, "intel");
                assert!(text.contains("Pilot > reported"), "{text}");
            }
            _ => panic!("expected an Info notification"),
        }
    }

    #[test]
    fn character_locations_keep_only_known_systems() {
        let located = |location: i64| Character {
            location,
            ..Character::default()
        };
        assert_eq!(
            character_locations(&[
                located(30000142),
                located(0),
                located(-5),
                located(30000001)
            ]),
            vec![30000142, 30000001]
        );
    }

    #[test]
    fn alarm_config_follows_the_settings() {
        let mut settings = Settings::default();
        settings.set_warning_area(4);
        settings.set_center_on_alert(true);
        settings.set_alert_duration_secs(20);
        let config = AlarmConfig::from_settings(&settings);
        assert_eq!(config.warning_area, 4);
        assert!(config.center_on_alert);
        assert_eq!(config.alert_duration, settings.get_alert_duration());
        assert_eq!(config.sound_path, settings.get_alert_sound_path());
    }

    #[test]
    fn update_reports_only_real_changes() {
        let shared = AlarmShared::default();
        let config = AlarmConfig::from_settings(&Settings::default());
        // The default shared config is not the settings' one.
        assert!(shared.update(config.clone(), vec![1]));
        assert!(!shared.update(config.clone(), vec![1]));
        assert!(shared.update(config.clone(), vec![2]));
        let mut other = config;
        other.warning_area += 1;
        assert!(shared.update(other.clone(), vec![2]));
        assert_eq!(*shared.config.read().unwrap(), other);
        assert_eq!(*shared.locations.read().unwrap(), vec![2]);
    }

    #[test]
    fn set_jumps_replaces_the_graph() {
        let shared = AlarmShared::default();
        shared.set_jumps(JumpGraph::from([(1, vec![2]), (2, vec![1])]));
        assert_eq!(shared.jumps.read().unwrap().len(), 2);
        shared.set_jumps(JumpGraph::new());
        assert!(shared.jumps.read().unwrap().is_empty());
    }

    #[test]
    fn a_line_without_activations_does_nothing() {
        let mut rig = rig(vec![1]);
        rig.dispatcher.process(detected(Vec::new()));
        assert!(drain_map(&mut rig).is_empty());
        assert!(rig.app.try_recv().is_err());
    }
}
