//! Writes the universe map of an `sde.db` as a JSON snapshot for the web build.
//!
//! ```text
//! cargo run --release -p telescope --example export_universe -- [sde.db] [out.json] [factor]
//! ```
//!
//! Defaults, whatever the current folder is: the workspace's `sde.db`,
//! `crates/telescope-web/assets/universe.json` and the same factor Telescope's
//! settings default to. It builds the map data with the same `SdeManager` calls
//! the native map uses, so both show the same universe.

use sde::SdeManager;
use sde::objects::ProjectedAxis;
use std::path::Path;
use universe_snapshot::{
    ConnectionRecord, FORMAT_VERSION, RegionRecord, SystemRecord, UniverseSnapshot,
};

/// Same as `Settings`' default map factor.
const DEFAULT_FACTOR: f64 = 50000000000000.0;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    // Relative to this crate, so the defaults work from any folder.
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let database = args.next().unwrap_or_else(|| {
        crate_dir
            .join("../../sde.db")
            .to_string_lossy()
            .into_owned()
    });
    let output = args.next().unwrap_or_else(|| {
        crate_dir
            .join("../telescope-web/assets/universe.json")
            .to_string_lossy()
            .into_owned()
    });
    if !Path::new(&database).is_file() {
        return Err(format!("{database} does not exist: pass the path to sde.db"));
    }
    let factor = match args.next() {
        Some(value) => value
            .parse::<f64>()
            .map_err(|error| format!("invalid factor {value:?}: {error}"))?,
        None => DEFAULT_FACTOR,
    };

    let manager = SdeManager::new(Path::new(&database), factor).map_err(|e| e.to_string())?;
    let points = manager.get_systems().map_err(|e| e.to_string())?;
    let segments = manager.get_connections().map_err(|e| e.to_string())?;
    let regions = manager
        .get_region_coordinates()
        .map_err(|e| e.to_string())?;

    let mut systems: Vec<SystemRecord> = points
        .into_iter()
        .map(|(id, point)| SystemRecord {
            id,
            pos: point.to_2d(ProjectedAxis::Z),
            name: point.name,
            color: point.color,
            connections: point.connections,
        })
        .collect();
    systems.sort_by_key(|system| system.id);

    let mut connections: Vec<ConnectionRecord> = segments
        .into_iter()
        .map(|(id, segment)| ConnectionRecord {
            id,
            from: [segment.point1[0] as f32, segment.point1[1] as f32],
            to: [segment.point2[0] as f32, segment.point2[1] as f32],
        })
        .collect();
    connections.sort_by_key(|connection| connection.id);

    // The same centre `UniversePane::generate_data` gives the region labels.
    let mut regions: Vec<RegionRecord> = regions
        .into_iter()
        .map(|region| RegionRecord {
            center: [
                ((region.min.x() + region.max.x()) / 2.0 / factor) as f32,
                ((region.min.y() + region.max.y()) / 2.0 / factor) as f32,
            ],
            name: region.name,
        })
        .collect();
    regions.sort_by(|a, b| a.name.cmp(&b.name));

    let snapshot = UniverseSnapshot {
        version: FORMAT_VERSION,
        systems,
        connections,
        regions,
    };
    let json = snapshot.to_json()?;
    if let Some(parent) = Path::new(&output).parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&output, &json).map_err(|e| e.to_string())?;
    println!(
        "{}: {} systems, {} connections, {} regions, {} KiB",
        output,
        snapshot.systems.len(),
        snapshot.connections.len(),
        snapshot.regions.len(),
        json.len() / 1024
    );
    Ok(())
}
