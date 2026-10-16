//! What the rule engine evaluates: an [`IntelEvent`].
//!
//! The engine does not care where an event comes from. Every source (a chat
//! log, the zKillboard feed...) reads and validates its own format and hands
//! the engine the same thing: the **text** of the event, which the text
//! detections (patterns and word lists) read, and the **facts** the source
//! already knows for sure, which the detections that can use them read
//! instead of guessing them back from the text.
//!
//! A chat line only has text. A killmail has both: a sentence for the log,
//! the tooltips and the user's own patterns, and the solar system, ships and
//! pilot count exactly as the feed gave them.

use crate::rules::{InputKind, IntelLine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// A solar system a source reports by itself.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemFact {
    /// The system's SDE id.
    pub id: usize,
    /// The system's name, as shown in messages and tooltips.
    pub name: String,
    /// Where the name stands in the event's text, if it does: what the
    /// tooltip leaves out of the leftover text, like the span of a system
    /// found by a pattern.
    pub span: Option<Range<usize>>,
}

/// A ship, and how many times an event names it.
///
/// The type id is what identifies a ship: a killmail carries it, while a chat
/// line only has the name a player typed, which has to be resolved (and may
/// not resolve: a nickname, a typo). So the id is optional, and the name is
/// always there to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipCount {
    /// The ship's type id in the SDE, when it is known.
    pub type_id: Option<u32>,
    /// The ship's name, as the source gave it or as it was typed.
    pub name: String,
    /// How many times the event names it.
    pub times: u32,
}

impl ShipCount {
    /// A ship known only by its name.
    pub fn new(name: impl Into<String>, times: u32) -> Self {
        Self {
            type_id: None,
            name: name.into(),
            times,
        }
    }

    /// The same ship with its type id.
    #[must_use]
    pub fn with_type_id(mut self, type_id: u32) -> Self {
        self.type_id = Some(type_id);
        self
    }

    /// Whether `other` is the same ship: the same type id, or, when either
    /// has none, the same name ignoring case.
    pub fn is_same_ship(&self, other: &Self) -> bool {
        match (self.type_id, other.type_id) {
            (Some(a), Some(b)) => a == b,
            _ => crate::graph::fold(&self.name) == crate::graph::fold(&other.name),
        }
    }

    /// Adds `ship` to `ships`: its times go to the ship already listed, if
    /// there is one (which gets the id if it had none), or it is listed after
    /// the others. Order of appearance is kept.
    pub fn tally(ships: &mut Vec<Self>, ship: Self) {
        match ships.iter_mut().find(|known| known.is_same_ship(&ship)) {
            Some(known) => {
                known.times += ship.times;
                known.type_id = known.type_id.or(ship.type_id);
            }
            None => ships.push(ship),
        }
    }
}

/// What a source knows about an event without reading its text.
///
/// Every fact is optional on purpose: `None` means the source says nothing
/// about it, and the detection reads the text instead. `Some` is final, even
/// when empty -- a source that reports no ships means there are none, and the
/// text is not searched for any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// The solar systems the event is about.
    pub systems: Option<Vec<SystemFact>>,
    /// The ships it involves, with how many times each appears.
    pub ships: Option<Vec<ShipCount>>,
    /// How many pilots it is about.
    pub pilots: Option<u32>,
}

impl Facts {
    /// Whether the source reported nothing: the event is only its text.
    pub fn is_empty(&self) -> bool {
        self.systems.is_none() && self.ships.is_none() && self.pilots.is_none()
    }
}

/// A source of events: reads and validates its own format and turns what it
/// reads into [`IntelEvent`]s.
///
/// This is the only place that knows a source's format. A chat log decodes a
/// text line; the killmail feed decodes a JSON file that was already
/// deserialized and validated. The engine and the detections never see
/// either.
pub trait EventSource {
    /// What the source reads before it is an event: a text line, a
    /// deserialized feed file...
    type Raw: ?Sized;

    /// The kind of source: only the Input nodes of this kind receive its
    /// events.
    fn kind(&self) -> InputKind;

    /// The event for `raw`, or `None` when it holds nothing the engine can
    /// use (a line that is not in the log's format, a kill that can't be
    /// placed).
    fn decode(&self, raw: &Self::Raw) -> Option<IntelEvent>;
}

/// Where the moment of an event comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeOrigin {
    /// The source gave it: the date of the line in the log, the time of the
    /// killmail.
    #[default]
    Reported,
    /// The source did not give one (or not a readable one) and it is the
    /// moment the event was read. The event is still evaluated, and the
    /// application warns about it.
    Assumed,
}

/// One event for the rule engine, whatever its source.
#[derive(Debug, Clone)]
pub struct IntelEvent {
    /// The event as a line: its moment, its author and its text. The text
    /// detections read the text, and the log and the tooltips show it.
    pub line: IntelLine,
    /// The chat channel, or the fixed channel of a source that has none (see
    /// [`InputKind::fixed_channel`]).
    pub channel: String,
    /// The kind of source that produced the event: only the Input nodes of
    /// this kind receive it.
    pub kind: InputKind,
    /// What the source knows about the event beyond its text.
    pub facts: Facts,
    /// Whether [`IntelEvent::at`] is the moment the source reported or one
    /// assumed (see [`TimeOrigin`]).
    pub time_origin: TimeOrigin,
}

impl IntelEvent {
    /// An event that is only text, like a chat line.
    pub fn text(line: IntelLine, channel: impl Into<String>, kind: InputKind) -> Self {
        Self {
            line,
            channel: channel.into(),
            kind,
            facts: Facts::default(),
            time_origin: TimeOrigin::Reported,
        }
    }

    /// When the event happened, in UTC: the date of its line in the log or the
    /// time of its killmail, or the moment it was read when the source gave
    /// none ([`IntelEvent::time_origin`]).
    pub fn at(&self) -> DateTime<Utc> {
        self.line.timestamp
    }

    /// The same event with the origin of its moment.
    #[must_use]
    pub fn with_time_origin(mut self, origin: TimeOrigin) -> Self {
        self.time_origin = origin;
        self
    }

    /// The same event with the facts its source knows.
    #[must_use]
    pub fn with_facts(mut self, facts: Facts) -> Self {
        self.facts = facts;
        self
    }
}
