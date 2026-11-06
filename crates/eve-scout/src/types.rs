//! What the EvE-Scout API sends and takes, as typed values.
//!
//! The shapes follow the API's OpenAPI document (`https://api.eve-scout.com/ui/`)
//! with the corrections the live API needs, found by reading its answers:
//!
//! * The ids of signatures, observations and donations are documented as
//!   integers and sent as strings (`"id": "77735"`). They are read from either.
//! * The `date` of a daily donation goal is documented as a date and sent as
//!   a timestamp.
//!
//! Every enumeration has an `Unknown` value for what the API adds later, so a
//! new storm type or system class does not make a whole list unreadable.
//! Fields the document does not mark as required are optional.

use chrono::{DateTime, NaiveDate, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

/// An integer the API documents as one and sends as either a number or a
/// string.
fn id_from_number_or_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(i64),
        Text(String),
    }
    match Id::deserialize(deserializer)? {
        Id::Number(number) => Ok(number),
        Id::Text(text) => text
            .trim()
            .parse()
            .map_err(|_| D::Error::custom(format!("`{text}` is not an integer id"))),
    }
}

/// [`id_from_number_or_text`] for a value that may be missing or `null`.
fn optional_id_from_number_or_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(i64),
        Text(String),
    }
    match Option::<Id>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Id::Number(number)) => Ok(Some(number)),
        Some(Id::Text(text)) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| D::Error::custom(format!("`{text}` is not an integer id"))),
    }
}

/// The two hubs EvE-Scout keeps the wormhole connections of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hub {
    Thera,
    Turnur,
}

impl Hub {
    /// Both hubs.
    pub const ALL: [Hub; 2] = [Hub::Thera, Hub::Turnur];

    /// The in-game id of the system.
    pub fn system_id(self) -> i64 {
        match self {
            Hub::Thera => 31_000_005,
            Hub::Turnur => 30_002_086,
        }
    }

    /// The hub of the system with this in-game id, if it is one.
    pub fn from_system_id(system_id: i64) -> Option<Hub> {
        Hub::ALL
            .into_iter()
            .find(|hub| hub.system_id() == system_id)
    }

    /// The name the API takes in its `system_name` parameter.
    pub fn query_name(self) -> &'static str {
        match self {
            Hub::Thera => "thera",
            Hub::Turnur => "turnur",
        }
    }
}

/// The class of a system, as the API names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemClass {
    #[serde(rename = "c1")]
    C1,
    #[serde(rename = "c2")]
    C2,
    #[serde(rename = "c3")]
    C3,
    #[serde(rename = "c4")]
    C4,
    #[serde(rename = "c5")]
    C5,
    #[serde(rename = "c6")]
    C6,
    #[serde(rename = "c10")]
    C10,
    #[serde(rename = "c11")]
    C11,
    #[serde(rename = "c12")]
    C12,
    #[serde(rename = "c13")]
    C13,
    #[serde(rename = "c14")]
    C14,
    #[serde(rename = "c15")]
    C15,
    #[serde(rename = "c16")]
    C16,
    #[serde(rename = "c17")]
    C17,
    #[serde(rename = "c18")]
    C18,
    #[serde(rename = "c25")]
    C25,
    #[serde(rename = "drone")]
    Drone,
    #[serde(rename = "exit")]
    Exit,
    /// High-sec.
    #[serde(rename = "hs")]
    HighSec,
    #[serde(rename = "jove")]
    Jove,
    /// Low-sec.
    #[serde(rename = "ls")]
    LowSec,
    /// Null-sec.
    #[serde(rename = "ns")]
    NullSec,
    /// `unknown`, or a class this crate doesn't know yet.
    #[serde(rename = "unknown", other)]
    Unknown,
}

impl SystemClass {
    /// Every class that has a number in the SDE, in the order of the numbers.
    const WITH_SDE_ID: [SystemClass; 19] = [
        SystemClass::C1,
        SystemClass::C2,
        SystemClass::C3,
        SystemClass::C4,
        SystemClass::C5,
        SystemClass::C6,
        SystemClass::HighSec,
        SystemClass::LowSec,
        SystemClass::NullSec,
        SystemClass::C10,
        SystemClass::C11,
        SystemClass::C12,
        SystemClass::C13,
        SystemClass::C14,
        SystemClass::C15,
        SystemClass::C16,
        SystemClass::C17,
        SystemClass::C18,
        SystemClass::C25,
    ];

    /// The `wormholeClassId` the SDE gives this class: the same number the
    /// game uses, which EvE-Scout writes into the name of the wormhole
    /// classes (`c12` is 12) and 7, 8 and 9 for high-sec, low-sec and
    /// null-sec. `None` for what is EvE-Scout's own and has no number in the
    /// SDE: `drone`, `exit`, `jove` and `unknown`.
    ///
    /// `c10` and `c11` follow the same numbering, but no system or region of
    /// the SDE has them, so nothing confirms them.
    pub fn wormhole_class_id(self) -> Option<i64> {
        Some(match self {
            SystemClass::C1 => 1,
            SystemClass::C2 => 2,
            SystemClass::C3 => 3,
            SystemClass::C4 => 4,
            SystemClass::C5 => 5,
            SystemClass::C6 => 6,
            SystemClass::HighSec => 7,
            SystemClass::LowSec => 8,
            SystemClass::NullSec => 9,
            SystemClass::C10 => 10,
            SystemClass::C11 => 11,
            SystemClass::C12 => 12,
            SystemClass::C13 => 13,
            SystemClass::C14 => 14,
            SystemClass::C15 => 15,
            SystemClass::C16 => 16,
            SystemClass::C17 => 17,
            SystemClass::C18 => 18,
            SystemClass::C25 => 25,
            SystemClass::Drone | SystemClass::Exit | SystemClass::Jove | SystemClass::Unknown => {
                return None;
            }
        })
    }

    /// The class of a `wormholeClassId` of the SDE, when EvE-Scout has one.
    /// The abyssal classes (19 to 23) have none.
    ///
    /// In the SDE the number is on the system (the low-sec ones are 8) or,
    /// when the system has none, on its region: 1 to 6 for the wormhole
    /// regions, 7 for high-sec, 9 for null-sec, 12 for Thera, 25 for Pochven.
    /// Reading it that way gives the `in_system_class` that EvE-Scout sends
    /// for every system it has connected. Deriving high-sec, low-sec and
    /// null-sec from the security status does not: a system the SDE counts as
    /// low-sec can show a security of 0.0.
    pub fn from_wormhole_class_id(id: i64) -> Option<SystemClass> {
        Self::WITH_SDE_ID
            .into_iter()
            .find(|class| class.wormhole_class_id() == Some(id))
    }
}

/// How a route is calculated. Wormholes are always used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum RoutePreference {
    /// High-sec where possible (the API's default).
    #[serde(rename = "safer")]
    Safer,
    /// The shortest route, whatever the security.
    #[serde(rename = "shortest")]
    Shortest,
    /// The shortest route, preferring gates.
    #[serde(rename = "shortest-gates")]
    ShortestGates,
}

/// A system, as routes and the system search describe it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PublicSystem {
    pub system_id: i64,
    pub system_name: String,
    pub region_id: i64,
    pub region_name: String,
    pub system_class: SystemClass,
    pub security_status: f64,
    /// Whether the system has a Jove Observatory. Only present when it does.
    #[serde(default)]
    pub jove_observatory: Option<bool>,
}

/// A route between two systems, as the API calculated it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Route {
    /// The name of the system it starts at.
    pub from: String,
    /// The name of the system it goes to.
    pub to: String,
    pub jumps: u32,
    /// The signature the route goes through: only in the routes to
    /// signatures.
    #[serde(default, deserialize_with = "optional_id_from_number_or_text")]
    pub signature_id: Option<i64>,
    /// The systems on the way, the first and the last included.
    pub route: Vec<PublicSystem>,
}

/// What [`crate::Client::routes`] asks for: the routes from one system to
/// up to [`RouteRequest::MAX_TARGETS`] others.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteRequest {
    /// The name of the system the routes start at.
    pub from: String,
    /// The names of the systems they go to.
    pub to: Vec<String>,
    /// How they are calculated; the API's default (`safer`) when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preference: Option<RoutePreference>,
}

impl RouteRequest {
    /// The most systems a request may go to.
    pub const MAX_TARGETS: usize = 250;
}

/// The biggest ship a wormhole lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShipSize {
    Small,
    Medium,
    Large,
    #[serde(rename = "xlarge")]
    XLarge,
    Capital,
    #[serde(other)]
    Unknown,
}

/// The kind of signature, as the probe scanner groups them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignatureType {
    Combat,
    Data,
    Gas,
    Ore,
    Relic,
    Wormhole,
    #[serde(other)]
    Unknown,
}

/// A wormhole signature in Thera or Turnur that the scouts have scanned and
/// that has not expired.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Signature {
    #[serde(deserialize_with = "id_from_number_or_text")]
    pub id: i64,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub created_by_name: Option<String>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    /// Whether it has been scanned completely.
    pub completed: bool,
    /// `true` when the wormhole type is on the hub's side and the K162 is on
    /// the outside.
    #[serde(default)]
    pub wh_exits_outward: Option<bool>,
    /// The type of the entrance (never `K162`).
    #[serde(default)]
    pub wh_type: Option<String>,
    #[serde(default)]
    pub max_ship_size: Option<ShipSize>,
    /// When it expires: under four hours left means close to its end of life.
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub remaining_hours: Option<i64>,
    pub signature_type: SignatureType,
    /// The id of the hub it is in.
    pub out_system_id: i64,
    pub out_system_name: String,
    /// The signature id on the hub's side (`EOQ-660`).
    pub out_signature: String,
    /// The system on the other side, once it is known.
    #[serde(default)]
    pub in_system_id: Option<i64>,
    #[serde(default)]
    pub in_system_class: Option<SystemClass>,
    #[serde(default)]
    pub in_system_name: Option<String>,
    #[serde(default)]
    pub in_region_id: Option<i64>,
    #[serde(default)]
    pub in_region_name: Option<String>,
    /// The signature id on the other side.
    #[serde(default)]
    pub in_signature: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl Signature {
    /// The hub it is in, if the API names a hub this crate knows.
    pub fn hub(&self) -> Option<Hub> {
        Hub::from_system_id(self.out_system_id)
    }
}

/// The kind of thing EvE-Scout observes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationType {
    ElectricA,
    ElectricB,
    ExoticA,
    ExoticB,
    GammaA,
    GammaB,
    PlasmaA,
    PlasmaB,
    TomsShuttle,
    #[serde(other)]
    Unknown,
}

/// What group an observation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCategory {
    Storm,
    SpaceOddity,
    #[serde(other)]
    Unknown,
}

/// Where something EvE-Scout tracks (a Pochven storm, Tom's Shuttle) was last
/// seen. The API always has the latest record of each object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Observation {
    #[serde(deserialize_with = "id_from_number_or_text")]
    pub id: i64,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub created_by_name: Option<String>,
    pub observation_type: ObservationType,
    #[serde(default = "unknown_category")]
    pub observation_category: ObservationCategory,
    /// A name to show to people (`Tom's Shuttle`).
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub observed_in_person: Option<bool>,
    /// For how many hours it has been in the system.
    #[serde(default)]
    pub hours_in_system: Option<i64>,
    pub system_id: i64,
    pub system_name: String,
    #[serde(default)]
    pub region_id: Option<i64>,
    #[serde(default)]
    pub region_name: Option<String>,
}

fn unknown_category() -> ObservationCategory {
    ObservationCategory::Unknown
}

/// A kind of wormhole.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WormholeType {
    /// What the game calls it (`A009`).
    pub identifier: String,
    #[serde(default)]
    pub comment_public: String,
    /// Mass it regenerates per cycle.
    pub mass_regeneration: f64,
    /// Most mass one ship may have to jump.
    pub max_jump_mass: f64,
    /// Mass it takes before it collapses.
    pub max_stable_mass: f64,
    /// Minutes it stays open.
    pub max_stable_time: f64,
    pub possible_static: bool,
    /// The signal strengths it can show (as the scanner's levels).
    pub signature_level: Vec<f64>,
    /// The classes of system it can open from.
    pub source: Vec<SystemClass>,
    /// The class of system it leads to.
    pub target_system_class: SystemClass,
    pub type_id: i64,
    pub wandering_only: bool,
}

/// What [`crate::Client::wormhole_types`] filters by. Every filter given must
/// match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WormholeTypeFilter {
    /// The most results.
    pub limit: Option<u32>,
    /// The start of the identifier (`z`).
    pub identifier: Option<String>,
    /// The class of system it opens from.
    pub source: Option<SystemClass>,
    /// The class of system it leads to.
    pub target: Option<SystemClass>,
    /// The class it either opens from or leads to.
    pub edge: Option<SystemClass>,
}

/// Which part of the world the system search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Space {
    /// Known space.
    K,
    /// Wormhole space.
    J,
}

impl Space {
    pub(crate) fn query_name(self) -> &'static str {
        match self {
            Space::K => "k-space",
            Space::J => "j-space",
        }
    }
}

/// How finely the scan statistics are cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Period {
    /// The last 30 days, one entry each.
    Daily,
    /// The last 26 weeks, dated the first day of each.
    Weekly,
    /// The last 12 months, dated the first day of each.
    Monthly,
}

impl Period {
    pub(crate) fn query_name(self) -> &'static str {
        match self {
            Period::Daily => "daily",
            Period::Weekly => "weekly",
            Period::Monthly => "monthly",
        }
    }
}

/// How many signatures the scouts scanned in a day, week or month.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScanStatistic {
    /// The day, or the first day of the week or month.
    pub date: NaiveDate,
    /// Every signature, wormholes included.
    pub signatures: i64,
    pub wormholes: i64,
}

/// One donation to EvE-Scout.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Donation {
    #[serde(deserialize_with = "id_from_number_or_text")]
    pub id: i64,
    /// The ISK donated, as the decimal text the API sends.
    pub amount: String,
    pub donated_at: DateTime<Utc>,
    /// Whether it was made on behalf of a corporation.
    #[serde(default)]
    pub corporation_donation: bool,
    pub character_id: i64,
    pub character_name: String,
    pub corporation_id: i64,
    pub corporation_name: String,
    #[serde(default)]
    pub alliance_id: Option<i64>,
    #[serde(default)]
    pub alliance_name: Option<String>,
}

impl Donation {
    /// The ISK donated as a number, if the amount reads as one.
    pub fn isk(&self) -> Option<f64> {
        self.amount.trim().parse().ok()
    }
}

/// How the donations of one day compare to what the service needs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DailyGoal {
    /// The day, at midnight UTC (the API documents a date and sends a
    /// timestamp).
    pub date: DateTime<Utc>,
    /// ISK donated that day.
    pub donations: f64,
    /// The average of the last 30 days.
    pub rolling_avg_30: f64,
    /// The share of the daily goal reached.
    pub percentage: f64,
}

/// The daily donation goal and how the last 30 days went against it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DailyGoals {
    /// ISK per day the service needs.
    pub goal: f64,
    #[serde(default)]
    pub timeline: Vec<DailyGoal>,
}

/// What [`crate::Client::donation_statistics`] returns.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DonationStatistics {
    /// Who donated in the last 30 days.
    #[serde(default)]
    pub donors: Vec<Donation>,
    #[serde(default)]
    pub daily_goals: Option<DailyGoals>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signature as the live API sent it: the id is a string.
    const SIGNATURE: &str = r#"{"id":"77735","created_at":"2026-11-04T21:17:26.000Z","created_by_id":2122375233,"created_by_name":"Troden Treadwell","updated_at":"2026-11-04T21:24:38.000Z","updated_by_id":2122375233,"updated_by_name":"Troden Treadwell","completed_at":"2026-11-04T21:24:38.000Z","completed_by_id":2122375233,"completed_by_name":"Troden Treadwell","completed":true,"wh_exits_outward":false,"wh_type":"J377","max_ship_size":"medium","expires_at":"2026-11-05T22:17:26.000Z","remaining_hours":4,"signature_type":"wormhole","out_system_id":30002086,"out_system_name":"Turnur","out_signature":"RRW-782","in_system_id":31000230,"in_system_class":"c1","in_system_name":"J100422","in_region_id":11000002,"in_region_name":"A-R00002","in_signature":"LGN-154"}"#;

    #[test]
    fn a_signature_is_read_as_the_live_api_sends_it() {
        let signature: Signature = serde_json::from_str(SIGNATURE).unwrap();
        assert_eq!(signature.id, 77735);
        assert_eq!(signature.hub(), Some(Hub::Turnur));
        assert_eq!(signature.signature_type, SignatureType::Wormhole);
        assert_eq!(signature.max_ship_size, Some(ShipSize::Medium));
        assert_eq!(signature.in_system_class, Some(SystemClass::C1));
        assert_eq!(signature.in_system_name.as_deref(), Some("J100422"));
        assert!(signature.completed);
        assert_eq!(
            signature.expires_at.to_rfc3339(),
            "2026-11-05T22:17:26+00:00"
        );
    }

    #[test]
    fn a_signature_still_being_scanned_has_no_other_side() {
        let json = r#"{"id":1420,"created_at":"2026-11-04T21:17:26.000Z","completed":false,"expires_at":"2026-11-05T22:17:26.000Z","signature_type":"unknown","out_system_id":31000005,"out_system_name":"Thera","out_signature":"EOQ-660"}"#;
        let signature: Signature = serde_json::from_str(json).unwrap();
        // The id may be a number too.
        assert_eq!(signature.id, 1420);
        assert_eq!(signature.hub(), Some(Hub::Thera));
        assert_eq!(signature.signature_type, SignatureType::Unknown);
        assert!(signature.in_system_id.is_none() && signature.wh_type.is_none());
    }

    #[test]
    fn what_the_api_adds_later_does_not_break_a_list() {
        let json = r#"[
            {"id":"1","observation_type":"hail_a","observation_category":"meteors","system_id":30000512,"system_name":"O5Q7-U"},
            {"id":"2","observation_type":"toms_shuttle","observation_category":"space_oddity","display_name":"Toms Shuttle","hours_in_system":5,"system_id":30000021,"system_name":"Kuharah","region_name":"Pochven"}
        ]"#;
        let observations: Vec<Observation> = serde_json::from_str(json).unwrap();
        assert_eq!(observations[0].observation_type, ObservationType::Unknown);
        assert_eq!(
            observations[0].observation_category,
            ObservationCategory::Unknown
        );
        assert_eq!(
            observations[1].observation_type,
            ObservationType::TomsShuttle
        );
        assert_eq!(
            observations[1].observation_category,
            ObservationCategory::SpaceOddity
        );
        assert_eq!(observations[1].hours_in_system, Some(5));
        let system: PublicSystem = serde_json::from_str(
            r#"{"system_id":1,"system_name":"X","region_id":2,"region_name":"Y","system_class":"c99","security_status":-0.3}"#,
        )
        .unwrap();
        assert_eq!(system.system_class, SystemClass::Unknown);
        assert_eq!(system.jove_observatory, None);
    }

    #[test]
    fn a_route_to_a_signature_names_the_signature() {
        let json = r#"{"from":"Jita","to":"J100422","jumps":2,"signature_id":"77735","route":[
            {"system_id":30000142,"system_name":"Jita","region_id":10000002,"region_name":"The Forge","system_class":"hs","security_status":0.9,"jove_observatory":true}]}"#;
        let route: Route = serde_json::from_str(json).unwrap();
        assert_eq!(route.signature_id, Some(77735));
        assert_eq!(route.route[0].jove_observatory, Some(true));
        let plain: Route = serde_json::from_str(
            r#"{"from":"A","to":"B","jumps":0,"signature_id":null,"route":[]}"#,
        )
        .unwrap();
        assert_eq!(plain.signature_id, None);
    }

    #[test]
    fn a_wormhole_type_is_read_with_its_classes() {
        let json = r#"{"identifier":"A009","comment_public":"","mass_regeneration":3000000000,"max_jump_mass":5000000,"max_stable_mass":3000000000,"max_stable_time":270,"possible_static":false,"signature_level":[1],"source":["c1","ns"],"target_system_class":"c13","type_id":34439,"wandering_only":true}"#;
        let wormhole: WormholeType = serde_json::from_str(json).unwrap();
        assert_eq!(wormhole.source, [SystemClass::C1, SystemClass::NullSec]);
        assert_eq!(wormhole.target_system_class, SystemClass::C13);
        assert_eq!(wormhole.max_stable_time, 270.0);
        assert!(wormhole.wandering_only);
    }

    #[test]
    fn the_statistics_are_read_with_the_corrections_the_live_api_needs() {
        let scans: Vec<ScanStatistic> =
            serde_json::from_str(r#"[{"date":"2026-11-05","signatures":40,"wormholes":29}]"#)
                .unwrap();
        assert_eq!(scans[0].date.to_string(), "2026-11-05");
        // Donations: string ids and amounts, an alliance that may be null, and
        // a timestamp where the document says date.
        let json = r#"{"donors":[{"id":"26124089911","amount":"15000000","donated_at":"2026-10-08T02:40:46.000Z","corporation_donation":false,"character_id":92816323,"character_name":"Taras Shtan","corporation_id":98239598,"corporation_name":"S.M.O.Y.","alliance_id":null,"alliance_name":null}],
            "daily_goals":{"goal":142857142.86,"timeline":[{"date":"2026-11-05T00:00:00.000Z","donations":15000000,"rolling_avg_30":465582491,"percentage":10.5}]}}"#;
        let donations: DonationStatistics = serde_json::from_str(json).unwrap();
        assert_eq!(donations.donors[0].id, 26_124_089_911);
        assert_eq!(donations.donors[0].isk(), Some(15_000_000.0));
        assert_eq!(donations.donors[0].alliance_id, None);
        let goals = donations.daily_goals.unwrap();
        assert_eq!(goals.timeline[0].percentage, 10.5);
        assert_eq!(
            goals.timeline[0].date.to_rfc3339(),
            "2026-11-05T00:00:00+00:00"
        );
    }

    #[test]
    fn an_id_that_is_not_a_number_is_an_error() {
        let json = r#"{"id":"abc","created_at":"2026-11-04T21:17:26.000Z","completed":false,"expires_at":"2026-11-05T22:17:26.000Z","signature_type":"wormhole","out_system_id":31000005,"out_system_name":"Thera","out_signature":"X"}"#;
        assert!(serde_json::from_str::<Signature>(json).is_err());
    }

    #[test]
    fn the_classes_are_the_wormhole_class_ids_of_the_sde() {
        // The numbers found in a real `sde.db`, checked against the class
        // EvE-Scout sends for the systems its signatures lead to.
        let known = [
            (SystemClass::C1, 1),
            (SystemClass::C3, 3),
            (SystemClass::C4, 4),
            (SystemClass::C6, 6),
            (SystemClass::HighSec, 7),
            (SystemClass::LowSec, 8),
            (SystemClass::NullSec, 9),
            (SystemClass::C12, 12),
            (SystemClass::C13, 13),
            (SystemClass::C14, 14),
            (SystemClass::C18, 18),
            (SystemClass::C25, 25),
        ];
        for (class, id) in known {
            assert_eq!(class.wormhole_class_id(), Some(id));
            assert_eq!(SystemClass::from_wormhole_class_id(id), Some(class));
        }
        // What is EvE-Scout's own has no number, and the abyssal classes of
        // the SDE have no class.
        for class in [
            SystemClass::Drone,
            SystemClass::Exit,
            SystemClass::Jove,
            SystemClass::Unknown,
        ] {
            assert_eq!(class.wormhole_class_id(), None);
        }
        for id in [0, 19, 20, 23, 26, -1] {
            assert_eq!(SystemClass::from_wormhole_class_id(id), None);
        }
        // Both ways agree for every class that has a number.
        for id in 1..=25 {
            if let Some(class) = SystemClass::from_wormhole_class_id(id) {
                assert_eq!(class.wormhole_class_id(), Some(id));
            }
        }
    }

    #[test]
    fn the_hubs_are_known_by_their_ids_and_names() {
        assert_eq!(Hub::from_system_id(31_000_005), Some(Hub::Thera));
        assert_eq!(Hub::from_system_id(30_000_142), None);
        assert_eq!(Hub::Turnur.query_name(), "turnur");
        for hub in Hub::ALL {
            assert_eq!(Hub::from_system_id(hub.system_id()), Some(hub));
        }
    }

    #[test]
    fn a_route_request_leaves_out_the_preference_it_was_not_given() {
        let request = RouteRequest {
            from: String::from("Zoohen"),
            to: vec![String::from("Jita")],
            preference: None,
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"from":"Zoohen","to":["Jita"]}"#
        );
        let request = RouteRequest {
            preference: Some(RoutePreference::ShortestGates),
            ..request
        };
        assert!(
            serde_json::to_string(&request)
                .unwrap()
                .contains(r#""preference":"shortest-gates""#)
        );
    }
}
