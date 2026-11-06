//! The heat layer of the maps: how busy each system was in the last hour,
//! drawn by egui-map as a [`LayerTemplate`].
//!
//! The heat is one hue, and how busy a system is shows in the size and the
//! intensity of its mark. The intensity builds up with the kills of the last
//! hour until it saturates ([`HeatStyle::saturation_kills`]); the size grows
//! with their square root (so the area of the mark is proportional to the
//! kills) and has no hard limit, so a very busy system keeps getting bigger. The scale is absolute: a system with
//! three kills looks the same in a quiet hour as in a busy one. A mark is a
//! disc that fades from its color at the centre to nothing at the edge. The universe map has no node boxes, so its
//! mark sits behind the dots ([`HeatMode::Halo`], `LayerSlot::BelowNodes`).
//! The region maps draw opaque boxes that would hide it, so there the mark is
//! inside each box, clipped to it ([`HeatMode::Tint`],
//! `LayerSlot::OverNodeBody`), which the region `Template` places between the
//! box's background and its name.
//!
//! The layer keeps its data behind an `Arc<RwLock<..>>` that a [`HeatHandle`]
//! shares, so whatever has the counts can hand them over without going
//! through the map. The layer itself stays an `Rc` in the map, like every
//! template of egui-map: the map is drawn on the interface thread only.

use super::intel::zkillboard::ZKillboardHandle;
use egui::epaint::Mesh;
use egui::{Color32, Painter, Pos2, Shape, Ui, vec2};
use egui_map::map::Map;
use egui_map::map::layer::{LayerContext, LayerNodeContext, LayerSlot, LayerTemplate};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// How the heat looks.
#[derive(Debug, Clone)]
pub(crate) struct HeatStyle {
    /// The color of a saturated mark; the alpha is its opacity at the centre.
    pub color: Color32,
    /// Kills of the last hour at which the color is as intense as it gets,
    /// and at which the disc has its base `radius`. The disc keeps growing
    /// above it, with the square root of the kills.
    pub saturation_kills: u32,
    /// Radius of the disc of a system with `saturation_kills`, in map units.
    pub radius: f32,
    /// The size of the disc of a system with the least heat (one kill), as a
    /// fraction of the base one (0.0 to 1.0).
    pub smallest: f32,
    /// The intensity of that same mark, as a fraction of a saturated one's
    /// (0.0 to 1.0): the color builds up from here.
    pub faintest: f32,
    /// A disc never shrinks below this many screen points, so it shows with
    /// the map zoomed out.
    pub min_screen_radius: f32,
    /// Where the tint of a node box meets the edge of the box, it fades to
    /// nothing over this fraction of the way from the middle to the edge
    /// (0.0 to 1.0: 0.0 is a hard edge).
    pub edge_fade: f32,
    /// How often the counts of the last hour are read again.
    pub refresh: Duration,
}

impl Default for HeatStyle {
    fn default() -> Self {
        Self {
            color: Color32::from_rgba_unmultiplied(235, 70, 40, 190),
            saturation_kills: 15,
            radius: 60.0,
            smallest: 0.25,
            faintest: 0.2,
            min_screen_radius: 5.0,
            edge_fade: 0.5,
            refresh: Duration::from_secs(15),
        }
    }
}

impl HeatStyle {
    /// How far `kills` take a mark towards saturation, from 0.0 (none) to
    /// 1.0 (`saturation_kills` or more).
    pub(crate) fn level(&self, kills: usize) -> f32 {
        (kills as f32 / self.saturation_kills.max(1) as f32).clamp(0.0, 1.0)
    }

    /// The size of the mark of `kills` as a fraction of the base one (the one
    /// of `saturation_kills`): `smallest + (1 - smallest) * sqrt(kills /
    /// saturation_kills)`. It is `smallest` with no kills and 1.0 at
    /// saturation, and past that it keeps growing with the square root, with
    /// no limit: the area of the mark is proportional to the kills.
    pub(crate) fn size_at(&self, kills: usize) -> f32 {
        let smallest = self.smallest.clamp(0.0, 1.0);
        let growth = (kills as f32 / self.saturation_kills.max(1) as f32).sqrt();
        smallest + (1.0 - smallest) * growth
    }

    /// The color of the centre of the mark at `level`: the saturated color
    /// multiplied by an intensity that builds up from `faintest` to 1.0.
    pub(crate) fn color_at(&self, level: f32) -> Color32 {
        let faintest = self.faintest.clamp(0.0, 1.0);
        self.color
            .gamma_multiply(faintest + (1.0 - faintest) * level.clamp(0.0, 1.0))
    }

    /// The radius of the disc behind a node with `kills`, in screen points.
    pub(crate) fn halo_radius(&self, kills: usize, zoom: f32) -> f32 {
        (self.radius * zoom * self.size_at(kills)).max(self.min_screen_radius)
    }
}

/// Where the heat is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeatMode {
    /// A disc behind the nodes.
    Halo,
    /// A circle inside the node boxes.
    Tint,
}

/// The kills of the last hour of every system.
#[derive(Debug, Default)]
struct HeatData {
    kills: HashMap<usize, usize>,
}

/// Hands counts to a [`HeatLayer`] from anywhere. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub(crate) struct HeatHandle {
    data: Arc<RwLock<HeatData>>,
}

impl HeatHandle {
    /// Replaces the heat with `counts` (kills per system).
    pub(crate) fn set_counts(&self, counts: impl IntoIterator<Item = (usize, usize)>) {
        let kills = counts.into_iter().filter(|(_, n)| *n > 0).collect();
        self.data
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kills = kills;
    }

    /// Removes all the heat.
    pub(crate) fn clear(&self) {
        self.data
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kills
            .clear();
    }

    /// The kills of `system` in the last hour, 0 when it is quiet.
    pub(crate) fn kills(&self, system: usize) -> usize {
        self.data
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .kills
            .get(&system)
            .copied()
            .unwrap_or(0)
    }
}

/// The layer egui-map paints.
#[derive(Debug)]
pub(crate) struct HeatLayer {
    handle: HeatHandle,
    style: HeatStyle,
    mode: HeatMode,
}

impl HeatLayer {
    /// Segments of the fan that makes a disc.
    const SEGMENTS: u32 = 20;

    pub(crate) fn new(mode: HeatMode, style: HeatStyle) -> Self {
        Self {
            handle: HeatHandle::default(),
            style,
            mode,
        }
    }

    /// Where the counts go.
    pub(crate) fn handle(&self) -> HeatHandle {
        self.handle.clone()
    }

    /// Adds to `mesh` a disc of `radius` around `centre`: `color` at the
    /// centre, fading to nothing at the edge.
    fn add_disc(mesh: &mut Mesh, centre: Pos2, radius: f32, color: Color32) {
        // Premultiplied colors: the rim fades the rgb out with the alpha.
        let rim = Color32::TRANSPARENT;
        let first = mesh.vertices.len() as u32;
        mesh.colored_vertex(centre, color);
        for step in 0..Self::SEGMENTS {
            let angle = std::f32::consts::TAU * step as f32 / Self::SEGMENTS as f32;
            mesh.colored_vertex(centre + vec2(angle.cos(), angle.sin()) * radius, rim);
        }
        for step in 0..Self::SEGMENTS {
            mesh.add_triangle(
                first,
                first + 1 + step,
                first + 1 + (step + 1) % Self::SEGMENTS,
            );
        }
    }

    /// Rays of the fan that makes a disc cut by an outline.
    const CUT_SEGMENTS: u32 = 64;

    /// How far from `centre` the ray `direction` (a unit vector) travels
    /// before it leaves the closed convex polygon `outline`, if it does.
    fn distance_to_outline(outline: &[Pos2], centre: Pos2, direction: egui::Vec2) -> Option<f32> {
        let cross = |a: egui::Vec2, b: egui::Vec2| a.x * b.y - a.y * b.x;
        let mut nearest: Option<f32> = None;
        for (index, start) in outline.iter().enumerate() {
            let end = outline[(index + 1) % outline.len()];
            let side = end - *start;
            let denominator = cross(direction, side);
            if denominator.abs() < 1e-6 {
                continue;
            }
            let from_centre = *start - centre;
            let along_ray = cross(from_centre, side) / denominator;
            let along_side = cross(from_centre, direction) / denominator;
            if along_ray > 0.0 && (0.0..=1.0).contains(&along_side) {
                nearest = Some(nearest.map_or(along_ray, |best| best.min(along_ray)));
            }
        }
        nearest
    }

    /// Adds to `mesh` the same disc as [`Self::add_disc`], cut by the closed
    /// convex polygon `outline` (which surrounds `centre`): along every ray
    /// the alpha fades with the distance to `centre`, reaching nothing at
    /// `radius`, and the ray ends where it meets the outline. There the tint
    /// is not cut: over the last `edge_fade` (0.0 to 1.0) of the way to the
    /// outline it fades to nothing, so it has no hard edge. Two rings of
    /// vertices make it: the first where the fade starts, with the alpha of
    /// the disc there, and the second at the end of the ray, transparent.
    fn add_disc_within(
        mesh: &mut Mesh,
        outline: &[Pos2],
        centre: Pos2,
        radius: f32,
        color: Color32,
        edge_fade: f32,
    ) {
        if outline.len() < 3 || radius <= 0.0 {
            return;
        }
        let fade = edge_fade.clamp(0.0, 1.0);
        let rays = Self::CUT_SEGMENTS;
        let first = mesh.vertices.len() as u32;
        mesh.colored_vertex(centre, color);
        // The colors of a mesh are premultiplied: a fade has to scale the rgb
        // with the alpha. Keeping the rgb of the centre while only the alpha
        // falls adds that color to what is behind, and the rim of the disc
        // shows as a hard edge.
        let faded = |distance: f32| color.gamma_multiply(1.0 - distance / radius);
        // The inner ring, then the outer one.
        let mut ends: Vec<Pos2> = Vec::with_capacity(rays as usize);
        for step in 0..rays {
            let angle = std::f32::consts::TAU * step as f32 / rays as f32;
            let direction = vec2(angle.cos(), angle.sin());
            let reach = Self::distance_to_outline(outline, centre, direction)
                .map_or(radius, |distance| distance.min(radius));
            let start = reach * (1.0 - fade);
            mesh.colored_vertex(centre + direction * start, faded(start));
            ends.push(centre + direction * reach);
        }
        for position in ends {
            mesh.colored_vertex(position, Color32::TRANSPARENT);
        }
        for step in 0..rays {
            let next = (step + 1) % rays;
            let (inner, inner_next) = (first + 1 + step, first + 1 + next);
            let (outer, outer_next) = (first + 1 + rays + step, first + 1 + rays + next);
            mesh.add_triangle(first, inner, inner_next);
            mesh.add_triangle(inner, outer, outer_next);
            mesh.add_triangle(inner, outer_next, inner_next);
        }
    }
}

impl LayerTemplate for HeatLayer {
    fn layer_ui(&self, painter: &Painter, ctx: LayerContext) {
        if self.mode != HeatMode::Halo {
            return;
        }
        let mut mesh = Mesh::default();
        for point in ctx.points {
            let kills = self.handle.kills(point.point.id);
            if kills == 0 {
                continue;
            }
            let level = self.style.level(kills);
            Self::add_disc(
                &mut mesh,
                point.position,
                self.style.halo_radius(kills, ctx.zoom),
                self.style.color_at(level),
            );
        }
        if !mesh.is_empty() {
            painter.add(Shape::mesh(mesh));
        }
    }

    fn node_ui(&self, painter: &Painter, ctx: LayerNodeContext) {
        if self.mode != HeatMode::Tint {
            return;
        }
        let kills = self.handle.kills(ctx.point.id);
        if kills == 0 {
            return;
        }
        let level = self.style.level(kills);
        // The mark grows from the middle of the box until it reaches the
        // corners; the box's own outline limits it, so it never paints over
        // the rounded corners or the border (a rectangular clip did).
        let bounds = ctx.outline.bounding_rect();
        let reach = bounds.size().length() * 0.5;
        let mut mesh = Mesh::default();
        Self::add_disc_within(
            &mut mesh,
            &ctx.outline.perimeter(),
            bounds.center(),
            reach * self.style.size_at(kills),
            self.style.color_at(level),
            self.style.edge_fade,
        );
        painter.add(Shape::mesh(mesh));
    }
}

/// What a map pane keeps of its heat: where the counts go and when they were
/// last read.
pub(crate) struct HeatControl {
    handle: HeatHandle,
    enabled: bool,
    read_at: Option<Instant>,
    refresh: Duration,
}

impl HeatControl {
    /// Id of the layer in the map.
    const LAYER: &'static str = "heat";

    /// Installs the heat in `map`: behind the nodes for [`HeatMode::Halo`],
    /// inside the node boxes for [`HeatMode::Tint`].
    pub(crate) fn install(map: &mut Map, mode: HeatMode) -> Self {
        let style = HeatStyle::default();
        let refresh = style.refresh;
        let layer = Rc::new(HeatLayer::new(mode, style));
        let slot = match mode {
            HeatMode::Halo => LayerSlot::BelowNodes,
            HeatMode::Tint => LayerSlot::OverNodeBody,
        };
        map.add_layer(Self::LAYER, slot, 0, layer.clone());
        // Off until the settings say so (see `set_enabled`).
        map.set_layer_visible(Self::LAYER, false);
        Self {
            handle: layer.handle(),
            enabled: false,
            read_at: None,
            refresh,
        }
    }

    /// Shows or hides the heat in `map`. Turning it on reads the kills again
    /// at the next frame.
    pub(crate) fn set_enabled(&mut self, map: &mut Map, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.read_at = None;
            map.set_layer_visible(Self::LAYER, enabled);
        }
        if !enabled {
            self.handle.clear();
        }
    }

    /// Reads the killmails of the last hour again when it is time. Does
    /// nothing while the heat is off.
    pub(crate) fn refresh(&mut self, ui: &Ui, zkillboard: Option<&ZKillboardHandle>) {
        if !self.enabled {
            return;
        }
        let due = self
            .read_at
            .is_none_or(|read| read.elapsed() >= self.refresh);
        if due {
            if let Some(zkillboard) = zkillboard {
                self.handle.set_counts(zkillboard.hour_kills());
            } else {
                self.handle.clear();
            }
            self.read_at = Some(Instant::now());
        }
        // Counts change slowly: look again when the next reading is due, not
        // every frame.
        ui.ctx().request_repaint_after(self.refresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tint_never_leaves_the_outline_and_fades_to_nothing() {
        // A square outline, the middle at the origin, and a disc that reaches
        // beyond its corners.
        let outline = [
            Pos2::new(-10.0, -10.0),
            Pos2::new(10.0, -10.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(-10.0, 10.0),
        ];
        let mut mesh = Mesh::default();
        HeatLayer::add_disc_within(
            &mut mesh,
            &outline,
            Pos2::ZERO,
            50.0,
            Color32::from_rgba_premultiplied(200, 0, 0, 100),
            0.5,
        );
        // The middle, a ring where the fade starts and a ring at the edge,
        // three triangles per ray.
        let rays = HeatLayer::CUT_SEGMENTS as usize;
        assert_eq!(mesh.vertices.len(), 2 * rays + 1);
        assert_eq!(mesh.indices.len(), rays * 9);
        assert_eq!(mesh.vertices[0].color.a(), 100);
        for vertex in &mesh.vertices {
            // Never outside the square.
            assert!(vertex.pos.x.abs() <= 10.001 && vertex.pos.y.abs() <= 10.001);
        }
        // The first ray goes along +x and meets the side at 10. The fade
        // starts halfway, at 5, where the disc keeps nine tenths of its
        // alpha; at the edge there is nothing: no hard edge.
        assert!((mesh.vertices[1].pos.x - 5.0).abs() < 1e-3);
        assert_eq!(mesh.vertices[1].color.a(), 90);
        let edge = &mesh.vertices[1 + rays];
        assert!((edge.pos.x - 10.0).abs() < 1e-3);
        assert_eq!(edge.color.a(), 0);
        assert!(
            mesh.vertices[rays + 1..]
                .iter()
                .all(|vertex| vertex.color.a() == 0)
        );
        // A disc smaller than the outline ends transparent, inside it.
        let mut small = Mesh::default();
        HeatLayer::add_disc_within(&mut small, &outline, Pos2::ZERO, 5.0, Color32::RED, 0.5);
        assert!(small.vertices[1 + rays..].iter().all(|v| v.color.a() == 0));
        assert!(
            small.vertices[1..]
                .iter()
                .all(|v| v.pos.to_vec2().length() <= 5.001)
        );
    }

    #[test]
    fn the_size_follows_the_square_root_and_is_the_base_one_at_saturation() {
        let style = HeatStyle {
            saturation_kills: 15,
            smallest: 0.25,
            ..HeatStyle::default()
        };
        // sqrt(15 / 15) = 1: the base size.
        assert!((style.size_at(15) - 1.0).abs() < 1e-6);
        // No kills is the smallest size.
        assert!((style.size_at(0) - 0.25).abs() < 1e-6);
        // Concave: the first kills add more than the later ones.
        let first = style.size_at(3) - style.size_at(0);
        let later = style.size_at(18) - style.size_at(15);
        assert!(first > later);
        // The area is proportional to the kills: four times the kills of
        // saturation make a disc twice as far above `smallest`.
        let growth = |kills| (style.size_at(kills) - 0.25) / 0.75;
        assert!((growth(60) - 2.0).abs() < 1e-5);
        assert!((growth(15) * growth(15) * 4.0 - growth(60) * growth(60)).abs() < 1e-4);
    }

    #[test]
    fn the_size_keeps_growing_past_saturation_with_no_limit() {
        let style = HeatStyle::default();
        let mut previous = style.size_at(style.saturation_kills as usize);
        for kills in [100, 1_000, 10_000, 1_000_000] {
            let size = style.size_at(kills);
            assert!(size > previous, "{kills} kills");
            previous = size;
        }
        assert!(previous > 1.0);
        assert!(previous.is_finite());
    }

    #[test]
    fn a_busier_system_gets_a_bigger_disc_and_zoom_scales_it() {
        let style = HeatStyle::default();
        assert!(style.halo_radius(30, 1.0) > style.halo_radius(3, 1.0));
        assert!(style.halo_radius(5, 2.0) > style.halo_radius(5, 1.0));
    }

    #[test]
    fn a_disc_never_shrinks_below_the_screen_minimum() {
        let style = HeatStyle::default();
        assert_eq!(style.halo_radius(100, 0.0001), style.min_screen_radius);
    }

    #[test]
    fn the_level_is_absolute_and_saturates() {
        let style = HeatStyle {
            saturation_kills: 10,
            ..HeatStyle::default()
        };
        assert_eq!(style.level(0), 0.0);
        assert!((style.level(5) - 0.5).abs() < 1e-6);
        assert_eq!(style.level(10), 1.0);
        // More kills do not make the color any more intense (the size does
        // keep growing, see `size_at`).
        assert_eq!(style.level(500), 1.0);
    }

    #[test]
    fn the_color_builds_up_from_faint_to_saturated() {
        let style = HeatStyle {
            faintest: 0.2,
            ..HeatStyle::default()
        };
        assert_eq!(style.color_at(1.0), style.color);
        assert!(style.color_at(0.0).a() < style.color_at(0.5).a());
        assert!(style.color_at(0.5).a() < style.color_at(1.0).a());
        // The hue is the same: only the intensity changes.
        assert!(style.color_at(0.0).a() > 0);
    }

    #[test]
    fn a_zero_saturation_does_not_divide_by_zero() {
        let style = HeatStyle {
            saturation_kills: 0,
            ..HeatStyle::default()
        };
        assert_eq!(style.level(1), 1.0);
    }

    #[test]
    fn the_handle_keeps_the_kills_as_they_are() {
        let heat = HeatHandle::default();
        heat.set_counts([(1, 99), (2, 3), (3, 0)]);
        assert_eq!(heat.kills(1), 99);
        assert_eq!(heat.kills(2), 3);
        // No kills, no heat.
        assert_eq!(heat.kills(3), 0);
        assert_eq!(heat.kills(4), 0);
    }

    #[test]
    fn new_counts_replace_the_old_ones_and_clear_empties_them() {
        let heat = HeatHandle::default();
        heat.set_counts([(1, 5)]);
        heat.set_counts([(2, 5)]);
        assert_eq!(heat.kills(1), 0);
        assert_eq!(heat.kills(2), 5);
        heat.clear();
        assert_eq!(heat.kills(2), 0);
    }

    #[test]
    fn a_handle_shares_its_data_with_the_layer() {
        let layer = HeatLayer::new(HeatMode::Halo, HeatStyle::default());
        let handle = layer.handle();
        handle.set_counts([(7, 3)]);
        assert_eq!(layer.handle().kills(7), 3);
    }
}
