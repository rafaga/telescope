//! A compact snapshot of the universe map: what the map widget needs to draw
//! the systems, the stargate lines and the region names, and nothing else.
//!
//! The native app reads this data from `sde.db` (SQLite). The web build cannot
//! ship that 49 MB database, so `cargo run -p telescope --example
//! export_universe` writes a [`UniverseSnapshot`] as JSON once, and the web
//! build downloads that file instead. This crate has no UI or database
//! dependency, so it builds for both targets.

use serde::{Deserialize, Serialize};

/// Bumped when the JSON layout changes, so an old file is rejected instead of
/// being drawn wrong.
pub const FORMAT_VERSION: u32 = 1;

/// Everything the map needs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UniverseSnapshot {
    /// [`FORMAT_VERSION`] of the file.
    pub version: u32,
    /// Solar systems, already projected to the 2D map space.
    pub systems: Vec<SystemRecord>,
    /// Stargate lines.
    pub connections: Vec<ConnectionRecord>,
    /// Region names with the point they are drawn at.
    pub regions: Vec<RegionRecord>,
}

/// One solar system on the map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemRecord {
    /// Solar system id.
    pub id: usize,
    /// Display name.
    pub name: Option<String>,
    /// Position in map space (the scaled 2D projection the native map uses).
    pub pos: [f32; 2],
    /// Star color as a hex string (`#rrggbb`), when known.
    pub color: Option<String>,
    /// Ids of the connections that touch this system.
    pub connections: Vec<(usize, usize)>,
}

/// A line between two systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRecord {
    /// The pair of system ids it joins.
    pub id: (usize, usize),
    /// Position of the first end, in map space.
    pub from: [f32; 2],
    /// Position of the second end, in map space.
    pub to: [f32; 2],
}

/// A region name drawn behind the map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionRecord {
    /// Region name.
    pub name: String,
    /// Centre of the region's bounding box, in map space.
    pub center: [f32; 2],
}

impl UniverseSnapshot {
    /// Serializes to compact JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|error| error.to_string())
    }

    /// Parses JSON produced by [`UniverseSnapshot::to_json`], rejecting a file
    /// with another [`FORMAT_VERSION`].
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let snapshot: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if snapshot.version != FORMAT_VERSION {
            return Err(format!(
                "universe file has format version {}, expected {FORMAT_VERSION}",
                snapshot.version
            ));
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UniverseSnapshot {
        UniverseSnapshot {
            version: FORMAT_VERSION,
            systems: vec![SystemRecord {
                id: 30000142,
                name: Some("Jita".into()),
                pos: [1.5, -2.0],
                color: Some("#ffcc99".into()),
                connections: vec![(30000142, 30000144)],
            }],
            connections: vec![ConnectionRecord {
                id: (30000142, 30000144),
                from: [1.5, -2.0],
                to: [3.0, 4.0],
            }],
            regions: vec![RegionRecord {
                name: "The Forge".into(),
                center: [0.0, 0.0],
            }],
        }
    }

    #[test]
    fn json_round_trip_keeps_everything() {
        let bytes = sample().to_json().unwrap();
        let back = UniverseSnapshot::from_json(&bytes).unwrap();
        assert_eq!(back.systems.len(), 1);
        assert_eq!(back.systems[0].name.as_deref(), Some("Jita"));
        assert_eq!(back.connections[0].id, (30000142, 30000144));
        assert_eq!(back.regions[0].name, "The Forge");
    }

    #[test]
    fn other_format_version_is_rejected() {
        let mut snapshot = sample();
        snapshot.version = FORMAT_VERSION + 1;
        let bytes = snapshot.to_json().unwrap();
        assert!(UniverseSnapshot::from_json(&bytes).is_err());
    }
}
