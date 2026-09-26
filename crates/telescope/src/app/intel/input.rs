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

/// One chunk of newly-appended, decoded intel text from a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputEvent {
    /// Name of the log file the text came from.
    pub source: String,
    /// Channel parsed from the file name.
    pub channel: String,
    /// Decoded text appended since the last read.
    pub text: String,
    /// File length after this read; the next read starts here.
    pub end_offset: u64,
}

/// Reads new bytes appended to a chat log and decodes them as UTF-16LE.
pub(crate) struct ChatLogSource;

impl ChatLogSource {
    /// Reads the bytes of `file_name` (inside `dir`) appended after `start`,
    /// decodes them and returns the resulting [`InputEvent`], or `None` when
    /// there is nothing new to read.
    ///
    /// A file that shrank below `start` (log rotation) is read from the
    /// beginning. A trailing byte that is half of a UTF-16 code unit split
    /// across two reads is left unconsumed (see [`decode_utf16le_chunk`]) and
    /// re-read, paired with its other half, next time.
    pub(crate) fn read_new(dir: &Path, file_name: &str, start: u64) -> Option<InputEvent> {
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
        let (text, consumed) = decode_utf16le_chunk(&raw[..bytes_read]);
        let channel = IntelLogName::parse(file_name)
            .map(|log| log.channel.to_string())
            .unwrap_or_default();
        Some(InputEvent {
            source: file_name.to_string(),
            channel,
            text,
            end_offset: start + consumed as u64,
        })
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
/// [`webb::patterns::PatternEngine::parse_line`]'s line regex is anchored on a
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
