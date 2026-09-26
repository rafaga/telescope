//! Resolvers shared by the intel output stages: turning reported text into
//! solar-system ids and measuring stargate distance.

use sde::objects::SolarSystem;
use std::collections::{HashMap, HashSet, VecDeque};

/// The id of the system in `systems` (id, name) named exactly `name`,
/// ignoring ASCII case.
pub(crate) fn exact_system<'a>(
    systems: impl IntoIterator<Item = (u32, &'a str)>,
    name: &str,
) -> Option<u32> {
    systems
        .into_iter()
        .find(|(_, system)| system.eq_ignore_ascii_case(name))
        .map(|(id, _)| id)
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
