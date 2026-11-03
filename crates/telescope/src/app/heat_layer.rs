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
        let rim = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
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
        // corners; the box clips it.
        let bounds = ctx.outline.bounding_rect();
        let reach = bounds.size().length() * 0.5;
        let mut mesh = Mesh::default();
        Self::add_disc(
            &mut mesh,
            bounds.center(),
            reach * self.style.size_at(kills),
            self.style.color_at(level),
        );
        painter
            .with_clip_rect(bounds.intersect(painter.clip_rect()))
            .add(Shape::mesh(mesh));
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
