//! Turns a [`UniverseSnapshot`] into the `egui-map` widget, with the same
//! settings the native universe map uses (see `UniversePane` in `telescope`).

use eframe::egui::{Color32, FontFamily, FontId, Pos2};
use egui_map::map::{
    Map,
    objects::{MapPoint, MapSegment, MapSettings, RegionLabel, VisibilitySetting},
};
use std::collections::HashMap;
use universe_snapshot::UniverseSnapshot;

/// Builds the map, or `None` when the snapshot has no systems: `egui-map` panics
/// when it draws a map without points.
pub fn build_map(snapshot: &UniverseSnapshot) -> Option<Map> {
    if snapshot.systems.is_empty() {
        return None;
    }
    let mut map = Map::new();

    let points: HashMap<usize, MapPoint> = snapshot
        .systems
        .iter()
        .map(|system| {
            let mut point = MapPoint::new(system.id, system.pos);
            if let Some(name) = &system.name {
                point.set_name(name.clone());
            }
            point.connections = system.connections.clone();
            // A missing or malformed color keeps the map's default node color.
            point.color = system
                .color
                .as_deref()
                .and_then(|hex| Color32::from_hex(hex).ok());
            (system.id, point)
        })
        .collect();
    map.add_hashmap_points(points);

    let lines: HashMap<(usize, usize), MapSegment> = snapshot
        .connections
        .iter()
        .map(|line| (line.id, MapSegment::new(line.id, line.from, line.to)))
        .collect();
    map.add_hashmap_lines(lines);

    let labels: Vec<RegionLabel> = snapshot
        .regions
        .iter()
        .map(|region| {
            let mut label = RegionLabel::new();
            label.text = region.name.clone();
            label.center = Pos2::new(region.center[0], region.center[1]);
            label
        })
        .collect();
    map.add_region_labels(labels);

    map.settings = MapSettings::default();
    map.settings.region_label_alpha = 0.25;
    map.settings.style.region_label_font = FontId::new(96.0, FontFamily::Proportional);
    map.settings.node_text_visibility = VisibilitySetting::Hover;
    Some(map)
}
