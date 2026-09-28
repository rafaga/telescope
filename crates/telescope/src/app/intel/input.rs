//! Intel input: the single place that knows the chat-log file naming format,
//! decodes the UTF-16LE wire format EVE writes and reads the bytes appended
//! since the last read, packaging them as [`InputEvent`]s.
//!
//! Reading a file and decoding it are deliberately kept here, away from any
//! interpretation of the text: the detection stage parses and matches what an
//! [`InputEvent`] carries.

use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;
use webb::rules::{IntelLine, parse_line};

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

/// One parsed chat-log line, ready for the detection stage.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InputEvent {
    /// Name of the log file the line came from.
    pub source: String,
    /// Channel parsed from the file name.
    pub channel: String,
    /// The parsed line (timestamp, author, text).
    pub line: IntelLine,
}

/// What a read of a chat log produced: the new lines (as [`InputEvent`]s) and
/// the file offset to resume from next time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InputRead {
    /// The lines appended since the last read that parse as chat-log lines.
    pub events: Vec<InputEvent>,
    /// File length after this read; the next read starts here.
    pub end_offset: u64,
}

/// Reads new bytes appended to a chat log and decodes them as UTF-16LE.
pub(crate) struct ChatLogSource;

impl ChatLogSource {
    /// Reads the bytes of `file_name` (inside `dir`) appended after `start`,
    /// decodes them and returns the parsed lines as [`InputRead`], or `None`
    /// when there is nothing new to read.
    ///
    /// A file that shrank below `start` (log rotation) is read from the
    /// beginning. A trailing byte that is half of a UTF-16 code unit split
    /// across two reads is left unconsumed (see [`decode_utf16le_chunk`]) and
    /// re-read, paired with its other half, next time.
    pub(crate) fn read_new(dir: &Path, file_name: &str, start: u64) -> Option<InputRead> {
        let mut file = File::open(dir.join(file_name)).ok()?;
        let file_length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        let start = if file_length < start { 0 } else { start };
        if file_length <= start {
            return None;
        }
        if file.seek(SeekFrom::Start(start)).is_err() {
            return None;
        }
        let mut chunk = file.take(file_length - start);
        let mut raw = Vec::new();
        let bytes_read = chunk.read_to_end(&mut raw).ok()?;
        // Only whole lines: the watcher can fire while EVE is still writing
        // one, and a truncated line would be parsed (the line pattern takes
        // any text) with the rest of it lost on the next read.
        let (text, consumed) = decode_complete_lines(&raw[..bytes_read]);
        let channel = IntelLogName::parse(file_name)
            .map(|log| log.channel.to_string())
            .unwrap_or_default();
        let source = file_name.to_string();
        let events = text
            .lines()
            .filter_map(parse_line)
            .map(|line| InputEvent {
                source: source.clone(),
                channel: channel.clone(),
                line,
            })
            .collect();
        Some(InputRead {
            events,
            end_offset: start + consumed as u64,
        })
    }
}

/// Where the reading of each chat log stopped, by file name: the next read
/// resumes there.
///
/// Only the files of monitored channels are tracked. [`Self::sync`] adds
/// the files it finds at their current length -- so what was written before
/// Telescope started (or before the channel was monitored) is never replayed
/// -- and never moves the offset of a file it already knows, so lines
/// written since the last read are not skipped. A file the watcher reports
/// before any sync has seen it (its creation event was missed) is read from
/// the start: it is a new, live log.
#[derive(Debug, Default)]
pub(crate) struct IntelOffsets(HashMap<String, u64>);

impl IntelOffsets {
    /// Where to resume reading `file_name` (0 for an unknown file).
    pub(crate) fn get(&self, file_name: &str) -> u64 {
        self.0.get(file_name).copied().unwrap_or(0)
    }

    /// Records where the last read of `file_name` stopped.
    pub(crate) fn set(&mut self, file_name: &str, offset: u64) {
        match self.0.get_mut(file_name) {
            Some(known) => *known = offset,
            None => {
                self.0.insert(file_name.to_string(), offset);
            }
        }
    }

    /// Brings the tracked files in line with `dir`: files of the `monitored`
    /// channels (sorted) that are new get their current length, files that
    /// are gone or no longer monitored are dropped, and known files keep
    /// their offset. Files that can't be read are skipped.
    pub(crate) fn sync(&mut self, dir: &Path, monitored: &[String]) {
        let Ok(entries) = dir.read_dir() else {
            return;
        };
        let mut present: HashMap<String, u64> = HashMap::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(log) = IntelLogName::parse(&name) else {
                continue;
            };
            if !monitored.iter().any(|channel| channel == log.channel) {
                continue;
            }
            // Only files the loop can still read count; a file removed
            // between `read_dir` and here is simply not there.
            if let Ok(meta) = entry.metadata() {
                present.insert(name, meta.len());
            }
        }
        self.0.retain(|name, _| present.contains_key(name));
        for (name, length) in present {
            self.0.entry(name).or_insert(length);
        }
    }

    /// Number of tracked files.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Names of the channels flagged as monitored in `available`, sorted: the
/// watcher's event handler binary-searches this list.
pub(crate) fn monitored_channel_names(available: &HashMap<String, bool>) -> Vec<String> {
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
/// [`webb::rules::parse_line`]'s line regex is anchored on a
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
pub(crate) fn decode_utf16le_chunk(raw: &[u8]) -> (String, usize) {
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

/// Like [`decode_utf16le_chunk`], but only up to the last complete line: the
/// code units after the last `\n` (a line EVE is still writing) are left
/// unconsumed, to be read whole next time. Returns an empty text and 0 when
/// there is no complete line yet.
pub(crate) fn decode_complete_lines(raw: &[u8]) -> (String, usize) {
    let usable_len = raw.len() - (raw.len() % 2);
    let complete = raw[..usable_len]
        .as_chunks::<2>()
        .0
        .iter()
        .rposition(|&pair| u16::from_le_bytes(pair) == u16::from(b'\n'))
        .map_or(0, |last| (last + 1) * 2);
    decode_utf16le_chunk(&raw[..complete])
}

#[cfg(test)]
mod decode_tests {
    use super::{decode_complete_lines, decode_utf16le_chunk};

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
    fn a_line_still_being_written_is_left_for_the_next_read() {
        let raw =
            utf16le_bytes("[ 2021.09.08 22:56:47 ] A > one\r\n[ 2021.09.08 22:56:48 ] B > tw");
        let (text, consumed) = decode_complete_lines(&raw);
        assert_eq!(text, "[ 2021.09.08 22:56:47 ] A > one\r\n");
        assert_eq!(
            consumed,
            utf16le_bytes("[ 2021.09.08 22:56:47 ] A > one\r\n").len()
        );
        // Nothing complete yet: nothing consumed.
        assert_eq!(
            decode_complete_lines(&utf16le_bytes("[ 2021")),
            (String::new(), 0)
        );
        // A CJK character whose code unit has 0x0A as a byte is not a newline.
        let (text, consumed) = decode_complete_lines(&utf16le_bytes("\u{0a0a}x"));
        assert_eq!((text.as_str(), consumed), ("", 0));
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
mod offsets_tests {
    use super::IntelOffsets;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "telescope-offsets-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const INTEL: &str = "wc.Vale+Tribute_20230101_000000_1.txt";
    const LOCAL: &str = "Local_20230101_000000_1.txt";

    #[test]
    fn new_files_start_at_their_end_and_known_ones_keep_their_offset() {
        let dir = temp_dir("sync");
        fs::write(dir.join(INTEL), [0u8; 10]).unwrap();
        fs::write(dir.join(LOCAL), [0u8; 10]).unwrap();
        let monitored = vec![String::from("wc.Vale+Tribute")];
        let mut offsets = IntelOffsets::default();

        offsets.sync(&dir, &monitored);
        assert_eq!(offsets.get(INTEL), 10);
        // Not monitored: not tracked.
        assert_eq!(offsets.len(), 1);

        // Lines written but not read yet survive a rescan.
        offsets.set(INTEL, 4);
        fs::write(dir.join(INTEL), [0u8; 20]).unwrap();
        offsets.sync(&dir, &monitored);
        assert_eq!(offsets.get(INTEL), 4);

        // A removed file is forgotten.
        fs::remove_file(dir.join(INTEL)).unwrap();
        offsets.sync(&dir, &monitored);
        assert_eq!(offsets.len(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_file_is_read_from_the_start() {
        let mut offsets = IntelOffsets::default();
        assert_eq!(offsets.get(INTEL), 0);
        offsets.set(INTEL, 42);
        assert_eq!(offsets.get(INTEL), 42);
    }
}
