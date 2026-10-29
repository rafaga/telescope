//! The map area of the main window, built on `egui_tiles`: `UniversePane` (the
//! whole universe) and `RegionPane` (a single region), both implementing
//! [`TabPane`], and `TreeBehavior`, which lays out the tabs and keeps the
//! per-region [`TileData`] (visibility, show on start-up).
//!
//! It also bridges the `sde` map points and connections into the types `egui-map`
//! renders.

use crate::app::messages::{MapSync, Message, Target, Type};
use crate::app::settings::NodeStyle;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, RichText, Sense, Shape,
    Stroke, Style, TextStyle, TextWrapMode, Ui, Vec2, WidgetText, epaint::RectShape, text::Galley,
    vec2,
};
use egui::PopupCloseBehavior;
use egui::containers::menu::{MenuButton, MenuConfig};
use egui_extras::{Column, TableBuilder};
use egui_map::map::{
    Map,
    minimap::MinimapSettings,
    objects::{
        HitContext, MapPoint, MapSegment, MapSettings, MarkerContext, NodeContext, NodeOutline,
        NodeTemplate, RegionLabel, VisibilitySetting,
    },
    overlay::Corner,
};
use egui_tiles::{Behavior, SimplificationOptions, TabState, TileId, Tiles, UiResponse};
use webb::map_alerts::{
    ALERT_ICON, AlertLog, AlertPart, CLEAR_ICON, IntelAlert, MAX_TOOLTIP_ALERTS, format_age,
};
//use futures::executor::ThreadPool;
use sde::SdeManager;
use sde::objects::{ProjectedAxis, SdePoint, SdeSegment};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use tokio::sync::broadcast::Receiver;

use super::intel::kill_context::{KillContext, Report, Source, Verdict};
use super::intel::kill_day::DayLine;
use super::intel::zkillboard::ZKillboardHandle;
use super::messages::MessageSpawner;

/// Bridges a single `sde` map point (native type, as of the `objects`
/// rewrite `sde` no longer depends on `egui-map` at all) into `egui-map`'s
/// own `MapPoint`, which the widget actually renders. Factored out of
/// [`sde_points_to_map`] so the per-point conversion logic has one home.
fn sde_point_to_map(id: usize, point: SdePoint) -> MapPoint {
    // `get_systems`/`get_abstract_systems` only ever fill the first two
    // components (`coords[2]` stays `0.0`), so dropping Z is the correct
    // 2D projection here.
    let coords = point.to_2d(ProjectedAxis::Z);
    let mut map_point = MapPoint::new(id, coords);
    if let Some(name) = point.name {
        map_point.set_name(name);
    }
    // `MapPoint::connections` is now `Vec<(usize, usize)>`, matching
    // `SdePoint::connections` exactly, so this is a plain move — no more
    // `format!`-based id synthesis needed.
    map_point.connections = point.connections;
    // `SdePoint::color` is the star's raw hex color (`sde` has no
    // rendering dependency, so it can't hand us a `Color32` directly);
    // parse it here, where it's actually needed. A missing star, or a
    // malformed hex value, both just fall back to `MapPoint`'s default
    // (`None`), which keeps the map's single default node color.
    map_point.color = point
        .color
        .as_deref()
        .and_then(|hex| Color32::from_hex(hex).ok());
    map_point
}

/// `HashMap<usize, SdePoint>` -> `HashMap<usize, MapPoint>`, for
/// [`Map::add_hashmap_points`] (used by both `UniversePane` and
/// `RegionPane`). Single pass; `collect()` into a `HashMap` from a
/// `HashMap`'s `into_iter()` already reserves capacity up front from the
/// exact `size_hint`, so there's no repeated reallocation as entries are
/// inserted.
fn sde_points_to_map(points: HashMap<usize, SdePoint>) -> HashMap<usize, MapPoint> {
    points
        .into_iter()
        .map(|(id, point)| (id, sde_point_to_map(id, point)))
        .collect()
}

/// Same idea as [`sde_point_to_map`], for `get_connections`/
/// `get_abstract_connections`. `MapSegment::id` is now `(usize, usize)`,
/// same as `SdeSegment::id` and the `HashMap` key itself, so `id` carries
/// straight across with no `Rc<str>`/`format!` synthesis.
fn sde_segment_to_map(id: (usize, usize), segment: SdeSegment) -> MapSegment {
    let point1 = [segment.point1[0] as f32, segment.point1[1] as f32];
    let point2 = [segment.point2[0] as f32, segment.point2[1] as f32];
    MapSegment::new(id, point1, point2)
}

/// `HashMap<(usize, usize), SdeSegment>` -> `HashMap<(usize, usize),
/// MapSegment>`, for [`Map::add_hashmap_lines`] (used by both
/// `UniversePane` and `RegionPane`).
fn sde_segments_to_map(
    segments: HashMap<(usize, usize), SdeSegment>,
) -> HashMap<(usize, usize), MapSegment> {
    segments
        .into_iter()
        .map(|(id, segment)| (id, sde_segment_to_map(id, segment)))
        .collect()
}

// use eframe::egui::include_image;
pub trait TabPane {
    fn ui(&mut self, ui: &mut Ui) -> UiResponse;
    fn get_title(&self) -> WidgetText;
    fn event_manager(&mut self);
    fn center_on_target(&mut self, message: (usize, Target));
    /// Places (or moves) the marker of a linked character on its solar
    /// system, and remembers its name for the node tooltip. Called directly
    /// by the app for every pane, visible or not -- see
    /// `TelescopeApp::update_player_location`.
    fn update_marker(&mut self, player_id: usize, system_id: usize, name: &str);
    /// Removes the marker of a character that is no longer linked.
    fn remove_marker(&mut self, player_id: usize);
    /// Redraws the nodes with `style` (Settings -> Maps). Maps that don't
    /// draw their nodes with it ignore it.
    fn set_node_style(&mut self, _style: NodeStyle) {}
    /// Loads the map's systems again from the SDE database at `path` (a
    /// rebuilt `sde.db`, or another one picked in Settings -> Application)
    /// and puts back the markers of the linked characters.
    fn reload_data(&mut self, path: &Path);
    /// Gives the map the zKillboard source, for the killmail context of its
    /// node tooltips. Maps that don't show it ignore it.
    fn set_zkillboard(&mut self, _handle: ZKillboardHandle) {}
}

/// Linked characters on a map, by character id: their solar system and name.
/// Each pane keeps its own copy for the node tooltips (the map itself only
/// knows the markers' ids).
type CharactersOnMap = HashMap<usize, (usize, String)>;

/// Icon shown to the left of each character name in the node tooltips.
pub(crate) const CHARACTER_ICON: &str = "👤";

/// How often a node with a linked character repaints its glow (~30 fps).
const GLOW_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Color of [`CLEAR_ICON`] in the node tooltips (a plain symbol, not a color
/// emoji: the others keep the colors of the emoji font).
const CLEAR_ICON_COLOR: Color32 = Color32::from_rgb(90, 200, 90);
/// Emoji of the ships section of a tooltip line.
const SHIP_ICON: &str = "🚀";
/// Emoji of the pilot-count section of a tooltip line.
const COUNT_ICON: &str = "👤";
/// Color of the word that says a system is active in the killmail context
/// (text, not an emoji).
const ACTIVE_COLOR: Color32 = Color32::from_rgb(235, 70, 40);

impl KillContext {
    /// The attackers' ships, as the alert lines show them: `Sabre, Rifter ×2`.
    fn ships_text(&self) -> String {
        self.ships
            .iter()
            .map(|(name, times)| {
                if *times > 1 {
                    format!("{name} ×{times}")
                } else {
                    name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The line with the killmails of the last day, if there is anything to
    /// say about them.
    fn show_day(&self, ui: &mut Ui) {
        match self.day {
            Some(DayLine::Known(count)) => {
                ui.weak(match (count.kills, count.more) {
                    (_, true) => t!("map.kills_day_more", count = count.kills),
                    (1, false) => t!("map.kills_day_one"),
                    (kills, false) => t!("map.kills_day_other", count = kills),
                });
            }
            Some(DayLine::Pending) => {
                ui.weak(t!("map.kills_day_pending"));
            }
            Some(DayLine::Unavailable) | None => {}
        }
    }

    /// The neighbouring systems with killmails: `2 jumps · Nourvukaiken · 1
    /// kill, 30m ago`.
    fn show_neighbours(&self, ui: &mut Ui) {
        for neighbour in &self.neighbours {
            let ago = format_age(neighbour.last_age.to_std().unwrap_or_default());
            let name = if neighbour.name.is_empty() {
                neighbour.system_id.to_string()
            } else {
                neighbour.name.clone()
            };
            let text = match (neighbour.jumps, neighbour.kills) {
                (1, 1) => t!("map.kills_near_1_one", name = name, ago = ago),
                (1, kills) => t!(
                    "map.kills_near_1_other",
                    name = name,
                    count = kills,
                    ago = ago
                ),
                (jumps, 1) => t!("map.kills_near_one", name = name, jumps = jumps, ago = ago),
                (jumps, kills) => t!(
                    "map.kills_near_other",
                    name = name,
                    jumps = jumps,
                    count = kills,
                    ago = ago
                ),
            };
            ui.weak(text);
        }
    }

    /// The block of a node tooltip that sets a report against the killmails
    /// of the last hour: whether they back it, how much is happening in the
    /// system, and who is shooting and who dies.
    fn show(&self, ui: &mut Ui) {
        ui.weak(match self.source {
            Source::Feed => t!("map.kills_header"),
            Source::Api => t!("map.kills_header_api"),
        });
        if self.loading && self.hour == 0 {
            ui.weak(t!("map.kills_loading"));
            return;
        }
        if let Some(fill) = self.fill {
            ui.weak(t!("map.kills_filling", percent = fill.percent()));
        }
        let ago = self
            .last_age
            .map(|age| format_age(age.to_std().unwrap_or_default()))
            .unwrap_or_default();
        let count_text = |count: usize, recent: bool| match (recent, count) {
            (true, 1) => t!("map.kills_recent_one", ago = ago),
            (true, _) => t!("map.kills_recent_other", count = count, ago = ago),
            (false, 1) => t!("map.kills_hour_one", ago = ago),
            (false, _) => t!("map.kills_hour_other", count = count, ago = ago),
        };
        match self.verdict {
            Verdict::Corroborated => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(CLEAR_ICON).color(CLEAR_ICON_COLOR));
                    ui.label(RichText::new(t!("map.kills_corroborated")).color(CLEAR_ICON_COLOR));
                    ui.label(RichText::new("·").weak());
                    ui.label(count_text(self.recent, true));
                });
            }
            Verdict::Active => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(ALERT_ICON);
                    ui.label(RichText::new(t!("map.kills_active")).color(ACTIVE_COLOR));
                    ui.label(RichText::new("·").weak());
                    ui.label(count_text(self.hour, false));
                });
            }
            Verdict::Quiet => {
                ui.label(t!("map.kills_quiet"));
                self.show_day(ui);
                self.show_neighbours(ui);
                ui.weak(t!("map.kills_quiet_hint"));
                return;
            }
        }
        if !self.ships.is_empty() {
            ui.horizontal(|ui| {
                ui.label(SHIP_ICON);
                ui.label(self.ships_text());
                ui.label(RichText::new("·").weak());
                ui.label(COUNT_ICON);
                ui.label(match self.pilots {
                    1 => t!("map.kills_pilots_one").into_owned(),
                    count => t!("map.kills_pilots_other", count = count).into_owned(),
                });
                if self.verdict == Verdict::Corroborated {
                    ui.weak(t!("map.kills_matches"));
                }
            });
        }
        if !self.victims.is_empty() {
            ui.horizontal(|ui| {
                ui.weak(t!("map.kills_victims"));
                ui.label(self.victims.join(", "));
            });
        }
        self.show_day(ui);
        self.show_neighbours(ui);
    }
}

/// The tooltip of the node under the pointer, only on nodes that have
/// something to show: the linked characters in that system, one per line
/// with [`CHARACTER_ICON`] as a marker (no header, to keep it short), then
/// its active intel alerts, newest first (see `map_alerts`).
fn show_node_tooltip(
    response: Response,
    map: &Map,
    characters: &CharactersOnMap,
    alerts: &mut AlertLog,
    kills: Option<&ZKillboardHandle>,
) {
    let Some(system_id) = map.hovered_node() else {
        return;
    };
    let mut names: Vec<&str> = characters
        .values()
        .filter(|(system, _)| *system == system_id)
        .map(|(_, name)| name.as_str())
        .collect();
    let now = Instant::now();
    let active = alerts.active(system_id, now);
    if names.is_empty() && active.is_empty() {
        return;
    }
    names.sort_unstable();
    // The killmails are set against the newest report that raises an alert (a
    // clear report has nothing to confirm). An alert that comes from a
    // killmail is no report: it would be corroborated by itself. With only
    // killmail alerts the system is just active, and with only clear reports
    // there is nothing to say.
    let report = active
        .iter()
        .find(|alert| alert.raises_visual() && !alert.killmail);
    let has_killmail = active.iter().any(|alert| alert.killmail);
    let context = kills
        .filter(|_| report.is_some() || has_killmail)
        .and_then(|handle| {
            let report = report.map_or_else(Report::default, |alert| {
                Report::from_summary(&alert.summary)
            });
            handle.context(system_id, &report)
        });
    response.on_hover_ui_at_pointer(|ui| {
        for name in &names {
            ui.label(format!("{CHARACTER_ICON} {name}"));
        }
        if active.is_empty() {
            return;
        }
        if !names.is_empty() {
            ui.separator();
        }
        for alert in active.iter().take(MAX_TOOLTIP_ALERTS) {
            alert_line(ui, alert, now);
        }
        if active.len() > MAX_TOOLTIP_ALERTS {
            ui.weak(t!(
                "map.alert_more",
                count = active.len() - MAX_TOOLTIP_ALERTS
            ));
        }
        if let Some(context) = &context {
            ui.separator();
            context.show(ui);
        }
        // The ages tick even when nothing else repaints (a clear report
        // has no animation).
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    });
}

/// One intel alert in a node tooltip: icon, age and summary. Each section of
/// the summary is prefixed with its own emoji (ships, pilot count) unless the
/// alert was built with `emojis` off.
fn alert_line(ui: &mut Ui, alert: &IntelAlert, now: Instant) {
    ui.horizontal(|ui| {
        if alert.raises_visual() {
            ui.label(ALERT_ICON);
        } else {
            ui.label(RichText::new(CLEAR_ICON).color(CLEAR_ICON_COLOR));
        }
        ui.label(RichText::new(format_age(now.saturating_duration_since(alert.received))).weak());
        let parts = alert.summary.parts(|count| match count {
            1 => t!("map.alert_pilots_one").into_owned(),
            _ => t!("map.alert_pilots_other", count = count).into_owned(),
        });
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                ui.label(RichText::new("·").weak());
            }
            match part {
                AlertPart::Clear(word) => {
                    ui.label(word);
                }
                AlertPart::Ships(text) => {
                    if alert.emojis {
                        ui.label(SHIP_ICON);
                    }
                    ui.label(text);
                }
                AlertPart::Count(text) => {
                    if alert.emojis {
                        ui.label(COUNT_ICON);
                    }
                    ui.label(text);
                }
                AlertPart::Text(text) => {
                    ui.label(text);
                }
            }
        }
    });
}

/// Starts an intel alert's visual on the universe map: a single pulse when
/// the report arrives. Unlike [`repeating_pulse_alert`] it does not repeat
/// for the alert's duration (that only governs its tooltip entry).
fn pulse_alert(map: &mut Map, alert: &IntelAlert) {
    if let Some(node) = map.node(alert.system_id) {
        node.pulse(alert.received);
    }
}

/// Starts an intel alert's visual on a region's map: a pulse repeated for
/// its duration, fading out over that time (egui-map's lasting
/// notifications).
fn repeating_pulse_alert(map: &mut Map, alert: &IntelAlert) {
    if let Some(node) = map.node(alert.system_id) {
        node.lasting(alert.duration).pulse(alert.received);
    }
}

/// Lists an intel line in a pane's node tooltips (no visual alert).
fn push_alert(alerts: &mut AlertLog, alert: IntelAlert) {
    alerts.push(alert);
}

pub struct UniversePane {
    map: Map,
    /// Whether `generate_data` loaded the systems into `map`. Without them
    /// (no usable `sde.db`: first run, or while `DatabaseUpdater` rebuilds
    /// it) markers are not passed on: egui-map 0.9.1 panics drawing a marker
    /// on a map that has no points (`map.rs`, `self.points.as_ref().unwrap()`
    /// in the marker loop). An empty map has nowhere to put them anyway.
    has_points: bool,
    mapsync_reciever: Receiver<MapSync>,
    /// See [`CharactersOnMap`].
    characters: CharactersOnMap,
    /// Intel alerts for the node tooltips.
    alerts: AlertLog,
    /// Where the killmail context of the node tooltips comes from.
    zkillboard: Option<ZKillboardHandle>,
    //generic_sender: Arc<Sender<Message>>,
    path: PathBuf,
    factor: f64,
    task_msg: Arc<MessageSpawner>,
    //tpool: Rc<ThreadPool>,
}

impl UniversePane {
    #[tracing::instrument(skip(receiver, task_msg))]
    pub fn new(
        receiver: Receiver<MapSync>,
        path: PathBuf,
        factor: f64,
        task_msg: Arc<MessageSpawner>,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor,
            task_msg,
            has_points: false,
            characters: HashMap::new(),
            alerts: AlertLog::default(),
            zkillboard: None,
        };
        object.generate_data();
        object.map.settings = MapSettings::default();
        object.map.settings.region_label_alpha = 0.25;
        object.map.settings.style.region_label_font =
            FontId::new(96.0, egui::FontFamily::Name("Custom".into()));
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        object.map.settings.minimap = MinimapSettings::default();
        object.map.settings.minimap.anchor = Corner::BottomRight;
        object.map.settings.minimap.scale = 0.35;
        object.map.settings.minimap.enabled = true;
        object.map.settings.slider.anchor = Corner::TopRight;
        object.map.settings.line_visible_zoom = 0.10;
        object
    }

    #[tracing::instrument(skip(self))]
    fn generate_data(&mut self) {
        if let Ok(t_sde) = SdeManager::new(&self.path, self.factor) {
            if let Ok(points) = t_sde.get_systems() {
                self.map.add_hashmap_points(sde_points_to_map(points));
                self.has_points = true;
            }
            if let Ok(hash_conns) = t_sde.get_connections() {
                self.map.add_hashmap_lines(sde_segments_to_map(hash_conns));
            }
        }
        if let Ok(t_sde) = SdeManager::new(&self.path, self.factor)
            && let Ok(region_areas) = t_sde.get_region_coordinates()
        {
            // Region names are backdrop for the whole-galaxy view, not
            // per-node annotations -- `RegionLabel`/`add_region_labels`
            // (egui-map 0.9) instead of the free-floating `MapLabel` this
            // used to piggyback on: it scales with zoom rather than staying
            // a fixed screen size, is always painted behind everything
            // else, and is drawn in a faded color by default, so it reads
            // as the region's name rather than competing with system names.
            let mut labels = Vec::new();
            for region in region_areas {
                let mut label = RegionLabel::new();
                label.text = region.name;
                // Sit the label at the centre of the region's bounding box,
                // in the same scaled map space the nodes use: `get_systems`
                // divides by `factor` and applies the same inversion
                // `get_region_coordinates` does, so `egui-map` can project it
                // with pan/zoom like any node. Using a corner (`region.min`)
                // would pin it to the region's edge instead.

                label.center = Pos2::new(
                    ((region.min.x() + region.max.x()) / 2.0 / self.factor) as f32,
                    ((region.min.y() + region.max.y()) / 2.0 / self.factor) as f32,
                );
                labels.push(label);
            }
            self.map.add_region_labels(labels);
        }
    }
}

impl TabPane for UniversePane {
    #[tracing::instrument(skip(self))]
    fn reload_data(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.has_points = false;
        self.generate_data();
        if self.has_points {
            for (player_id, (system_id, _)) in &self.characters {
                self.map.update_marker(*player_id, *system_id);
            }
        }
    }

    fn update_marker(&mut self, player_id: usize, system_id: usize, name: &str) {
        self.characters
            .insert(player_id, (system_id, name.to_owned()));
        if self.has_points {
            self.map.update_marker(player_id, system_id);
        }
    }

    fn remove_marker(&mut self, player_id: usize) {
        self.characters.remove(&player_id);
        self.map.remove_marker(player_id);
    }

    fn set_zkillboard(&mut self, handle: ZKillboardHandle) {
        self.zkillboard = Some(handle);
    }

    #[tracing::instrument(skip(self, ui))]
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.event_manager();
        let response = ui.add(&mut self.map);
        show_node_tooltip(
            response,
            &self.map,
            &self.characters,
            &mut self.alerts,
            self.zkillboard.as_ref(),
        );
        UiResponse::None
    }

    #[tracing::instrument(skip(self))]
    fn get_title(&self) -> WidgetText {
        t!("map.universe").into_owned().into()
    }

    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        // `while let`, not `if let`: `mapsync_reciever` is a broadcast
        // channel that keeps filling up whether or not this pane's `ui()`
        // gets called every frame (e.g. while its tab is hidden behind
        // another one). A single `try_recv()` only ever drains one message
        // per frame, so a backlog built up while hidden -- or just a burst
        // of updates -- would fall permanently behind instead of catching up
        // once this pane is visible again.
        while let Ok(msg) = self.mapsync_reciever.try_recv() {
            match msg {
                MapSync::NodeEffect((system_id, effect)) => {
                    if let Some(node) = self.map.node(system_id) {
                        effect.apply(node);
                    }
                }
                MapSync::SystemAlert(alert) => {
                    pulse_alert(&mut self.map, &alert);
                }
                MapSync::SystemTooltip(alert) => {
                    push_alert(&mut self.alerts, alert);
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
            };
        }
    }

    #[tracing::instrument(skip(self, message))]
    fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                // Center through the widget's own node index instead of
                // re-reading coordinates from the SDE.
                //
                // This used to call `get_system_coords()` (which reads
                // `centerX/centerY/centerZ`) and project it with
                // `to_2d(ProjectedAxis::Y)` -> `(centerX, centerZ)`. But the
                // nodes on this map come from `get_systems()`, which reads
                // `position2DX/position2DY` -- and those are NOT `(centerX,
                // centerZ)`: the SDE builder computes them with
                // `isometric_projection_2d()`, which for `ProjectedAxis::Y` is
                // `(x - y, z + (x + y) / 2)`. A shear, not an axis drop; the
                // two agree only at the origin. So the view was being moved to
                // a point in a coordinate space no node lives in, and the
                // target system ended up off-screen.
                //
                // `set_pos_from_nodeid` uses the coordinates already loaded
                // into the widget, so it cannot disagree with them. It returns
                // `false` when the id was never loaded -- which is meaningful
                // here, since this map only carries K-space systems that have
                // at least one stargate connection.
                if !self.map.set_pos_from_nodeid(message.0) {
                    let mut msg = String::from("System with Id ");
                    msg += (message.0.to_string() + " is not present on this map").as_str();
                    self.task_msg.spawn(Message::GenericNotification((
                        Type::Warning,
                        String::from("UniversePane"),
                        String::from("center_on_target"),
                        msg,
                    )));
                }
            }
            Target::Region => {}
        }
    }
}

pub struct RegionPane {
    map: Map,
    /// Whether `generate_data` loaded the systems into `map`. Without them
    /// (no usable `sde.db`: first run, or while `DatabaseUpdater` rebuilds
    /// it) markers are not passed on: egui-map 0.9.1 panics drawing a marker
    /// on a map that has no points (`map.rs`, `self.points.as_ref().unwrap()`
    /// in the marker loop). An empty map has nowhere to put them anyway.
    has_points: bool,
    mapsync_reciever: Receiver<MapSync>,
    path: PathBuf,
    factor: f64,
    region_id: usize,
    tab_name: String,
    task_msg: Arc<MessageSpawner>,
    /// See [`CharactersOnMap`].
    characters: CharactersOnMap,
    /// Intel alerts for the node tooltips.
    alerts: AlertLog,
    /// Where the killmail context of the node tooltips comes from.
    zkillboard: Option<ZKillboardHandle>,
}

impl RegionPane {
    #[tracing::instrument(skip(receiver, task_msg))]
    pub fn new(
        receiver: Receiver<MapSync>,
        path: PathBuf,
        factor: f64,
        region_id: usize,
        task_msg: Arc<MessageSpawner>,
        node_style: NodeStyle,
    ) -> Self {
        let mut object = Self {
            map: Map::new(),
            mapsync_reciever: receiver,
            path,
            factor,
            region_id,
            tab_name: String::from("Region"),
            task_msg,
            has_points: false,
            characters: HashMap::new(),
            alerts: AlertLog::default(),
            zkillboard: None,
        };
        object.generate_data();
        object.map.settings = MapSettings::default();
        object.map.settings.node_text_visibility = VisibilitySetting::Hover;
        // A region is a much smaller area than the whole universe, so the
        // intel alert pulse spreads over a shorter radius here (default is
        // 40.0) -- otherwise it covers the neighbouring systems on the
        // tighter regional view.
        object.map.settings.animation.pulse.spread = 12.0;
        object
            .map
            .set_node_template(Rc::new(Template::new(node_style)));
        object.map.settings.line_visible_zoom = 0.10;
        object
    }

    #[tracing::instrument(skip(self))]
    fn generate_data(&mut self) {
        let t_sde = match SdeManager::new(&self.path, self.factor) {
            Ok(t_sde) => t_sde,
            Err(t_err) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    "RegionPane".to_string(),
                    "generate_data".to_string(),
                    t_err.to_string(),
                )));
                return;
            }
        };

        match t_sde.get_abstract_systems(vec![self.region_id as u32]) {
            Ok(points) => {
                self.map.add_hashmap_points(sde_points_to_map(points));
                self.has_points = true;
                if let Ok(lines) = t_sde.get_abstract_connections(vec![self.region_id as u32]) {
                    self.map.add_hashmap_lines(sde_segments_to_map(lines));
                }
            }
            Err(t_err) => {
                self.task_msg.spawn(Message::GenericNotification((
                    Type::Error,
                    "RegionPane".to_string(),
                    "generate_data".to_string(),
                    t_err.to_string(),
                )));
                return;
            }
        }
        // A region missing from the database (it can change under a pane,
        // see `TabPane::reload_data`) keeps the name the tab had.
        if let Ok(region) = t_sde.get_region(vec![self.region_id as u32], None)
            && let Some(region) = region.values().next()
        {
            self.tab_name.clone_from(&region.name);
        }
    }
}

impl TabPane for RegionPane {
    // Characters are painted by `Template::node_ui` (a glow over the node,
    // from `NodeContext::marker`), not by `marker_ui`, which egui-map draws
    // after every node, on top of the node's label.
    fn update_marker(&mut self, player_id: usize, system_id: usize, name: &str) {
        self.characters
            .insert(player_id, (system_id, name.to_owned()));
        if self.has_points {
            self.map.update_marker(player_id, system_id);
        }
    }

    fn remove_marker(&mut self, player_id: usize) {
        self.characters.remove(&player_id);
        self.map.remove_marker(player_id);
    }

    fn set_zkillboard(&mut self, handle: ZKillboardHandle) {
        self.zkillboard = Some(handle);
    }

    fn set_node_style(&mut self, style: NodeStyle) {
        self.map.set_node_template(Rc::new(Template::new(style)));
    }

    #[tracing::instrument(skip(self))]
    fn reload_data(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.has_points = false;
        self.generate_data();
        if self.has_points {
            for (player_id, (system_id, _)) in &self.characters {
                self.map.update_marker(*player_id, *system_id);
            }
        }
    }

    #[tracing::instrument(skip(self))]
    fn event_manager(&mut self) {
        // See `UniversePane::event_manager`'s comment: `while let`, not
        // `if let`, so a backlog from being hidden or from a burst of
        // updates gets fully drained instead of leaking one message behind
        // every frame.
        while let Ok(msg) = self.mapsync_reciever.try_recv() {
            match msg {
                MapSync::NodeEffect((system_id, effect)) => {
                    if let Some(node) = self.map.node(system_id) {
                        effect.apply(node);
                    }
                }
                MapSync::SystemAlert(alert) => {
                    repeating_pulse_alert(&mut self.map, &alert);
                }
                MapSync::SystemTooltip(alert) => {
                    push_alert(&mut self.alerts, alert);
                }
                MapSync::CenterOn(message) => {
                    let t_msg = message.clone();
                    self.center_on_target(t_msg);
                }
            };
        }
    }

    #[tracing::instrument(skip(self))]
    fn get_title(&self) -> WidgetText {
        self.tab_name.clone().into()
    }

    #[tracing::instrument(skip(self, ui))]
    fn ui(&mut self, ui: &mut Ui) -> UiResponse {
        self.event_manager();
        let response = ui.add(&mut self.map);
        show_node_tooltip(
            response,
            &self.map,
            &self.characters,
            &mut self.alerts,
            self.zkillboard.as_ref(),
        );
        UiResponse::None
    }

    #[tracing::instrument(skip(self, message))]
    fn center_on_target(&mut self, message: (usize, Target)) {
        match message.1 {
            Target::System => {
                self.map.set_pos_from_nodeid(message.0);
            }
            Target::Region => {}
        }
    }
}

pub struct TileData {
    tile_id: Option<TileId>,
    name: String,
    visible: bool,
    pub(crate) show_on_startup: bool,
}

impl TileData {
    #[tracing::instrument]
    pub fn new(name: String, show_on_startup: bool) -> Self {
        Self {
            tile_id: None,
            name,
            visible: false,
            show_on_startup,
        }
    }

    // Deliberately uninstrumented from here down: these are single-field
    // accessors called many times per frame (the settings window walks every
    // region's `TileData` on every repaint). A Tracy zone costs orders of
    // magnitude more than the field access it would be measuring, so spans
    // here only bury the zones that matter.
    pub fn set_visible(&mut self, value: bool) {
        self.visible = value;
    }

    pub fn get_visible(&self) -> bool {
        self.visible
    }

    pub fn set_tile_id(&mut self, value: Option<TileId>) {
        self.tile_id = value;
    }

    pub fn get_tile_id(&self) -> Option<TileId> {
        self.tile_id
    }

    pub fn get_name(&self) -> String {
        self.name.clone()
    }
}

pub struct TreeBehavior {
    simplification_options: SimplificationOptions,
    tab_bar_height: f32,
    gap_width: f32,
    task_msg: Arc<MessageSpawner>,
    search_text: String,
    factor: f64,
    path: PathBuf,
    search_regions: Vec<usize>,
    pub tile_data: HashMap<usize, TileData>,
}

impl TreeBehavior {
    #[tracing::instrument(skip(task_msg))]
    pub fn new(task_msg: Arc<MessageSpawner>, factor: f64, path: PathBuf) -> Self {
        Self {
            simplification_options: SimplificationOptions {
                prune_empty_containers: true,
                prune_single_child_containers: true,
                prune_empty_tabs: true,
                prune_single_child_tabs: false,
                all_panes_must_have_tabs: false,
                join_nested_linear_containers: true,
                flatten_tabs_in_tabs: true,
            },
            tab_bar_height: 24.0,
            gap_width: 2.0,
            task_msg,
            factor,
            path,
            search_text: String::new(),
            tile_data: HashMap::new(),
            search_regions: Vec::new(),
        }
    }

    /// The SDE database the region search reads (Settings -> Application).
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

    #[tracing::instrument(skip(self))]
    fn toggle_regions(&mut self, region_id: usize) {
        let visible = self.tile_data.get_mut(&region_id).unwrap().get_visible();
        let tile_id = self.tile_data.get_mut(&region_id).unwrap().get_tile_id();
        //let ttx = Arc::clone(&self.generic_sender);
        if visible {
            if tile_id.is_some() {
                self.task_msg.spawn(Message::MapShown(region_id));
            } else {
                self.task_msg.spawn(Message::NewRegionalPane(region_id));
            }
        } else {
            self.task_msg.spawn(Message::MapHidden(region_id));
        }
    }
}

impl TreeBehavior {
    /// Corner radius of the folder-style tabs.
    const TAB_RADIUS: f32 = 6.0;
    /// Side of the close button's hit area.
    const TAB_CLOSE_SIZE: f32 = 16.0;
    /// Space between the title and the close button.
    const TAB_CLOSE_GAP: f32 = 4.0;
    /// How much lower the inactive tabs sit, as the tabs of a folder do.
    const TAB_INACTIVE_DROP: f32 = 2.0;

    /// The outline of a tab: up the left side, over the top with rounded
    /// corners and down the right side. It is open at the bottom, where an
    /// active tab joins the content.
    fn tab_outline(rect: egui::Rect, radius: f32) -> Vec<egui::Pos2> {
        const STEPS: usize = 6;
        let mut points = vec![egui::pos2(rect.left(), rect.bottom())];
        let corners = [
            (
                egui::pos2(rect.left() + radius, rect.top() + radius),
                std::f32::consts::PI,
            ),
            (
                egui::pos2(rect.right() - radius, rect.top() + radius),
                1.5 * std::f32::consts::PI,
            ),
        ];
        for (center, start) in corners {
            for step in 0..=STEPS {
                let angle = start + std::f32::consts::FRAC_PI_2 * step as f32 / STEPS as f32;
                points.push(egui::pos2(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                ));
            }
        }
        points.push(egui::pos2(rect.right(), rect.bottom()));
        points
    }

    #[tracing::instrument(skip_all)]
    fn on_close_tab(&self, tile_id: TileId, button_response: Response) {
        if button_response.clicked() {
            for tile in self.tile_data.iter() {
                if tile.1.get_tile_id() == Some(tile_id) {
                    //let ttx = Arc::clone(&self.generic_sender);
                    let region_id = *tile.0;
                    /*let future = async move {
                        let _x = ttx.send(Message::MapHidden(region_id)).await;
                    };
                    self.tpool.spawn_ok(future);*/
                    self.task_msg.spawn(Message::MapHidden(region_id));
                }
            }
        }
    }
}

impl Behavior<Box<dyn TabPane>> for TreeBehavior {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        view: &mut Box<dyn TabPane>,
    ) -> UiResponse {
        view.ui(ui)
    }

    #[tracing::instrument(skip_all)]
    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        id: eframe::egui::Id,
        tile_id: TileId,
        tab_state: &TabState,
    ) -> eframe::egui::Response {
        let text = self.tab_title_for_tile(tiles, tile_id);
        let str_text = text.text().to_string();
        let font_id = TextStyle::Button.resolve(ui.style());
        let galley = text.into_galley(ui, Some(TextWrapMode::Truncate), f32::INFINITY, font_id);

        // The Universe tab is permanent, and a tab being dragged has no button.
        let closable = str_text != t!("map.universe") && !tab_state.is_being_dragged;
        let x_margin = self.tab_title_spacing(ui.visuals());
        let close_size = Self::TAB_CLOSE_SIZE;
        let width = if closable {
            x_margin + galley.size().x + Self::TAB_CLOSE_GAP + close_size + 0.5 * x_margin
        } else {
            galley.size().x + 2.0 * x_margin
        };
        // One rectangle for the whole tab, title and button: no division.
        let rect = ui.allocate_space(vec2(width, ui.available_height())).1;
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let close_rect = egui::Rect::from_center_size(
            egui::pos2(
                rect.right() - 0.5 * x_margin - 0.5 * close_size,
                rect.center().y + 0.5 * Self::TAB_INACTIVE_DROP * f32::from(!tab_state.active),
            ),
            vec2(close_size, close_size),
        );
        let close_response =
            closable.then(|| ui.interact(close_rect, id.with("close"), Sense::click()));

        // An inactive tab sits a little lower than the active one.
        let shape_rect = if tab_state.active {
            rect
        } else {
            egui::Rect::from_min_max(
                egui::pos2(rect.min.x, rect.min.y + Self::TAB_INACTIVE_DROP),
                rect.max,
            )
        };
        let text_color = self.tab_text_color(ui.visuals(), tiles, tile_id, tab_state);
        // Show a gap when dragged
        if ui.is_rect_visible(rect) && !tab_state.is_being_dragged {
            let mut bg_color = self.tab_bg_color(ui.visuals(), tiles, tile_id, tab_state);
            if !tab_state.active && response.hovered() {
                bg_color = bg_color.lerp_to_gamma(ui.visuals().panel_fill, 0.5);
            }
            let mut stroke = self.tab_outline_stroke(ui.visuals(), tiles, tile_id, tab_state);
            if stroke.width == 0.0 || stroke.color == egui::Color32::TRANSPARENT {
                // inactive tabs have an outline too, a softer one
                stroke = Stroke::new(
                    1.0,
                    ui.visuals()
                        .widgets
                        .noninteractive
                        .bg_stroke
                        .color
                        .gamma_multiply(0.7),
                );
            }
            let radius = Self::TAB_RADIUS;
            ui.painter().rect_filled(
                shape_rect,
                egui::CornerRadius {
                    nw: radius as u8,
                    ne: radius as u8,
                    sw: 0,
                    se: 0,
                },
                bg_color,
            );
            ui.painter().add(egui::epaint::PathShape::line(
                Self::tab_outline(shape_rect, radius),
                stroke,
            ));
            if tab_state.active {
                // Make the tab join the content below it: cover the line
                // between them.
                ui.painter().hline(
                    rect.left() + 1.0..=rect.right() - 1.0,
                    rect.bottom(),
                    Stroke::new(stroke.width + 1.0, bg_color),
                );
            }
        }
        // the title (also on the tab that is being dragged)
        let title_pos = egui::pos2(
            rect.left() + x_margin,
            shape_rect.center().y - 0.5 * galley.size().y,
        );
        ui.painter().galley(title_pos, galley, text_color);

        if let Some(close_response) = close_response {
            if ui.is_rect_visible(close_rect) {
                let center = close_rect.center();
                if close_response.hovered() {
                    ui.painter().circle_filled(
                        center,
                        0.5 * close_size,
                        ui.visuals().widgets.hovered.weak_bg_fill,
                    );
                }
                let strength = if close_response.hovered() {
                    1.0
                } else if tab_state.active {
                    0.7
                } else {
                    0.5
                };
                let cross = Stroke::new(1.3, text_color.gamma_multiply(strength));
                let half = 3.5;
                ui.painter().line_segment(
                    [center + vec2(-half, -half), center + vec2(half, half)],
                    cross,
                );
                ui.painter().line_segment(
                    [center + vec2(half, -half), center + vec2(-half, half)],
                    cross,
                );
            }
            self.on_close_tab(tile_id, close_response);
        }

        self.on_tab_button(tiles, tile_id, response)
    }

    #[tracing::instrument(skip_all)]
    fn tab_title_for_pane(&mut self, view: &Box<dyn TabPane>) -> WidgetText {
        view.get_title()
    }

    #[tracing::instrument(skip_all)]
    fn top_bar_right_ui(
        &mut self,
        _tiles: &Tiles<Box<dyn TabPane>>,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        _tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        ui.add_space(1.5);
        // `ui.menu_button` defaults to `PopupCloseBehavior::CloseOnClick`, which closes the
        // popup on *any* click once it has been open for a frame -- including the click used
        // to place the caret in the search box below. That made the text field appear to lose
        // focus as soon as it was clicked. Building the menu explicitly lets us ask for
        // `CloseOnClickOutside` instead, the same behavior egui's own `SubMenuButton` uses so
        // that interactive widgets inside a popup keep working normally.
        // A plain "+" of the interface font: the emoji of the platform sits
        // high in the button.
        MenuButton::new(egui::RichText::new("+").strong())
            .config(MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                let mut _data: Vec<usize> = Vec::new();
                ui.label(t!("map.search_region"));
                if ui.text_edit_singleline(&mut self.search_text).changed() {
                    self.search_regions.clear();
                    if self.search_text.len() > 3 {
                        let t_sde = SdeManager::new(Path::new(&self.path), self.factor);
                        if let Ok(regions) =
                            t_sde.and_then(|s| s.get_region(vec![], Some(self.search_text.clone())))
                        {
                            // The SDE query can match regions (e.g. wormhole space)
                            // that were filtered out of `tile_data` at startup; keep
                            // only ids we can actually look up below.
                            self.search_regions = regions
                                .keys()
                                .copied()
                                .map(|x| x as usize)
                                .filter(|id| self.tile_data.contains_key(id))
                                .collect();
                        }
                    }
                }

                if self.search_regions.is_empty() {
                    _data = self.tile_data.keys().copied().collect();
                } else {
                    _data.clone_from(&self.search_regions);
                }
                ui.add_space(7.0);
                TableBuilder::new(ui)
                    .column(Column::remainder())
                    .striped(true)
                    .vscroll(true)
                    .max_scroll_height(100.00)
                    .body(|body| {
                        body.rows(25.0, _data.len(), |mut row| {
                            let key_index = row.index();
                            let name = self
                                .tile_data
                                .get_mut(&(_data[key_index]))
                                .unwrap()
                                .get_name();
                            row.col(|ui: &mut egui::Ui| {
                                if ui
                                    .checkbox(
                                        &mut self
                                            .tile_data
                                            .get_mut(&(_data[key_index]))
                                            .unwrap()
                                            .visible,
                                        name,
                                    )
                                    .changed()
                                {
                                    self.toggle_regions(_data[key_index]);
                                };
                            });
                        });
                    });
            });
    }

    // ---
    // Settings:

    // Uninstrumented for the same reason as `TileData`'s accessors: egui_tiles
    // queries these repeatedly per tile per frame, and a span costs far more
    // than returning a `f32`.
    fn tab_bar_height(&self, _style: &Style) -> f32 {
        self.tab_bar_height
    }

    fn gap_width(&self, _style: &Style) -> f32 {
        self.gap_width
    }

    fn simplification_options(&self) -> SimplificationOptions {
        self.simplification_options
    }
}

struct Template {
    // Cached per-node label `Galley`s, keyed by node id. `node_ui` runs once
    // per visible node, every frame; `FontsView::layout_no_wrap` already
    // memoizes by content internally (see its own doc comment), but that
    // memoization still pays for a fresh `String` allocation of the label
    // text plus hashing the whole layout job on *every* call, hit or miss.
    // Keying our own cache on the node's id instead (a `usize`, hashed for
    // free) skips all of that whenever a node's label hasn't actually
    // changed since last frame -- the common case unless the map is being
    // actively zoomed, renamed, or re-themed.
    label_cache: RefCell<HashMap<usize, CachedLabel>>,
    node_style: NodeStyle,
}

/// One entry in [`Template::label_cache`]: the inputs that produced
/// `galley` (plus its own `pixels_per_point`, the screen scale it was laid
/// out for), so a lookup can tell a still-valid entry from a stale one --
/// `name`/`color`/`font_size` are exactly the arguments `node_ui` passes to
/// [`FontsView::layout_no_wrap`](egui::text::Fonts) to build the label, so
/// comparing them against the node's current values is the same test
/// egui's own content-hash cache is already making internally, just without
/// re-deriving that hash (or re-allocating the `String`) on every call.
struct CachedLabel {
    name: Option<String>,
    color: Color32,
    font_size: f32,
    galley: Arc<Galley>,
}

impl Template {
    #[tracing::instrument(skip(node_style))]
    fn new(node_style: NodeStyle) -> Self {
        Self {
            label_cache: RefCell::new(HashMap::new()),
            node_style,
        }
    }
}

/// A node's box, without its border. Every hook of [`Template`] derives its
/// geometry from `style`, so the box drawn, its hit area and the rings
/// around it always agree.
fn node_rect(position: Pos2, zoom: f32, style: NodeStyle) -> Rect {
    Rect::from_center_size(position, Vec2::new(style.width, style.height) * zoom)
}

/// Corner radius of a node's box.
fn node_corner_radius(zoom: f32, style: NodeStyle) -> CornerRadius {
    CornerRadius::same((style.corner_radius * zoom).round() as u8)
}

/// A node's box including its border: what the user sees and points at.
fn node_outer_rect(position: Pos2, zoom: f32, style: NodeStyle) -> Rect {
    node_rect(position, zoom, style).expand(style.border * zoom)
}

impl NodeTemplate for Template {
    // `skip_all`, not `skip(self, ui)`: this runs once per visible node per
    // frame, and `tracing-tracy` formats a span's recorded fields into the
    // Tracy *zone name* (`Config::format_fields_in_zone_name` defaults to
    // true). Leaving `system` recorded meant `{:?}`-formatting the whole
    // `MapPoint` -- its `name: Option<String>` and `connections:
    // Vec<(usize, usize)>` included -- into a fresh string on every call, and
    // then allocating a distinct Tracy zone per distinct value. A capture of
    // this app produced 8522 separate zone names from 42349 `node_ui` calls,
    // with this function accounting for 68% of all frame time. The same
    // reasoning applies to the three hooks below.
    #[tracing::instrument(skip_all)]
    fn node_ui(&self, ui: &mut Ui, ctx: NodeContext) {
        let mut shapes = Vec::new();
        let rect = node_rect(ctx.position, ctx.zoom, self.node_style);
        // `ctx.color`: the same per-node color (a system's own star-color
        // override, falling back to the theme's node color) the built-in
        // circle paints with when no template is installed -- see
        // `Map::paint_map_points`'s `node_color` and `NodeContext::color`'s
        // doc comment. Using the theme's flat `theme.node` here instead would
        // silently drop every star-color override reaching this node.
        let corner_radius = node_corner_radius(ctx.zoom, self.node_style);
        // Background and border as one shape: drawn as two, each with its
        // own anti-aliased edge meeting at `rect`'s outline, a faint seam
        // showed between them. `ctx.background_color` is the surrounding
        // UI's own faint background (`extreme_bg_color`), so the chip suits
        // both light and dark themes.
        shapes.push(Shape::Rect(RectShape::new(
            rect,
            corner_radius,
            ctx.background_color,
            Stroke::new(self.node_style.border * ctx.zoom, ctx.theme.node),
            egui::StrokeKind::Outside,
        )));
        ui.painter().extend(std::mem::take(&mut shapes));
        // A linked character here (a marker, in egui-map's terms): a pulsing
        // tint over the background and under the label, faded in and out by
        // `ctx.marker` when the character arrives or leaves. Drawn over an
        // opaque background, the map behind never shows through, and the
        // label's `Galley` is untouched.
        if ctx.marker > 0.0 && self.node_style.glow_max_alpha > 0.0 {
            ctx.animation.glow_outline(
                ui.painter(),
                &NodeOutline::RoundedRect {
                    rect,
                    corner_radius: self.node_style.corner_radius * ctx.zoom,
                },
                ui.input(|input| input.time) as f32,
                ctx.theme
                    .marker
                    .gamma_multiply(self.node_style.glow_max_alpha),
                ctx.marker,
            );
            // The glow is a slow fade: 30 frames per second look the same as
            // the display's full rate (60-144 Hz) at a fraction of the work,
            // since every frame runs the whole UI, not just this node.
            ui.ctx().request_repaint_after(GLOW_FRAME_INTERVAL);
        }
        // Snap to the nearest half-pixel: egui's font atlas caches rasterized
        // glyphs keyed on the exact `FontId` size, and `12.0 * ctx.zoom` is
        // continuous in `ctx.zoom` -- during a zoom gesture it changes on
        // essentially every frame, so the atlas never gets a cache hit and
        // re-rasterizes this label from scratch each time. The 0.5px
        // granularity is not perceptible but turns that continuous stream of
        // atlas misses into a small, reusable set of sizes.
        let font_size = ((12.0 * ctx.zoom) * 2.0).round() / 2.0;
        // `ctx.theme.text`: the same field the built-in label painting uses
        // (`Map`'s own `text_color: self.theme_colors().text`) so this
        // label's color tracks the active theme instead of a hand-rolled
        // dark/light check.
        let text_color = ctx.theme.text;
        // Reuse this node's cached `Galley` if nothing that would change its
        // shape has changed (name, color, quantized size); only fall back to
        // laying it out again -- opening our own narrowly-scoped
        // `fonts_mut`, held only for the layout call itself -- when it's
        // genuinely stale. See `Template::label_cache`'s doc comment for why
        // this is worth doing on top of egui's own font-layout cache.
        //
        // This must NOT be widened to wrap the rest of this function:
        // `ui.painter().extend(shapes)` below also needs to touch
        // `egui::Context`'s single whole-object lock, and `fonts_mut` holds
        // that same lock for as long as its closure runs -- nesting the two
        // is a same-thread reentrant lock attempt, which deadlocks (egui
        // panics after a 10s timeout in debug builds: "Failed to acquire
        // RwLock write ... Deadlock?").
        // A galley is laid out for one pixel density: moving the window to a
        // screen with another scale makes every cached one stale (epaint
        // warns "pixels_per_point ... have changed between text layout and
        // tessellation" for each, and draws them at the old resolution).
        let pixels_per_point = ui.ctx().pixels_per_point();
        let mut label_cache = self.label_cache.borrow_mut();
        let galley = match label_cache.get(&ctx.point.id) {
            Some(cached)
                if cached.name == ctx.point.name
                    && cached.color == text_color
                    && cached.font_size == font_size
                    && cached.galley.pixels_per_point == pixels_per_point =>
            {
                Arc::clone(&cached.galley)
            }
            _ => {
                let name = ctx.point.name.clone();
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        name.clone().unwrap_or_default(),
                        FontId::proportional(font_size),
                        text_color,
                    )
                });
                label_cache.insert(
                    ctx.point.id,
                    CachedLabel {
                        name,
                        color: text_color,
                        font_size,
                        galley: Arc::clone(&galley),
                    },
                );
                galley
            }
        };
        drop(label_cache);
        // Same anchoring `Shape::text` does internally (`Align2::anchor_size`
        // then `Shape::galley`) -- we can't call `Shape::text` itself here
        // since it always lays the text out fresh; this is the equivalent
        // that accepts an already-built `Galley` instead.
        let text_rect = Align2::CENTER_CENTER.anchor_size(ctx.position, galley.size());
        shapes.push(Shape::galley(text_rect.min, galley, text_color));
        ui.painter().extend(shapes);
    }

    /// Nothing to draw. `egui-map` calls this for its markers and for a
    /// node's lasting state (halo, blink, orbit), always after or before
    /// `node_ui` on the same node, so anything drawn here would either
    /// cover the label or be covered by the node's background. Linked
    /// characters are drawn by `node_ui` instead, from `NodeContext::marker`.
    fn marker_ui(&self, _ui: &mut Ui, _ctx: MarkerContext) {}

    /// The node's box as the user sees it, border included. egui-map
    /// derives everything else from it: the hit area for the node tooltips
    /// (`Map::hovered_node`), the selection ring, and the intel alert pulse
    /// (the default `notification_ui`), which grows in the node's own shape.
    fn outline(&self, ctx: HitContext) -> NodeOutline {
        NodeOutline::RoundedRect {
            rect: node_outer_rect(ctx.position, ctx.zoom, self.node_style),
            corner_radius: (self.node_style.corner_radius + self.node_style.border) * ctx.zoom,
        }
    }
}

#[cfg(test)]
mod no_sde_tests {
    use super::*;
    use crate::app::messages::MapSync;

    /// A spawner and the receiver that keeps its channel open (the pane
    /// reports the missing `sde.db` through it).
    fn spawner() -> (
        Arc<MessageSpawner>,
        tokio::sync::mpsc::Receiver<crate::app::messages::Message>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        (Arc::new(MessageSpawner::new(Arc::new(sender))), receiver)
    }

    /// Draws `pane` once in a headless egui context.
    fn draw(pane: &mut dyn TabPane) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = pane.ui(ui);
            });
        })
        .textures_delta
        .clear();
    }

    // Regression test: with no usable `sde.db` (first run, or while
    // `DatabaseUpdater` rebuilds it) a pane has no systems, and a linked
    // character's marker made egui-map 0.9.1 panic on its first frame
    // (`map.rs:739`, an `unwrap` on the missing points).
    #[test]
    fn panes_without_sde_data_ignore_markers() {
        let missing = PathBuf::from("/nonexistent/telescope-test/sde.db");
        let (sender, _) = tokio::sync::broadcast::channel::<MapSync>(4);
        let (spawner, _messages) = spawner();

        let mut universe =
            UniversePane::new(sender.subscribe(), missing.clone(), 1.0, spawner.clone());
        universe.update_marker(1, 30000142, "Pilot");
        draw(&mut universe);

        let mut region = RegionPane::new(
            sender.subscribe(),
            missing,
            1.0,
            10000002,
            spawner,
            NodeStyle::default(),
        );
        region.update_marker(1, 30000142, "Pilot");
        draw(&mut region);
    }

    // Reloading a map against a database that isn't there (a path picked
    // before it was built) leaves it empty, keeps the markers for later and
    // switches to the new path, without panicking.
    #[test]
    fn reloading_without_sde_data_keeps_the_markers() {
        let missing = PathBuf::from("/nonexistent/telescope-test/sde.db");
        let other = PathBuf::from("/nonexistent/telescope-test/other.db");
        let (sender, _) = tokio::sync::broadcast::channel::<MapSync>(4);
        let (spawner, _messages) = spawner();

        let mut universe =
            UniversePane::new(sender.subscribe(), missing.clone(), 1.0, spawner.clone());
        universe.update_marker(1, 30000142, "Pilot");
        universe.reload_data(&other);
        assert!(!universe.has_points);
        assert_eq!(universe.path, other);
        assert!(universe.characters.contains_key(&1));
        draw(&mut universe);

        let mut region = RegionPane::new(
            sender.subscribe(),
            missing,
            1.0,
            10000002,
            spawner,
            NodeStyle::default(),
        );
        region.update_marker(1, 30000142, "Pilot");
        region.reload_data(&other);
        assert!(!region.has_points);
        assert_eq!(region.path, other);
        draw(&mut region);
    }

    #[test]
    fn the_outline_is_the_node_box_with_its_border() {
        let zoom = 2.0;
        let center = Pos2::new(100.0, 100.0);
        let style = NodeStyle::default();
        let outer = node_outer_rect(center, zoom, style);
        let outline = NodeOutline::RoundedRect {
            rect: outer,
            corner_radius: (style.corner_radius + style.border) * zoom,
        };
        assert!(outline.contains(outer.center_top() + Vec2::new(0.0, 0.5)));
        assert!(!outline.contains(outer.center_top() - Vec2::new(0.0, 1.0)));
        assert_eq!(outline.bounding_rect(), outer);
    }
}

#[cfg(test)]
mod logic_tests {
    use super::*;
    use crate::app::messages::Message;
    use webb::map_alerts::AlertSummary;

    fn sde_point(x: f64, y: f64) -> SdePoint {
        SdePoint {
            coords: [x, y, 0.0],
            ..SdePoint::default()
        }
    }

    #[test]
    fn an_sde_point_becomes_a_map_point_with_its_name_links_and_color() {
        let mut point = sde_point(1.5, -2.5);
        point.name = Some(String::from("Jita"));
        point.connections = vec![(1, 2), (1, 3)];
        point.color = Some(String::from("#FFE996"));
        let map_point = sde_point_to_map(7, point);
        assert_eq!(map_point.get_id(), 7);
        assert_eq!(map_point.coords, [1.5, -2.5]);
        assert_eq!(map_point.get_name(), "Jita");
        assert_eq!(map_point.connections, vec![(1, 2), (1, 3)]);
        assert_eq!(map_point.color, Color32::from_hex("#FFE996").ok());
        assert!(map_point.color.is_some());
    }

    #[test]
    fn a_missing_or_malformed_star_color_keeps_the_default_node_color() {
        let mut point = sde_point(0.0, 0.0);
        assert!(sde_point_to_map(1, point.clone()).color.is_none());
        point.color = Some(String::from("not a color"));
        assert!(sde_point_to_map(1, point).color.is_none());
    }

    #[test]
    fn a_nameless_sde_point_stays_nameless() {
        assert!(sde_point_to_map(1, sde_point(0.0, 0.0)).name.is_none());
    }

    #[test]
    fn points_keep_their_ids_when_converted_in_bulk() {
        let points = HashMap::from([(1, sde_point(1.0, 1.0)), (2, sde_point(2.0, 2.0))]);
        let converted = sde_points_to_map(points);
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[&2].get_id(), 2);
        assert_eq!(converted[&2].coords, [2.0, 2.0]);
    }

    #[test]
    fn segments_are_narrowed_to_f32_and_keep_their_id() {
        let segment = SdeSegment {
            id: (1, 2),
            point1: [1.25, 2.5],
            point2: [-3.0, 4.0],
        };
        let converted = sde_segment_to_map((1, 2), segment);
        assert_eq!(converted.id, (1, 2));
        assert_eq!(converted.point1, [1.25, 2.5]);
        assert_eq!(converted.point2, [-3.0, 4.0]);

        let all = sde_segments_to_map(HashMap::from([((1, 2), segment)]));
        assert_eq!(all.len(), 1);
        assert_eq!(all[&(1, 2)].id, (1, 2));
    }

    #[test]
    fn tile_data_remembers_its_name_and_visibility() {
        let mut data = TileData::new(String::from("The Forge"), true);
        assert_eq!(data.get_name(), "The Forge");
        assert!(!data.get_visible());
        assert!(data.show_on_startup);
        data.set_visible(true);
        assert!(data.get_visible());
    }

    fn behavior() -> (TreeBehavior, tokio::sync::mpsc::Receiver<Message>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let spawner = Arc::new(MessageSpawner::new(Arc::new(sender)));
        (
            TreeBehavior::new(spawner, 1.0, PathBuf::from("sde.db")),
            receiver,
        )
    }

    #[test]
    fn toggling_a_region_asks_for_the_right_change_of_its_map() {
        let (mut behavior, mut messages) = behavior();
        behavior
            .tile_data
            .insert(1, TileData::new(String::from("Forge"), false));

        // Hidden -> hide its map.
        behavior.toggle_regions(1);
        assert!(matches!(messages.try_recv(), Ok(Message::MapHidden(1))));

        // Visible without a map yet -> create it.
        behavior.tile_data.get_mut(&1).unwrap().set_visible(true);
        behavior.toggle_regions(1);
        assert!(matches!(
            messages.try_recv(),
            Ok(Message::NewRegionalPane(1))
        ));

        // Visible with a map -> show it.
        behavior
            .tile_data
            .get_mut(&1)
            .unwrap()
            .set_tile_id(Some(TileId::from_u64(3)));
        behavior.toggle_regions(1);
        assert!(matches!(messages.try_recv(), Ok(Message::MapShown(1))));
    }

    #[test]
    fn the_region_search_path_can_be_replaced() {
        let (mut behavior, _messages) = behavior();
        behavior.set_path(PathBuf::from("other.db"));
        assert_eq!(behavior.path, PathBuf::from("other.db"));
    }

    fn alert(system_id: usize, text: &str) -> IntelAlert {
        IntelAlert::new(
            system_id,
            Instant::now(),
            Duration::from_secs(30),
            text,
            AlertSummary::default(),
            true,
        )
    }

    #[test]
    fn pushed_alerts_are_listed_under_their_system() {
        let mut log = AlertLog::default();
        push_alert(&mut log, alert(1, "first"));
        push_alert(&mut log, alert(1, "second"));
        push_alert(&mut log, alert(2, "other"));
        let now = Instant::now();
        assert_eq!(log.active(1, now).len(), 2);
        assert_eq!(log.active(2, now).len(), 1);
        assert!(log.active(3, now).is_empty());
    }

    #[test]
    fn an_alert_pulse_tolerates_nodes_the_map_does_not_have() {
        let mut map = Map::new();
        map.add_points(vec![MapPoint::new(1, [0.0, 0.0])]);
        pulse_alert(&mut map, &alert(1, "on the map"));
        pulse_alert(&mut map, &alert(99, "not on this map"));
    }

    #[test]
    fn a_repeating_alert_pulse_tolerates_nodes_the_map_does_not_have() {
        let mut map = Map::new();
        map.add_points(vec![MapPoint::new(1, [0.0, 0.0])]);
        repeating_pulse_alert(&mut map, &alert(1, "on the map"));
        repeating_pulse_alert(&mut map, &alert(99, "not on this map"));
    }

    #[test]
    fn node_geometry_scales_with_the_zoom() {
        let style = NodeStyle::default();
        let center = Pos2::new(100.0, 50.0);
        let rect = node_rect(center, 2.0, style);
        assert_eq!(rect.center(), center);
        assert_eq!(rect.width(), style.width * 2.0);
        assert_eq!(rect.height(), style.height * 2.0);

        let outer = node_outer_rect(center, 2.0, style);
        assert_eq!(outer.width(), rect.width() + 2.0 * style.border * 2.0);

        assert_eq!(
            node_corner_radius(2.0, style),
            CornerRadius::same((style.corner_radius * 2.0).round() as u8)
        );
    }
}
