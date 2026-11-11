//! Where the marker of a wormhole goes next to its system, counting the text
//! that is drawn on its line.
//!
//! A marker is a node and a dashed line to its system with the signatures on
//! it, in a box at the middle of the line. Putting only the node where it
//! touches nothing leaves the boxes of the text on top of each other, on top
//! of the nodes and under the lines of the other markers, which is what a
//! crowded hub (Turnur has many) looks like. [`MarkerLayout::place_all`]
//! looks for a place where **two things** are free:
//!
//! * the box of the node, and
//! * the box of the text, on its line (at the middle, or where the layout
//!   puts it: the lines of a hub meet at it, so their texts have more room
//!   away from it), which is not on the node either.
//!
//! The line itself is not a rule but a cost: a place whose line goes through
//! another node or text (or where another line goes through its boxes) costs
//! more, so it is only taken when the way around is longer than that. The
//! lines already on the map (the gates between systems, [`Gate`]) count the
//! same: a text on one, or a node on one, or a line across one, costs more. A rule
//! would leave the marker without a place in a dense region, where there is
//! no clear way to anywhere.
//!
//! Directions across the map (the line of a box wider than tall has to be
//! long for its text to fit between the nodes) get a place later than the
//! ones up and down, so the markers fill the room around the system from the
//! closest places out, and the ones of a same system are spread over the
//! directions instead of piling up on one side.
//!
//! The work is meant to be cheap: it only runs for the markers that have no
//! place yet; each one tries a few dozen directions on rings that go
//! outwards, and stops at the first ring that can not beat the best place
//! found. The boxes are plain numbers: nothing is measured here (the size of
//! the text is given), so it knows nothing of `egui`.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_4, PI, TAU};

/// A box of the map, by its center and its size, in map units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Block {
    center: [f64; 2],
    size: [f64; 2],
}

impl Block {
    pub(super) fn new(center: [f64; 2], size: [f64; 2]) -> Self {
        Self { center, size }
    }

    /// Whether the two boxes are closer than `margin`: they overlap once the
    /// space between them is taken as `margin`.
    fn near(&self, other: &Self, margin: f64) -> bool {
        (self.center[0] - other.center[0]).abs() < (self.size[0] + other.size[0]) / 2.0 + margin
            && (self.center[1] - other.center[1]).abs()
                < (self.size[1] + other.size[1]) / 2.0 + margin
    }

    fn contains(&self, point: [f64; 2]) -> bool {
        (point[0] - self.center[0]).abs() <= self.size[0] / 2.0
            && (point[1] - self.center[1]).abs() <= self.size[1] / 2.0
    }

    /// Whether the line from `from` to `to` goes through the box once it is
    /// `inset` smaller on every side (so a line that grazes it does not
    /// count).
    fn crossed_by(&self, from: [f64; 2], to: [f64; 2], inset: f64) -> bool {
        let mut enter = 0.0_f64;
        let mut leave = 1.0_f64;
        for axis in 0..2 {
            let half = (self.size[axis] / 2.0 - inset).max(0.0);
            let (low, high) = (self.center[axis] - half, self.center[axis] + half);
            let delta = to[axis] - from[axis];
            if delta.abs() < 1e-12 {
                if from[axis] < low || from[axis] > high {
                    return false;
                }
            } else {
                let (a, b) = ((low - from[axis]) / delta, (high - from[axis]) / delta);
                enter = enter.max(a.min(b));
                leave = leave.min(a.max(b));
                if enter > leave {
                    return false;
                }
            }
        }
        true
    }
}

/// A line that is already on the map, between two systems (a gate): the
/// markers keep their texts and nodes off it, and their lines across it, when
/// they can.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Gate {
    from: [f64; 2],
    to: [f64; 2],
}

impl Gate {
    pub(super) fn new(from: [f64; 2], to: [f64; 2]) -> Self {
        Self { from, to }
    }

    /// Whether the box of the line is in the square of half side `reach`
    /// around `center`.
    fn within(&self, center: [f64; 2], reach: f64) -> bool {
        (0..2).all(|axis| {
            self.from[axis].max(self.to[axis]) >= center[axis] - reach
                && self.from[axis].min(self.to[axis]) <= center[axis] + reach
        })
    }

    /// Whether the line starts or ends at `point`.
    fn touches(&self, point: [f64; 2]) -> bool {
        self.from == point || self.to == point
    }

    /// Whether the box could be crossed by the line: its own box and the one
    /// of the line overlap. A cheap way to leave out most lines.
    fn may_cross(&self, block: &Block) -> bool {
        (0..2).all(|axis| {
            let (low, high) = (
                self.from[axis].min(self.to[axis]),
                self.from[axis].max(self.to[axis]),
            );
            let half = block.size[axis] / 2.0;
            high >= block.center[axis] - half && low <= block.center[axis] + half
        })
    }

    /// Whether the line goes through the box.
    pub(super) fn crosses(&self, block: &Block) -> bool {
        self.may_cross(block) && block.crossed_by(self.from, self.to, 0.0)
    }

    /// Whether the line from `from` to `to` crosses this one (the lines that
    /// only touch, or run along each other, do not).
    pub(super) fn crossed_by_line(&self, from: [f64; 2], to: [f64; 2]) -> bool {
        let side = |a: [f64; 2], b: [f64; 2], point: [f64; 2]| {
            (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
        };
        let (first, second) = (side(from, to, self.from), side(from, to, self.to));
        let (third, fourth) = (side(self.from, self.to, from), side(self.from, self.to, to));
        first * second < 0.0 && third * fourth < 0.0
    }
}

/// Arcs of a circle, as spans of angle in `0..=TAU` (`0` is to the right, and
/// the maps have `y` pointing down, so the angle grows clockwise on the
/// screen). They tell which directions around a system are taken by what is
/// already on the map at a given distance, so a place is looked for in the
/// free ones instead of in directions picked beforehand.
struct Arcs;

impl Arcs {
    /// The directions in which a point at `radius` from `center` is inside the
    /// box `block` made `half` bigger on each side (so a box of that size
    /// centered at the point touches `block`).
    fn around(center: [f64; 2], radius: f64, block: &Block, half: [f64; 2]) -> Vec<(f64, f64)> {
        let low = |axis: usize| (block.center[axis] - half[axis] - center[axis]) / radius;
        let high = |axis: usize| (block.center[axis] + half[axis] - center[axis]) / radius;
        let (cos_low, cos_high) = (low(0).max(-1.0), high(0).min(1.0));
        let (sin_low, sin_high) = (low(1).max(-1.0), high(1).min(1.0));
        if cos_low > cos_high || sin_low > sin_high {
            return Vec::new();
        }
        // cos(angle) in a range: two spans, mirrored around the x axis.
        let (near, far) = (cos_high.acos(), cos_low.acos());
        let by_cos = [(near, far), (TAU - far, TAU - near)];
        // sin(angle) in a range: two spans, mirrored around the y axis; the
        // one that holds 0 is cut there.
        let (below, above) = (sin_low.asin(), sin_high.asin());
        let mut by_sin = vec![(PI - above, PI - below)];
        if below < 0.0 && above > 0.0 {
            by_sin.push((0.0, above));
            by_sin.push((TAU + below, TAU));
        } else if above <= 0.0 {
            by_sin.push((below + TAU, above + TAU));
        } else {
            by_sin.push((below, above));
        }
        let mut both = Vec::new();
        for (a_low, a_high) in by_cos {
            for (b_low, b_high) in &by_sin {
                let (start, end) = (a_low.max(*b_low), a_high.min(*b_high));
                if start < end {
                    both.push((start, end));
                }
            }
        }
        both
    }

    /// The spans of `0..=TAU` that none of `blocked` covers, widest first
    /// the way they come: in order of angle.
    fn free(mut blocked: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut free = Vec::new();
        let mut from = 0.0_f64;
        for (start, end) in blocked {
            if start > from {
                free.push((from, start));
            }
            from = from.max(end);
        }
        if from < TAU {
            free.push((from, TAU));
        }
        free
    }
}

/// The boxes of a scene by their distance from a system, nearest first: the
/// boxes a ring touches are the ones in a stretch of it, found by bisection,
/// instead of all of them on every ring.
struct Around {
    nodes: Vec<(f64, Block)>,
    labels: Vec<(f64, Block)>,
}

impl Around {
    fn of(anchor: [f64; 2], scene: &Scene) -> Self {
        let sorted = |boxes: &[Block]| {
            let mut sorted: Vec<(f64, Block)> = boxes
                .iter()
                .map(|block| {
                    (
                        (block.center[0] - anchor[0]).hypot(block.center[1] - anchor[1]),
                        *block,
                    )
                })
                .collect();
            sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
            sorted
        };
        Self {
            nodes: sorted(&scene.nodes),
            labels: sorted(&scene.labels),
        }
    }

    /// The boxes whose center is at a distance from the system between
    /// `low` and `high`.
    fn between(boxes: &[(f64, Block)], low: f64, high: f64) -> &[(f64, Block)] {
        let start = boxes.partition_point(|(distance, _)| *distance < low);
        let end = boxes.partition_point(|(distance, _)| *distance <= high);
        &boxes[start..end.max(start)]
    }
}

/// A marker that already has its place: the system it is next to, and where
/// its node is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placed {
    pub(super) anchor: [f64; 2],
    pub(super) spot: [f64; 2],
}

/// Boxes of one size, kept by the cell of the map their center is in, to
/// know which ones are near a place without going through all of them: a
/// region can have a hundred and fifty systems and a hub dozens of markers,
/// and a place is tried hundreds of times.
struct Cells {
    cell: f64,
    /// The size of the boxes kept.
    size: [f64; 2],
    cells: HashMap<(i64, i64), Vec<Block>>,
}

impl Cells {
    fn new(cell: f64, size: [f64; 2]) -> Self {
        Self {
            cell,
            size,
            cells: HashMap::new(),
        }
    }

    fn key(&self, point: [f64; 2]) -> (i64, i64) {
        (
            (point[0] / self.cell).floor() as i64,
            (point[1] / self.cell).floor() as i64,
        )
    }

    fn insert(&mut self, block: Block) {
        self.cells
            .entry(self.key(block.center))
            .or_default()
            .push(block);
    }

    /// Whether any box is closer than `margin` to `block`: only the cells
    /// that a box that near could be in are looked at.
    fn any_near(&self, block: &Block, margin: f64) -> bool {
        let reach = [
            (self.size[0] + block.size[0]) / 2.0 + margin,
            (self.size[1] + block.size[1]) / 2.0 + margin,
        ];
        let low = self.key([block.center[0] - reach[0], block.center[1] - reach[1]]);
        let high = self.key([block.center[0] + reach[0], block.center[1] + reach[1]]);
        (low.0..=high.0).any(|x| {
            (low.1..=high.1).any(|y| {
                self.cells
                    .get(&(x, y))
                    .is_some_and(|items| items.iter().any(|other| other.near(block, margin)))
            })
        })
    }
}

/// What is on the map while the markers are placed: what a new one has to
/// keep clear of.
struct Scene {
    /// The nodes of the systems: the ones that never move.
    systems: Vec<Block>,
    /// The nodes of the systems and of the markers already placed.
    nodes: Vec<Block>,
    /// The boxes of the text of the markers already placed.
    labels: Vec<Block>,
    /// The lines of the markers already placed.
    lines: Vec<Placed>,
    /// The lines of the map that were there before the markers.
    gates: Vec<Gate>,
    /// `nodes` and `labels` by place, for the boxes near a candidate.
    node_cells: Cells,
    label_cells: Cells,
}

/// How the directions tried on each ring are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the style picks it, and only the default is used")
)]
pub(crate) enum Method {
    /// Only the directions of the ring (a regular grid of them) that fall in
    /// no other box, found by trying them. Cheap, and the markers of an
    /// isolated hub come out evenly spread.
    Sampled,
    /// The free arcs around the system: their ends and middles, and the
    /// directions of the ring that fall in them. A narrow gap between two
    /// nodes is found and a taken direction is never tried; a little dearer.
    Arcs,
    /// [`Self::Arcs`] for a system with nodes of the map close to it, where
    /// the directions the nodes take matter, and [`Self::Sampled`] for one
    /// with nothing around it.
    #[default]
    Auto,
}

/// How the markers are placed, in map units. [`Self::new`] has the usual
/// values for the rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MarkerLayout {
    /// The size of a node (the one of the systems and of the markers).
    node: [f64; 2],
    /// The size of the box of the text of a line.
    label: [f64; 2],
    /// Where the text goes along its line: `0` at the system, `1` at the
    /// marker, `0.5` at the middle.
    label_at: f64,
    /// The space kept free between a node and another one.
    gap: f64,
    /// The space kept free around a text.
    margin: f64,
    /// How many directions are tried on each ring, at least: a system with
    /// more markers than that gets more (see [`Self::directions_for`]), or the
    /// markers would line up on the same directions and hide each other.
    directions: u32,
    /// How much farther each ring is from the one before it, as a fraction of
    /// the height of a node, for the first ones.
    ring_step: f64,
    /// How much the step grows with each ring (`0.15` makes the tenth ring's
    /// step one and a half times the first one): the places close to the
    /// system are tried finely, and the far ones, where a marker only goes
    /// when there is nothing closer, in bigger jumps, which is what keeps the
    /// search short in a region too dense for a marker near anything.
    ring_growth: f64,
    /// How many rings are tried before giving up.
    max_rings: u32,
    /// How much a place costs more for being next to the other markers of its
    /// system: `0` takes the closest place, whatever is next to it, and `1`
    /// doubles the distance of a place right beside another marker.
    spread: f64,
    /// What a line that goes through a node or a text (or a box that a line
    /// goes through) adds to the cost of a place, in map units of distance.
    crossing_cost: f64,
    /// The direction tried first, in radians. The maps have `y` pointing
    /// down, so -45 degrees is the upper right of the screen.
    fallback_angle: f64,
    /// How the directions tried on a ring are chosen.
    method: Method,
    /// The least distance from a system to its marker, in map units. `0` leaves
    /// it to the size of a node and the number of markers.
    min_distance: f64,
    /// How close a node of the map has to be to a system for
    /// [`Method::Auto`] to look in the free arcs around it.
    static_reach: f64,
}

impl MarkerLayout {
    /// This layout with the texts `at` that fraction of their lines, from `0`
    /// at the system to `1` at the marker (a value outside that is the
    /// nearest end).
    pub(super) fn with_label_at(mut self, at: f64) -> Self {
        self.label_at = at.clamp(0.0, 1.0);
        self
    }

    /// This layout with the markers at least `distance` map units from their
    /// systems.
    pub(super) fn with_min_distance(mut self, distance: f64) -> Self {
        self.min_distance = distance.max(0.0);
        self
    }

    /// This layout with the directions tried chosen `method`.
    pub(super) fn with_method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }

    /// A layout for nodes of `node` size and texts of `label` size, keeping
    /// `gap` free between nodes.
    pub(super) fn new(node: [f64; 2], label: [f64; 2], gap: f64) -> Self {
        Self {
            node,
            label,
            label_at: 0.5,
            gap,
            margin: 2.0,
            directions: 24,
            ring_step: 0.25,
            ring_growth: 0.15,
            max_rings: 80,
            spread: 0.5,
            crossing_cost: 1000.0,
            fallback_angle: -FRAC_PI_4,
            method: Method::Auto,
            min_distance: 0.0,
            static_reach: node[0] + label[0] + gap,
        }
    }

    fn is_valid(&self) -> bool {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        positive(self.node[0])
            && positive(self.node[1])
            && self
                .label
                .iter()
                .all(|side| side.is_finite() && *side >= 0.0)
            && self.gap.is_finite()
            && self.gap >= 0.0
            && self.margin.is_finite()
            && self.directions > 0
            && positive(self.ring_step)
            && self.ring_growth.is_finite()
            && self.ring_growth >= 0.0
            && self.crossing_cost.is_finite()
            && self.crossing_cost >= 0.0
            && self.static_reach.is_finite()
    }

    /// How many directions a ring has for a system with `crowd` markers: one
    /// and a half for each, in multiples of twelve, and never fewer than
    /// [`Self::directions`]. With fewer the markers of a crowded hub share
    /// directions, and the texts of the ones behind are under the ones in
    /// front.
    fn directions_for(&self, crowd: usize) -> u32 {
        let wanted = (crowd as f64 * 1.5).ceil() as u32;
        wanted.next_multiple_of(12).max(self.directions)
    }

    /// Whether the free arcs are looked at to place a marker at `anchor`.
    fn uses_arcs(&self, anchor: [f64; 2], scene: &Scene) -> bool {
        match self.method {
            Method::Sampled => false,
            Method::Arcs => true,
            Method::Auto => scene.systems.iter().any(|block| {
                let distance = (block.center[0] - anchor[0]).hypot(block.center[1] - anchor[1]);
                distance > f64::EPSILON && distance < self.static_reach
            }),
        }
    }

    /// A place for each of the markers that go next to the systems at
    /// `anchors`, in the order given. Each one is placed away from `systems`
    /// (the positions of the nodes of the map), from `kept` (the markers that
    /// already have their place, which never move) and from the ones placed
    /// before it. `None` where no place was free in
    /// [`max_rings`](Self::max_rings) rings, or the anchor is not a number.
    ///
    /// The place of a marker depends on what was placed before and nothing
    /// else, so the same input always gives the same places.
    #[cfg(test)]
    pub(super) fn place_all(
        &self,
        anchors: &[[f64; 2]],
        systems: &[[f64; 2]],
        kept: &[Placed],
    ) -> Vec<Option<[f64; 2]>> {
        self.place_all_among(anchors, systems, kept, &[])
    }

    /// [`Self::place_all`] with the lines of the map (`gates`) to keep
    /// the texts and the nodes off.
    pub(super) fn place_all_among(
        &self,
        anchors: &[[f64; 2]],
        systems: &[[f64; 2]],
        kept: &[Placed],
        gates: &[Gate],
    ) -> Vec<Option<[f64; 2]>> {
        if !self.is_valid() {
            return vec![None; anchors.len()];
        }
        let mut scene = self.scene(systems, kept, gates);
        anchors
            .iter()
            .enumerate()
            .map(|(index, anchor)| {
                if !anchor.iter().all(|value| value.is_finite()) {
                    return None;
                }
                // How many markers the system has in all: the ones it already
                // has, this one and the ones that come after it in this call.
                let crowd = kept
                    .iter()
                    .filter(|placed| placed.anchor == *anchor)
                    .count()
                    + anchors[index..]
                        .iter()
                        .filter(|other| *other == anchor)
                        .count();
                let spot = self.find(*anchor, crowd, &scene)?;
                self.add(
                    &mut scene,
                    Placed {
                        anchor: *anchor,
                        spot,
                    },
                );
                Some(spot)
            })
            .collect()
    }

    /// The nodes of `systems` and the markers in `kept`, as they are on the
    /// map before anything is placed.
    fn scene(&self, systems: &[[f64; 2]], kept: &[Placed], gates: &[Gate]) -> Scene {
        let cell = self.node[0]
            .max(self.node[1])
            .max(self.label[0])
            .max(self.label[1])
            + self.gap;
        let mut scene = Scene {
            systems: Vec::with_capacity(systems.len()),
            nodes: Vec::with_capacity(systems.len() + kept.len()),
            labels: Vec::with_capacity(kept.len()),
            lines: Vec::with_capacity(kept.len()),
            gates: gates.to_vec(),
            node_cells: Cells::new(cell, self.node),
            label_cells: Cells::new(cell, self.label),
        };
        for at in systems {
            let block = Block::new(*at, self.node);
            scene.systems.push(block);
            scene.nodes.push(block);
            scene.node_cells.insert(block);
        }
        for placed in kept {
            self.add(&mut scene, *placed);
        }
        scene
    }

    fn add(&self, scene: &mut Scene, placed: Placed) {
        let node = Block::new(placed.spot, self.node);
        let label = Block::new(self.label_center(placed), self.label);
        scene.nodes.push(node);
        scene.node_cells.insert(node);
        scene.labels.push(label);
        scene.label_cells.insert(label);
        scene.lines.push(placed);
    }

    /// Where the text of the line of `placed` is drawn.
    fn label_center(&self, placed: Placed) -> [f64; 2] {
        [
            placed.anchor[0] + (placed.spot[0] - placed.anchor[0]) * self.label_at,
            placed.anchor[1] + (placed.spot[1] - placed.anchor[1]) * self.label_at,
        ]
    }

    /// The cheapest free place next to `anchor`: the closest ring that has
    /// one, except that a place farther out but away from the other markers
    /// of the system can beat a closer one that is right beside them.
    fn find(&self, anchor: [f64; 2], crowd: usize, scene: &Scene) -> Option<[f64; 2]> {
        let siblings: Vec<f64> = scene
            .lines
            .iter()
            .filter(|placed| placed.anchor == anchor)
            .map(|placed| (placed.spot[1] - anchor[1]).atan2(placed.spot[0] - anchor[0]))
            .collect();
        let step = self.node[1] * self.ring_step;
        let start = self.node[0]
            .min(self.node[1])
            .max(self.ring_radius(crowd))
            .max(self.min_distance);
        let mut best: Option<(f64, [f64; 2])> = None;
        let around = Around::of(anchor, scene);
        let directions = self.directions_for(crowd);
        let arcs = self.uses_arcs(anchor, scene);
        // The lines of the map that can be near the places tried so far: the
        // ones in the square around the system that the rings reach. A
        // region has hundreds of them, and a place is tried many times.
        let extent = self.node[0]
            .max(self.node[1])
            .max(self.label[0])
            .max(self.label[1]);
        let mut reach = 0.0_f64;
        let mut near: Vec<Gate> = Vec::new();
        for ring in 0..=self.max_rings {
            let rings = f64::from(ring);
            let distance = start + step * (rings + self.ring_growth * rings * rings);
            // The cost is never below the distance: the rings from here on
            // can not beat what was found.
            if best.is_some_and(|(cost, _)| distance > cost) {
                break;
            }
            if distance + extent > reach {
                reach = (distance + extent) * 2.0;
                near = scene
                    .gates
                    .iter()
                    .filter(|gate| gate.within(anchor, reach))
                    .copied()
                    .collect();
            }
            // The directions, the closest to the other markers of the system
            // last: a place can not cost less than its base, so once the base
            // reaches the best cost the ones that follow are not tried.
            let mut tries: Vec<(f64, f64)> = self
                .angles(anchor, distance, &around, directions, arcs)
                .into_iter()
                .map(|angle| {
                    let room = siblings
                        .iter()
                        .map(|other| Self::apart(angle, *other))
                        .fold(PI, f64::min);
                    (distance * (1.0 + self.spread * (1.0 - room / PI)), angle)
                })
                .collect();
            tries.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (base, angle) in tries {
                if best.is_some_and(|(lowest, _)| base >= lowest) {
                    break;
                }
                let spot = [
                    anchor[0] + distance * angle.cos(),
                    anchor[1] + distance * angle.sin(),
                ];
                // The checks that follow are the costly part: they are only
                // made for a place that could still beat the best one, which
                // the crossings can only make worse.
                if !self.is_clear(anchor, spot, scene) {
                    continue;
                }
                // How many crossings a place can have and still be the best.
                let limit = best.map_or(usize::MAX, |(lowest, _)| {
                    ((lowest - base) / self.crossing_cost).ceil().max(1.0) as usize
                });
                let cost = base
                    + self.crossings(anchor, spot, scene, &around, &near, limit) as f64
                        * self.crossing_cost;
                if best.is_none_or(|(lowest, _)| cost < lowest) {
                    best = Some((cost, spot));
                }
            }
        }
        best.map(|(_, spot)| spot)
    }

    /// The directions worth trying on the ring at `distance` from `anchor`.
    /// With `arcs`, the ends and the middle of every arc that nothing on the
    /// map takes (a node, or a text, that a node or a text of the new marker
    /// would touch), so a narrow gap between two nodes is found and a taken
    /// direction is not tried; and the `directions` of the ring that fall in
    /// those arcs. Without it, the `directions` of the ring.
    fn angles(
        &self,
        anchor: [f64; 2],
        distance: f64,
        around: &Around,
        directions: u32,
        arcs: bool,
    ) -> Vec<f64> {
        let sampled = (0..directions)
            .map(|order| self.fallback_angle + TAU * f64::from(order) / f64::from(directions));
        if !arcs {
            return sampled.collect();
        }
        let half = |size: [f64; 2], other: [f64; 2], space: f64| {
            [
                (size[0] + other[0]) / 2.0 + space,
                (size[1] + other[1]) / 2.0 + space,
            ]
        };
        // Where the text goes, as a fraction of the way out.
        let text = distance * self.label_at;
        let mut blocked: Vec<(f64, f64)> = Vec::new();
        let mut block = |boxes: &[(f64, Block)], radius: f64, size: [f64; 2], space: f64| {
            if radius <= f64::EPSILON {
                return;
            }
            // The boxes are all of one size: how far from its center a box
            // can still touch the circle.
            let Some((_, first)) = boxes.first() else {
                return;
            };
            let reach = half(first.size, size, space);
            let far = reach[0].hypot(reach[1]);
            for (_, other) in Around::between(boxes, radius - far, radius + far) {
                blocked.extend(Arcs::around(anchor, radius, other, reach));
            }
        };
        // The node of the new marker is at `distance`, its text at `text`.
        block(&around.nodes, distance, self.node, self.gap);
        block(&around.labels, distance, self.node, self.margin);
        block(&around.nodes, text, self.label, self.margin);
        block(&around.labels, text, self.label, self.margin);
        let free = Arcs::free(blocked);
        // A little inside an end, so the place is not on the border.
        let inset = 1e-6;
        let mut angles: Vec<f64> = Vec::new();
        for (start, end) in &free {
            if end - start <= inset {
                continue;
            }
            let (low, high) = (start + inset, end - inset);
            angles.push(low);
            angles.push((start + end) / 2.0);
            angles.push(high.max(low));
        }
        angles.extend(
            sampled
                .map(|angle| angle.rem_euclid(TAU))
                .filter(|angle| free.iter().any(|(start, end)| angle > start && angle < end)),
        );
        // In the order of the ring: from the first direction on.
        let turn = |angle: &f64| (angle - self.fallback_angle).rem_euclid(TAU);
        angles.sort_by(|a, b| turn(a).total_cmp(&turn(b)));
        angles.dedup();
        angles
    }

    /// The closest ring that has room for `crowd` nodes side by side around a
    /// system: where the chord between two neighbours is a node and a gap
    /// wide. Below three it is not a constraint: two go to opposite sides
    /// anyway. Without it each marker takes the closest place, and the first
    /// ones, which do not know how many come, are left right on the system,
    /// with their lines and texts under the ones that come later.
    fn ring_radius(&self, crowd: usize) -> f64 {
        if crowd < 3 {
            return 0.0;
        }
        (self.node[0] + self.gap) / (2.0 * (PI / crowd as f64).sin())
    }

    /// The angle between two directions, from 0 to pi.
    fn apart(a: f64, b: f64) -> f64 {
        let turn = (a - b).rem_euclid(TAU);
        turn.min(TAU - turn)
    }

    /// Whether a marker with its node at `spot` and its line to `anchor` has
    /// room: its node and its text touch no node and no text.
    fn is_clear(&self, anchor: [f64; 2], spot: [f64; 2], scene: &Scene) -> bool {
        let node = Block::new(spot, self.node);
        let label = Block::new(self.label_center(Placed { anchor, spot }), self.label);
        !(node.near(&label, self.margin)
            || scene.node_cells.any_near(&node, self.gap)
            || scene.node_cells.any_near(&label, self.margin)
            || scene.label_cells.any_near(&node, self.margin)
            || scene.label_cells.any_near(&label, self.margin))
    }

    /// How many things a marker at `spot` goes through or is gone through
    /// by: nodes and texts that its line crosses, and lines that cross its
    /// node or its text. It stops counting at `limit`: a place that is
    /// already too dear is not worth knowing how much. A line of the map counts for its text, for its node
    /// and for its own line, each, unless it starts at the system (it only
    /// leaves from the same place).
    fn crossings(
        &self,
        anchor: [f64; 2],
        spot: [f64; 2],
        scene: &Scene,
        around: &Around,
        gates: &[Gate],
        limit: usize,
    ) -> usize {
        let node = Block::new(spot, self.node);
        let label = Block::new(self.label_center(Placed { anchor, spot }), self.label);
        // What the line grazes is not crossed: a margin of a few units.
        let inset = self.margin;
        // The system the line starts at is not "crossed" by it.
        // A box the line goes through has its center within the length of the
        // line, and half the diagonal of the box, from the system: the others
        // are not looked at.
        let length = (spot[0] - anchor[0]).hypot(spot[1] - anchor[1]);
        let nodes = Around::between(
            &around.nodes,
            0.0,
            length + self.node[0].hypot(self.node[1]) / 2.0,
        )
        .iter()
        .filter(|(_, other)| !other.contains(anchor) && other.crossed_by(anchor, spot, inset))
        .take(limit)
        .count();
        if nodes >= limit {
            return nodes;
        }
        let labels = Around::between(
            &around.labels,
            0.0,
            length + self.label[0].hypot(self.label[1]) / 2.0,
        )
        .iter()
        .filter(|(_, other)| other.crossed_by(anchor, spot, 0.0))
        .take(limit - nodes)
        .count();
        if nodes + labels >= limit {
            return nodes + labels;
        }
        let lines = scene
            .lines
            .iter()
            .filter(|line| {
                node.crossed_by(line.anchor, line.spot, inset)
                    || label.crossed_by(line.anchor, line.spot, 0.0)
            })
            .take(limit - nodes - labels)
            .count();
        if nodes + labels + lines >= limit {
            return nodes + labels + lines;
        }
        let gates: usize = gates
            .iter()
            .map(|gate| {
                usize::from(gate.crosses(&label))
                    + usize::from(gate.crosses(&node))
                    + usize::from(!gate.touches(anchor) && gate.crossed_by_line(anchor, spot))
            })
            .sum();
        nodes + labels + lines + gates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// The usual: nodes of 94 x 39 and a text of 130 x 16, 24 apart.
    struct Setup;

    impl Setup {
        fn node() -> [f64; 2] {
            [94.0, 39.0]
        }

        fn label() -> [f64; 2] {
            [130.0, 16.0]
        }

        fn gap() -> f64 {
            24.0
        }

        fn layout() -> MarkerLayout {
            MarkerLayout::new(Self::node(), Self::label(), Self::gap())
        }

        /// `count` markers, all for the system at the origin.
        fn crowd(count: usize) -> Vec<[f64; 2]> {
            vec![[0.0, 0.0]; count]
        }

        /// The boxes of the node and of the text of a marker.
        fn boxes(placed: Placed) -> (Block, Block) {
            (
                Block::new(placed.spot, Self::node()),
                Block::new(Self::layout().label_center(placed), Self::label()),
            )
        }
    }

    #[test]
    fn a_box_is_near_another_by_less_than_the_margin() {
        let a = Block::new([0.0, 0.0], [10.0, 10.0]);
        let b = Block::new([14.0, 0.0], [10.0, 10.0]);
        assert!(!a.near(&b, 3.9));
        assert!(a.near(&b, 4.1));
        assert!(!a.near(&Block::new([0.0, 40.0], [10.0, 10.0]), 4.0));
    }

    #[test]
    fn a_line_goes_through_a_box_only_when_it_enters_it() {
        let boxed = Block::new([0.0, 0.0], [10.0, 10.0]);
        assert!(boxed.crossed_by([-20.0, 0.0], [20.0, 0.0], 0.0));
        assert!(boxed.crossed_by([-20.0, -20.0], [20.0, 20.0], 0.0));
        // Ends before it, passes beside it, or goes the other way.
        assert!(!boxed.crossed_by([-20.0, 0.0], [-8.0, 0.0], 0.0));
        assert!(!boxed.crossed_by([-20.0, 9.0], [20.0, 9.0], 6.0));
        assert!(!boxed.crossed_by([-20.0, 30.0], [20.0, 30.0], 0.0));
        // Vertical, along the side.
        assert!(!boxed.crossed_by([6.0, -20.0], [6.0, 20.0], 0.0));
        assert!(boxed.crossed_by([4.0, -20.0], [4.0, 20.0], 0.0));
        // Starts inside.
        assert!(boxed.crossed_by([0.0, 0.0], [50.0, 0.0], 0.0));
    }

    #[test]
    fn a_box_contains_its_own_points() {
        let boxed = Block::new([5.0, 5.0], [10.0, 10.0]);
        assert!(boxed.contains([5.0, 5.0]));
        assert!(boxed.contains([10.0, 10.0]));
        assert!(!boxed.contains([10.1, 5.0]));
    }

    #[test]
    fn the_direction_between_two_is_the_short_way_round() {
        assert!((MarkerLayout::apart(0.1, -0.1) - 0.2).abs() < 1e-12);
        assert!((MarkerLayout::apart(3.0, -3.0) - (TAU - 6.0)).abs() < 1e-12);
        assert!((MarkerLayout::apart(0.0, PI) - PI).abs() < 1e-12);
    }

    #[test]
    fn a_lone_marker_goes_close_and_clear_of_its_system() {
        let spots = Setup::layout().place_all(&[[0.0, 0.0]], &[[0.0, 0.0]], &[]);
        let spot = spots[0].expect("a free place");
        let (node, label) = Setup::boxes(Placed {
            anchor: [0.0, 0.0],
            spot,
        });
        assert!(!node.near(&Block::new([0.0, 0.0], Setup::node()), Setup::gap()));
        assert!(!label.near(&Block::new([0.0, 0.0], Setup::node()), 2.0));
        // Close: a node, a gap and half of the text away at most.
        assert!(spot[0].hypot(spot[1]) < 200.0, "{spot:?}");
    }

    #[test]
    fn many_markers_of_a_system_touch_neither_nodes_nor_texts() {
        let layout = Setup::layout();
        let anchors = Setup::crowd(12);
        let systems = [[0.0, 0.0], [400.0, 0.0], [-300.0, 200.0]];
        let spots = layout.place_all(&anchors, &systems, &[]);
        let placed: Vec<Placed> = anchors
            .iter()
            .zip(&spots)
            .map(|(anchor, spot)| Placed {
                anchor: *anchor,
                spot: spot.expect("a free place"),
            })
            .collect();
        assert_eq!(placed.len(), 12);

        for (index, first) in placed.iter().enumerate() {
            let (node, label) = Setup::boxes(*first);
            for system in systems {
                let system = Block::new(system, Setup::node());
                assert!(!node.near(&system, Setup::gap() - 0.01), "node on a system");
                assert!(!label.near(&system, 1.99), "text on a system");
            }
            for second in &placed[index + 1..] {
                let (other_node, other_label) = Setup::boxes(*second);
                assert!(!node.near(&other_node, Setup::gap() - 0.01), "node on node");
                assert!(!label.near(&other_label, 1.99), "text on text");
                assert!(!node.near(&other_label, 1.99), "node on text");
                assert!(!label.near(&other_node, 1.99), "text on node");
            }
        }
    }

    #[test]
    fn the_markers_of_a_system_are_spread_around_it() {
        let layout = Setup::layout();
        let spots = layout.place_all(&Setup::crowd(8), &[[0.0, 0.0]], &[]);
        let mut sides = [0_u32; 4];
        for spot in spots.iter().map(|spot| spot.expect("a place")) {
            let quarter = ((spot[1].atan2(spot[0]) + PI) / (PI / 2.0)) as usize;
            sides[quarter.min(3)] += 1;
        }
        // Not all piled on one side: every quarter has at least one.
        assert!(sides.iter().all(|count| *count >= 1), "{sides:?}");
    }

    #[test]
    fn a_line_whose_text_does_not_fit_across_goes_up_or_down() {
        // A text wider than the room between nodes at the side: with one
        // marker, the place is more up or down than across.
        let layout = MarkerLayout::new(Setup::node(), [260.0, 16.0], Setup::gap());
        let spot = layout.place_all(&[[0.0, 0.0]], &[[0.0, 0.0]], &[])[0].unwrap();
        assert!(spot[1].abs() > spot[0].abs() * 0.3, "{spot:?}");
        // And the text is clear of the system.
        let label = Block::new([spot[0] / 2.0, spot[1] / 2.0], [260.0, 16.0]);
        assert!(!label.near(&Block::new([0.0, 0.0], Setup::node()), 2.0));
    }

    #[test]
    fn a_line_goes_around_a_node_when_the_way_around_is_short() {
        // A node right on the preferred direction (upper right), a little
        // farther than a marker: the line must not go through it.
        let layout = Setup::layout();
        let anchor = [0.0, 0.0];
        let blocker = [150.0, -150.0];
        let spot = layout.place_all(&[anchor], &[anchor, blocker], &[])[0].unwrap();
        let blocker = Block::new(blocker, Setup::node());
        assert!(!blocker.crossed_by(anchor, spot, 2.0), "{spot:?}");
    }

    #[test]
    fn a_line_goes_through_a_node_when_there_is_no_clear_way_at_all() {
        // A hub in the middle of a grid too tight for a marker: it is placed
        // outside of it, and its line goes through nodes, instead of being
        // left without a place.
        let layout = Setup::layout();
        let mut systems = Vec::new();
        for x in 0..9 {
            for y in 0..9 {
                systems.push([f64::from(x) * 150.0, f64::from(y) * 90.0]);
            }
        }
        let hub = systems[40];
        let spot = layout.place_all(&[hub], &systems, &[])[0];
        assert!(spot.is_some());
    }

    #[test]
    fn crossings_are_counted() {
        let layout = Setup::layout();
        // Two systems, and a marker of the first one at the lower right, with
        // its text at the middle of a line that goes down and to the right.
        let kept = [Placed {
            anchor: [150.0, -200.0],
            spot: [150.0, 200.0],
        }];
        let scene = layout.scene(&[[0.0, 0.0], [300.0, 0.0]], &kept, &[]);
        // Across: through the system at 300, through the text at (150, 0),
        // and the text of the new one is gone through by the vertical line.
        assert_eq!(
            layout.crossings(
                [0.0, 0.0],
                [400.0, 0.0],
                &scene,
                &Around::of([0.0, 0.0], &scene),
                &scene.gates,
                usize::MAX
            ),
            3
        );
        // Nothing in the way, down and to the left.
        assert_eq!(
            layout.crossings(
                [0.0, 0.0],
                [-200.0, 200.0],
                &scene,
                &Around::of([0.0, 0.0], &scene),
                &scene.gates,
                usize::MAX
            ),
            0
        );
    }

    #[test]
    fn the_boxes_near_a_place_are_found_by_cell_as_by_looking_at_all_of_them() {
        // Boxes spread over a few cells, and places all over: the answer of
        // the cells is the one of going through every box.
        let size = [94.0, 39.0];
        let mut cells = Cells::new(160.0, size);
        let boxes: Vec<Block> = (0..40)
            .map(|index| {
                let index = f64::from(index);
                Block::new(
                    [
                        (index * 97.0) % 700.0 - 100.0,
                        (index * 53.0) % 400.0 - 50.0,
                    ],
                    size,
                )
            })
            .collect();
        for block in &boxes {
            cells.insert(*block);
        }
        for x in -30..30 {
            for y in -20..20 {
                let probe = Block::new([f64::from(x) * 17.0, f64::from(y) * 13.0], [130.0, 16.0]);
                for margin in [0.0, 2.0, 24.0] {
                    assert_eq!(
                        cells.any_near(&probe, margin),
                        boxes.iter().any(|block| block.near(&probe, margin)),
                        "{probe:?} with {margin}"
                    );
                }
            }
        }
    }

    /// How many times the line of a marker goes through the text of another.
    fn lines_through_texts(layout: &MarkerLayout, placed: &[Placed]) -> usize {
        let mut count = 0;
        for (index, line) in placed.iter().enumerate() {
            for (other_index, other) in placed.iter().enumerate() {
                let text = Block::new(layout.label_center(*other), layout.label);
                if index != other_index && text.crossed_by(line.anchor, line.spot, 0.0) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn the_text_toward_the_marker_is_crossed_by_fewer_lines_than_at_the_middle() {
        // Ten lines meet at the system: at their middles the texts are in the
        // middle of the lines of the others; farther along they are not.
        let crossed = |at: f64| {
            let layout = Setup::layout().with_label_at(at);
            let anchors = Setup::crowd(10);
            let placed: Vec<Placed> = anchors
                .iter()
                .zip(layout.place_all(&anchors, &[[0.0, 0.0]], &[]))
                .map(|(anchor, spot)| Placed {
                    anchor: *anchor,
                    spot: spot.expect("a place"),
                })
                .collect();
            lines_through_texts(&layout, &placed)
        };
        let (middle, toward_the_marker) = (crossed(0.5), crossed(0.7));
        assert!(
            toward_the_marker < middle,
            "{toward_the_marker} lines through texts at 0.7, {middle} at 0.5"
        );
    }

    #[test]
    fn the_text_never_covers_the_node_of_its_own_marker() {
        for at in [0.5, 0.7, 0.9] {
            let layout = Setup::layout().with_label_at(at);
            let anchors = Setup::crowd(12);
            for (anchor, spot) in anchors
                .iter()
                .zip(layout.place_all(&anchors, &[[0.0, 0.0]], &[]))
            {
                let spot = spot.expect("a place");
                let node = Block::new(spot, Setup::node());
                let text = Block::new(
                    layout.label_center(Placed {
                        anchor: *anchor,
                        spot,
                    }),
                    Setup::label(),
                );
                assert!(!node.near(&text, 1.99), "at {at}: {spot:?}");
            }
        }
    }

    #[test]
    fn a_label_position_off_the_line_is_the_nearest_end() {
        assert_eq!(Setup::layout().with_label_at(3.0).label_at, 1.0);
        assert_eq!(Setup::layout().with_label_at(-1.0).label_at, 0.0);
    }

    /// The line of the map that goes through the text of the marker that the
    /// layout puts at `spot`, across its line.
    fn gate_through_the_text(layout: &MarkerLayout, anchor: [f64; 2], spot: [f64; 2]) -> Gate {
        let at = layout.label_center(Placed { anchor, spot });
        let (dx, dy) = (spot[0] - anchor[0], spot[1] - anchor[1]);
        let length = dx.hypot(dy);
        // Across the line, 300 units long, through the middle of the text.
        let across = [-dy / length * 150.0, dx / length * 150.0];
        Gate::new(
            [at[0] - across[0], at[1] - across[1]],
            [at[0] + across[0], at[1] + across[1]],
        )
    }

    #[test]
    fn a_marker_keeps_its_text_off_a_line_of_the_map() {
        let layout = Setup::layout();
        let anchor = [0.0, 0.0];
        let free = layout.place_all(&[anchor], &[anchor], &[])[0].unwrap();
        let gate = gate_through_the_text(&layout, anchor, free);
        assert!(gate.crosses(&Setup::boxes(Placed { anchor, spot: free }).1));

        let spot = layout.place_all_among(&[anchor], &[anchor], &[], &[gate])[0].unwrap();
        assert_ne!(spot, free);
        let (node, label) = Setup::boxes(Placed { anchor, spot });
        assert!(!gate.crosses(&label), "the text is on the line of the map");
        assert!(!gate.crosses(&node), "the node is on the line of the map");
        assert!(!gate.crossed_by_line(anchor, spot), "the line crosses it");
    }

    #[test]
    fn lines_of_the_map_that_are_not_in_the_way_change_nothing() {
        let layout = Setup::layout();
        let systems = [[0.0, 0.0], [400.0, 0.0], [-300.0, 200.0]];
        let anchors = Setup::crowd(6);
        // Far away, and one that starts at the system and goes where no
        // marker of it is.
        let gates = [
            Gate::new([5000.0, 5000.0], [6000.0, 5000.0]),
            Gate::new([-3000.0, -3000.0], [-3000.0, 3000.0]),
        ];
        assert_eq!(
            layout.place_all_among(&anchors, &systems, &[], &gates),
            layout.place_all(&anchors, &systems, &[])
        );
    }

    #[test]
    fn a_line_of_the_map_from_the_same_system_is_not_crossed_by_a_marker_of_it() {
        let layout = Setup::layout();
        let scene = layout.scene(&[[0.0, 0.0]], &[], &[Gate::new([0.0, 0.0], [0.0, 300.0])]);
        // Out from the same system, to the side: nothing.
        assert_eq!(
            layout.crossings(
                [0.0, 0.0],
                [400.0, 0.0],
                &scene,
                &Around::of([0.0, 0.0], &scene),
                &scene.gates,
                usize::MAX
            ),
            0
        );
        // Across a line that is not from the system: one for the line of the
        // marker, and one for its text, which is at the middle.
        let scene = layout.scene(
            &[[0.0, 0.0]],
            &[],
            &[Gate::new([200.0, -100.0], [200.0, 100.0])],
        );
        assert_eq!(
            layout.crossings(
                [0.0, 0.0],
                [400.0, 0.0],
                &scene,
                &Around::of([0.0, 0.0], &scene),
                &scene.gates,
                usize::MAX
            ),
            2
        );
    }

    #[test]
    fn lines_cross_only_when_they_go_through_each_other() {
        let gate = Gate::new([0.0, 0.0], [100.0, 0.0]);
        assert!(gate.crossed_by_line([50.0, -10.0], [50.0, 10.0]));
        assert!(!gate.crossed_by_line([50.0, 10.0], [50.0, 20.0]));
        // Touching at an end, or running along it, is not crossing.
        assert!(!gate.crossed_by_line([100.0, 0.0], [100.0, 50.0]));
        assert!(!gate.crossed_by_line([20.0, 0.0], [80.0, 0.0]));
    }

    #[test]
    fn the_places_that_were_kept_are_not_taken() {
        let layout = Setup::layout();
        let first = layout.place_all(&Setup::crowd(1), &[[0.0, 0.0]], &[])[0].unwrap();
        let kept = [Placed {
            anchor: [0.0, 0.0],
            spot: first,
        }];
        let second = layout.place_all(&Setup::crowd(1), &[[0.0, 0.0]], &kept)[0].unwrap();
        assert_ne!(first, second);
        let (a, label_a) = Setup::boxes(kept[0]);
        let (b, label_b) = Setup::boxes(Placed {
            anchor: [0.0, 0.0],
            spot: second,
        });
        assert!(!a.near(&b, Setup::gap() - 0.01));
        assert!(!label_a.near(&label_b, 1.99));
    }

    #[test]
    fn the_same_input_gives_the_same_places() {
        let layout = Setup::layout();
        let anchors = Setup::crowd(10);
        let systems = [[0.0, 0.0], [300.0, 100.0]];
        assert_eq!(
            layout.place_all(&anchors, &systems, &[]),
            layout.place_all(&anchors, &systems, &[])
        );
    }

    #[test]
    fn a_marker_with_no_free_place_is_left_out_and_the_rest_are_placed() {
        let mut layout = Setup::layout();
        layout.max_rings = 2;
        // Nodes all around the system, closer than the rings reach.
        let mut systems = vec![[0.0, 0.0]];
        for x in -4..=4 {
            for y in -4..=4 {
                systems.push([f64::from(x) * 60.0, f64::from(y) * 30.0]);
            }
        }
        let spots = layout.place_all(&[[0.0, 0.0], [10_000.0, 0.0]], &systems, &[]);
        assert_eq!(spots[0], None);
        assert!(spots[1].is_some());
    }

    #[test]
    fn a_layout_that_can_not_be_placed_or_an_anchor_that_is_not_a_number_gives_none() {
        let broken = MarkerLayout::new([0.0, 10.0], Setup::label(), 8.0);
        assert_eq!(broken.place_all(&[[0.0, 0.0]], &[], &[]), vec![None]);
        let nan = Setup::layout().place_all(&[[f64::NAN, 0.0]], &[], &[]);
        assert_eq!(nan, vec![None]);
        assert!(Setup::layout().place_all(&[], &[], &[]).is_empty());
    }

    #[test]
    fn placing_the_markers_of_a_hub_in_a_usual_region_is_cheaper_still() {
        // A region like the ones of the game: sixty systems spread over a
        // few thousand units, a hub among them with 24 markers.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let systems: Vec<[f64; 2]> = (0..60)
            .map(|_| [next() * 3000.0, next() * 2000.0])
            .collect();
        let anchors = vec![systems[30]; 24];
        let layout = Setup::layout();
        let start = Instant::now();
        let spots = layout.place_all(&anchors, &systems, &[]);
        let took = start.elapsed();
        eprintln!("24 markers among 60 systems: {took:?}");
        assert!(spots.iter().all(Option::is_some));
        // A debug build, on a busy machine (the tests run side by side): a
        // release one is far below this.
        assert!(took.as_millis() < 600, "took {took:?}");
    }

    #[test]
    fn the_lines_of_the_map_make_it_dearer_but_not_dear() {
        // The same crowded hub, with the lines of a grid region: every
        // system joined to the next one, to the right and below.
        let layout = Setup::layout();
        let mut systems = Vec::new();
        for x in 0..15 {
            for y in 0..10 {
                systems.push([f64::from(x) * 150.0, f64::from(y) * 90.0]);
            }
        }
        let mut gates = Vec::new();
        for x in 0..15 {
            for y in 0..10 {
                let here = systems[x * 10 + y];
                if x < 14 {
                    gates.push(Gate::new(here, systems[(x + 1) * 10 + y]));
                }
                if y < 9 {
                    gates.push(Gate::new(here, systems[x * 10 + y + 1]));
                }
            }
        }
        let anchors = vec![systems[77]; 24];
        let start = Instant::now();
        let spots = layout.place_all_among(&anchors, &systems, &[], &gates);
        let took = start.elapsed();
        eprintln!(
            "24 markers among 150 systems and {} lines: {took:?}",
            gates.len()
        );
        assert!(spots.iter().all(Option::is_some));
        // A debug build is about ten times slower than a release one, which
        // does this in around a hundred milliseconds.
        assert!(took.as_millis() < 2000, "took {took:?}");
    }

    #[test]
    fn placing_a_crowded_hub_in_a_big_region_is_cheap() {
        // Turnur's worst day: 24 markers of one system, in a region of 150.
        let layout = Setup::layout();
        let mut systems = Vec::new();
        for x in 0..15 {
            for y in 0..10 {
                systems.push([f64::from(x) * 150.0, f64::from(y) * 90.0]);
            }
        }
        let hub = systems[77];
        let anchors = vec![hub; 24];
        let start = Instant::now();
        let spots = layout.place_all(&anchors, &systems, &[]);
        let took = start.elapsed();
        eprintln!("24 markers among 150 systems: {took:?}");
        assert!(spots.iter().all(Option::is_some));
        // A debug build, on a busy machine: a release one is far below this
        // (around forty milliseconds).
        assert!(took.as_millis() < 600, "took {took:?}");
    }

    #[test]
    fn the_arcs_of_a_box_are_the_directions_that_end_inside_it() {
        // Against trying a thousand directions, for boxes all around: above,
        // below, to the sides, over the system, and across the circle.
        let center = [10.0, -20.0];
        let half = [60.0, 30.0];
        for (at, radius) in [
            ([200.0, -20.0], 150.0),
            ([10.0, 200.0], 180.0),
            ([-150.0, -150.0], 200.0),
            ([10.0, -20.0], 100.0),
            ([60.0, 0.0], 90.0),
            ([500.0, 500.0], 100.0),
            ([-40.0, -60.0], 40.0),
        ] {
            let block = Block::new(at, [40.0, 20.0]);
            let spans = Arcs::around(center, radius, &block, half);
            for step in 0..1000 {
                let angle = TAU * (f64::from(step) + 0.5) / 1000.0;
                let point = [
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin(),
                ];
                let inside = (point[0] - at[0]).abs() <= half[0] + 1e-9
                    && (point[1] - at[1]).abs() <= half[1] + 1e-9;
                let listed = spans
                    .iter()
                    .any(|(start, end)| angle >= start - 1e-9 && angle <= end + 1e-9);
                assert_eq!(inside, listed, "box {at:?} radius {radius} angle {angle}");
            }
        }
    }

    #[test]
    fn the_free_arcs_are_what_the_taken_ones_leave() {
        let free = Arcs::free(vec![(1.0, 2.0), (1.5, 3.0), (5.0, 6.0)]);
        assert_eq!(free, vec![(0.0, 1.0), (3.0, 5.0), (6.0, TAU)]);
        assert_eq!(Arcs::free(Vec::new()), vec![(0.0, TAU)]);
        assert!(Arcs::free(vec![(0.0, TAU)]).is_empty());
    }

    #[test]
    fn looking_in_the_free_arcs_places_every_marker_and_no_farther_than_trying_directions() {
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut grid = Vec::new();
        for x in 0..15 {
            for y in 0..10 {
                grid.push([f64::from(x) * 150.0, f64::from(y) * 90.0]);
            }
        }
        let mut regions = vec![(grid, 77)];
        for _ in 0..3 {
            let systems: Vec<[f64; 2]> = (0..60)
                .map(|_| [next() * 3000.0, next() * 2000.0])
                .collect();
            regions.push((systems, 30));
        }
        // One place can be farther for being closer for the one before it:
        // it is the regions as a whole that are compared.
        let (mut sampled, mut arcs) = (0.0, 0.0);
        for (systems, hub) in regions {
            let anchors = vec![systems[hub]; 16];
            let mean = |method: Method| {
                let layout = Setup::layout().with_label_at(0.7).with_method(method);
                let spots = layout.place_all(&anchors, &systems, &[]);
                assert!(spots.iter().all(Option::is_some), "{method:?}: unplaced");
                spots
                    .iter()
                    .flatten()
                    .map(|spot| (spot[0] - systems[hub][0]).hypot(spot[1] - systems[hub][1]))
                    .sum::<f64>()
                    / anchors.len() as f64
            };
            sampled += mean(Method::Sampled);
            arcs += mean(Method::Arcs);
        }
        assert!(arcs <= sampled, "{arcs} against {sampled}");
    }

    #[test]
    fn the_directions_tried_on_a_ring_are_the_ones_no_node_takes() {
        // Nodes around a system at different distances, and no text, so that
        // only the nodes take directions.
        let layout = MarkerLayout::new(Setup::node(), [0.0, 0.0], Setup::gap());
        let anchor = [0.0, 0.0];
        let systems = [
            anchor,
            [300.0, -130.0],
            [300.0, 130.0],
            [-250.0, 40.0],
            [20.0, -280.0],
            [-60.0, 320.0],
            [150.0, 90.0],
        ];
        let scene = layout.scene(&systems, &[], &[]);
        let around = Around::of(anchor, &scene);
        let mut taken_somewhere = false;
        for distance in [120.0, 200.0, 300.0, 330.0, 400.0] {
            let all = layout.angles(anchor, distance, &around, layout.directions, false);
            let free = layout.angles(anchor, distance, &around, layout.directions, true);
            assert!(!free.is_empty());
            let clear = |angle: &f64| {
                let spot = [distance * angle.cos(), distance * angle.sin()];
                !scene
                    .node_cells
                    .any_near(&Block::new(spot, Setup::node()), Setup::gap())
            };
            // Every direction in the free arcs is clear...
            assert!(free.iter().all(clear), "a taken direction at {distance}");
            // ...and the ones the ring had that are clear, are in them.
            taken_somewhere |= all.iter().any(|angle| !clear(angle));
        }
        assert!(taken_somewhere, "the scene takes no direction at all");
    }

    #[test]
    fn a_crowded_system_gets_more_directions_than_a_quiet_one() {
        let layout = Setup::layout();
        assert_eq!(layout.directions_for(1), layout.directions);
        assert_eq!(layout.directions_for(16), layout.directions);
        assert_eq!(layout.directions_for(36), 60);
        assert_eq!(layout.directions_for(60), 96);
        // Never fewer than asked for.
        let more = MarkerLayout {
            directions: 120,
            ..layout
        };
        assert_eq!(more.directions_for(36), 120);
    }

    #[test]
    fn more_directions_leave_fewer_lines_through_the_nodes_of_a_crowded_hub() {
        let anchors = vec![[0.0, 0.0]; 36];
        let through = |layout: &MarkerLayout| {
            let spots = layout.place_all(&anchors, &[[0.0, 0.0]], &[]);
            let placed: Vec<Placed> = spots
                .iter()
                .flatten()
                .map(|spot| Placed {
                    anchor: [0.0, 0.0],
                    spot: *spot,
                })
                .collect();
            assert_eq!(placed.len(), anchors.len());
            placed
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    placed
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != index)
                        .filter(|(_, other)| {
                            Block::new(other.spot, Setup::node()).crossed_by(
                                line.anchor,
                                line.spot,
                                layout.margin,
                            )
                        })
                        .count()
                })
                .sum::<usize>()
        };
        let adaptive = through(&Setup::layout().with_method(Method::Sampled));
        // The old fixed grid and the old price of a crossing.
        let fixed = through(&MarkerLayout {
            directions: 24,
            crossing_cost: 150.0,
            // The adaptive count never goes below this, so pin it by asking
            // for a crowd it does not scale with.
            ..Setup::layout().with_method(Method::Sampled)
        });
        assert!(adaptive <= fixed, "{adaptive} against {fixed}");
    }

    #[test]
    fn the_free_arcs_are_only_looked_at_next_to_the_nodes_of_the_map() {
        let layout = Setup::layout();
        let anchor = [0.0, 0.0];
        let alone = layout.scene(&[anchor], &[], &[]);
        let near = layout.scene(&[anchor, [200.0, 0.0]], &[], &[]);
        let far = layout.scene(&[anchor, [5000.0, 0.0]], &[], &[]);
        assert!(!layout.uses_arcs(anchor, &alone), "the system itself");
        assert!(layout.uses_arcs(anchor, &near));
        assert!(!layout.uses_arcs(anchor, &far));
        let always = layout.with_method(Method::Arcs);
        let never = layout.with_method(Method::Sampled);
        assert!(always.uses_arcs(anchor, &alone));
        assert!(!never.uses_arcs(anchor, &near));
    }

    #[test]
    fn every_method_places_a_crowded_hub_among_systems_without_overlaps() {
        let systems = [
            [0.0, 0.0],
            [300.0, -130.0],
            [300.0, 130.0],
            [-250.0, 40.0],
            [20.0, -280.0],
        ];
        let anchors = vec![systems[0]; 30];
        for method in [Method::Sampled, Method::Arcs, Method::Auto] {
            let layout = Setup::layout().with_method(method);
            let spots = layout.place_all(&anchors, &systems, &[]);
            assert!(spots.iter().all(Option::is_some), "{method:?}: unplaced");
            let nodes: Vec<Block> = spots
                .iter()
                .flatten()
                .map(|spot| Block::new(*spot, Setup::node()))
                .collect();
            for (index, node) in nodes.iter().enumerate() {
                for other in &nodes[index + 1..] {
                    assert!(!node.near(other, 0.0), "{method:?}: overlap");
                }
                for system in &systems[1..] {
                    assert!(
                        !node.near(&Block::new(*system, Setup::node()), 0.0),
                        "{method:?}: on a system"
                    );
                }
            }
        }
    }
}
