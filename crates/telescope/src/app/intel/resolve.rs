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
