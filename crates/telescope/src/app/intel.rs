//! Intel (EVE chat log) handling.
//!
//! Two parts live here: [`IntelLogName`], the single place that knows the
//! chatlog file naming format (the directory scan, the file watcher and the
//! log reader all go through [`IntelLogName::parse`], so a format change only
//! needs to be made here), and the `TelescopeApp` methods that read a log
//! file, decode it and dispatch pattern matches.

use crate::app::TelescopeApp;
use crate::app::map_alerts::{AlertSummary, IntelAlert, is_query};
use crate::app::messages::{MapSync, Message, Target, Type};
use crate::app::patterns::{ActionConfig, PatternMatch};
use chrono::Utc;
use notify::{RecursiveMode, Watcher};
use regex::Regex;
use sde::SdeManager;
use sde::objects::SolarSystem;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;

/// A chatlog file name split into its channel and the rest.
///
/// EVE names chatlogs `<channel>_<YYYYMMDD>_<HHMMSS>[_<charid>].txt`. The
/// channel itself may contain underscores, so it can't be found by splitting
/// on the first `_`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct IntelLogName<'a> {
    /// Channel name, e.g. `Local` or `wc.Vale+Tribute`.
    pub channel: &'a str,
    /// Everything after the channel's trailing underscore:
    /// `<YYYYMMDD>_<HHMMSS>[_<charid>].txt`.
    pub suffix: &'a str,
}

impl<'a> IntelLogName<'a> {
    /// Parses a chatlog file name, or returns `None` if `file_name` is not one.
    pub(crate) fn parse(file_name: &'a str) -> Option<Self> {
        static LOG_NAME: OnceLock<Regex> = OnceLock::new();
        let re = LOG_NAME.get_or_init(|| {
            Regex::new(r"^(?P<channel>.+?)_(?P<suffix>\d{8}_\d{6}(?:_\d+)?\.txt)$")
                .expect("hardcoded chatlog name regex must compile")
        });
        let caps = re.captures(file_name)?;
        Some(Self {
            channel: caps.name("channel")?.as_str(),
            suffix: caps.name("suffix")?.as_str(),
        })
    }
}

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
        let path = self.settings.get_intel();
        let path = &path.join(file_name.as_str());
        let mut log_files_map = self.settings.get_log_files_channels();

        //getting the first byte to read from the last recorded file lenght
        let mut start = 0;
        if let Some(log_entry) = log_files_map.get(&file_name) {
            start = log_entry.0;
        }

        let channel = IntelLogName::parse(&file_name)
            .map(|log| log.channel.to_string())
            .unwrap_or_default();

        if let Ok(mut intel_file) = File::open(path) {
            let file_length = intel_file.metadata().map(|meta| meta.len()).unwrap_or(0);
            //if the file shrank (log rotation), read it from the beginning
            if file_length < start {
                start = 0;
            }
            if file_length > start && intel_file.seek(SeekFrom::Start(start)).is_ok() {
                // EVE Online writes chat logs as UTF-16LE, never UTF-8, so
                // this cannot use read_to_string (it requires valid UTF-8
                // and fails on the very first byte of every real log file,
                // silently, since the caller only checks `is_ok()`). Read
                // the raw bytes and decode them ourselves.
                let mut chunk = intel_file.take(file_length - start);
                let mut raw = Vec::new();
                if let Ok(bytes_read) = chunk.read_to_end(&mut raw) {
                    let (new_data, consumed) = decode_utf16le_chunk(&raw[..bytes_read]);
                    let _ = self.parse_intel_data(&channel, &new_data);
                    log_files_map.entry(file_name).and_modify(|hash_entry| {
                        hash_entry.0 = start + consumed as u64;
                        hash_entry.1 = Utc::now();
                    });
                }
            }
        }
        self.settings.set_log_files_channels(log_files_map);
    }

    /// Runs the pattern engine over `data` (chat-log lines from `channel`)
    /// and dispatches every match. Returns the ids of the matched rules.
    #[tracing::instrument(skip(self, data))]
    pub(crate) fn parse_intel_data(&self, channel: &str, data: &str) -> Vec<String> {
        let matches = self.pattern_engine.evaluate(channel, data);
        // Lines with a `map_alert` match, in order: each is dispatched once,
        // with all of its matches, so the tooltip summary sees the whole line.
        let mut alert_lines: Vec<usize> = Vec::new();
        for intel_match in &matches {
            match &intel_match.action {
                ActionConfig::Notify => {
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Info,
                        String::from("PatternEngine"),
                        intel_match.rule_id.clone(),
                        intel_match.line.to_string(),
                    )));
                }
                ActionConfig::MapAlert { .. } => {
                    if !alert_lines.contains(&intel_match.line_index) {
                        alert_lines.push(intel_match.line_index);
                    }
                }
                // Dropped inside `PatternEngine::evaluate`; never matched.
                ActionConfig::Ignore => {}
            }
        }
        for line_index in alert_lines {
            let line_matches: Vec<&PatternMatch> = matches
                .iter()
                .filter(|intel_match| intel_match.line_index == line_index)
                .collect();
            // A question about a system ("H-5GUI status?") is not a report:
            // no visual alert, no sound, no tooltip entry.
            if is_query(&line_matches) {
                continue;
            }
            self.dispatch_map_alert(&line_matches);
        }
        matches
            .into_iter()
            .map(|intel_match| intel_match.rule_id)
            .collect()
    }

    /// Sends the map alert of one intel line (`line_matches` are all the
    /// matches of that line) to every system it reports. A line may name
    /// several candidates (a pilot name also fits the system pattern); only
    /// those that resolve to a real system count. A `clear` report only
    /// reaches the tooltips: no visual alert, no sound, no centering. The
    /// alarm sounds at most once per line.
    #[tracing::instrument(skip(self, line_matches))]
    fn dispatch_map_alert(&self, line_matches: &[&PatternMatch]) {
        let Some(first) = line_matches.first() else {
            return;
        };
        let text = &first.line.text;
        let mut systems: Vec<usize> = Vec::new();
        let mut system_spans = Vec::new();
        for intel_match in line_matches {
            if let ActionConfig::MapAlert { system_group } = &intel_match.action
                && let Some(system_id) = self.resolve_reported_system(intel_match, system_group)
            {
                system_spans.push(intel_match.span.clone());
                if !systems.contains(&system_id) {
                    systems.push(system_id);
                }
            }
        }
        if systems.is_empty() {
            return;
        }
        let summary = AlertSummary::from_line(text, line_matches, system_spans);
        let received = std::time::Instant::now();
        let raises_visual = !summary.is_clear();
        for system_id in &systems {
            let alert = IntelAlert::new(
                *system_id,
                received,
                self.settings.get_alert_duration(),
                text,
                summary.clone(),
            );
            let _ = self.map_msg.0.send(MapSync::SystemAlert(alert));
        }
        if !raises_visual {
            return;
        }
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

    /// The solar system named by the `system_group` capture of
    /// `intel_match`, if the text is a plausible name of a real system.
    ///
    /// An exact name (any case) is looked up in the loaded universe. Only a
    /// code-like text (see [`allows_partial_match`]) may also resolve to a
    /// system whose name merely contains it (SQL `LIKE` against the SDE),
    /// e.g. "H-5GU" or a nickname such as "4-h"; plain words never do, so a
    /// pilot name next to the system can't turn into some unrelated system.
    /// Without a loaded universe everything goes to the SDE as before.
    fn resolve_reported_system(
        &self,
        intel_match: &PatternMatch,
        system_group: &str,
    ) -> Option<usize> {
        let system_name = intel_match.named.get(system_group)?;
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
                    String::from("PatternEngine"),
                    String::from("dispatch_map_alert"),
                    t_error.to_string(),
                )));
                None
            }
        }
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

/// The id of the system in `systems` (id, name) named exactly `name`,
/// ignoring ASCII case.
fn exact_system<'a>(systems: impl IntoIterator<Item = (u32, &'a str)>, name: &str) -> Option<u32> {
    systems
        .into_iter()
        .find(|(_, system)| system.eq_ignore_ascii_case(name))
        .map(|(id, _)| id)
}

/// Whether `name` may resolve to a system whose name only contains it:
/// code-like text (a digit or a dash, as in "H-5GU", "4-h" or "J1234"),
/// never plain words such as a pilot name.
fn allows_partial_match(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_digit() || c == '-')
}

/// The member of `origins` closest to `target` in stargate jumps, if it is at
/// most `max_jumps` away (breadth-first search over
/// [`SolarSystem::connections`], so the first origin to reach `target` is the
/// nearest one; ties go to the earlier origin in the list).
fn nearest_origin_within(
    systems: &HashMap<u32, SolarSystem>,
    origins: &[u32],
    target: u32,
    max_jumps: u8,
) -> Option<u32> {
    let mut seen: HashSet<u32> = origins.iter().copied().collect();
    // (system reached, origin it was reached from, jumps from that origin)
    let mut queue: VecDeque<(u32, u32, u8)> = origins.iter().map(|&id| (id, id, 0)).collect();
    while let Some((system, origin, jumps)) = queue.pop_front() {
        if system == target {
            return Some(origin);
        }
        if jumps == max_jumps {
            continue;
        }
        let Some(solar_system) = systems.get(&system) else {
            continue;
        };
        for &next in &solar_system.connections {
            if seen.insert(next) {
                queue.push_back((next, origin, jumps + 1));
            }
        }
    }
    None
}

/// Names of the channels flagged as monitored in `available`, sorted: the
/// watcher's event handler binary-searches this list.
fn monitored_channel_names(available: &HashMap<String, bool>) -> Vec<String> {
    let mut names: Vec<String> = available
        .iter()
        .filter(|(_, monitored)| **monitored)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort_unstable();
    names
}

/// Decodes a raw byte chunk read from an EVE Online chat log as UTF-16LE.
///
/// EVE writes chat logs as UTF-16LE and re-emits a byte-order mark (U+FEFF)
/// not only at the start of the file but at the start of every appended
/// line (each flush is encoded as its own fragment, BOM included) -- every
/// occurrence is stripped here, not just a single leading one, since
/// [`crate::app::patterns::PatternEngine::parse_line`]'s line regex is anchored on a
/// literal `[` and would otherwise fail to match every line but the first.
///
/// Returns the decoded text and the number of bytes actually consumed from
/// `raw`. If `raw`'s length is odd, the trailing byte is half of a UTF-16
/// code unit split across two reads (the writer flushed mid-character); it
/// is left unconsumed (excluded from the returned count) so the caller
/// re-reads it, paired with its other half, on the next chunk instead of
/// corrupting decoding here. Malformed code units (unpaired surrogates)
/// are replaced with U+FFFD via [`String::from_utf16_lossy`] rather than
/// failing the whole read.
fn decode_utf16le_chunk(raw: &[u8]) -> (String, usize) {
    let usable_len = raw.len() - (raw.len() % 2);
    let code_units: Vec<u16> = raw[..usable_len]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let text = String::from_utf16_lossy(&code_units).replace('\u{feff}', "");
    (text, usable_len)
}

#[cfg(test)]
mod decode_tests {
    use super::decode_utf16le_chunk;

    /// Encodes `s` as raw UTF-16LE bytes, the same wire format EVE writes,
    /// without needing an encoding crate as a test dependency.
    fn utf16le_bytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn decodes_plain_ascii_line() {
        let raw = utf16le_bytes("[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear\r\n");
        let (text, consumed) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2021.09.08 22:56:47 ] Some Pilot > 1DQ1-A clear\r\n"
        );
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn strips_leading_file_bom() {
        let raw = utf16le_bytes("\u{feff}[ 2021.09.08 22:56:47 ] A > hi\r\n");
        let (text, _) = decode_utf16le_chunk(&raw);
        assert_eq!(text, "[ 2021.09.08 22:56:47 ] A > hi\r\n");
    }

    #[test]
    fn strips_every_per_line_bom_not_just_the_first() {
        // Real EVE logs re-emit U+FEFF at the start of every appended
        // line, not only once at the top of the file.
        let raw = utf16le_bytes(
            "\u{feff}[ 2021.09.08 22:56:47 ] A > line one\r\n\u{feff}[ 2021.09.08 22:56:48 ] B > line two\r\n",
        );
        let (text, _) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2021.09.08 22:56:47 ] A > line one\r\n[ 2021.09.08 22:56:48 ] B > line two\r\n"
        );
        assert!(!text.contains('\u{feff}'));
    }

    #[test]
    fn decodes_non_latin_script_correctly() {
        // The same corpus this fix was validated against has a large
        // Chinese-speaking population; a naive UTF-8 read does not just
        // mis-decode this text, it fails to decode the file at all (0xFF,
        // the first byte of the UTF-16LE BOM, is never a valid UTF-8
        // start byte).
        let raw = utf16le_bytes("[ 2023.03.27 02:19:05 ] Algae Roben > 有萨沙甲亢的配置吗\r\n");
        let (text, consumed) = decode_utf16le_chunk(&raw);
        assert_eq!(
            text,
            "[ 2023.03.27 02:19:05 ] Algae Roben > 有萨沙甲亢的配置吗\r\n"
        );
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn holds_back_a_trailing_split_code_unit() {
        let full = utf16le_bytes("[ 2021.09.08 22:56:47 ] A > hi\r\n");
        // Simulate a read landing mid-character: drop the last byte,
        // leaving a dangling first byte of the final code unit ('\n').
        let raw = &full[..full.len() - 1];
        let (text, consumed) = decode_utf16le_chunk(raw);
        // The dangling byte must not be consumed nor corrupt the decoded
        // text.
        assert_eq!(consumed, raw.len() - 1);
        assert_eq!(text, "[ 2021.09.08 22:56:47 ] A > hi\r");
    }

    #[test]
    fn empty_input_decodes_to_empty_output() {
        let (text, consumed) = decode_utf16le_chunk(&[]);
        assert_eq!(text, "");
        assert_eq!(consumed, 0);
    }
}

#[cfg(test)]
mod log_name_tests {
    use super::*;

    #[test]
    fn splits_channel_and_suffix() {
        let log = IntelLogName::parse("Local_20230101_000000_12345.txt").unwrap();
        assert_eq!(log.channel, "Local");
        assert_eq!(log.suffix, "20230101_000000_12345.txt");
    }

    #[test]
    fn accepts_a_name_without_character_id() {
        let log = IntelLogName::parse("Local_20230101_000000.txt").unwrap();
        assert_eq!(log.channel, "Local");
        assert_eq!(log.suffix, "20230101_000000.txt");
    }

    #[test]
    fn keeps_underscores_inside_the_channel_name() {
        let log = IntelLogName::parse("My_Intel_Channel_20230101_000000_12345.txt").unwrap();
        assert_eq!(log.channel, "My_Intel_Channel");
        assert_eq!(log.suffix, "20230101_000000_12345.txt");
    }

    #[test]
    fn keeps_channel_punctuation() {
        let log = IntelLogName::parse("wc.Vale+Tribute_20230101_000000_12345.txt").unwrap();
        assert_eq!(log.channel, "wc.Vale+Tribute");
    }

    #[test]
    fn rejects_files_that_are_not_chatlogs() {
        for name in [
            "notes.txt",
            "nounderscore",
            ".DS_Store",
            "Local_20230101_000000_12345.log",
            "Local_2023_000000_12345.txt",
            "_20230101_000000_12345.txt",
            "",
        ] {
            assert!(
                IntelLogName::parse(name).is_none(),
                "{name:?} should not parse"
            );
        }
    }
}

#[cfg(test)]
mod monitored_channel_names_tests {
    use super::*;

    #[test]
    fn keeps_only_monitored_channels_sorted() {
        let available = HashMap::from([
            (String::from("Local"), false),
            (String::from("wc.Vale"), true),
            (String::from("Alliance"), true),
        ]);
        assert_eq!(
            monitored_channel_names(&available),
            vec![String::from("Alliance"), String::from("wc.Vale")]
        );
    }

    #[test]
    fn is_empty_when_nothing_is_monitored() {
        let available = HashMap::from([(String::from("Local"), false)]);
        assert!(monitored_channel_names(&available).is_empty());
        assert!(monitored_channel_names(&HashMap::new()).is_empty());
    }
}

#[cfg(test)]
mod nearest_origin_tests {
    use super::nearest_origin_within;
    use sde::objects::SolarSystem;
    use std::collections::HashMap;

    /// A straight chain 1 - 2 - 3 - 4 - 5, plus 6 connected to nothing.
    fn chain() -> HashMap<u32, SolarSystem> {
        let links: [(u32, &[u32]); 6] = [
            (1, &[2]),
            (2, &[1, 3]),
            (3, &[2, 4]),
            (4, &[3, 5]),
            (5, &[4]),
            (6, &[]),
        ];
        links
            .into_iter()
            .map(|(id, connections)| {
                let mut system = SolarSystem::new(1.0);
                system.id = id;
                system.connections = connections.to_vec();
                (id, system)
            })
            .collect()
    }

    #[test]
    fn the_origin_itself_is_in_range() {
        assert_eq!(nearest_origin_within(&chain(), &[3], 3, 0), Some(3));
    }

    #[test]
    fn systems_up_to_the_radius_are_in_range() {
        assert_eq!(nearest_origin_within(&chain(), &[1], 3, 2), Some(1));
        assert_eq!(nearest_origin_within(&chain(), &[1], 4, 2), None);
    }

    #[test]
    fn the_closest_origin_is_returned() {
        assert_eq!(nearest_origin_within(&chain(), &[1, 5], 4, 7), Some(5));
        assert_eq!(nearest_origin_within(&chain(), &[1, 5], 2, 7), Some(1));
    }

    #[test]
    fn unconnected_systems_are_never_in_range() {
        assert_eq!(nearest_origin_within(&chain(), &[1], 6, 7), None);
    }
}

#[cfg(test)]
mod system_lookup_tests {
    use super::{allows_partial_match, exact_system};

    const SYSTEMS: [(u32, &str); 3] = [(1, "H-5GUI"), (2, "Jita"), (3, "Old Man Star")];

    #[test]
    fn exact_names_match_in_any_case() {
        assert_eq!(exact_system(SYSTEMS, "h-5gui"), Some(1));
        assert_eq!(exact_system(SYSTEMS, "old man star"), Some(3));
        assert_eq!(exact_system(SYSTEMS, "H-5GU"), None);
        assert_eq!(exact_system(SYSTEMS, "Floris Saucus"), None);
    }

    #[test]
    fn only_code_like_text_may_match_partially() {
        assert!(allows_partial_match("H-5GU"));
        assert!(allows_partial_match("4-h"));
        assert!(allows_partial_match("J1234"));
        assert!(!allows_partial_match("Floris Saucus"));
        assert!(!allows_partial_match("Jit"));
    }
}
