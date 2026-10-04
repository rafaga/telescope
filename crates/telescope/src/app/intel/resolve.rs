//! Resolvers shared by the intel output stages: turning reported text into
//! solar-system ids and measuring stargate distance.

use sde::objects::Universe;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::RwLock;
use webb::graph::SystemResolver;

/// A [`SystemResolver`] backed by the loaded SDE universe: exact names (any
/// case) first, and -- only for code-like text (see [`allows_partial_match`])
/// -- the first system (alphabetically) whose name starts with it.
pub(crate) struct UniverseResolver {
    /// Lowercase name -> id, for exact lookups.
    exact: HashMap<String, usize>,
    /// Lowercase names sorted, for prefix lookups by binary search.
    sorted: Vec<(String, usize)>,
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
        Self { exact, sorted }
    }
}

impl SystemResolver for UniverseResolver {
    fn resolve(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        if let Some(id) = self.exact.get(&lower) {
            return Some(*id);
        }
        if !allows_partial_match(name) {
            return None;
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
pub(crate) struct SharedResolver(RwLock<UniverseResolver>);

impl SharedResolver {
    pub(crate) fn new(universe: &Universe) -> Self {
        Self(RwLock::new(UniverseResolver::new(universe)))
    }

    /// Rebuilds the lookup tables from `universe`.
    pub(crate) fn replace(&self, universe: &Universe) {
        let resolver = UniverseResolver::new(universe);
        *self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = resolver;
    }
}

impl SystemResolver for SharedResolver {
    fn resolve(&self, name: &str) -> Option<usize> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resolve(name)
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
    use super::{JumpGraph, nearest_origin_within};

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
    use webb::graph::{Data, Executor, LineContext, RuleGraph};
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
        let context = LineContext {
            line: IntelLine {
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                author: String::from("Pilot"),
                text: text.to_string(),
            },
            channel: String::from("Intel"),
        };
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
