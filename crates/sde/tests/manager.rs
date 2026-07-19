//! Integration tests for `SdeManager` using a minimal SQLite fixture that
//! mimics the SDE database schema used by the crate's queries.
//!
//! The fixture contains:
//! - 2 regions (10000001 "Region Alpha", 10000002 "Region Beta")
//! - 2 constellations (one per region)
//! - 4 solar systems (3 in K-Space range 30000000..=30999999, 1 outside)
//! - 2 stargate connections (1-2 and 2-3)
//! - 3 planets and 1 moon
//! - 3 abstract systems (2 in Region Alpha, 1 in Region Beta)

use rusqlite::Connection;
use sde::SdeManager;
use sde::objects::SdePoint;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Factor used by most tests: coordinates are divided by 100.
const FACTOR: i64 = 100;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Creates a temporary SDE-like database and removes it on drop.
struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(test_name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "sde_test_{}_{}_{}.db",
            test_name,
            std::process::id(),
            id
        ));
        let conn = Connection::open(&path).expect("cannot create fixture database");
        conn.execute_batch(
            "
            CREATE TABLE mapRegions (regionId INTEGER PRIMARY KEY, regionName TEXT NOT NULL);
            CREATE TABLE mapConstellations (
                constellationId INTEGER PRIMARY KEY,
                constellationName TEXT NOT NULL,
                regionId INTEGER NOT NULL,
                centerX REAL, centerY REAL, centerZ REAL
            );
            CREATE TABLE mapSolarSystems (
                solarSystemId INTEGER PRIMARY KEY,
                solarSystemName TEXT NOT NULL,
                constellationId INTEGER NOT NULL,
                projX REAL, projY REAL, projZ REAL
            );
            CREATE TABLE mapSystemConnections (
                systemConnectionId TEXT PRIMARY KEY,
                systemA INTEGER NOT NULL,
                systemB INTEGER NOT NULL
            );
            CREATE TABLE mapPlanets (
                planetId INTEGER PRIMARY KEY,
                planetaryIndex INTEGER NOT NULL,
                solarSystemId INTEGER NOT NULL
            );
            CREATE TABLE mapMoons (
                moonId INTEGER PRIMARY KEY,
                moonIndex INTEGER NOT NULL,
                solarSystemId INTEGER NOT NULL,
                planetId INTEGER NOT NULL
            );
            CREATE TABLE mapAbstractSystems (
                solarSystemId INTEGER PRIMARY KEY,
                x REAL, y REAL,
                regionId INTEGER NOT NULL
            );

            INSERT INTO mapRegions (regionId, regionName) VALUES
                (10000001, 'Region Alpha'),
                (10000002, 'Region Beta');
            INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) VALUES
                (20000001, 'Const One', 10000001, 100.0, 200.0, 300.0),
                (20000002, 'Const Two', 10000002, -100.0, -200.0, -300.0);
            INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, projX, projY, projZ) VALUES
                (30000001, 'Sys One',   20000001,  1000.0,  2000.0,  3000.0),
                (30000002, 'Sys Two',   20000001, -1000.0, -2000.0, -3000.0),
                (30000003, 'Sys Three', 20000002,  5000.0,  5000.0,  5000.0),
                (31000001, 'W-Sys',     20000002,  9000.0,  9000.0,  9000.0);
            INSERT INTO mapSystemConnections (systemConnectionId, systemA, systemB) VALUES
                ('conn-1-2', 30000001, 30000002),
                ('conn-2-3', 30000002, 30000003);
            INSERT INTO mapPlanets (planetId, planetaryIndex, solarSystemId) VALUES
                (40000001, 1, 30000001),
                (40000002, 2, 30000001),
                (40000003, 1, 30000003);
            INSERT INTO mapMoons (moonId, moonIndex, solarSystemId, planetId) VALUES
                (50000001, 1, 30000001, 40000001);
            INSERT INTO mapAbstractSystems (solarSystemId, x, y, regionId) VALUES
                (30000001, 10.0, 20.0, 10000001),
                (30000002, 30.0, 40.0, 10000001),
                (30000003, 50.0, 60.0, 10000002);
            ",
        )
        .expect("cannot populate fixture database");
        conn.close().expect("cannot close fixture database");
        Fixture { path }
    }

    fn manager(&self) -> SdeManager<'_> {
        SdeManager::new(&self.path, FACTOR)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// -------------------------------------------------------------------------
// get_systempoints / get_system_connections
// -------------------------------------------------------------------------

#[test]
fn systempoints_returns_only_k_space_systems() {
    let fixture = Fixture::new("systempoints_k_space");
    let manager = fixture.manager();
    let points = manager.get_systempoints().unwrap();
    // W-Sys (31000001) is outside the K-Space id range and must be excluded
    assert_eq!(points.len(), 3);
    assert!(!points.contains_key(&31000001));
}

#[test]
fn systempoints_applies_factor_and_coordinate_inversion() {
    let fixture = Fixture::new("systempoints_factor");
    let manager = fixture.manager();
    let points = manager.get_systempoints().unwrap();

    let point = &points[&30000001];
    assert_eq!(point.get_id(), 30000001);
    assert_eq!(point.get_name(), "Sys One");
    // (1000, 2000, 3000) / 100 = (10, 20, 30), inverted -> (-10, -20, -30)
    // RawPoint holds (x, z)
    assert_eq!(point.raw_point.components, [-10.0, -30.0]);

    let point = &points[&30000002];
    assert_eq!(point.raw_point.components, [10.0, 30.0]);
}

#[test]
fn systempoints_without_inversion_keeps_original_sign() {
    let fixture = Fixture::new("systempoints_no_invert");
    let mut manager = fixture.manager();
    manager.invert_coordinates = false;
    let points = manager.get_systempoints().unwrap();
    assert_eq!(points[&30000001].raw_point.components, [10.0, 30.0]);
}

#[test]
fn systempoints_with_negative_factor_multiplies() {
    let fixture = Fixture::new("systempoints_neg_factor");
    let mut manager = fixture.manager();
    manager.factor = -100; // negative factor multiplies by its absolute value
    let points = manager.get_systempoints().unwrap();
    // (1000 * 100) inverted -> -100000
    assert_eq!(
        points[&30000001].raw_point.components,
        [-100000.0, -300000.0]
    );
}

#[test]
fn system_connections_are_added_bidirectionally() {
    let fixture = Fixture::new("system_connections");
    let manager = fixture.manager();
    let points = manager.get_systempoints().unwrap();
    let points = manager.get_system_connections(points).unwrap();

    assert_eq!(points[&30000001].connections, vec!["conn-1-2"]);
    assert_eq!(points[&30000002].connections, vec!["conn-1-2", "conn-2-3"]);
    assert_eq!(points[&30000003].connections, vec!["conn-2-3"]);
}

// -------------------------------------------------------------------------
// get_connections (map lines)
// -------------------------------------------------------------------------

#[test]
fn connections_returns_lines_with_scaled_inverted_coords() {
    let fixture = Fixture::new("connections");
    let manager = fixture.manager();
    let lines = manager.get_connections().unwrap();

    assert_eq!(lines.len(), 2);
    let line = &lines["conn-1-2"];
    assert_eq!(line.id, Some(String::from("conn-1-2")));
    // point1 = system A (30000001): (x, z) scaled and inverted
    assert_eq!(line.raw_line.points[0].components, [-10.0, -30.0]);
    // point2 = system B (30000002)
    assert_eq!(line.raw_line.points[1].components, [10.0, 30.0]);
}

// -------------------------------------------------------------------------
// get_region
// -------------------------------------------------------------------------

#[test]
fn region_without_filters_returns_all_with_constellations() {
    let fixture = Fixture::new("region_all");
    let manager = fixture.manager();
    let regions = manager.get_region(vec![], None).unwrap();

    assert_eq!(regions.len(), 2);
    let alpha = &regions[&10000001];
    assert_eq!(alpha.name, "Region Alpha");
    assert_eq!(alpha.constellations, vec![20000001]);
    let beta = &regions[&10000002];
    assert_eq!(beta.name, "Region Beta");
    assert_eq!(beta.constellations, vec![20000002]);
}

#[test]
fn region_filtered_by_ids() {
    let fixture = Fixture::new("region_by_ids");
    let manager = fixture.manager();
    let regions = manager.get_region(vec![10000002], None).unwrap();

    assert_eq!(regions.len(), 1);
    assert!(regions.contains_key(&10000002));
}

#[test]
fn region_filtered_by_name() {
    let fixture = Fixture::new("region_by_name");
    let manager = fixture.manager();
    let regions = manager
        .get_region(vec![], Some(String::from("alpha")))
        .unwrap();

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[&10000001].name, "Region Alpha");
    assert_eq!(regions[&10000001].constellations, vec![20000001]);
}

#[test]
fn region_name_filter_is_case_insensitive() {
    // SQLite's LIKE is case-insensitive for ASCII by default, so the region
    // is found even with an uppercase needle.
    let fixture = Fixture::new("region_case");
    let manager = fixture.manager();
    let regions = manager
        .get_region(vec![], Some(String::from("ALPHA")))
        .unwrap();
    assert_eq!(regions.len(), 1);
    assert!(regions.contains_key(&10000001));
}

// -------------------------------------------------------------------------
// get_universe
// -------------------------------------------------------------------------

#[test]
fn universe_with_empty_filters_currently_fails() {
    // Documents the current behavior: get_constellation/get_solarsystem pass
    // an rarray parameter even when the filter is empty and the query has no
    // placeholders, and rusqlite rejects the extra parameter
    // (Error::InvalidParameterCount).
    let fixture = Fixture::new("universe_empty");
    let mut manager = fixture.manager();
    assert!(manager.get_universe().is_err());
}

// -------------------------------------------------------------------------
// get_system_id / get_system_coords
// -------------------------------------------------------------------------

#[test]
fn system_id_searches_with_like() {
    let fixture = Fixture::new("system_id_like");
    let manager = fixture.manager();

    let results = manager.get_system_id(String::from("one")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0],
        (
            30000001,
            String::from("Sys One"),
            10000001,
            String::from("Region Alpha")
        )
    );

    // substring search matches all four systems (including W-Sys)
    let results = manager.get_system_id(String::from("sys")).unwrap();
    assert_eq!(results.len(), 4);

    let results = manager.get_system_id(String::from("nonexistent")).unwrap();
    assert!(results.is_empty());
}

#[test]
fn system_coords_applies_factor_and_inversion() {
    let fixture = Fixture::new("system_coords");
    let manager = fixture.manager();

    let coords = manager.get_system_coords(30000001).unwrap();
    assert_eq!(coords, Some(SdePoint::new(-10, -20, -30)));
}

#[test]
fn system_coords_returns_none_for_unknown_id() {
    let fixture = Fixture::new("system_coords_none");
    let manager = fixture.manager();
    assert_eq!(manager.get_system_coords(30000999).unwrap(), None);
}

// -------------------------------------------------------------------------
// get_planet / get_moon
// -------------------------------------------------------------------------

#[test]
fn planet_filtered_by_solar_system() {
    let fixture = Fixture::new("planet_filtered");
    let manager = fixture.manager();
    let planets = manager.get_planet(vec![30000001]).unwrap();

    assert_eq!(planets.len(), 2);
    assert_eq!(planets[0].id, 40000001);
    assert_eq!(planets[0].index, 1);
    assert_eq!(planets[0].solar_system, 30000001);
    assert_eq!(planets[1].id, 40000002);
    assert_eq!(planets[1].index, 2);
}

#[test]
fn planet_with_empty_filter_currently_fails() {
    // Documents the current behavior: an rarray parameter is passed although
    // the query has no placeholders (Error::InvalidParameterCount).
    let fixture = Fixture::new("planet_empty");
    let manager = fixture.manager();
    assert!(manager.get_planet(vec![]).is_err());
}

#[test]
fn moon_with_empty_filter_currently_fails() {
    // Documents the current behavior: an rarray parameter is passed although
    // the query has no placeholders (Error::InvalidParameterCount).
    let fixture = Fixture::new("moon_empty");
    let manager = fixture.manager();
    assert!(manager.get_moon(vec![]).is_err());
}

#[test]
fn moon_filtered_by_planet_currently_returns_nothing() {
    // Documents the current behavior: the query binds the carray pointer to a
    // scalar comparison (`planetId = ?`), which never matches any row.
    let fixture = Fixture::new("moon_filtered");
    let manager = fixture.manager();
    let moons = manager.get_moon(vec![40000001]).unwrap();
    assert!(moons.is_empty());
}

// -------------------------------------------------------------------------
// Abstract map
// -------------------------------------------------------------------------

#[test]
fn abstract_systems_without_filter_returns_all() {
    let fixture = Fixture::new("abstract_all");
    let manager = fixture.manager();
    let points = manager.get_abstract_systems(vec![]).unwrap();

    assert_eq!(points.len(), 3);
    // coordinates are divided by the factor (no inversion on the abstract map)
    assert_eq!(points[&30000001].raw_point.components, [0.1, 0.2]);
    assert_eq!(points[&30000002].raw_point.components, [0.3, 0.4]);
    assert_eq!(points[&30000003].raw_point.components, [0.5, 0.6]);
}

#[test]
fn abstract_systems_filtered_by_region() {
    let fixture = Fixture::new("abstract_by_region");
    let manager = fixture.manager();

    let points = manager.get_abstract_systems(vec![10000001]).unwrap();
    assert_eq!(points.len(), 2);
    assert!(points.contains_key(&30000001));
    assert!(points.contains_key(&30000002));

    let points = manager.get_abstract_systems(vec![10000002]).unwrap();
    assert_eq!(points.len(), 1);
    assert!(points.contains_key(&30000003));
}

#[test]
fn abstract_system_connections_fill_names_and_connections() {
    let fixture = Fixture::new("abstract_sys_conn");
    let manager = fixture.manager();
    let points = manager.get_abstract_systems(vec![]).unwrap();
    let points = manager
        .get_abstract_system_connections(points, vec![])
        .unwrap();

    assert_eq!(points[&30000001].get_name(), "Sys One");
    assert_eq!(points[&30000001].connections, vec!["conn-1-2"]);
    assert_eq!(points[&30000002].connections, vec!["conn-1-2", "conn-2-3"]);
    assert_eq!(points[&30000003].connections, vec!["conn-2-3"]);
}

#[test]
fn abstract_system_connections_respect_region_filter() {
    let fixture = Fixture::new("abstract_sys_conn_region");
    let manager = fixture.manager();
    let points = manager.get_abstract_systems(vec![]).unwrap();
    let points = manager
        .get_abstract_system_connections(points, vec![10000001])
        .unwrap();

    // Only abstract systems inside Region Alpha are updated
    assert_eq!(points[&30000001].get_name(), "Sys One");
    assert_eq!(points[&30000002].get_name(), "Sys Two");
    assert_eq!(points[&30000003].get_name(), "");
    assert!(points[&30000003].connections.is_empty());
}

#[test]
fn abstract_connections_without_filter_returns_all_lines() {
    let fixture = Fixture::new("abstract_conn_all");
    let manager = fixture.manager();
    let lines = manager.get_abstract_connections(vec![]).unwrap();

    assert_eq!(lines.len(), 2);
    let line = &lines["conn-1-2"];
    assert_eq!(line.id, Some(String::from("conn-1-2")));
    assert_eq!(line.raw_line.points[0].components, [0.1, 0.2]);
    assert_eq!(line.raw_line.points[1].components, [0.3, 0.4]);
    let line = &lines["conn-2-3"];
    assert_eq!(line.raw_line.points[0].components, [0.3, 0.4]);
    assert_eq!(line.raw_line.points[1].components, [0.5, 0.6]);
}

#[test]
fn abstract_connections_filtered_by_region_requires_both_ends_inside() {
    let fixture = Fixture::new("abstract_conn_region");
    let manager = fixture.manager();
    let lines = manager.get_abstract_connections(vec![10000001]).unwrap();

    // conn-2-3 spans Region Alpha and Region Beta, so it is excluded
    assert_eq!(lines.len(), 1);
    assert!(lines.contains_key("conn-1-2"));
}

// -------------------------------------------------------------------------
// get_region_coordinates
// -------------------------------------------------------------------------

#[test]
fn region_coordinates_currently_fails() {
    // Documents the current behavior: the query contains a typo
    // (`AX(reg.max_x)` instead of `MAX(reg.max_x)`) so SQLite rejects it.
    let fixture = Fixture::new("region_coordinates");
    let manager = fixture.manager();
    assert!(manager.get_region_coordinates().is_err());
}
