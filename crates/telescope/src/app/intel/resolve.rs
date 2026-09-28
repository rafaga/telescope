//! Resolvers shared by the intel output stages: turning reported text into
//! solar-system ids and measuring stargate distance.

use sde::objects::{SolarSystem, Universe};
use std::collections::{HashMap, HashSet, VecDeque};
use webb::graph::SystemResolver;

/// A [`SystemResolver`] backed by the loaded SDE universe: exact names (any
/// case) first, and -- only for code-like text (see [`allows_partial_match`])
/// -- a system whose name starts with it.
pub(crate) struct UniverseResolver {
    systems: Vec<(String, usize)>,
}

impl UniverseResolver {
    /// Builds the resolver from the universe's solar systems.
    pub(crate) fn new(universe: &Universe) -> Self {
        let mut systems: Vec<(String, usize)> = universe
            .solar_systems
            .values()
            .filter_map(|system| {
                usize::try_from(system.id)
                    .ok()
                    .map(|id| (system.name.to_lowercase(), id))
            })
            .collect();
        systems.sort();
        Self { systems }
    }
}

impl SystemResolver for UniverseResolver {
    fn resolve(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        if let Some((_, id)) = self.systems.iter().find(|(known, _)| known == &lower) {
            return Some(*id);
        }
        if allows_partial_match(name)
            && let Some((_, id)) = self
                .systems
                .iter()
                .find(|(known, _)| known.starts_with(&lower))
        {
            return Some(*id);
        }
        None
    }
}

/// Whether `name` may resolve to a system whose name only contains it:
/// code-like text (a digit or a dash, as in "H-5GU", "4-h" or "J1234"),
/// never plain words such as a pilot name.
pub(crate) fn allows_partial_match(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_digit() || c == '-')
}

/// The member of `origins` closest to `target` in stargate jumps, if it is at
/// most `max_jumps` away (breadth-first search over
/// [`SolarSystem::connections`], so the first origin to reach `target` is the
/// nearest one; ties go to the earlier origin in the list).
pub(crate) fn nearest_origin_within(
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
    use super::allows_partial_match;

    #[test]
    fn only_code_like_text_may_match_partially() {
        assert!(allows_partial_match("H-5GU"));
        assert!(allows_partial_match("4-h"));
        assert!(allows_partial_match("J1234"));
        assert!(!allows_partial_match("Floris Saucus"));
        assert!(!allows_partial_match("Jit"));
    }
}
