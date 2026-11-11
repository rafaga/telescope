//! The wormholes EvE-Scout knows, as the links of the layer that `sde` keeps
//! next to the universe.
//!
//! `eve-scout` does not know `sde` (and the other way round), so this is where
//! a [`Signature`] becomes a [`Link`]. The layer is what the maps and the
//! route planner ask for the neighbours of a system that include the
//! wormholes ([`LinkLayer::neighbours`], [`sde::gate_graph::GateGraph::with_links`]);
//! the universe itself is not changed.

use super::TelescopeApp;
use super::system_header::SystemHeader;
use chrono::{DateTime, TimeDelta, Utc};
use egui::Color32;
use egui_map::map::Map;
use egui_map::map::objects::{MapPoint, MapSegment};
use eve_scout::{Signature, SignatureType};
use sde::links::{Link, LinkLayer, LinkSource, Wormhole};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod layout;

use layout::{Gate, MarkerLayout, Placed};

pub(crate) use layout::Method;

/// The hubs of the wormholes: Thera and Turnur.
pub(crate) struct Hub;

impl Hub {
    /// The region of Thera (G-R00031), where it is alone. Turnur is in a region
    /// of k-space and needs nothing more.
    pub(crate) const THERA_REGION: usize = 11_000_031;

    /// The system of Thera. It has no place on the map of the universe, so
    /// the wormholes that leave from it are drawn next to the systems they
    /// lead to.
    pub(crate) const THERA_SYSTEM: usize = 31_000_005;

    /// Whether the region gets a map: those of k-space and the one of Thera.
    /// The other regions of wormhole space and the abyssal ones have none.
    pub(crate) fn has_map(region_id: usize) -> bool {
        region_id < 11_000_000 || region_id == Self::THERA_REGION
    }

    /// The name of the map of a region: Thera is known by its system, not by
    /// the name of its region.
    pub(crate) fn region_title(region_id: usize, name: &str) -> String {
        if region_id == Self::THERA_REGION {
            format!("Thera ({name})")
        } else {
            name.to_owned()
        }
    }
}

/// The wormholes of the hubs (Thera and Turnur) that the scouts have found,
/// kept as a layer of links.
#[derive(Debug, Default)]
pub(crate) struct WormholeLinks {
    layer: LinkLayer,
}

impl WormholeLinks {
    /// The link a signature stands for, from the hub to the system it leads
    /// to. `None` while it is not a connection between two systems: it is not
    /// a wormhole, the scouts have not found the other side yet, or a system
    /// id does not fit what `sde` uses.
    pub(crate) fn link_of(signature: &Signature) -> Option<Link> {
        if signature.signature_type != SignatureType::Wormhole {
            return None;
        }
        let origin = u32::try_from(signature.out_system_id).ok()?;
        let destination = u32::try_from(signature.in_system_id?).ok()?;
        Some(Link::wormhole(
            origin,
            destination,
            Wormhole {
                out_signature: signature.out_signature.clone(),
                in_signature: signature.in_signature.clone(),
                expires_at: Some(signature.expires_at),
            },
        ))
    }

    /// Replaces every link with the ones `signatures` stand for: the
    /// signatures are read again as a whole each time, so what is not in them
    /// anymore has collapsed.
    pub(crate) fn replace(&mut self, signatures: &[Signature]) {
        self.layer
            .replace(signatures.iter().filter_map(Self::link_of).collect());
    }

    /// The layer, to hand to whoever plans a route or draws a map.
    pub(crate) fn layer(&self) -> &LinkLayer {
        &self.layer
    }
}

/// What the housekeeping thread, which asks EvE-Scout for the wormholes, and
/// the UI, which draws them and plans routes through them, share.
#[derive(Debug)]
pub(crate) struct WormholeShared {
    links: RwLock<WormholeLinks>,
    /// The setting (`[eve_scout] enabled`): off never asks.
    enabled: AtomicBool,
    /// When a map that shows the wormholes was last drawn, in milliseconds
    /// since [`Self::started`]; `NEVER` if none was.
    last_seen: AtomicU64,
    /// Counts the times the wormholes were replaced: a map draws them again
    /// when it changes.
    version: AtomicU64,
    started: Instant,
    /// How long after a map was last drawn the wormholes are still wanted.
    /// The UI draws only when something moves, so a map that is open but
    /// still goes without being drawn for a while.
    grace: Duration,
}

impl Default for WormholeShared {
    /// On, as the setting is by default, and not wanted by any map yet.
    fn default() -> Self {
        Self {
            links: RwLock::new(WormholeLinks::default()),
            enabled: AtomicBool::new(true),
            last_seen: AtomicU64::new(Self::NEVER),
            version: AtomicU64::new(0),
            started: Instant::now(),
            grace: Duration::from_secs(2 * 60),
        }
    }
}

impl WormholeShared {
    /// `last_seen` while no map has been drawn.
    const NEVER: u64 = u64::MAX;

    fn millis(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX - 1)
    }

    /// Follows the setting.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// Whether the setting lets the wormholes be asked for and shown.
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// A map that shows the wormholes is being drawn: they are wanted for a
    /// while.
    pub(crate) fn touch(&self) {
        self.last_seen.store(self.millis(), Ordering::Release);
    }

    /// Whether the wormholes are to be asked for now: the setting is on and a
    /// map that shows them was drawn not long ago.
    pub(crate) fn is_wanted(&self) -> bool {
        self.is_wanted_at(self.millis())
    }

    fn is_wanted_at(&self, now: u64) -> bool {
        let seen = self.last_seen.load(Ordering::Acquire);
        self.is_enabled()
            && seen != Self::NEVER
            && now.saturating_sub(seen) <= self.grace.as_millis() as u64
    }

    /// How many times the wormholes were replaced.
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Replaces the wormholes with the ones `signatures` stand for.
    pub(crate) fn replace(&self, signatures: &[Signature]) {
        self.links
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(signatures);
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// A copy of the layer as it is now.
    pub(crate) fn layer(&self) -> LinkLayer {
        self.links
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .layer()
            .clone()
    }
}

/// How the wormholes look on a region map and where they go. Not
/// user-editable.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WormholeMarkerStyle {
    /// The ids of the marker nodes start here, above any id of the SDE.
    pub(crate) id_base: usize,
    /// The border of a marker and the line to it.
    pub(crate) accent: Color32,
    /// The same for a wormhole close to collapsing.
    pub(crate) expiring: Color32,
    /// How much life left makes a wormhole close to collapsing.
    pub(crate) expiring_within: TimeDelta,
    /// Space kept free around a marker, in map units: between two markers, and
    /// between a marker and the nodes of the map. About two thirds of the
    /// height of a node, enough to tell the markers of a crowded hub (Turnur
    /// has many) and their lines apart.
    pub(crate) gap: f64,
    /// The space the box of the text of a line takes around the text, in
    /// points (what `egui-map` pads it with: 4 on each side and 1 above and
    /// below).
    pub(crate) label_padding: [f64; 2],
    /// The zoom the text of the lines is given room for. The text follows the
    /// zoom like the nodes do (`TextScaling::Zoom`, set on the maps that
    /// show the markers), so the room it takes in map units is the same at
    /// every zoom, the one it has at zoom `1`: that is the value to use. A
    /// lower one reserves more room (what a text that keeps its size in
    /// points, `TextScaling::Screen`, needs at that zoom). The room is
    /// worked out for this one whatever the zoom of the map, so zooming never
    /// moves the markers.
    pub(crate) label_zoom: f64,
    /// The widest text a signature can make; the room is never less than the
    /// one it takes (a wide letter and the digits, on both sides).
    pub(crate) label_reference: &'static str,
    /// Where the text goes along its line, from `0` at the system to `1` at
    /// the marker. The lines of a hub meet at it, so their texts crowd one
    /// another near it; toward the marker they have room.
    pub(crate) label_at: f64,
    /// The room reserved for the text grows in steps of this many map units,
    /// so a text a little wider does not move the markers.
    pub(crate) label_step: f64,
    /// How the places around a system are looked for: in the free arcs
    /// between the nodes of the map, or among a regular set of directions, or
    /// the first near the nodes and the second away from them.
    pub(crate) method: Method,
    /// The least time between two refreshes of the markers when nothing was
    /// replaced (a wormhole expires, or gets close to it).
    pub(crate) refresh_every: Duration,
    /// Only the wormholes that have this system at one end get a marker; all
    /// of them when `None`.
    pub(crate) hub: Option<usize>,
    /// The least distance from a system to its marker, in map units; `0` puts
    /// the marker as close as the nodes around let it.
    pub(crate) min_distance: f64,
    /// Whether the line of a marker carries the signatures. Off on a map where
    /// the text would not fit between the systems.
    pub(crate) labels: bool,
}

impl Default for WormholeMarkerStyle {
    fn default() -> Self {
        Self {
            id_base: 1 << 40,
            accent: SystemHeader::WORMHOLE_COLOR,
            expiring: Color32::from_rgb(0xe5, 0x73, 0x4f),
            expiring_within: TimeDelta::hours(4),
            gap: 24.0,
            label_padding: [8.0, 2.0],
            label_zoom: 1.0,
            label_reference: "WWW-000 → WWW-000",
            label_step: 16.0,
            method: Method::default(),
            label_at: 0.7,
            refresh_every: Duration::from_secs(30),
            hub: None,
            min_distance: 0.0,
            labels: true,
        }
    }
}

impl WormholeMarkerStyle {
    /// The wormholes of Thera on the map of the universe: a marker is a node
    /// the size of a system, close to it, joined by a short dashed line with
    /// no text. The systems of that map are a few dozen units apart, so the
    /// gap is small and nothing is reserved for text.
    pub(crate) fn universe() -> Self {
        Self {
            gap: 6.0,
            label_padding: [0.5, 0.5],
            label_reference: "",
            label_step: 1.0,
            hub: Some(Hub::THERA_SYSTEM),
            // Three times the distance the markers got with no minimum on the
            // real universe (25.5 units: the node, the gap and the lines of the
            // gates decide it), so each one reads as a node of its own.
            min_distance: 76.5,
            labels: false,
            ..Self::default()
        }
    }
}

/// The size of the box of a node of a region map, in map units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NodeSize {
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// How the text of the lines is drawn.
#[derive(Clone, Copy)]
pub(crate) struct LabelSpace<'a> {
    /// The size in points of a text, as the map draws it.
    pub(crate) measure: &'a dyn Fn(&str) -> [f32; 2],
}

/// A wormhole as drawn on a region map: a node next to the system it leaves
/// from, named after the system on the other side, and a dashed line to the
/// system with the signatures on it. The node is only a drawing: the system on
/// the other side is the one with its real id, [`Self::other`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WormholeMarker {
    /// The id of the node.
    pub(crate) id: usize,
    /// The system of the map the wormhole leaves from.
    pub(crate) system: usize,
    /// The system on the other side, with its real id.
    pub(crate) other: usize,
    /// Its name.
    pub(crate) name: String,
    /// Where the node goes.
    pub(crate) position: [f32; 2],
    /// Where the system is.
    pub(crate) anchor: [f32; 2],
    /// The signatures, the one of the hub first.
    pub(crate) label: String,
    /// Whether it is close to collapsing.
    pub(crate) expiring: bool,
}

impl WormholeMarker {
    /// The node.
    fn point(&self, style: &WormholeMarkerStyle) -> MapPoint {
        let mut point = MapPoint::new(self.id, self.position);
        point.set_name(self.name.clone());
        point.color = Some(self.color(style));
        point.connections = vec![(self.system, self.id)];
        point
    }

    /// The line from the system to the node.
    fn segment(&self, style: &WormholeMarkerStyle) -> MapSegment {
        let mut segment = MapSegment::new((self.system, self.id), self.anchor, self.position);
        segment = segment.dashed().with_label_at(style.label_at as f32);
        if !self.label.is_empty() {
            segment.label = Some(self.label.clone());
        }
        segment.color = Some(self.color(style));
        segment
    }

    fn color(&self, style: &WormholeMarkerStyle) -> Color32 {
        if self.expiring {
            style.expiring
        } else {
            style.accent
        }
    }
}

/// The wormholes drawn on one region map.
#[derive(Debug)]
pub(crate) struct WormholeLayer {
    style: WormholeMarkerStyle,
    /// What is on the map now.
    shown: Vec<WormholeMarker>,
    /// The lines between systems already on the map, as the ids of the two
    /// systems: the texts and nodes of the markers are kept off them.
    gates: Vec<(usize, usize)>,
    /// The version of the wormholes that was drawn, and when.
    drawn: Option<(u64, Instant)>,
    /// Where each marker was put and its id, for as long as its wormhole
    /// lasts, so that it stays there when others come or go.
    remembered: HashMap<MarkerKey, Remembered>,
    /// What [`Self::remembered`] was worked out for; when it changes the
    /// places are worked out again.
    surroundings: Option<Surroundings>,
    /// The id the next new marker gets. Ids are not reused while the layer
    /// lives, so a marker never takes the id of one that was on the map.
    next_id: usize,
    /// The room reserved for the text of a line, in map units: the longest
    /// one seen, which only grows (until the map or the nodes change), so the
    /// markers are not moved when a shorter text comes.
    reserved: [f64; 2],
}

/// Which wormhole a marker stands for: the system of the map it leaves from,
/// the system on the other side and the signature at the first end.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MarkerKey {
    system: usize,
    other: usize,
    out_signature: String,
}

/// Where a marker was put, and the id it was given.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Remembered {
    id: usize,
    position: [f64; 2],
}

/// What decides where the markers can go: the systems of the map and the lines
/// between them, the size of a node and the gap. Two markers placed against different surroundings
/// can't be told to keep their places.
#[derive(Debug, Clone, PartialEq)]
struct Surroundings {
    /// The systems, by id.
    systems: Vec<(usize, [f32; 2])>,
    /// The lines between systems, by the ids of their ends.
    gates: Vec<(usize, usize)>,
    width: f64,
    height: f64,
    gap: f64,
}

impl Surroundings {
    fn new(
        systems: &HashMap<usize, [f32; 2]>,
        gates: &[(usize, usize)],
        node: NodeSize,
        gap: f64,
    ) -> Self {
        let mut systems: Vec<(usize, [f32; 2])> =
            systems.iter().map(|(id, at)| (*id, *at)).collect();
        systems.sort_by_key(|(id, _)| *id);
        Self {
            systems,
            gates: gates.to_vec(),
            width: node.width,
            height: node.height,
            gap,
        }
    }

    /// The lines between the systems, by their ends, for the ones whose two
    /// ends are on the map.
    fn lines(&self) -> Vec<Gate> {
        let at: HashMap<usize, [f64; 2]> = self
            .systems
            .iter()
            .map(|(id, at)| (*id, [f64::from(at[0]), f64::from(at[1])]))
            .collect();
        self.gates
            .iter()
            .filter_map(|(from, to)| Some(Gate::new(*at.get(from)?, *at.get(to)?)))
            .collect()
    }

    /// The positions of the systems, in the order of their ids.
    fn positions(&self) -> Vec<[f64; 2]> {
        self.systems
            .iter()
            .map(|(_, at)| [f64::from(at[0]), f64::from(at[1])])
            .collect()
    }
}

impl WormholeLayer {
    pub(crate) fn new(style: WormholeMarkerStyle) -> Self {
        Self {
            next_id: style.id_base,
            reserved: [0.0, 0.0],
            style,
            shown: Vec::new(),
            gates: Vec::new(),
            drawn: None,
            remembered: HashMap::new(),
            surroundings: None,
        }
    }

    /// The text on the line of a wormhole: the signature at each end, or the
    /// one at the first end while the other is not known.
    fn label_of(wormhole: &Wormhole) -> String {
        match &wormhole.in_signature {
            Some(inside) => format!("{} → {inside}", wormhole.out_signature),
            None => wormhole.out_signature.clone(),
        }
    }

    /// Tells the layer which systems of the map are joined by a line (the ids
    /// of the two ends of each), so that the texts and nodes of the markers
    /// stay off them. Worked out again at the next refresh, when it changes.
    pub(crate) fn set_gates(&mut self, gates: impl IntoIterator<Item = (usize, usize)>) {
        let mut gates: Vec<(usize, usize)> = gates.into_iter().collect();
        gates.sort_unstable();
        gates.dedup();
        if gates != self.gates {
            self.gates = gates;
            self.drawn = None;
        }
    }

    /// Whether the markers are to be worked out again: the wormholes were
    /// replaced since they were drawn, or it was a while ago (a wormhole
    /// expires with time). The zoom does not make it due: the room for the
    /// texts is worked out for a fixed one, so the markers stay where they
    /// are. Cheap: a few comparisons.
    pub(crate) fn is_due(&self, version: u64, now: Instant) -> bool {
        match self.drawn {
            Some((drawn, at)) => {
                drawn != version || now.saturating_duration_since(at) >= self.style.refresh_every
            }
            None => true,
        }
    }

    /// The markers for the active links of `links` at `now`, next to the
    /// systems of the map (`systems`, with their positions). Every link that
    /// leaves from a system of the map gets one, named by `names` after the
    /// system on the other side, and put where it touches no node of the map
    /// and no other marker.
    ///
    /// A marker keeps its place and its id for as long as its wormhole is
    /// active, so the ones that stay do not move when others come or go. A
    /// new marker goes where the order of the links (system, system on the
    /// other side, signature) puts it among the new ones, away from
    /// everything already there. For the same wormholes, and nothing
    /// remembered, the answer is always the same. When the map, the size of
    /// the nodes or the gap change, the places are worked out again.
    ///
    /// The text of a line takes room too (see [`layout`]): the box of the
    /// longest text, or of the reference if it is longer, at the zoom it is
    /// given, is kept free at the line. That room only grows: it is worked
    /// out again, and the markers with it, only when a text longer than the
    /// reference comes, never when it would shrink, and never with the zoom.
    pub(crate) fn markers(
        &mut self,
        links: &LinkLayer,
        now: DateTime<Utc>,
        systems: &HashMap<usize, [f32; 2]>,
        names: &dyn Fn(usize) -> Option<String>,
        node: NodeSize,
        labels: LabelSpace<'_>,
    ) -> Vec<WormholeMarker> {
        // (system on the map, system on the other side, wormhole)
        let mut ends: Vec<(usize, usize, &Wormhole)> = Vec::new();
        for link in links.active(now) {
            let LinkSource::Wormhole(wormhole) = &link.source else {
                continue;
            };
            let (origin, destination) = (link.origin as usize, link.destination as usize);
            if self
                .style
                .hub
                .is_some_and(|hub| origin != hub && destination != hub)
            {
                continue;
            }
            if systems.contains_key(&origin) {
                ends.push((origin, destination, wormhole));
            }
            if systems.contains_key(&destination) {
                ends.push((destination, origin, wormhole));
            }
        }
        ends.sort_by(|a, b| (a.0, a.1, &a.2.out_signature).cmp(&(b.0, b.1, &b.2.out_signature)));
        // The same wormhole twice would share one place: keep the first.
        ends.dedup_by(|a, b| (a.0, a.1, &a.2.out_signature) == (b.0, b.1, &b.2.out_signature));
        let keys: Vec<MarkerKey> = ends
            .iter()
            .map(|(system, other, wormhole)| MarkerKey {
                system: *system,
                other: *other,
                out_signature: wormhole.out_signature.clone(),
            })
            .collect();

        let texts: Vec<String> = ends
            .iter()
            .map(|(_, _, wormhole)| {
                if self.style.labels {
                    Self::label_of(wormhole)
                } else {
                    String::new()
                }
            })
            .collect();

        // The room the texts need, in map units: their size in points over
        // the zoom they are given room for ([`WormholeMarkerStyle::label_zoom`],
        // fixed, so zooming moves nothing), up to a whole step. At least the
        // room of the widest text a signature can make
        // ([`WormholeMarkerStyle::label_reference`]), so a text that is wider
        // than the ones before, by a few points, does not move the markers.
        let zoom = self.style.label_zoom.max(f64::EPSILON);
        let mut needed = [0.0_f64; 2];
        for text in texts
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.style.label_reference))
        {
            let size = (labels.measure)(text);
            for axis in 0..2 {
                let points = f64::from(size[axis]) + self.style.label_padding[axis];
                let step = self.style.label_step.max(1.0);
                needed[axis] = needed[axis].max((points / zoom / step).ceil() * step);
            }
        }

        // What was worked out for other surroundings is not valid anymore, and
        // so is what was worked out for less room for the texts. What belongs
        // to a wormhole that is gone is forgotten.
        let surroundings = Surroundings::new(systems, &self.gates, node, self.style.gap);
        if self.surroundings.as_ref() != Some(&surroundings) {
            self.remembered.clear();
            self.reserved = needed;
            self.surroundings = Some(surroundings);
        } else if needed[0] > self.reserved[0] || needed[1] > self.reserved[1] {
            self.remembered.clear();
            self.reserved = [
                self.reserved[0].max(needed[0]),
                self.reserved[1].max(needed[1]),
            ];
        }
        let wanted: HashSet<&MarkerKey> = keys.iter().collect();
        self.remembered.retain(|key, _| wanted.contains(key));

        let fresh: Vec<usize> = (0..keys.len())
            .filter(|index| !self.remembered.contains_key(&keys[*index]))
            .collect();
        if !fresh.is_empty() {
            let layout =
                MarkerLayout::new([node.width, node.height], self.reserved, self.style.gap)
                    .with_label_at(self.style.label_at)
                    .with_min_distance(self.style.min_distance)
                    .with_method(self.style.method);
            // The nodes of the map, and the markers that already have their
            // place: they are in the way of the new ones, and nothing of that
            // is moved.
            let nodes: Vec<[f64; 2]> = self
                .surroundings
                .as_ref()
                .map(Surroundings::positions)
                .unwrap_or_default();
            let lines: Vec<Gate> = self
                .surroundings
                .as_ref()
                .map(Surroundings::lines)
                .unwrap_or_default();
            let kept: Vec<Placed> = (0..keys.len())
                .filter_map(|index| {
                    let remembered = self.remembered.get(&keys[index])?;
                    let at = systems[&ends[index].0];
                    Some(Placed {
                        anchor: [f64::from(at[0]), f64::from(at[1])],
                        spot: remembered.position,
                    })
                })
                .collect();
            let anchors: Vec<[f64; 2]> = fresh
                .iter()
                .map(|index| {
                    let at = systems[&ends[*index].0];
                    [f64::from(at[0]), f64::from(at[1])]
                })
                .collect();
            for (index, spot) in fresh
                .into_iter()
                .zip(layout.place_all_among(&anchors, &nodes, &kept, &lines))
            {
                // No free place: the marker is left out, and tried again the
                // next time the markers are worked out.
                if let Some(position) = spot {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.remembered
                        .insert(keys[index].clone(), Remembered { id, position });
                }
            }
        }

        ends.iter()
            .zip(&keys)
            .zip(texts)
            .filter_map(|(((system, other, wormhole), key), label)| {
                let kept = self.remembered.get(key)?;
                Some(WormholeMarker {
                    id: kept.id,
                    system: *system,
                    other: *other,
                    name: names(*other).unwrap_or_else(|| other.to_string()),
                    position: [kept.position[0] as f32, kept.position[1] as f32],
                    anchor: systems[system],
                    label,
                    expiring: wormhole
                        .expires_at
                        .is_some_and(|at| at - now < self.style.expiring_within),
                })
            })
            .collect()
    }

    /// Puts `markers` on `map` in place of the ones that were there. Nothing
    /// is touched when they are the same.
    pub(crate) fn show(
        &mut self,
        map: &mut Map,
        markers: Vec<WormholeMarker>,
        version: u64,
        now: Instant,
    ) {
        self.drawn = Some((version, now));
        if self.shown == markers {
            return;
        }
        self.clear_markers(map);
        for marker in &markers {
            map.insert_point(marker.point(&self.style));
            map.insert_line(marker.segment(&self.style));
        }
        self.shown = markers;
    }

    /// The nodes of the markers on the map and the systems they stand for:
    /// `(node id, system id)`.
    pub(crate) fn aliases(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.shown.iter().map(|marker| (marker.id, marker.other))
    }

    /// Takes the markers off `map` and forgets what was drawn.
    pub(crate) fn clear(&mut self, map: &mut Map) {
        self.clear_markers(map);
        self.drawn = None;
    }

    fn clear_markers(&mut self, map: &mut Map) {
        for marker in self.shown.drain(..) {
            // The line goes with its node.
            map.remove_point(marker.id);
        }
    }
}

impl TelescopeApp {
    /// Hands the housekeeping thread the setting of EvE-Scout. Cheap and
    /// idempotent, like [`Self::sync_alarm_shared`], which it runs next to.
    pub(crate) fn sync_wormholes(&self) {
        self.wormholes
            .set_enabled(self.settings.get_eve_scout().enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::layout::Block;
    use super::*;
    use chrono::{DateTime, TimeDelta, Utc};
    use serde_json::{Value, json};

    const THERA: u32 = 31_000_005;
    const TURNUR: u32 = 30_002_086;

    /// A signature as the live API sends it, with `changes` over it.
    fn signature(changes: Value) -> Signature {
        let mut base = json!({
            "id": "77735",
            "created_at": "2026-11-04T21:17:26.000Z",
            "completed": true,
            "expires_at": "2026-11-05T22:17:26.000Z",
            "signature_type": "wormhole",
            "out_system_id": 31_000_005,
            "out_system_name": "Thera",
            "out_signature": "RRW-782",
            "in_system_id": 30_000_142,
            "in_system_name": "Jita",
            "in_signature": "LGN-154"
        });
        for (name, value) in changes.as_object().unwrap() {
            base[name] = value.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn expires_at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-05T22:17:26Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_scanned_wormhole_is_a_link_from_the_hub_to_the_system() {
        let link = WormholeLinks::link_of(&signature(json!({}))).unwrap();
        assert_eq!(link.origin, THERA);
        assert_eq!(link.destination, 30_000_142);
        let sde::links::LinkSource::Wormhole(wormhole) = &link.source else {
            panic!("a wormhole signature must make a wormhole link");
        };
        assert_eq!(wormhole.out_signature, "RRW-782");
        assert_eq!(wormhole.in_signature.as_deref(), Some("LGN-154"));
        assert_eq!(wormhole.expires_at, Some(expires_at()));
    }

    #[test]
    fn a_signature_without_the_other_side_is_not_a_link_yet() {
        let scanning = signature(json!({ "in_system_id": null, "in_signature": null }));
        assert!(WormholeLinks::link_of(&scanning).is_none());
    }

    #[test]
    fn what_is_not_a_wormhole_is_not_a_link() {
        assert!(WormholeLinks::link_of(&signature(json!({ "signature_type": "gas" }))).is_none());
    }

    #[test]
    fn an_id_that_does_not_fit_is_not_a_link() {
        let negative = signature(json!({ "in_system_id": -1 }));
        let huge = signature(json!({ "out_system_id": 5_000_000_000_i64 }));
        assert!(WormholeLinks::link_of(&negative).is_none());
        assert!(WormholeLinks::link_of(&huge).is_none());
    }

    #[test]
    fn thera_has_a_map_titled_with_its_system() {
        assert!(Hub::has_map(10_000_002));
        assert!(Hub::has_map(Hub::THERA_REGION));
        assert!(!Hub::has_map(11_000_001));
        assert!(!Hub::has_map(12_000_001));
        assert_eq!(
            Hub::region_title(Hub::THERA_REGION, "G-R00031"),
            "Thera (G-R00031)"
        );
        assert_eq!(Hub::region_title(10_000_002, "The Forge"), "The Forge");
    }

    #[test]
    fn turnur_is_a_hub_like_thera() {
        let from_turnur = signature(json!({ "out_system_id": 30_002_086 }));
        assert_eq!(WormholeLinks::link_of(&from_turnur).unwrap().origin, TURNUR);
    }

    #[test]
    fn the_links_are_replaced_as_a_whole() {
        let mut links = WormholeLinks::default();
        links.replace(&[
            signature(json!({})),
            signature(json!({ "in_system_id": null })),
            signature(json!({ "in_system_id": 30_000_144 })),
        ]);
        assert_eq!(links.layer().len(), 2);
        links.replace(&[signature(json!({ "in_system_id": 30_000_139 }))]);
        assert_eq!(links.layer().len(), 1);
        assert_eq!(links.layer().links()[0].destination, 30_000_139);
        links.replace(&[]);
        assert!(links.layer().is_empty());
    }

    #[test]
    fn the_wormholes_are_wanted_only_when_the_setting_is_on_and_a_map_shows_them() {
        let shared = WormholeShared::default();
        assert!(!shared.is_wanted());
        shared.touch();
        assert!(shared.is_wanted());
        shared.set_enabled(false);
        assert!(!shared.is_wanted());
        shared.set_enabled(true);
        assert!(shared.is_wanted());
    }

    #[test]
    fn the_wormholes_stop_being_wanted_when_no_map_has_been_drawn_for_a_while() {
        let shared = WormholeShared::default();
        shared.touch();
        let seen = shared.last_seen.load(Ordering::Acquire);
        let grace = shared.grace.as_millis() as u64;
        assert!(shared.is_wanted_at(seen + grace));
        assert!(!shared.is_wanted_at(seen + grace + 1));
    }

    #[test]
    fn replacing_the_wormholes_changes_the_version() {
        let shared = WormholeShared::default();
        let before = shared.version();
        shared.replace(&[]);
        assert_ne!(shared.version(), before);
    }

    fn node() -> NodeSize {
        NodeSize {
            width: 90.0,
            height: 35.0,
        }
    }

    /// A region of two systems far enough apart not to touch.
    fn region() -> HashMap<usize, [f32; 2]> {
        HashMap::from([(30_000_142, [0.0, 0.0]), (30_000_144, [300.0, 0.0])])
    }

    fn layer() -> WormholeLayer {
        WormholeLayer::new(WormholeMarkerStyle::default())
    }

    fn names(id: usize) -> Option<String> {
        (id == THERA as usize).then(|| String::from("Thera"))
    }

    fn links(signatures: &[Signature]) -> LinkLayer {
        let mut links = WormholeLinks::default();
        links.replace(signatures);
        links.layer().clone()
    }

    fn now() -> DateTime<Utc> {
        expires_at() - TimeDelta::hours(10)
    }

    #[test]
    fn a_wormhole_leaving_from_a_system_of_the_map_has_a_marker_named_after_the_other_side() {
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        let markers = layer().markers(
            &links(&[from_thera]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers.len(), 1);
        let marker = &markers[0];
        assert_eq!(marker.system, 30_000_142);
        assert_eq!(marker.other, THERA as usize);
        assert_eq!(marker.name, "Thera");
        assert_eq!(marker.label, "RRW-782 → LGN-154");
        assert!(!marker.expiring);
        assert!(marker.id >= WormholeMarkerStyle::default().id_base);
        assert_eq!(marker.anchor, [0.0, 0.0]);
    }

    #[test]
    fn on_the_universe_only_the_wormholes_of_thera_get_a_marker_and_it_has_no_text() {
        let mut layer = WormholeLayer::new(WormholeMarkerStyle::universe());
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        // A wormhole between two systems of the map, none of them Thera.
        let between = signature(json!({
            "out_system_id": 30_000_144,
            "in_system_id": 30_000_142,
            "out_signature": "AAA-111"
        }));
        let size = NodeSize {
            width: 8.0,
            height: 8.0,
        };
        let markers = layer.markers(
            &links(&[from_thera, between]),
            now(),
            &region(),
            &names,
            size,
            LabelSpace {
                measure: &|_| [0.0, 0.0],
            },
        );
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].system, 30_000_142);
        assert_eq!(markers[0].other, THERA as usize);
        assert_eq!(markers[0].name, "Thera");
        assert!(markers[0].label.is_empty());
        // The line carries no text, and the marker is close to its system.
        assert!(markers[0].segment(&layer.style).label.is_none());
        let [x, y] = markers[0].position;
        let distance = f64::from(x).hypot(f64::from(y));
        assert!(
            (76.5..90.0).contains(&distance),
            "the marker is {distance} units away"
        );
    }

    #[test]
    fn a_marker_does_not_touch_the_nodes_of_the_map() {
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        let markers = layer().markers(
            &links(&[from_thera]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        let marker = markers[0].position;
        for (_, other) in region() {
            let apart_x = (marker[0] - other[0]).abs();
            let apart_y = (marker[1] - other[1]).abs();
            // Boxes of 90 x 35 do not overlap when either side clears.
            assert!(
                apart_x >= 90.0 || apart_y >= 35.0,
                "the marker at {marker:?} touches the node at {other:?}"
            );
        }
    }

    #[test]
    fn many_wormholes_of_a_system_get_places_of_their_own() {
        let signatures: Vec<Signature> = (0..5)
            .map(|index| {
                signature(json!({
                    "in_system_id": 30_000_142,
                    "out_system_id": 31_000_000 + index,
                    "out_signature": format!("AAA-00{index}"),
                }))
            })
            .collect();
        let markers = layer().markers(
            &links(&signatures),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers.len(), 5);
        for (index, first) in markers.iter().enumerate() {
            for second in &markers[index + 1..] {
                let apart_x = (first.position[0] - second.position[0]).abs();
                let apart_y = (first.position[1] - second.position[1]).abs();
                assert!(apart_x >= 90.0 || apart_y >= 35.0);
                assert_ne!(first.id, second.id);
            }
        }
    }

    /// The wormholes of a crowded system, and a way to see where they went.
    struct Crowd;

    impl Crowd {
        /// `count` wormholes of the hub that leave from Jita, one per hub
        /// signature, in the order the markers are worked out.
        fn signatures(count: i64) -> Vec<Signature> {
            (0..count)
                .map(|index| {
                    signature(json!({
                        "in_system_id": 30_000_142,
                        "out_system_id": 31_000_000 + index,
                        "out_signature": format!("AAA-00{index}"),
                    }))
                })
                .collect()
        }

        /// A stand-in for the font: 6 points wide each character, 14 high.
        fn measure(text: &str) -> [f32; 2] {
            [text.chars().count() as f32 * 6.0, 14.0]
        }

        /// The texts of the lines.
        fn labels() -> LabelSpace<'static> {
            LabelSpace {
                measure: &Self::measure,
            }
        }

        /// The box of the text of a marker, in the room the layer reserves.
        fn text_box(marker: &WormholeMarker, reserved: [f64; 2]) -> [[f64; 2]; 2] {
            let at = WormholeMarkerStyle::default().label_at;
            let center = |axis: usize| {
                f64::from(marker.anchor[axis])
                    + (f64::from(marker.position[axis]) - f64::from(marker.anchor[axis])) * at
            };
            [[center(0), center(1)], reserved]
        }

        /// Whether two boxes (center and size) overlap.
        fn overlap(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
            (a[0][0] - b[0][0]).abs() < (a[1][0] + b[1][0]) / 2.0
                && (a[0][1] - b[0][1]).abs() < (a[1][1] + b[1][1]) / 2.0
        }

        /// Where each marker is and its id, by the system on the other side.
        fn places(markers: &[WormholeMarker]) -> HashMap<usize, ([f32; 2], usize)> {
            markers
                .iter()
                .map(|marker| (marker.other, (marker.position, marker.id)))
                .collect()
        }
    }

    #[test]
    fn the_text_of_a_marker_is_kept_off_the_lines_already_on_the_map() {
        let one = Crowd::signatures(1);
        let mut systems = region();
        let mut shown = layer();
        let free = shown.markers(
            &links(&one),
            now(),
            &systems,
            &names,
            node(),
            Crowd::labels(),
        );
        let text = Crowd::text_box(&free[0], shown.reserved);

        // A line of the map right across that text.
        systems.insert(1, [text[0][0] as f32, text[0][1] as f32 - 150.0]);
        systems.insert(2, [text[0][0] as f32, text[0][1] as f32 + 150.0]);
        let mut shown = layer();
        shown.set_gates([(1, 2)]);
        let moved = shown.markers(
            &links(&one),
            now(),
            &systems,
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(moved.len(), 1);
        assert_ne!(moved[0].position, free[0].position);
        let gate = Gate::new(systems[&1].map(f64::from), systems[&2].map(f64::from));
        let text = Crowd::text_box(&moved[0], shown.reserved);
        assert!(!gate.crosses(&Block::new(text[0], text[1])));
        assert!(!gate.crossed_by_line(
            moved[0].anchor.map(f64::from),
            moved[0].position.map(f64::from)
        ));
    }

    #[test]
    fn the_lines_of_the_map_are_worked_out_again_only_when_they_change() {
        let now = Instant::now();
        let mut shown = layer();
        shown.markers(
            &links(&Crowd::signatures(2)),
            chrono::Utc::now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        shown.show(&mut Map::new(), Vec::new(), 1, now);
        assert!(!shown.is_due(1, now));
        shown.set_gates([(30_000_142, 30_000_144)]);
        assert!(shown.is_due(1, now));
        shown.show(&mut Map::new(), Vec::new(), 1, now);
        // The same lines, in another order and twice: nothing to do.
        shown.set_gates([(30_000_142, 30_000_144), (30_000_142, 30_000_144)]);
        assert!(!shown.is_due(1, now));
    }

    #[test]
    fn the_markers_of_a_crowded_system_are_apart_by_the_gap_and_not_only_not_touching() {
        let gap = WormholeMarkerStyle::default().gap as f32;
        let markers = layer().markers(
            &links(&Crowd::signatures(8)),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers.len(), 8);
        let mut centers: Vec<[f32; 2]> = markers.iter().map(|marker| marker.position).collect();
        centers.extend(region().values().copied());
        for (index, first) in centers.iter().enumerate() {
            for second in &centers[index + 1..] {
                let apart_x = (first[0] - second[0]).abs();
                let apart_y = (first[1] - second[1]).abs();
                assert!(
                    apart_x >= 90.0 + gap - 0.01 || apart_y >= 35.0 + gap - 0.01,
                    "{first:?} and {second:?} are closer than the gap"
                );
            }
        }
    }

    #[test]
    fn a_marker_stays_where_it_was_when_one_worked_out_before_it_collapses() {
        let all = Crowd::signatures(6);
        let mut shown = layer();
        let before = Crowd::places(&shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));
        assert_eq!(before.len(), 6);

        // The first one in the order collapses: the others must not move.
        let after = Crowd::places(&shown.markers(
            &links(&all[1..]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));
        assert_eq!(after.len(), 5);
        for (other, place) in &after {
            assert_eq!(before[other], *place, "the marker of {other} moved");
        }
    }

    #[test]
    fn a_new_wormhole_does_not_move_the_markers_that_were_already_there() {
        let all = Crowd::signatures(6);
        let mut shown = layer();
        let before = Crowd::places(&shown.markers(
            &links(&all[1..]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));

        // The new one is the first in the order.
        let markers = shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        let after = Crowd::places(&markers);
        assert_eq!(after.len(), 6);
        for (other, place) in &before {
            assert_eq!(after[other], *place, "the marker of {other} moved");
        }
        // Without moving anything, the new one still found a free place.
        let gap = WormholeMarkerStyle::default().gap as f32;
        for (index, first) in markers.iter().enumerate() {
            for second in &markers[index + 1..] {
                let apart_x = (first.position[0] - second.position[0]).abs();
                let apart_y = (first.position[1] - second.position[1]).abs();
                assert!(apart_x >= 90.0 + gap - 0.01 || apart_y >= 35.0 + gap - 0.01);
            }
        }
    }

    #[test]
    fn the_default_gap_is_more_than_half_the_height_of_a_node() {
        // 6 left the markers of Turnur almost touching.
        assert!(WormholeMarkerStyle::default().gap > node().height / 2.0);
    }

    #[test]
    fn the_same_wormholes_give_the_same_markers() {
        let all = Crowd::signatures(6);
        let links = links(&all);
        let first = layer().markers(&links, now(), &region(), &names, node(), Crowd::labels());
        let second = layer().markers(&links, now(), &region(), &names, node(), Crowd::labels());
        assert_eq!(first, second);
        let mut shown = layer();
        let once = shown.markers(&links, now(), &region(), &names, node(), Crowd::labels());
        let again = shown.markers(&links, now(), &region(), &names, node(), Crowd::labels());
        assert_eq!(once, again);
    }

    #[test]
    fn the_markers_are_worked_out_again_when_the_map_or_the_nodes_change() {
        let all = Crowd::signatures(4);
        let links = links(&all);
        let mut shown = layer();
        shown.markers(&links, now(), &region(), &names, node(), Crowd::labels());

        // The same map moved far away: the markers follow their systems.
        let moved: HashMap<usize, [f32; 2]> = region()
            .into_iter()
            .map(|(id, at)| (id, [at[0] + 5000.0, at[1]]))
            .collect();
        let markers = shown.markers(&links, now(), &moved, &names, node(), Crowd::labels());
        assert_eq!(markers.len(), 4);
        for marker in &markers {
            assert_eq!(marker.anchor, [5000.0, 0.0]);
            assert!((marker.position[0] - 5000.0).abs() < 1000.0);
        }

        // Bigger nodes: no marker touches another with the new size.
        let big = NodeSize {
            width: 200.0,
            height: 80.0,
        };
        let markers = shown.markers(&links, now(), &moved, &names, big, Crowd::labels());
        for (index, first) in markers.iter().enumerate() {
            for second in &markers[index + 1..] {
                let apart_x = (first.position[0] - second.position[0]).abs();
                let apart_y = (first.position[1] - second.position[1]).abs();
                assert!(apart_x >= 200.0 || apart_y >= 80.0);
            }
        }
    }

    #[test]
    fn a_marker_keeps_its_id_while_its_wormhole_lasts() {
        let all = Crowd::signatures(4);
        let mut shown = layer();
        let before = Crowd::places(&shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));
        let after = Crowd::places(&shown.markers(
            &links(&all[2..]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));
        for (other, (_, id)) in &after {
            assert_eq!(before[other].1, *id);
        }
    }

    #[test]
    fn the_text_of_a_line_has_room_of_its_own_and_touches_nothing() {
        let mut shown = layer();
        let markers = shown.markers(
            &links(&Crowd::signatures(10)),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers.len(), 10);
        // "AAA-000 → LGN-154": 17 characters of 6 points and 8 of padding,
        // over the zoom the room is worked out for (1), up to a step of 16.
        assert_eq!(shown.reserved, [112.0, 16.0]);
        let texts: Vec<[[f64; 2]; 2]> = markers
            .iter()
            .map(|marker| Crowd::text_box(marker, shown.reserved))
            .collect();
        let nodes: Vec<[[f64; 2]; 2]> = region()
            .values()
            .map(|at| [[f64::from(at[0]), f64::from(at[1])], [90.0, 35.0]])
            .chain(markers.iter().map(|marker| {
                [
                    [f64::from(marker.position[0]), f64::from(marker.position[1])],
                    [90.0, 35.0],
                ]
            }))
            .collect();
        for (index, text) in texts.iter().enumerate() {
            for other in &texts[index + 1..] {
                assert!(!Crowd::overlap(*text, *other), "two texts on each other");
            }
            for node in &nodes {
                assert!(!Crowd::overlap(*text, *node), "a text on a node");
            }
        }
    }

    #[test]
    fn the_room_for_the_text_does_not_depend_on_the_texts_unless_one_is_longer_than_the_reference()
    {
        let mut shown = layer();
        let mut all = Crowd::signatures(4);
        let short = signature(json!({
            "in_system_id": 30_000_142,
            "out_system_id": 31_000_100,
            "out_signature": "AAA-100",
            "in_signature": null,
        }));
        // Only the short one: the room is the one of the reference text.
        shown.markers(
            &links(std::slice::from_ref(&short)),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(shown.reserved, [112.0, 16.0]);
        let before = Crowd::places(&shown.markers(
            &links(std::slice::from_ref(&short)),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        ));

        // Others come, as long as the reference: the room is the same, and the
        // one that was there does not move.
        all.push(short);
        let markers = shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers.len(), 5);
        assert_eq!(shown.reserved, [112.0, 16.0]);
        assert_eq!(Crowd::places(&markers)[&31_000_100], before[&31_000_100]);

        // One longer than the reference: the room grows, and it stays when
        // that one goes.
        let long = signature(json!({
            "in_system_id": 30_000_142,
            "out_system_id": 31_000_200,
            "out_signature": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA-200",
            "in_signature": null,
        }));
        all.push(long);
        shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        let grown = shown.reserved;
        assert!(grown[0] > 112.0, "{grown:?}");
        all.pop();
        shown.markers(
            &links(&all),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(shown.reserved, grown);
    }

    #[test]
    fn working_the_markers_out_again_with_nothing_new_is_cheap_and_changes_nothing() {
        // A region of 150 systems and the 24 wormholes of one of them.
        let systems: HashMap<usize, [f32; 2]> = (0..150)
            .map(|index| {
                (
                    30_000_000 + index,
                    [(index % 15) as f32 * 150.0, (index / 15) as f32 * 90.0],
                )
            })
            .collect();
        let hub = 30_000_077;
        let signatures: Vec<Signature> = (0..24)
            .map(|index| {
                signature(json!({
                    "in_system_id": hub,
                    "out_system_id": 31_000_000 + index,
                    "out_signature": format!("AAA-{index:03}"),
                }))
            })
            .collect();
        let links = links(&signatures);
        let mut shown = layer();

        let start = Instant::now();
        let first = shown.markers(&links, now(), &systems, &names, node(), Crowd::labels());
        let placing = start.elapsed();
        assert_eq!(first.len(), 24);

        let start = Instant::now();
        let again = shown.markers(&links, now(), &systems, &names, node(), Crowd::labels());
        let nothing_new = start.elapsed();
        eprintln!("24 markers placed in {placing:?}; the same again in {nothing_new:?}");
        assert_eq!(first, again);
        // A debug build, on a busy machine: a release one is far below this.
        assert!(placing.as_millis() < 300, "placing took {placing:?}");
        assert!(
            nothing_new.as_millis() < 30,
            "nothing new took {nothing_new:?}"
        );
    }

    #[test]
    fn a_wormhole_with_no_end_on_the_map_has_no_marker() {
        let elsewhere = signature(json!({ "in_system_id": 30_001_000 }));
        let markers = layer().markers(
            &links(&[elsewhere]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert!(markers.is_empty());
    }

    #[test]
    fn the_hub_end_of_a_wormhole_gets_a_marker_too_when_it_is_on_the_map() {
        // A map with Thera but not Jita, where the default signature leads.
        let region = HashMap::from([(THERA as usize, [600.0, 0.0]), (30_000_144, [300.0, 0.0])]);
        let markers = layer().markers(
            &links(&[signature(json!({}))]),
            now(),
            &region,
            &|id| Some(format!("system {id}")),
            node(),
            Crowd::labels(),
        );
        // The wormhole leaves from Thera (on this map) toward Jita (not).
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].system, THERA as usize);
        assert_eq!(markers[0].name, "system 30000142");
    }

    #[test]
    fn a_wormhole_close_to_collapsing_is_marked_and_a_collapsed_one_is_left_out() {
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        let links = links(&[from_thera]);
        let near_end = expires_at() - TimeDelta::hours(3);
        let markers = layer().markers(&links, near_end, &region(), &names, node(), Crowd::labels());
        assert!(markers[0].expiring);
        let after = layer().markers(
            &links,
            expires_at(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert!(after.is_empty());
    }

    #[test]
    fn a_marker_without_the_other_signature_is_labelled_with_one() {
        let scanning = signature(json!({ "in_system_id": 30_000_142, "in_signature": null }));
        let markers = layer().markers(
            &links(&[scanning]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        assert_eq!(markers[0].label, "RRW-782");
    }

    #[test]
    fn the_markers_go_on_the_map_with_their_dashed_line_and_come_off_again() {
        let mut map = Map::new();
        for (id, at) in region() {
            map.insert_point(MapPoint::new(id, at));
        }
        let mut shown = layer();
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        let markers = shown.markers(
            &links(&[from_thera]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        let id = markers[0].id;
        let markers_for_line = markers[0].clone();
        shown.show(&mut map, markers.clone(), 1, Instant::now());
        assert!(map.contains_point(id));
        assert!(map.segment((30_000_142, id)).is_some());
        // What is put on the map for the line.
        let line = markers_for_line.segment(&WormholeMarkerStyle::default());
        assert!(line.is_dashed());
        assert_eq!(line.label.as_deref(), Some("RRW-782 → LGN-154"));
        assert_eq!(line.id, (30_000_142, id));
        // The text is toward the marker, where the lines of a hub have room.
        assert_eq!(
            line.label_position(),
            WormholeMarkerStyle::default().label_at as f32
        );

        // The same markers again change nothing.
        shown.show(&mut map, markers, 2, Instant::now());
        assert!(map.contains_point(id));

        shown.show(&mut map, Vec::new(), 3, Instant::now());
        assert!(!map.contains_point(id));
        assert!(map.segment((30_000_142, id)).is_none());
        // The systems are still there.
        assert!(map.contains_point(30_000_142));
    }

    #[test]
    fn the_markers_are_worked_out_again_when_the_wormholes_change_or_after_a_while() {
        let mut shown = layer();
        let now = Instant::now();
        assert!(shown.is_due(0, now));
        shown.show(&mut Map::new(), Vec::new(), 4, now);
        assert!(!shown.is_due(4, now));
        assert!(shown.is_due(5, now));
        let later = now + WormholeMarkerStyle::default().refresh_every;
        assert!(shown.is_due(4, later));
    }

    #[test]
    fn clearing_forgets_what_was_drawn() {
        let mut map = Map::new();
        for (id, at) in region() {
            map.insert_point(MapPoint::new(id, at));
        }
        let mut shown = layer();
        let from_thera = signature(json!({ "in_system_id": 30_000_142 }));
        let markers = shown.markers(
            &links(&[from_thera]),
            now(),
            &region(),
            &names,
            node(),
            Crowd::labels(),
        );
        let id = markers[0].id;
        shown.show(&mut map, markers, 1, Instant::now());
        shown.clear(&mut map);
        assert!(!map.contains_point(id));
        assert!(shown.is_due(1, Instant::now()));
    }

    #[test]
    fn what_is_replaced_is_seen_through_a_copy_of_the_layer() {
        let shared = WormholeShared::default();
        assert!(shared.layer().is_empty());
        shared.replace(&[signature(json!({}))]);
        let copy = shared.layer();
        shared.replace(&[]);
        assert_eq!(copy.len(), 1);
        assert!(shared.layer().is_empty());
    }

    #[test]
    fn a_wormhole_stops_counting_when_it_expires() {
        let mut links = WormholeLinks::default();
        links.replace(&[signature(json!({}))]);
        let active = |now| links.layer().active(now).count();
        assert_eq!(active(expires_at() - TimeDelta::hours(1)), 1);
        assert_eq!(active(expires_at()), 0);
    }
}
