//! Resolvers shared by the intel output stages: turning reported text into
//! solar-system ids and measuring stargate distance.

use sde::objects::Universe;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;
use webb::graph::{ShipResolver, SystemResolver};

/// A [`SystemResolver`] backed by the loaded SDE universe: exact names (any
/// case) first, and -- only for code-like text (see [`allows_partial_match`])
/// -- the first system (alphabetically) whose name starts with it.
pub(crate) struct UniverseResolver {
    /// Lowercase name -> id, for exact lookups.
    exact: HashMap<String, usize>,
    /// Lowercase names sorted, for prefix lookups by binary search.
    sorted: Vec<(String, usize)>,
    /// Systems by the letters before the first dash of their name
    /// (`ymjg` -> `YMJG-4`), for the abbreviations players write.
    heads: HashMap<String, Vec<usize>>,
}

impl UniverseResolver {
    /// Builds the resolver from the universe's solar systems.
    pub(crate) fn new(universe: &Universe) -> Self {
        Self::from_names(
            universe.solar_systems.values().filter_map(|system| {
                Some((system.name.as_str(), usize::try_from(system.id).ok()?))
            }),
        )
    }

    fn from_names<'a>(names: impl IntoIterator<Item = (&'a str, usize)>) -> Self {
        let mut sorted: Vec<(String, usize)> = names
            .into_iter()
            .map(|(name, id)| (name.to_lowercase(), id))
            .collect();
        sorted.sort();
        let exact = sorted.iter().cloned().collect();
        let mut heads: HashMap<String, Vec<usize>> = HashMap::new();
        for (name, id) in &sorted {
            if let Some((head, _)) = name.split_once('-')
                && Self::is_abbreviation(head)
            {
                heads.entry(head.to_string()).or_default().push(*id);
            }
        }
        Self {
            exact,
            sorted,
            heads,
        }
    }

    /// The fewest letters an abbreviation has: with fewer, the words players
    /// write (`fyi`, `ion`, `lul`) are mistaken for systems.
    const MIN_ABBREVIATION_LETTERS: usize = 4;

    /// Whether `text` can be an abbreviation: only letters, enough of them.
    fn is_abbreviation(text: &str) -> bool {
        text.len() >= Self::MIN_ABBREVIATION_LETTERS
            && text.chars().all(|c| c.is_ascii_alphabetic())
    }
}

impl SystemResolver for UniverseResolver {
    fn resolve(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        if let Some(id) = self.exact.get(&lower) {
            return Some(*id);
        }
        if !allows_partial_match(name) {
            // The letters before the dash, when only one system has them:
            // `ymjg` is `YMJG-4`, `msh` is nothing.
            return match self.heads.get(&lower).map(Vec::as_slice) {
                Some([only]) => Some(*only),
                _ => None,
            };
        }
        // The first name >= `lower` is the smallest one that could start with
        // it.
        let first = self
            .sorted
            .partition_point(|(known, _)| known.as_str() < lower.as_str());
        self.sorted
            .get(first)
            .filter(|(known, _)| known.starts_with(&lower))
            .map(|(_, id)| *id)
    }
}

/// The resolver the detection thread uses, replaceable in place: the SDE can
/// be (re)built while Telescope runs (first run, an update from *Settings ->
/// Application*), and the systems it adds must become resolvable without a
/// restart.
pub(crate) struct SharedResolver {
    systems: RwLock<UniverseResolver>,
    ships: SdeShips,
    aliases: SystemAliases,
}

impl SharedResolver {
    pub(crate) fn new(universe: &Universe) -> Self {
        Self {
            systems: RwLock::new(UniverseResolver::new(universe)),
            ships: SdeShips::default(),
            aliases: SystemAliases::default(),
        }
    }

    /// Points the aliases at the file at `path`; see [`SystemAliases::set_path`].
    pub(crate) fn set_aliases_path(&self, path: &Path) {
        self.aliases.set_path(path);
    }

    /// Points the ship index at the SDE database at `path`; see
    /// [`SdeShips::set_path`].
    pub(crate) fn set_sde_path(&self, path: &Path) {
        self.ships.set_path(path);
    }

    /// Rebuilds the lookup tables from `universe`.
    pub(crate) fn replace(&self, universe: &Universe) {
        let resolver = UniverseResolver::new(universe);
        *self
            .systems
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = resolver;
    }
}

impl SystemResolver for SharedResolver {
    fn resolve(&self, name: &str) -> Option<usize> {
        let systems = self
            .systems
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The user's own names come first: they say what a word means in
        // their channels, whatever else it could be.
        match self.aliases.target(name) {
            Some(target) => systems.resolve(&target),
            None => systems.resolve(name),
        }
    }

    fn ship_resolver(&self) -> Option<&dyn ShipResolver> {
        Some(&self.ships)
    }
}

/// The user's own names for systems, from a file next to the SDE database
/// ([`SystemAliases::FILE`]):
///
/// ```toml
/// [aliases]
/// ipay = "IPAY-2"
/// umi = "UMI-KK"
/// ```
///
/// Each key is a word as players write it (any case) and each value the full
/// name of a system of the universe. They cover what the automatic
/// abbreviations do not: the ones shorter than four letters, and the ones
/// that could be more than one system. The file is read when it is first
/// pointed at and again whenever it changes, never while resolving.
#[derive(Default)]
pub(crate) struct SystemAliases {
    index: RwLock<AliasIndex>,
}

/// The aliases read from one version of one file.
#[derive(Default)]
struct AliasIndex {
    path: PathBuf,
    /// When the file was last modified and how big it was; `None` when it
    /// does not exist or could not be read.
    stamp: Option<(SystemTime, u64)>,
    /// Lowercase alias -> the system's name.
    targets: HashMap<String, String>,
}

/// The shape of the aliases file.
#[derive(serde::Deserialize)]
struct AliasFile {
    #[serde(default)]
    aliases: HashMap<String, String>,
}

impl SystemAliases {
    /// The name of the aliases file, next to the SDE database.
    pub(crate) const FILE: &'static str = "system_aliases.toml";

    /// Points the aliases at the file at `path`, reading it again when it is
    /// another file or it changed. Cheap otherwise (one `stat`), so it can run
    /// on every pass of the event loop. A missing file means no aliases; an
    /// unreadable one is reported and means none either.
    pub(crate) fn set_path(&self, path: &Path) {
        let stamp = Self::stamp(path);
        {
            let index = self
                .index
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index.path == path && index.stamp == stamp {
                return;
            }
        }
        // Read outside the lock: the detection thread keeps resolving with
        // the old aliases meanwhile.
        let targets = if stamp.is_some() {
            Self::load(path).unwrap_or_default()
        } else {
            HashMap::new()
        };
        *self
            .index
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = AliasIndex {
            path: path.to_path_buf(),
            stamp,
            targets,
        };
    }

    /// The system `word` stands for, if the user named it.
    fn target(&self, word: &str) -> Option<String> {
        self.index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .targets
            .get(&Self::normalize(word))
            .cloned()
    }

    /// `word` as the aliases are keyed: lowercase, one blank between words.
    fn normalize(word: &str) -> String {
        word.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    /// The aliases of the file at `path`; `None` if it cannot be read or is not
    /// valid.
    fn load(path: &Path) -> Option<HashMap<String, String>> {
        let text = std::fs::read_to_string(path)
            .inspect_err(|error| tracing::warn!("could not read {}: {error}", path.display()))
            .ok()?;
        let file: AliasFile = toml::from_str(&text)
            .inspect_err(|error| tracing::warn!("invalid {}: {error}", path.display()))
            .ok()?;
        Some(
            file.aliases
                .into_iter()
                .map(|(alias, system)| (Self::normalize(&alias), system))
                .filter(|(alias, _)| !alias.is_empty())
                .collect(),
        )
    }
}

/// A [`ShipResolver`] backed by the ship types of the SDE database: the
/// published types of the ship category, by lowercase name. The database is
/// read when it is first pointed at and again whenever the file changes (an
/// SDE update replaces it while Telescope runs), never while resolving.
#[derive(Default)]
pub(crate) struct SdeShips {
    index: Mutex<ShipIndex>,
}

/// The ships read from one version of one database file.
#[derive(Default)]
struct ShipIndex {
    path: PathBuf,
    /// When the file was last modified and how big it was; `None` when it
    /// could not be read.
    stamp: Option<(SystemTime, u64)>,
    ids: HashMap<String, u32>,
}

impl SdeShips {
    /// The ship category of the SDE.
    const SHIP_CATEGORY: u32 = 6;
    const SHIPS_SQL: &'static str = "SELECT t.typeId, t.typeName FROM invTypes t \
         JOIN invGroups g ON g.groupId = t.groupId \
         WHERE g.categoryId = ?1 AND t.published = 1";

    /// Points the index at the database at `path`, reading its ships again
    /// when it is another file or the file changed. Cheap otherwise (one
    /// `stat`), so it can run on every pass of the event loop.
    pub(crate) fn set_path(&self, path: &Path) {
        let stamp = Self::stamp(path);
        {
            let index = self
                .index
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index.path == path && index.stamp == stamp {
                return;
            }
        }
        // Read outside the lock: the detection thread keeps resolving with
        // the old index meanwhile.
        let ids = if stamp.is_some() {
            Self::load(path).unwrap_or_default()
        } else {
            HashMap::new()
        };
        *self
            .index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ShipIndex {
            path: path.to_path_buf(),
            stamp,
            ids,
        };
    }

    fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.modified().ok()?, metadata.len()))
    }

    /// The lowercase name -> type id of every published ship in the database
    /// at `path`, opened read-only. `None` if it can't be read or has no
    /// such tables.
    fn load(path: &Path) -> Option<HashMap<String, u32>> {
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        let mut statement = connection.prepare(Self::SHIPS_SQL).ok()?;
        let rows = statement
            .query_map([Self::SHIP_CATEGORY], |row| {
                Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
            })
            .ok()?;
        Some(
            rows.flatten()
                .map(|(id, name)| (name.to_lowercase(), id))
                .collect(),
        )
    }
}

impl ShipResolver for SdeShips {
    fn resolve_ship(&self, name: &str) -> Option<u32> {
        self.index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ids
            .get(&name.to_lowercase())
            .copied()
    }
}

/// Whether `name` may resolve to a system whose name only contains it:
/// code-like text (a digit or a dash, as in "H-5GU", "4-h" or "J1234"),
/// never plain words such as a pilot name.
pub(crate) fn allows_partial_match(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_digit() || c == '-')
}

/// Stargate connections by system id: all `nearest_origin_within` needs of
/// the universe. Unlike the full SDE map it is cheap to share with the
/// alarm thread behind an `Arc<RwLock<..>>` (see `dispatch::AlarmShared`).
pub(crate) type JumpGraph = HashMap<u32, Vec<u32>>;

/// Extracts the [`JumpGraph`] from the loaded universe.
pub(crate) fn jump_graph(universe: &Universe) -> JumpGraph {
    universe
        .solar_systems
        .iter()
        .map(|(id, system)| (*id, system.connections.clone()))
        .collect()
}

/// The member of `origins` closest to `target` in stargate jumps, if it is at
/// most `max_jumps` away (breadth-first search over
/// [`JumpGraph`], so the first origin to reach `target` is the
/// nearest one; ties go to the earlier origin in the list).
pub(crate) fn nearest_origin_within(
    systems: &JumpGraph,
    origins: &[u32],
    target: u32,
    max_jumps: u8,
) -> Option<u32> {
    jumps_to_nearest_origin(systems, origins, target, max_jumps).map(|(origin, _)| origin)
}

/// Like [`nearest_origin_within`], and also says how many jumps away `target`
/// is from that origin.
pub(crate) fn jumps_to_nearest_origin(
    systems: &JumpGraph,
    origins: &[u32],
    target: u32,
    max_jumps: u8,
) -> Option<(u32, u8)> {
    let mut seen: HashSet<u32> = origins.iter().copied().collect();
    // (system reached, origin it was reached from, jumps from that origin)
    let mut queue: VecDeque<(u32, u32, u8)> = origins.iter().map(|&id| (id, id, 0)).collect();
    while let Some((system, origin, jumps)) = queue.pop_front() {
        if system == target {
            return Some((origin, jumps));
        }
        if jumps == max_jumps {
            continue;
        }
        let Some(connections) = systems.get(&system) else {
            continue;
        };
        for &next in connections {
            if seen.insert(next) {
                queue.push_back((next, origin, jumps + 1));
            }
        }
    }
    None
}

#[cfg(test)]
mod nearest_origin_tests {
    use super::{JumpGraph, jumps_to_nearest_origin, nearest_origin_within};

    /// A straight chain 1 - 2 - 3 - 4 - 5, plus 6 connected to nothing.
    fn chain() -> JumpGraph {
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
            .map(|(id, connections)| (id, connections.to_vec()))
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
    fn the_distance_is_counted_from_the_closest_origin() {
        assert_eq!(jumps_to_nearest_origin(&chain(), &[1], 3, 5), Some((1, 2)));
        assert_eq!(
            jumps_to_nearest_origin(&chain(), &[1, 5], 4, 7),
            Some((5, 1))
        );
        assert_eq!(jumps_to_nearest_origin(&chain(), &[3], 3, 0), Some((3, 0)));
        assert_eq!(jumps_to_nearest_origin(&chain(), &[1], 4, 2), None);
    }

    #[test]
    fn unconnected_systems_are_never_in_range() {
        assert_eq!(nearest_origin_within(&chain(), &[1], 6, 7), None);
    }
}

#[cfg(test)]
mod system_lookup_tests {
    use super::{UniverseResolver, allows_partial_match};
    use webb::graph::SystemResolver;

    fn resolver() -> UniverseResolver {
        UniverseResolver::from_names([
            ("H-5GUI", 1),
            ("H-5GUQ", 2),
            ("Jita", 3),
            ("Old Man Star", 4),
        ])
    }

    #[test]
    fn exact_names_resolve_in_any_case() {
        assert_eq!(resolver().resolve("h-5gui"), Some(1));
        assert_eq!(resolver().resolve("OLD MAN STAR"), Some(4));
        assert_eq!(resolver().resolve("Floris Saucus"), None);
    }

    #[test]
    fn code_like_prefixes_resolve_to_the_first_match() {
        assert_eq!(resolver().resolve("H-5GU"), Some(1));
        assert_eq!(resolver().resolve("h-5"), Some(1));
        assert_eq!(resolver().resolve("H-6"), None);
        // Plain words never match partially.
        assert_eq!(resolver().resolve("Jit"), None);
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

/// Lines the way players write them, through the shipped default rules and
/// the resolver the detection thread uses (names from the real universe, ids
/// made up). They pin down what is read, what is not, and what is left over:
/// the text no detection recognised is the only trace of it.
#[cfg(test)]
mod real_line_tests {
    use super::UniverseResolver;
    use webb::event::IntelEvent;
    use webb::graph::{Data, Executor, RuleGraph};
    use webb::intel::IntelLine;
    use webb::map_alerts::{AlertPart, AlertSummary, leftover};
    use webb::rules::OutputKind;

    /// Real system names, in the shapes EVE uses, and `AD001`..`AD012`.
    const SYSTEMS: &[&str] = &[
        // Letters and digits around a dash.
        "H-5GUI",
        "1DQ1-A",
        "9-GBPD",
        "4-HWWF",
        "05R-7A",
        "7-UH4Z",
        "A-HZYL",
        "PUIG-F",
        // Wormhole space and the `AA000` shape.
        "J105443",
        "J100744",
        "AD001",
        "AD002",
        "AD003",
        "AD004",
        "AD005",
        "AD006",
        "AD007",
        "AD008",
        "AD009",
        "AD010",
        "AD011",
        "AD012",
        // Names written as words.
        "Jita",
        "Amarr",
        "Dodixie",
        "Rens",
        "Hek",
        "Ala",
        "Vale",
        "Old Man Star",
        "Tash-Murkon Prime",
        "Iyen-Oursta",
        "Du Annes",
        "New Caldari",
    ];

    /// What the output stage would have in front of it for one line.
    struct Outcome {
        /// Reported systems that resolved, in order of appearance.
        systems: Vec<&'static str>,
        kinds: Vec<OutputKind>,
        /// What no detection recognised, as the Tooltip output sees it.
        leftover: Option<String>,
        /// What the tooltip shows (the leftover only when nothing else is).
        parts: Vec<AlertPart>,
        ships: Vec<(String, usize)>,
    }

    fn outcome(text: &str) -> Outcome {
        let resolver = UniverseResolver::from_names(
            SYSTEMS
                .iter()
                .enumerate()
                .map(|(index, name)| (*name, index + 1)),
        );
        let (executor, errors) = Executor::new(RuleGraph::default_graph());
        assert!(errors.is_empty(), "{errors:?}");
        let context = IntelEvent::text(
            IntelLine {
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                author: String::from("Pilot"),
                text: text.to_string(),
            },
            "Intel",
            webb::rules::InputKind::ChatLog,
        );
        let activations = executor.run(&context, &resolver);

        let mut systems = Vec::new();
        for activation in &activations {
            for message in &activation.messages {
                if let Data::Systems(ids) = &message.data {
                    for id in ids {
                        let name = SYSTEMS[id - 1];
                        if !systems.contains(&name) {
                            systems.push(name);
                        }
                    }
                }
            }
        }
        let tooltip = activations
            .iter()
            .find(|activation| activation.kind == OutputKind::Tooltip);
        let (leftover_text, parts, ships) = match tooltip {
            Some(activation) => {
                let messages: Vec<&webb::graph::Mensaje> = activation.messages.iter().collect();
                let mut summary = AlertSummary::from_messages(&messages);
                summary.leftover = leftover(text, &messages);
                (
                    Some(summary.leftover.clone()),
                    summary.parts(|count| format!("{count} pilots")),
                    summary.ships.clone(),
                )
            }
            None => (None, Vec::new(), Vec::new()),
        };
        Outcome {
            systems,
            kinds: activations
                .iter()
                .map(|activation| activation.kind)
                .collect(),
            leftover: leftover_text,
            parts,
            ships,
        }
    }

    fn systems_of(text: &str) -> Vec<&'static str> {
        outcome(text).systems
    }

    fn numbered(count: usize) -> String {
        (1..=count)
            .map(|n| format!("AD{n:03}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    // ---- What is read ----

    #[test]
    fn the_usual_ways_of_writing_a_system_are_detected() {
        let cases: &[(&str, &[&str])] = &[
            // A bare name, and the decoration players put around it.
            ("H-5GUI", &["H-5GUI"]),
            ("H-5GUI*", &["H-5GUI"]),
            ("*H-5GUI", &["H-5GUI"]),
            ("H-5GUI +3", &["H-5GUI"]),
            ("H-5GUI nv", &["H-5GUI"]),
            ("H-5GUI  Floris Saucus  nv", &["H-5GUI"]),
            ("A-HZYL.", &["A-HZYL"]),
            ("Hek? Rens? Dodixie?", &["Hek", "Rens", "Dodixie"]),
            ("Amarr 4 reds", &["Amarr"]),
            ("Hek spike", &["Hek"]),
            // Digits first, digits last, and wormholes.
            ("1DQ1-A", &["1DQ1-A"]),
            ("9-GBPD", &["9-GBPD"]),
            ("05R-7A", &["05R-7A"]),
            ("7-UH4Z blue", &["7-UH4Z"]),
            ("J105443", &["J105443"]),
            ("J100744 +2", &["J100744"]),
            ("AD001 AD002", &["AD001", "AD002"]),
            // Names with spaces and hyphens between words.
            ("Jita", &["Jita"]),
            ("Old Man Star", &["Old Man Star"]),
            ("Tash-Murkon Prime", &["Tash-Murkon Prime"]),
            ("Iyen-Oursta", &["Iyen-Oursta"]),
            ("Du Annes", &["Du Annes"]),
            ("New Caldari", &["New Caldari"]),
            // Several systems in one line (a jump, a list).
            ("H-5GUI > 1DQ1-A", &["H-5GUI", "1DQ1-A"]),
            (
                "H-5GUI, 1DQ1-A, 9-GBPD clear",
                &["H-5GUI", "1DQ1-A", "9-GBPD"],
            ),
            // Text from other scripts around the name.
            ("破坏者 在 Jita", &["Jita"]),
            // A terminal escape code glued to the name.
            ("H-5GUI\u{1b}[31m red", &["H-5GUI"]),
        ];
        for (text, expected) in cases {
            assert_eq!(systems_of(text).as_slice(), *expected, "{text:?}");
        }
    }

    #[test]
    fn a_system_named_twice_is_reported_once() {
        assert_eq!(
            systems_of("H-5GUI H-5GUI 1DQ1-A H-5GUI"),
            ["H-5GUI", "1DQ1-A"]
        );
    }

    #[test]
    fn the_start_of_a_code_like_name_resolves_to_it() {
        assert_eq!(systems_of("H-5G"), ["H-5GUI"]);
        assert_eq!(systems_of("H-5 nv"), ["H-5GUI"]);
    }

    #[test]
    fn names_are_detected_in_any_case() {
        for (text, expected) in [
            ("h-5gui", "H-5GUI"),
            ("H-5gui", "H-5GUI"),
            ("j105443", "J105443"),
            ("1dq1-a", "1DQ1-A"),
            ("05r-7a", "05R-7A"),
            ("ad001", "AD001"),
            ("jita", "Jita"),
            ("JITA", "Jita"),
            ("jItA", "Jita"),
            ("old man star", "Old Man Star"),
            ("OLD MAN STAR", "Old Man Star"),
            ("tash-murkon prime", "Tash-Murkon Prime"),
            ("TASH-MURKON PRIME", "Tash-Murkon Prime"),
            ("iyen-oursta", "Iyen-Oursta"),
            ("du annes", "Du Annes"),
            ("new caldari", "New Caldari"),
        ] {
            assert_eq!(systems_of(text), [expected], "{text:?}");
        }
        assert_eq!(
            systems_of("hek? rens? dodixie?"),
            ["Hek", "Rens", "Dodixie"]
        );
        assert_eq!(systems_of("h-5g"), ["H-5GUI"]);
    }

    #[test]
    fn a_line_gives_the_same_alert_in_any_case() {
        for text in [
            "H-5GUI  Floris Saucus  nv",
            "1DQ1-A gate camp 5 Rifter",
            "Hek spike",
            "H-5GUI > 1DQ1-A",
            "H-5GUI Rifter Rifter Loki",
            "Rifter in Ala",
            "7-UH4Z blue",
            "H-5GUI status?",
            "H-5GUI Clear",
            "Old Man Star +3",
        ] {
            let usual = outcome(text);
            for variant in [text.to_lowercase(), text.to_uppercase()] {
                let other = outcome(&variant);
                assert_eq!(other.systems, usual.systems, "{variant:?}");
                assert_eq!(other.kinds, usual.kinds, "{variant:?}");
                assert_eq!(other.parts.len(), usual.parts.len(), "{variant:?}");
                assert_eq!(
                    other.leftover.clone().map(|text| text.to_lowercase()),
                    usual.leftover.clone().map(|text| text.to_lowercase()),
                    "{variant:?}"
                );
                let ships = |outcome: &Outcome| -> Vec<(String, usize)> {
                    outcome
                        .ships
                        .iter()
                        .map(|(name, times)| (name.to_lowercase(), *times))
                        .collect()
                };
                assert_eq!(ships(&other), ships(&usual), "{variant:?}");
            }
        }
    }

    #[test]
    fn a_system_is_found_among_ordinary_lowercase_words() {
        assert_eq!(systems_of("hostile in jita now"), ["Jita"]);
        assert_eq!(
            systems_of("gate camp in old man star now"),
            ["Old Man Star"]
        );
        assert_eq!(
            systems_of("jita to h-5gui via hek"),
            ["Jita", "H-5GUI", "Hek"]
        );
    }

    // ---- What is not read ----

    #[test]
    fn everyday_lowercase_chat_is_not_taken_for_systems() {
        for text in [
            "hostile in local",
            "gate camp, 5 reds",
            "clear now thanks",
            "anyone have eyes",
            "x-up",
            "re-ship",
            "o7",
        ] {
            let outcome = outcome(text);
            assert!(
                outcome.systems.is_empty(),
                "{text:?}: {:?}",
                outcome.systems
            );
            assert!(
                !outcome.kinds.contains(&OutputKind::Visual),
                "{text:?} raised a visual alert"
            );
        }
    }

    #[test]
    fn words_and_codes_that_are_not_systems_raise_nothing() {
        for text in [
            "Floris Saucus",
            "Xyz-12 Abc-99",
            "5GUI",
            "Jit",
            "",
            "?",
            "*****",
            "\u{0}\u{7}\u{1b}",
        ] {
            let outcome = outcome(text);
            assert!(outcome.systems.is_empty(), "{text:?}");
            for kind in [OutputKind::Visual, OutputKind::Sound, OutputKind::Log] {
                assert!(!outcome.kinds.contains(&kind), "{text:?} fired {kind:?}");
            }
        }
    }

    #[test]
    fn a_plain_word_is_never_completed_to_a_longer_name() {
        // `Jit` is shaped like a system name, but only code-like text may
        // match the start of one (see `allows_partial_match`).
        assert!(systems_of("Jit").is_empty());
        assert_eq!(systems_of("Jita"), ["Jita"]);
    }

    #[test]
    fn a_capitalised_word_that_is_a_system_name_is_taken_for_the_system() {
        // `Vale` is a pilot's word and a system. The rules cannot tell.
        let outcome = outcome("Vale of the Silent");
        assert_eq!(outcome.systems, ["Vale"]);
        assert_eq!(outcome.leftover.as_deref(), Some("of the Silent"));
    }

    // ---- What is left over ----

    #[test]
    fn a_candidate_that_does_not_resolve_is_left_over_not_reported() {
        let outcome = outcome("Xyz-12 nv");
        assert!(outcome.systems.is_empty());
        for kind in [OutputKind::Visual, OutputKind::Sound, OutputKind::Log] {
            assert!(!outcome.kinds.contains(&kind), "{kind:?}");
        }
        // The keyword was recognised and removed; the unknown code stays.
        assert_eq!(outcome.leftover.as_deref(), Some("Xyz-12"));
    }

    #[test]
    fn what_no_detection_recognised_is_what_remains() {
        for (text, expected) in [
            ("H-5GUI  Floris Saucus  nv", "Floris Saucus"),
            ("1DQ1-A gate camp", "gate camp"),
            ("Hek spike", "spike"),
            ("Rifter in Ala", "in"),
            ("7-UH4Z blue", "blue"),
            ("破坏者 在 Jita", "破坏者 在"),
        ] {
            assert_eq!(
                outcome(text).leftover.as_deref(),
                Some(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn everything_recognised_leaves_nothing() {
        for text in [
            "H-5GUI",
            "H-5GUI*",
            "H-5GUI +3",
            "H-5GUI nv",
            "H-5GUI > 1DQ1-A",
            "H-5GUI Rifter Rifter Loki",
            "Hek? Rens? Dodixie?",
        ] {
            assert_eq!(outcome(text).leftover.as_deref(), Some(""), "{text:?}");
        }
    }

    #[test]
    fn the_leftover_is_shown_when_it_is_all_the_line_says() {
        let outcome = outcome("1DQ1-A gate camp");
        assert_eq!(outcome.parts, [AlertPart::Text(String::from("gate camp"))]);
    }

    #[test]
    fn the_leftover_is_hidden_once_there_are_ships() {
        // "gate camp 5" was not recognised either, but the tooltip has
        // something better to show, so it is not shown.
        let outcome = outcome("1DQ1-A gate camp 5 Rifter");
        assert_eq!(outcome.leftover.as_deref(), Some("gate camp 5"));
        assert_eq!(outcome.parts, [AlertPart::Ships(String::from("Rifter"))]);
    }

    #[test]
    fn a_question_about_a_system_is_suppressed_and_has_no_tooltip() {
        let outcome = outcome("H-5GUI status?");
        assert!(outcome.kinds.contains(&OutputKind::Suppress));
        assert_eq!(outcome.leftover, None);
    }

    // ---- The limits ----

    #[test]
    fn at_most_eight_system_candidates_are_read_per_line() {
        for (count, read) in [(7, 7), (8, 8), (9, 8), (12, 8)] {
            let outcome = outcome(&numbered(count));
            assert_eq!(outcome.systems.len(), read, "{count} candidates");
            assert_eq!(
                outcome.systems.as_slice(),
                &SYSTEMS[10..10 + read],
                "{count} candidates"
            );
        }
    }

    #[test]
    fn candidates_beyond_the_limit_are_left_over() {
        let outcome = outcome(&numbered(12));
        assert_eq!(outcome.leftover.as_deref(), Some("AD009 AD010 AD011 AD012"));
    }

    #[test]
    fn repeating_one_system_uses_up_the_candidate_limit() {
        // The limit counts mentions, not different systems: a line that names
        // the same system eight times never gets to its second one.
        let spam = format!("{} 1DQ1-A", "H-5GUI ".repeat(8));
        let outcome = outcome(&spam);
        assert_eq!(outcome.systems, ["H-5GUI"]);
        assert_eq!(outcome.leftover.as_deref(), Some("1DQ1-A"));

        let one_short = format!("{} 1DQ1-A", "H-5GUI ".repeat(7));
        assert_eq!(systems_of(&one_short), ["H-5GUI", "1DQ1-A"]);
    }

    #[test]
    fn at_most_a_hundred_matches_of_one_detection_are_counted() {
        let outcome = outcome(&"Rifter ".repeat(150));
        assert_eq!(outcome.ships, [(String::from("Rifter"), 100)]);
        // The mentions past the cap were not recognised: they are left over.
        assert!(
            outcome
                .leftover
                .as_deref()
                .is_some_and(|text| text.starts_with("Rifter Rifter")),
            "{:?}",
            outcome.leftover
        );
    }

    #[test]
    fn a_very_long_line_is_handled() {
        // Far past the length the chat log parser keeps, with candidates
        // that never resolve: it must finish and raise nothing.
        let outcome = outcome(&"A-1 ".repeat(3000));
        assert!(outcome.systems.is_empty());
        assert!(!outcome.kinds.contains(&OutputKind::Visual));

        let names = "Floris Saucus ".repeat(500);
        assert!(names.len() > 5000);
        assert!(systems_of(&names).is_empty());
    }
}

#[cfg(test)]
mod ship_index_tests {
    use super::{SdeShips, SharedResolver};
    use std::path::{Path, PathBuf};
    use webb::graph::{ShipResolver, SystemResolver};

    /// A database of its own, removed when dropped.
    struct TempDb(PathBuf);

    impl TempDb {
        /// A database with the two tables the index reads: the ship group
        /// (category 6) and a module group (category 7).
        fn with_ships(tag: &str, ships: &[(u32, &str, u32, bool)]) -> Self {
            let path = std::env::temp_dir()
                .join(format!("telescope-ships-{tag}-{}.db", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE invGroups (groupId INTEGER, categoryId INTEGER, groupName TEXT, anchorable INTEGER);
                     CREATE TABLE invTypes (typeId INTEGER, groupId INTEGER, typeName TEXT, iconId INTEGER, published INTEGER, volume REAL);
                     INSERT INTO invGroups VALUES (25, 6, 'Frigate', 0), (60, 7, 'Module', 0);",
                )
                .unwrap();
            for (id, name, group, published) in ships {
                connection
                    .execute(
                        "INSERT INTO invTypes VALUES (?1, ?2, ?3, NULL, ?4, 0)",
                        rusqlite::params![id, group, name, published],
                    )
                    .unwrap();
            }
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn published_ships_resolve_by_name_in_any_case() {
        let db = TempDb::with_ships(
            "resolve",
            &[
                (587, "Rifter", 25, true),
                (584, "Merlin", 25, true),
                (1, "Damage Control I", 60, true),
                (999, "Retired Frigate", 25, false),
            ],
        );
        let ships = SdeShips::default();
        ships.set_path(db.path());
        assert_eq!(ships.resolve_ship("Rifter"), Some(587));
        assert_eq!(ships.resolve_ship("MERLIN"), Some(584));
        // Not a ship, not published, not in the SDE.
        assert_eq!(ships.resolve_ship("Damage Control I"), None);
        assert_eq!(ships.resolve_ship("Retired Frigate"), None);
        assert_eq!(ships.resolve_ship("Zzyzx Hauler"), None);
    }

    #[test]
    fn a_missing_or_unreadable_database_resolves_nothing() {
        let ships = SdeShips::default();
        assert_eq!(ships.resolve_ship("Rifter"), None);
        ships.set_path(Path::new("/nonexistent/sde.db"));
        assert_eq!(ships.resolve_ship("Rifter"), None);
    }

    #[test]
    fn another_database_replaces_the_ships_and_the_same_one_is_not_read_again() {
        let first = TempDb::with_ships("first", &[(587, "Rifter", 25, true)]);
        let second = TempDb::with_ships("second", &[(584, "Merlin", 25, true)]);
        let ships = SdeShips::default();
        ships.set_path(first.path());
        ships.set_path(first.path());
        assert_eq!(ships.resolve_ship("Rifter"), Some(587));
        ships.set_path(second.path());
        assert_eq!(ships.resolve_ship("Rifter"), None);
        assert_eq!(ships.resolve_ship("Merlin"), Some(584));
    }

    #[test]
    fn the_shared_resolver_offers_the_ships_of_its_database() {
        let db = TempDb::with_ships("shared", &[(587, "Rifter", 25, true)]);
        let resolver = SharedResolver::new(&sde::objects::Universe::default());
        assert!(resolver.ship_resolver().is_some());
        assert_eq!(
            resolver.ship_resolver().unwrap().resolve_ship("Rifter"),
            None
        );
        resolver.set_sde_path(db.path());
        assert_eq!(
            resolver.ship_resolver().unwrap().resolve_ship("Rifter"),
            Some(587)
        );
    }
}

#[cfg(test)]
mod abbreviation_tests {
    use super::{SharedResolver, SystemAliases, UniverseResolver};
    use sde::objects::{SolarSystem, Universe};
    use std::path::PathBuf;
    use webb::graph::SystemResolver;

    /// Resolvers over a handful of systems, and a place for alias files.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("telescope-alias-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn systems() -> UniverseResolver {
            UniverseResolver::from_names([
                ("YMJG-4", 1),
                ("MSHD-4", 2),
                ("UMI-KK", 3),
                ("ABCD-1", 4),
                ("ABCD-2", 5),
                ("FYI-49", 6),
                ("Jita", 7),
                ("Tash-Murkon Prime", 8),
            ])
        }

        fn shared() -> SharedResolver {
            let mut universe = Universe::new(1.0);
            for (id, name) in [(3, "UMI-KK"), (8, "IPAY-2"), (9, "YMJG-4")] {
                let mut system = SolarSystem::new(1.0);
                system.id = id;
                system.name = name.to_string();
                universe.solar_systems.insert(id, system);
            }
            SharedResolver::new(&universe)
        }

        fn write(&self, text: &str) -> PathBuf {
            let path = self.dir.join(SystemAliases::FILE);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn four_letters_before_the_dash_name_the_only_system_that_has_them() {
        let resolver = Fixture::systems();
        assert_eq!(resolver.resolve("ymjg"), Some(1));
        assert_eq!(resolver.resolve("YMJG"), Some(1));
        assert_eq!(resolver.resolve("mshd"), Some(2));
        assert_eq!(resolver.resolve("tash"), Some(8));
    }

    #[test]
    fn a_shorter_abbreviation_is_not_a_system() {
        let resolver = Fixture::systems();
        assert_eq!(resolver.resolve("umi"), None);
        assert_eq!(resolver.resolve("fyi"), None);
        assert_eq!(resolver.resolve("ym"), None);
    }

    #[test]
    fn an_abbreviation_that_fits_two_systems_is_not_guessed() {
        let resolver = Fixture::systems();
        assert_eq!(resolver.resolve("abcd"), None);
        // Written in full it is not ambiguous.
        assert_eq!(resolver.resolve("abcd-2"), Some(5));
    }

    #[test]
    fn the_whole_name_still_wins_and_words_are_not_abbreviations() {
        let resolver = Fixture::systems();
        assert_eq!(resolver.resolve("Jita"), Some(7));
        assert_eq!(resolver.resolve("ymjg4"), None);
        assert_eq!(resolver.resolve("ymjg blue"), None);
    }

    #[test]
    fn an_alias_names_a_system_the_user_chose() {
        let fixture = Fixture::new("names");
        let resolver = Fixture::shared();
        assert_eq!(resolver.resolve("umi"), None);
        resolver
            .set_aliases_path(&fixture.write("[aliases]\numi = \"UMI-KK\"\nIPAY = \"IPAY-2\"\n"));
        assert_eq!(resolver.resolve("umi"), Some(3));
        assert_eq!(resolver.resolve("UMI"), Some(3));
        assert_eq!(resolver.resolve("ipay"), Some(8));
    }

    #[test]
    fn an_alias_wins_over_the_automatic_abbreviation() {
        let fixture = Fixture::new("wins");
        let resolver = Fixture::shared();
        assert_eq!(resolver.resolve("ymjg"), Some(9));
        resolver.set_aliases_path(&fixture.write("[aliases]\nymjg = \"UMI-KK\"\n"));
        assert_eq!(resolver.resolve("ymjg"), Some(3));
    }

    #[test]
    fn aliases_follow_the_file_when_it_changes_or_goes_away() {
        let fixture = Fixture::new("changes");
        let resolver = Fixture::shared();
        let path = fixture.write("[aliases]\numi = \"UMI-KK\"\n");
        resolver.set_aliases_path(&path);
        assert_eq!(resolver.resolve("umi"), Some(3));
        fixture.write("[aliases]\nthe-other-one = \"IPAY-2\"\n");
        resolver.set_aliases_path(&path);
        assert_eq!(resolver.resolve("umi"), None);
        assert_eq!(resolver.resolve("the-other-one"), Some(8));
        std::fs::remove_file(&path).unwrap();
        resolver.set_aliases_path(&path);
        assert_eq!(resolver.resolve("the-other-one"), None);
    }

    #[test]
    fn an_invalid_aliases_file_means_no_aliases() {
        let fixture = Fixture::new("invalid");
        let resolver = Fixture::shared();
        resolver.set_aliases_path(&fixture.write("this is = not [valid"));
        assert_eq!(resolver.resolve("umi"), None);
        assert_eq!(resolver.resolve("ymjg"), Some(9));
    }
}
