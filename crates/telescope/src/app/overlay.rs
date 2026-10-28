//! Experimental overlay: a transparent, borderless window
//! that stays above the game and follows the EVE client window.
//!
//! It shows the events near the linked characters (see
//! `intel::nearby`), with one window per character, all visible while the
//! overlay is on (or a single one for all of them, by setting): a small
//! semi-transparent panel with one row per event, in the chosen order (newest
//! or closest first). With nothing to show it draws only
//! its header when locked, and an example while it is being positioned. The
//! header names the characters, counts the events and holds the padlock, which
//! stays when locked (click-through everywhere else). Nothing is
//! injected into the game; see `native_tools::overlay`.

use crate::app::settings::{OverlayAnchor, OverlayOrder, OverlayPosition};
use eframe::egui;
use native_tools::overlay::{
    Anchor, Blending, FullScreenState, GameWindow, GameWindowTracker, OverlayError,
    OverlayPlacement, OverlayWindow, ScreenRect,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One event near a character, as the widget lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NearbyRow {
    /// Stargate jumps from the closest linked character.
    pub jumps: u8,
    pub system: String,
    pub text: String,
    /// Seconds since the event was received.
    pub age_secs: u64,
}

impl crate::app::TelescopeApp {
    /// Opens or closes the overlay to match the setting (at start-up, when
    /// the settings are applied and when they are cancelled).
    pub(crate) fn sync_overlay(&mut self) {
        if self.settings.get_overlay_enabled() != self.overlay.is_open() {
            self.overlay.toggle();
        }
    }

    /// The user closed the overlay with its own button: the setting follows.
    /// It is saved right away, unless the Settings screen is open with edits
    /// of its own, which wait for Apply (the snapshot Cancel returns to is
    /// updated too, so Cancel does not bring the overlay back).
    pub(crate) fn overlay_closed_by_user(&mut self) {
        self.settings.set_overlay_enabled(false);
        if let Some(snapshot) = self.settings_snapshot.as_mut() {
            snapshot.set_overlay_enabled(false);
        }
        if !self.open[2] {
            self.save_settings();
        }
        // The close button was pressed in the overlay's own window: repaint
        // the main one too, so the Settings switch shows the new value.
        crate::repaint::request();
    }

    /// The overlays to show: a single one for all the characters (the
    /// default), or one for each linked character turned on in the settings.
    pub(crate) fn overlay_views(&self) -> Vec<OverlayView> {
        let characters = &self.esi.characters;
        if self.settings.get_overlay_all_characters() {
            // shown also with no character linked (its header then has no name)
            let name = characters
                .iter()
                .map(|character| character.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let (rows, active) = self.nearby_rows(0);
            return vec![OverlayView {
                key: 0,
                name,
                rows,
                active,
                position: self.settings.get_overlay_position(),
            }];
        }
        characters
            .iter()
            .filter(|character| character.overlay.enabled)
            .map(|character| {
                let (rows, active) = self.nearby_rows(character.id);
                OverlayView {
                    key: character.id,
                    name: character.name.clone(),
                    rows,
                    active,
                    position: character.overlay.position,
                }
            })
            .collect()
    }

    /// Keeps the places the user left the overlays at: a character's in the
    /// player database, with the character, and the unified one in the
    /// settings (saved unless the Settings screen is open: its Apply saves
    /// then).
    pub(crate) fn save_overlay_positions(&mut self) {
        let positions = self.overlay.take_positions();
        if positions.is_empty() {
            return;
        }
        let mut settings_changed = false;
        for (key, position) in positions {
            if key == 0 {
                self.settings.set_overlay_position(position);
                settings_changed = true;
                continue;
            }
            let Some(character) = self.esi.characters.iter().find(|c| c.id == key) else {
                continue;
            };
            let overlay = webb::objects::CharacterOverlay {
                position: Some(position),
                ..character.overlay
            };
            self.write_character_overlay(key, overlay);
        }
        if settings_changed && !self.open[2] {
            self.save_settings();
        }
    }

    /// Stores the overlay of a linked character in the player database.
    pub(crate) fn write_character_overlay(
        &mut self,
        id: i64,
        overlay: webb::objects::CharacterOverlay,
    ) {
        if let Err(error) = self.esi.write_overlay(id, overlay) {
            tracing::warn!(%error, id, "could not store the character's overlay");
        }
    }

    /// Moves what the first builds kept in `telescope.toml` about each
    /// character's overlay (on or off, and where it was left) to the player
    /// database, once: the entries are taken out of the settings and the file
    /// is saved without them.
    pub(crate) fn migrate_overlay_settings(&mut self) {
        if !self.settings.has_legacy_overlay_characters() {
            return;
        }
        for entry in self.settings.take_legacy_overlay_characters() {
            // an unlinked character has nothing to keep it for
            if self.esi.characters.iter().any(|c| c.id == entry.id) {
                let overlay = webb::objects::CharacterOverlay {
                    enabled: entry.enabled,
                    position: entry.placement,
                };
                self.write_character_overlay(entry.id, overlay);
            }
        }
        self.save_settings();
    }

    /// The events near the character `key` (0: all of them), as the widget
    /// lists them: in the chosen order, with the system's name, and only those
    /// still within the alert duration. Also how many are active in all, which
    /// can be more than the rows listed.
    pub(crate) fn nearby_rows(&self, key: i64) -> (Vec<NearbyRow>, usize) {
        let now = std::time::Instant::now();
        let mut rows: Vec<NearbyRow> = self
            .alarm_shared
            .nearby
            .recent(key, now, self.settings.get_alert_duration())
            .into_iter()
            .map(|event| NearbyRow {
                jumps: event.jumps,
                system: self
                    .universe
                    .solar_systems
                    .get(&event.system_id)
                    .map(|system| system.name.clone())
                    .unwrap_or_else(|| event.system_id.to_string()),
                text: event.text,
                age_secs: now.saturating_duration_since(event.received).as_secs(),
            })
            .collect();
        // the header counts every event still active, the panel lists some
        let active = rows.len();
        arrange_rows(
            &mut rows,
            usize::from(self.settings.get_overlay_max_rows()),
            self.settings.get_overlay_order(),
        );
        (rows, active)
    }
}

/// The events in the queue: only the newest `max_rows` stay (the oldest leave
/// first, whatever the order shown), then they are put in the chosen order.
fn arrange_rows(rows: &mut Vec<NearbyRow>, max_rows: usize, order: OverlayOrder) {
    rows.sort_by_key(|row| row.age_secs);
    rows.truncate(max_rows);
    sort_rows(rows, order);
}

/// What one overlay window shows.
#[derive(Debug, Clone)]
pub(crate) struct OverlayView {
    /// The character's id (0: the overlay that joins them all).
    pub key: i64,
    /// The header's name.
    pub name: String,
    pub rows: Vec<NearbyRow>,
    /// The events still active for it, listed or not: what the header counts.
    pub active: usize,
    /// Where the user left this overlay, if they did.
    pub position: Option<OverlayPosition>,
}

/// The overlay windows: one per character shown, all visible while the
/// overlay is on.
#[derive(Default)]
pub(crate) struct Overlays {
    open: bool,
    windows: Vec<(i64, OverlayState)>,
    /// Places the user left windows at, not yet handed to the settings (see
    /// [`Self::take_positions`]).
    positions: Vec<(i64, OverlayPosition)>,
}

impl Overlays {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.drop_windows(|_| true);
        }
    }

    /// Closes the windows of the keys `drop` accepts, keeping where the user
    /// left them.
    fn drop_windows(&mut self, drop: impl Fn(i64) -> bool) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.windows)
            .into_iter()
            .partition(|(key, _)| drop(*key));
        self.windows = kept;
        for (key, mut window) in gone {
            if let Some(position) = window.take_position() {
                self.positions.push((key, position));
            }
        }
    }

    /// The places the user left windows at since the last call, for the
    /// settings to keep.
    pub(crate) fn take_positions(&mut self) -> Vec<(i64, OverlayPosition)> {
        std::mem::take(&mut self.positions)
    }

    /// Draws one window per view, creating the windows of new characters and
    /// dropping those of characters that are gone. A window the user closes
    /// closes them all.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        views: &[OverlayView],
        max_rows: usize,
        opacity: f32,
    ) {
        if !self.open {
            return;
        }
        self.drop_windows(|key| !views.iter().any(|view| view.key == key));
        for view in views {
            if self.windows.iter().any(|(key, _)| *key == view.key) {
                continue;
            }
            let slot = (0..)
                .find(|slot| self.windows.iter().all(|(_, window)| window.slot != *slot))
                .unwrap_or(0);
            let mut window = OverlayState::for_window(view.key, slot, view.position);
            window.open = true;
            self.windows.push((view.key, window));
        }
        for view in views {
            if let Some((_, window)) = self.windows.iter_mut().find(|(key, _)| *key == view.key) {
                // the overlay of a character follows that character's client
                // (the one for all of them follows any client)
                if view.key != 0 {
                    window.follow_character(&view.name);
                }
                window.show(ctx, &view.rows, view.active, &view.name, max_rows, opacity);
            }
        }
        // the place of a window the user locked is kept right away
        for (key, window) in &mut self.windows {
            if window.locked
                && let Some(position) = window.take_position()
            {
                self.positions.push((*key, position));
            }
        }
        if self.windows.iter().any(|(_, window)| !window.is_open()) {
            self.open = false;
            self.drop_windows(|_| true);
        }
    }

    /// A message for the status log, if any window has one to show.
    pub(crate) fn take_warning(&mut self) -> Option<String> {
        self.windows
            .iter_mut()
            .filter_map(|(_, window)| window.take_warning())
            .collect::<Vec<_>>()
            .into_iter()
            .next()
    }
}

/// Orders the widget's rows: the newest first, or the closest first (the
/// newest first among those at the same distance).
fn sort_rows(rows: &mut [NearbyRow], order: OverlayOrder) {
    match order {
        OverlayOrder::Age => rows.sort_by_key(|row| row.age_secs),
        OverlayOrder::Jumps => rows.sort_by_key(|row| (row.jumps, row.age_secs)),
    }
}

/// State of the overlay window.
pub(crate) struct OverlayState {
    open: bool,
    /// Position fixed: no title bar, the mouse goes to the game. Unlocked is
    /// the positioning mode: an opaque title bar to drag the window.
    locked: bool,
    /// The parts of the window shown (window pixels), as last sent to it.
    region: Option<Vec<ScreenRect>>,
    tracker: GameWindowTracker,
    /// The character whose client `tracker` looks for (empty: any client).
    followed: String,
    native: OverlayWindow,
    placement: OverlayPlacement,
    /// The placement has been made for a game window (see `place`).
    placed: bool,
    game: Option<GameWindow>,
    last_poll: Option<Instant>,
    /// The game rectangle the window was last positioned for.
    last_game_rect: Option<ScreenRect>,
    /// Where the window is known to be, in screen pixels: what was last sent
    /// to it, or where the user left it after dragging.
    last_target: Option<ScreenRect>,
    native_applied: bool,
    native_attempts: u32,
    /// The overlay window has the keyboard focus.
    overlay_focused: bool,
    /// The window is shown (it is hidden while the game is not in front).
    visible: bool,
    fullscreen: FullScreenState,
    /// The game is in exclusive full screen and the user was told.
    warned_exclusive: bool,
    pending_warning: Option<String>,
    /// The events listed, as last given to `show`.
    rows: Vec<NearbyRow>,
    /// Opacity of the panel, from `Settings::get_overlay_opacity`.
    opacity: f32,
    /// The linked characters the overlay is for, for the header.
    characters: String,
    /// The events detected for the character that are still active (within
    /// the grace period), also those past the rows the panel lists: the
    /// number the header shows.
    active: usize,
    /// Most rows the panel grows to, from `Settings::get_overlay_max_rows`.
    max_rows: usize,
    /// The window's own title, which finds it natively; it is unique per
    /// window (see [`Self::for_window`]).
    title: String,
    /// Which of the side-by-side starting places the window takes (0 is the
    /// top-right corner, the next ones go to its left).
    slot: usize,
    /// The user moved or resized the window since its place was last handed
    /// over (see [`Self::take_position`]).
    moved: bool,
    /// The right edge is being dragged.
    resizing: bool,
    /// The width (points) the user gave the window while there was no game
    /// window to place it by; `None` otherwise.
    free_width: Option<f32>,
    /// When each listed event (its system and text) first appeared, for the
    /// fade-in of the new ones.
    seen: HashMap<(String, String), Instant>,
    /// The window has not been given its rows yet.
    first_rows: bool,
    /// Sizes, colors and timings.
    style: OverlayStyle,
    /// The texts shown, in the language in use.
    texts: OverlayTexts,
    /// The events listed the last time, to find those that left.
    listed_before: Vec<NearbyRow>,
    /// The events that left the list and are fading out.
    ghosts: Vec<Ghost>,
}

/// An event that left the list and fades out under it.
struct Ghost {
    row: NearbyRow,
    since: Instant,
}

/// The texts the overlay shows. They are read in the language in use
/// ([`Self::current`]) every frame, so the overlay follows a change of
/// language; a test can give fixed ones instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverlayTexts {
    /// The header's title when there is no character to name.
    pub nearby: String,
    /// Added to the title while the rows are an example.
    pub example: String,
    pub example_hostiles: String,
    pub example_kill: String,
    pub example_gate: String,
    /// A placeholder system name, with [`Self::NAME`] where its letter goes.
    pub example_system: String,
}

impl OverlayTexts {
    /// Where the letter of a placeholder system goes in `example_system`.
    pub const NAME: &'static str = "{name}";

    /// The texts in the language in use.
    pub fn current() -> Self {
        Self {
            nearby: t!("overlay.nearby").into_owned(),
            example: t!("overlay.example").into_owned(),
            example_hostiles: t!("overlay.example_hostiles").into_owned(),
            example_kill: t!("overlay.example_kill").into_owned(),
            example_gate: t!("overlay.example_gate").into_owned(),
            example_system: t!("overlay.example_system", name = Self::NAME).into_owned(),
        }
    }
}

/// How the overlay looks and behaves: sizes, colors, timings and the limits of
/// the animations. Every window has its own copy, so these are values to
/// configure, not constants.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayStyle {
    /// Width of a new window, in points, and the limits of the width the user
    /// can give it.
    pub width: f32,
    pub min_width: f32,
    pub max_width: f32,
    pub row_height: f32,
    pub header_height: f32,
    /// Width of the padlock button, and of the right edge that is dragged to
    /// resize.
    pub lock_button_width: f32,
    pub grip_width: f32,
    /// Points between two windows placed side by side.
    pub slot_gap: f32,
    /// How often the game window is looked up.
    pub poll: Duration,
    /// How often the pointer is looked at while the overlay is locked.
    pub locked_poll: Duration,
    /// Repaint interval while something animates.
    pub frame: Duration,
    /// How long a new event takes to fade in.
    pub fade_in: Duration,
    /// How long an event pushed out of the panel takes to fade out.
    pub fade_out: Duration,
    /// Tries to apply the native settings (the window does not exist on the
    /// first frames).
    pub native_attempts: u32,
    /// Pixels the window may differ from its target before it counts as moved
    /// by the user.
    pub move_tolerance: i32,
    pub panel_color: (u8, u8, u8),
    pub header_color: (u8, u8, u8),
    pub panel_border: (u8, u8, u8),
    /// Hue (degrees) by distance: `(jumps, hue)`, blended in between. Red at
    /// one jump, yellow at three, green at five and blue at seven or more.
    pub hue_stops: [(f32, f32); 4],
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            width: 260.0,
            min_width: 180.0,
            max_width: 640.0,
            row_height: 22.0,
            header_height: 22.0,
            lock_button_width: 30.0,
            grip_width: 6.0,
            slot_gap: 8.0,
            poll: Duration::from_millis(250),
            locked_poll: Duration::from_millis(50),
            frame: Duration::from_millis(16),
            fade_in: Duration::from_millis(450),
            fade_out: Duration::from_millis(500),
            native_attempts: 30,
            move_tolerance: 1,
            panel_color: (12, 16, 23),
            header_color: (24, 32, 46),
            panel_border: (70, 90, 120),
            hue_stops: [(1.0, 0.0), (3.0, 60.0), (5.0, 120.0), (7.0, 210.0)],
        }
    }
}

impl OverlayStyle {
    /// Size of a window, in points: room for the header and every event the
    /// widget can list, so the panel grows downwards inside it as events come
    /// (the rest of the window is cut away).
    pub fn size(&self) -> egui::Vec2 {
        egui::vec2(
            self.width,
            self.header_height + 2.0 + crate::app::intel::nearby::MAX_ROWS as f32 * self.row_height,
        )
    }

    /// Distance from the game's right edge of the window at place `slot`.
    fn slot_offset(&self, slot: usize, pixels_per_point: f32) -> i32 {
        let points = 40.0 + slot as f32 * (self.width + self.slot_gap);
        (points * pixels_per_point).round() as i32
    }

    /// The padlock button, at the right end of the panel's header (inside the
    /// resize edge). It is the only button: it stays when the overlay is
    /// locked, to unlock it.
    fn lock_button(&self, panel: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(
                panel.right() - self.grip_width - self.lock_button_width,
                panel.top(),
            ),
            egui::vec2(self.lock_button_width, self.header_height),
        )
    }

    /// The edge of the panel that is dragged to change its width.
    fn grip_rect(&self, panel: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(panel.right() - self.grip_width, panel.top()),
            panel.max,
        )
    }

    /// The colors of a row by distance: its stripe (the accent), its text and
    /// whether its background is tinted (only the closest ones; the rest stay
    /// black). The distance is always written too, so the color never carries
    /// it alone.
    fn jump_colors(&self, jumps: u8) -> (egui::Color32, egui::Color32, bool) {
        let hue = self.jump_hue(jumps) / 360.0;
        let color = |saturation: f32, value: f32| -> egui::Color32 {
            egui::ecolor::Hsva::new(hue, saturation, value, 1.0).into()
        };
        (color(1.0, 1.0), color(0.5, 1.0), jumps <= 1)
    }

    /// The hue (degrees) of a distance, blended between the `hue_stops`.
    fn jump_hue(&self, jumps: u8) -> f32 {
        let stops = &self.hue_stops;
        let last = stops[stops.len() - 1];
        let jumps = f32::from(jumps).clamp(stops[0].0, last.0);
        for pair in stops.windows(2) {
            let ((from, from_hue), (to, to_hue)) = (pair[0], pair[1]);
            if jumps <= to {
                return from_hue + (to_hue - from_hue) * (jumps - from) / (to - from);
            }
        }
        last.1
    }
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayState {
    const TITLE: &'static str = "Telescope Overlay";
    const EXCLUSIVE_WARNING: &'static str = "EVE is in exclusive full screen, so the overlay cannot be shown over it. Switch EVE to windowed or borderless full screen mode.";

    pub(crate) fn new() -> Self {
        Self::for_window(0, 0, None)
    }

    /// The overlay of the character `key` (0 is the one that joins them all),
    /// where the user left it (`saved`) or else at place `slot`.
    fn for_window(key: i64, slot: usize, saved: Option<OverlayPosition>) -> Self {
        let style = OverlayStyle::default();
        // the window must take mouse input, otherwise it cannot be dragged
        let mut native = OverlayWindow::new();
        native.set_click_through(false);
        Self {
            open: false,
            locked: false,
            region: None,
            tracker: GameWindowTracker::default(),
            followed: String::new(),
            native,
            placement: saved.map(Self::placement_from).unwrap_or_else(|| {
                OverlayPlacement::new(
                    Anchor::TopRight,
                    style.slot_offset(slot, 1.0),
                    80,
                    style.size().x as u32,
                    style.size().y as u32,
                )
            }),
            // a saved place is already in the game window's terms
            placed: saved.is_some(),
            game: None,
            last_poll: None,
            last_game_rect: None,
            last_target: None,
            native_applied: false,
            native_attempts: 0,
            overlay_focused: false,
            visible: true,
            fullscreen: FullScreenState::new(),
            warned_exclusive: false,
            pending_warning: None,
            rows: Vec::new(),
            opacity: 1.0,
            characters: String::new(),
            max_rows: crate::app::intel::nearby::MAX_ROWS,
            active: 0,
            title: if key == 0 {
                String::from(Self::TITLE)
            } else {
                format!("{} {key}", Self::TITLE)
            },
            slot,
            moved: false,
            resizing: false,
            free_width: None,
            seen: HashMap::new(),
            first_rows: true,
            style,
            texts: OverlayTexts::current(),
            listed_before: Vec::new(),
            ghosts: Vec::new(),
        }
    }

    fn placement_from(position: OverlayPosition) -> OverlayPlacement {
        let anchor = match position.anchor {
            OverlayAnchor::TopLeft => Anchor::TopLeft,
            OverlayAnchor::TopRight => Anchor::TopRight,
            OverlayAnchor::BottomLeft => Anchor::BottomLeft,
            OverlayAnchor::BottomRight => Anchor::BottomRight,
            OverlayAnchor::Center => Anchor::Center,
        };
        OverlayPlacement::new(
            anchor,
            position.offset_x,
            position.offset_y,
            position.width.max(1),
            position.height.max(1),
        )
        .with_reference_height(position.reference_height)
    }

    fn position_of(placement: &OverlayPlacement) -> OverlayPosition {
        let (anchor, offset_x, offset_y, width, height) = placement.parts();
        OverlayPosition {
            anchor: match anchor {
                Anchor::TopLeft => OverlayAnchor::TopLeft,
                Anchor::TopRight => OverlayAnchor::TopRight,
                Anchor::BottomLeft => OverlayAnchor::BottomLeft,
                Anchor::BottomRight => OverlayAnchor::BottomRight,
                Anchor::Center => OverlayAnchor::Center,
            },
            offset_x,
            offset_y,
            width,
            height,
            reference_height: placement.reference_height(),
        }
    }

    /// Where the window was left, if the user moved it since the last time
    /// this was asked.
    fn take_position(&mut self) -> Option<OverlayPosition> {
        std::mem::take(&mut self.moved).then(|| Self::position_of(&self.placement))
    }

    fn viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(&self.title)
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Fixes the window where it is (`true`: the title bar goes away and the
    /// mouse reaches the game) or goes back to the positioning mode.
    pub(crate) fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
        self.native.set_click_through(locked);
        self.native_applied = false;
        self.native_attempts = 0;
        self.region = None;
    }

    fn close(&mut self) {
        self.open = false;
        // reopened, it is in the positioning mode again
        self.locked = false;
        self.native.set_click_through(false);
        self.region = None;
        self.native_applied = false;
        self.native_attempts = 0;
        self.last_game_rect = None;
        self.last_target = None;
        self.overlay_focused = false;
        self.native.window_closed();
        self.visible = true;
    }

    /// Pixels per point of the units the window system positions windows in:
    /// macOS already works in points.
    fn pixels_per_point(ctx: &egui::Context) -> f32 {
        if cfg!(target_os = "macos") {
            1.0
        } else {
            ctx.pixels_per_point()
        }
    }

    fn to_points(pixels: i32, pixels_per_point: f32) -> f32 {
        pixels as f32 / pixels_per_point
    }

    fn to_pixels(points: f32, pixels_per_point: f32) -> i32 {
        (points * pixels_per_point).round() as i32
    }

    /// Notes when each event first showed up (the rows already there when the
    /// window opens count as old, they do not fade in) and forgets those that
    /// are gone. Returns whether a row is still fading in.
    fn track_new_rows(&mut self) -> bool {
        let now = Instant::now();
        let first = self.seen.is_empty() && !self.rows.is_empty() && self.first_rows;
        self.first_rows = false;
        let long_ago = now.checked_sub(self.style.fade_in).unwrap_or(now);
        for row in &self.rows {
            self.seen
                .entry((row.system.clone(), row.text.clone()))
                .or_insert(if first { long_ago } else { now });
        }
        let rows = &self.rows;
        self.seen.retain(|(system, text), _| {
            rows.iter()
                .any(|row| &row.system == system && &row.text == text)
        });
        self.seen
            .values()
            .any(|seen| now.saturating_duration_since(*seen) < self.style.fade_in)
    }

    /// How far a new row has faded in, 0 (just arrived, invisible) to 1
    /// (fully shown), easing out.
    fn enter_progress(&self, row: &NearbyRow) -> f32 {
        let Some(seen) = self.seen.get(&(row.system.clone(), row.text.clone())) else {
            return 1.0;
        };
        let t = (seen.elapsed().as_secs_f32() / self.style.fade_in.as_secs_f32()).clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// Draws the overlay window, when it is open. Called every frame by the
    /// app; it also keeps a slow repaint going to follow the game window.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        rows: &[NearbyRow],
        active: usize,
        characters: &str,
        max_rows: usize,
        opacity: f32,
    ) {
        if !self.open {
            return;
        }
        if self.rows != rows {
            self.rows = rows.to_vec();
        }
        // follows a change of language
        let texts = OverlayTexts::current();
        if self.texts != texts {
            self.texts = texts;
        }
        if self.characters != characters {
            self.characters = characters.to_string();
        }
        self.max_rows = max_rows.clamp(1, crate::app::intel::nearby::MAX_ROWS);
        self.active = active;
        self.opacity = opacity.clamp(0.0, 1.0);
        let animating = self.track_new_rows() | self.track_ghosts();
        // locked, the pointer is watched to let the padlock be clicked
        ctx.request_repaint_after(if animating {
            self.style.frame
        } else if self.locked && !OverlayWindow::supports_input_rect() {
            self.style.locked_poll
        } else {
            self.style.poll
        });
        self.poll_game(ctx);
        self.update_pointer_capture(ctx);

        let mut size = self.style.size();
        if let Some(width) = self.free_width {
            size.x = width;
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(&self.title)
            .with_inner_size(size)
            .with_decorations(false)
            .with_transparent(OverlayWindow::blending() == Blending::Pixels)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false);
        if let Some(target) = self.last_target {
            let ppp = Self::pixels_per_point(ctx);
            builder = builder
                .with_position(egui::pos2(
                    Self::to_points(target.x, ppp),
                    Self::to_points(target.y, ppp),
                ))
                .with_inner_size(egui::vec2(
                    Self::to_points(target.width as i32, ppp),
                    Self::to_points(target.height as i32, ppp),
                ));
        }
        ctx.show_viewport_immediate(self.viewport_id(), builder, |ui, _class| {
            self.draw(ui);
        });
    }

    /// Looks for the game window (a few times a second) and moves the overlay
    /// when the game moved or was resized.
    fn poll_game(&mut self, ctx: &egui::Context) {
        if self
            .last_poll
            .is_some_and(|last| last.elapsed() < self.style.poll)
        {
            return;
        }
        self.last_poll = Some(Instant::now());
        self.game = self.tracker.locate();
        self.update_visibility(ctx);
        self.check_exclusive_full_screen();
        let Some(game) = self.game else {
            // no game: the overlay is a free window
            self.last_game_rect = None;
            return;
        };
        let ppp = Self::pixels_per_point(ctx);
        if !self.placed {
            self.place(&game.rect, ppp);
        }
        // the width given while there was no game goes with the place now
        if let Some(width) = self.free_width.take() {
            self.set_placement_width(&game.rect, Self::to_pixels(width, ppp).max(1) as u32);
            self.last_game_rect = None;
        }
        if self.last_game_rect == Some(game.rect) {
            return;
        }
        self.last_game_rect = Some(game.rect);
        let target = self.placement.rect_in(&game.rect);
        self.last_target = Some(target);
        let id = self.viewport_id();
        ctx.send_viewport_cmd_to(
            id,
            egui::ViewportCommand::OuterPosition(egui::pos2(
                Self::to_points(target.x, ppp),
                Self::to_points(target.y, ppp),
            )),
        );
        ctx.send_viewport_cmd_to(
            id,
            egui::ViewportCommand::InnerSize(egui::vec2(
                Self::to_points(target.width as i32, ppp),
                Self::to_points(target.height as i32, ppp),
            )),
        );
    }

    /// Exclusive full screen hides every other window, the overlay too: tell
    /// the user once (see [`Self::take_warning`]) how to fix it.
    fn check_exclusive_full_screen(&mut self) {
        let exclusive =
            self.game.is_some_and(|game| game.focused) && self.fullscreen.is_exclusive();
        if exclusive && !self.warned_exclusive {
            self.pending_warning = Some(String::from(Self::EXCLUSIVE_WARNING));
        }
        self.warned_exclusive = exclusive;
    }

    /// A message for the status log, if there is one to show.
    pub(crate) fn take_warning(&mut self) -> Option<String> {
        self.pending_warning.take()
    }

    /// Shows the overlay while the game is in front (or Telescope itself, to
    /// move the overlay); with no game running it is a free window. Hidden, the
    /// window stays alive so it does not flicker when the focus comes back.
    /// Makes this overlay follow the client window of `name` (`EVE - <name>`)
    /// instead of any client, so it hides when that client closes. Only where
    /// window titles are readable (Windows, Linux): macOS names the windows'
    /// owner, the same for every client, and keeps following any of them.
    fn follow_character(&mut self, name: &str) {
        if self.followed == name {
            return;
        }
        self.followed = name.to_owned();
        self.tracker = GameWindowTracker::new(format!(
            "{} - {name}",
            GameWindowTracker::DEFAULT_TITLE_PREFIX
        ));
        // look for the window again right away
        self.last_poll = None;
    }

    /// Whether the window is shown: as long as the client exists, whoever has
    /// the focus; with no client, only while it is being positioned, since a
    /// locked overlay has nothing to sit on once the client is closed.
    fn should_show(game: Option<GameWindow>, locked: bool) -> bool {
        game.is_some() || !locked
    }

    fn update_visibility(&mut self, ctx: &egui::Context) {
        let visible = Self::should_show(self.game, self.locked);
        if visible != self.visible {
            self.visible = visible;
            ctx.send_viewport_cmd_to(self.viewport_id(), egui::ViewportCommand::Visible(visible));
        }
    }

    /// First placement for a game window: the top-right corner, scaled to
    /// this screen from now on (`calibrate`).
    fn place(&mut self, game: &ScreenRect, pixels_per_point: f32) {
        self.placement = OverlayPlacement::new(
            Anchor::TopRight,
            self.style.slot_offset(self.slot, pixels_per_point),
            Self::to_pixels(80.0, pixels_per_point),
            Self::to_pixels(
                self.free_width.unwrap_or(self.style.size().x),
                pixels_per_point,
            )
            .max(1) as u32,
            Self::to_pixels(self.style.size().y, pixels_per_point).max(1) as u32,
        );
        self.placement.calibrate(game);
        self.placed = true;
    }

    /// The user dragged the window: remember where, relative to the game.
    fn track_drag(&mut self, outer: egui::Rect, pixels_per_point: f32) {
        let Some(game) = self.game else {
            return;
        };
        let x = Self::to_pixels(outer.min.x, pixels_per_point);
        let y = Self::to_pixels(outer.min.y, pixels_per_point);
        let Some(last) = self.last_target else {
            return;
        };
        if (x - last.x).abs() <= self.style.move_tolerance
            && (y - last.y).abs() <= self.style.move_tolerance
        {
            return;
        }
        self.placement.move_to(&game.rect, x, y);
        self.moved = true;
        self.last_target = Some(ScreenRect::new(x, y, last.width, last.height));
    }

    /// Applies the native settings once the window exists.
    fn apply_native(&mut self) {
        if self.native_applied || self.native_attempts >= self.style.native_attempts {
            return;
        }
        self.native_attempts += 1;
        match self.native.apply_by_title(&self.title) {
            Ok(()) => self.native_applied = true,
            Err(OverlayError::Unsupported) => {
                tracing::warn!("the overlay's native settings are not supported here");
                self.native_applied = true;
            }
            Err(error) if self.native_attempts >= self.style.native_attempts => {
                tracing::warn!(%error, "could not apply the overlay's native settings");
            }
            Err(_) => {}
        }
    }

    /// Gives the window its opacity once it exists (`native_tools` does it
    /// the way each system can, and only touches the window when it changes).
    fn apply_opacity(&mut self) {
        if self.native_applied {
            // a window that is not ready yet answers an error: try again
            let _ = self.native.set_opacity_by_title(&self.title, self.opacity);
        }
    }

    /// The opacity the panel's pixels are drawn with: opaque when the system
    /// blends the whole window, as they are when it blends the pixels.
    fn paint_opacity(&self) -> f32 {
        match OverlayWindow::blending() {
            Blending::Window => 1.0,
            Blending::Pixels => self.opacity,
        }
    }

    /// The contents of the overlay window.
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ctx.input(|input| input.viewport().close_requested()) {
            self.close();
            return;
        }
        self.overlay_focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        self.apply_native();
        self.apply_opacity();
        let ppp = Self::pixels_per_point(&ctx);
        if let Some(outer) = ctx.input(|input| input.viewport().outer_rect) {
            self.track_drag(outer, ppp);
        }

        let rect = ui.max_rect();
        let lock = self.style.lock_button(self.panel_rect(rect));
        self.sync_input_rect(&ctx, rect, lock, ppp);

        // Only what is painted is shown: the window has no background and no
        // border. The rest is cut away natively, so this needs no
        // transparent swapchain.
        let shown = self.shown_parts(rect);
        self.apply_region(rect, &shown, ppp);

        // positioning mode: pressing on the right edge resizes the window,
        // anywhere else but on the padlock moves it; the native drag has to
        // start right at the press
        if !self.locked {
            let grip = self.style.grip_rect(self.panel_rect(rect));
            let (pressed, down, pointer) = ui.input(|input| {
                (
                    input.pointer.primary_pressed(),
                    input.pointer.primary_down(),
                    input.pointer.interact_pos(),
                )
            });
            // the pointer tells that this edge resizes
            if self.resizing || pointer.is_some_and(|pos| grip.contains(pos) && !lock.contains(pos))
            {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if pressed
                && let Some(pos) = pointer
                && rect.contains(pos)
                && !lock.contains(pos)
            {
                if grip.contains(pos) {
                    self.resizing = true;
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            }
            if self.resizing {
                match pointer {
                    Some(pos) if down => self.resize_to(&ctx, pos.x - rect.min.x, ppp),
                    _ if !down => self.resizing = false,
                    _ => {}
                }
            }
        } else {
            self.resizing = false;
        }
        // The padlock is read straight from the pointer instead of a widget
        // response, which needs a stable id across the press and the release.
        let (hovered, clicked) = ui.input(|input| {
            let over = input
                .pointer
                .interact_pos()
                .is_some_and(|pos| lock.contains(pos));
            (over, over && input.pointer.primary_clicked())
        });
        if clicked {
            self.set_locked(!self.locked);
            return;
        }
        self.paint_content(ui, rect, hovered);
    }

    /// The user drags the right edge: the window takes `width` points (within
    /// limits), its top-left corner staying where it is, and the new width is
    /// remembered with the place.
    ///
    /// With the game window found the width goes with the place, relative to
    /// the game. Without it (EVE is not running) the window is a free one: it
    /// takes the width all the same, and the placement keeps it for when the
    /// game shows up.
    fn resize_to(&mut self, ctx: &egui::Context, width: f32, pixels_per_point: f32) {
        let width = width.clamp(self.style.min_width, self.style.max_width);
        let width_px = Self::to_pixels(width, pixels_per_point).max(1) as u32;
        let height = match (self.game, self.last_target) {
            (Some(game), Some(target)) => {
                if width_px == target.width {
                    return;
                }
                self.set_placement_width(&game.rect, width_px);
                self.placement.move_to(&game.rect, target.x, target.y);
                self.last_target =
                    Some(ScreenRect::new(target.x, target.y, width_px, target.height));
                Self::to_points(target.height as i32, pixels_per_point)
            }
            _ => {
                if self.free_width == Some(width) {
                    return;
                }
                self.free_width = Some(width);
                // real pixels, as a placement that was never calibrated has
                if self.placement.reference_height().is_none() {
                    let (_, _, _, _, stored_height) = self.placement.parts();
                    self.placement.resize(width_px, stored_height);
                }
                self.style.size().y
            }
        };
        self.moved = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
    }

    /// Gives the placement a width of `width_px` real pixels in `game`. Once
    /// calibrated it keeps its size in reference pixels: converts from the
    /// real pixels the pointer gives.
    fn set_placement_width(&mut self, game: &ScreenRect, width_px: u32) {
        let (_, _, _, stored_width, stored_height) = self.placement.parts();
        let shown = self.placement.rect_in(game);
        let factor = if stored_width > 0 {
            f64::from(shown.width) / f64::from(stored_width)
        } else {
            1.0
        };
        let stored = (f64::from(width_px) / factor).round().max(1.0) as u32;
        self.placement.resize(stored, stored_height);
    }

    /// Where the system can keep a part of a click-through window clickable
    /// (X11), the padlock is that part while locked: the rest of the window
    /// lets the mouse through, with no pointer watching.
    fn sync_input_rect(
        &mut self,
        ctx: &egui::Context,
        window: egui::Rect,
        lock: egui::Rect,
        pixels_per_point: f32,
    ) {
        if !OverlayWindow::supports_input_rect() {
            return;
        }
        let wanted = self.locked.then(|| {
            let local = lock.translate(-window.min.to_vec2());
            ScreenRect::new(
                Self::to_pixels(local.min.x, pixels_per_point),
                Self::to_pixels(local.min.y, pixels_per_point),
                Self::to_pixels(local.width(), pixels_per_point).max(1) as u32,
                Self::to_pixels(local.height(), pixels_per_point).max(1) as u32,
            )
        });
        if self.native.input_rect() != wanted {
            self.native.set_input_rect(wanted);
            self.native_applied = false;
            self.native_attempts = 0;
            ctx.request_repaint();
        }
    }

    /// While locked the window is click-through so the mouse reaches the game;
    /// only when the pointer is over the padlock is it not, so the padlock can
    /// be clicked. Where the pointer position cannot be known without the
    /// window's events (macOS), the window is not click-through. On X11 the
    /// padlock stays clickable by itself (see `sync_input_rect`).
    fn update_pointer_capture(&mut self, ctx: &egui::Context) {
        if !self.locked || !self.native_applied || OverlayWindow::supports_input_rect() {
            return;
        }
        let over_lock = match (native_tools::overlay::cursor_position(), self.last_target) {
            (Some((x, y)), Some(target)) => {
                let ppp = Self::pixels_per_point(ctx);
                let width = Self::to_points(target.width as i32, ppp);
                let window = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, self.style.size().y),
                );
                let lock = self.style.lock_button(self.panel_rect(window));
                let left = target.x + Self::to_pixels(lock.min.x, ppp);
                let top = target.y + Self::to_pixels(lock.min.y, ppp);
                let right = target.x + Self::to_pixels(lock.max.x, ppp);
                let bottom = target.y + Self::to_pixels(lock.max.y, ppp);
                (left..right).contains(&x) && (top..bottom).contains(&y)
            }
            _ => true,
        };
        if self.native.click_through() == over_lock {
            self.native.set_click_through(!over_lock);
            self.native_applied = false;
            self.native_attempts = 0;
        }
    }

    /// Cuts the window down to `shown` (screen-space egui rectangles of the
    /// viewport), once per change.
    fn apply_region(&mut self, window: egui::Rect, shown: &[egui::Rect], pixels_per_point: f32) {
        if !self.native_applied {
            return;
        }
        let region: Vec<ScreenRect> = shown
            .iter()
            .map(|part| {
                let local = part.translate(-window.min.to_vec2());
                ScreenRect::new(
                    Self::to_pixels(local.min.x, pixels_per_point),
                    Self::to_pixels(local.min.y, pixels_per_point),
                    Self::to_pixels(local.width(), pixels_per_point).max(1) as u32,
                    Self::to_pixels(local.height(), pixels_per_point).max(1) as u32,
                )
            })
            .collect();
        if self.region.as_ref() == Some(&region) {
            return;
        }
        match self
            .native
            .set_visible_region_by_title(&self.title, &region)
        {
            Ok(()) | Err(OverlayError::Unsupported) => self.region = Some(region),
            // the window is not ready yet: the next frame tries again
            Err(_) => {}
        }
    }

    /// A padlock drawn with shapes (no icon font needed), centered on
    /// `center`: closed when `locked`, with the shackle lifted otherwise.
    fn paint_padlock(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        color: egui::Color32,
        locked: bool,
    ) {
        let body =
            egui::Rect::from_center_size(center + egui::vec2(0.0, 2.5), egui::vec2(11.0, 8.0));
        painter.rect_filled(body, 1.5, color);
        // the shackle: a half circle over the body
        let radius = 3.5;
        let base = if locked {
            egui::pos2(center.x, body.top())
        } else {
            egui::pos2(center.x + 3.0, body.top() - 2.5)
        };
        let shackle: Vec<egui::Pos2> = (0..=12)
            .map(|step| {
                let angle = std::f32::consts::PI * step as f32 / 12.0;
                egui::pos2(base.x - radius * angle.cos(), base.y - radius * angle.sin())
            })
            .collect();
        painter.add(egui::Shape::line(shackle, egui::Stroke::new(1.8, color)));
        painter.circle_filled(
            body.center(),
            1.2,
            Self::with_opacity(self.style.header_color, 1.0),
        );
    }

    /// The overlay's font (Oxanium, see `OVERLAY_FONT`).
    fn font(size: f32) -> egui::FontId {
        egui::FontId::new(
            size,
            egui::FontFamily::Name(crate::app::OVERLAY_FONT.into()),
        )
    }

    /// The bold weight, for the system name and the jumps.
    fn font_bold(size: f32) -> egui::FontId {
        egui::FontId::new(
            size,
            egui::FontFamily::Name(crate::app::OVERLAY_FONT_BOLD.into()),
        )
    }

    /// Whether the panel shows made-up rows: while positioning with no real
    /// event, so the user sees what the overlay looks like.
    fn is_example(&self) -> bool {
        self.rows.is_empty() && !self.locked
    }

    /// The rows the panel lists: the events, or the example while positioning
    /// without any.
    fn panel_rows(&self) -> std::borrow::Cow<'_, [NearbyRow]> {
        if self.is_example() {
            std::borrow::Cow::Owned(self.example_rows(self.max_rows))
        } else {
            std::borrow::Cow::Borrowed(&self.rows)
        }
    }

    /// `count` made-up events at growing distances, with placeholder system
    /// names (no real ones), so the panel shows the height it will have.
    fn example_rows(&self, count: usize) -> Vec<NearbyRow> {
        let texts = &self.texts;
        let lines = [
            &texts.example_hostiles,
            &texts.example_kill,
            &texts.example_gate,
        ];
        (0..count)
            .map(|index| NearbyRow {
                jumps: (index as u8).saturating_add(1).min(7),
                system: texts
                    .example_system
                    .replace(OverlayTexts::NAME, &Self::example_letter(index).to_string()),
                text: lines[index % lines.len()].clone(),
                age_secs: 12 + index as u64 * 36,
            })
            .collect()
    }

    /// `A`, `B`, `C`... for the placeholder systems.
    fn example_letter(index: usize) -> char {
        char::from(b'A' + (index % 26) as u8)
    }

    /// The rows that fit in `area`, below the header.
    fn room_rows(&self, area: egui::Rect) -> usize {
        let room =
            ((area.height() - self.style.header_height - 2.0) / self.style.row_height).floor();
        room.max(0.0) as usize
    }

    /// The rows listed in `area`: the events, up to the maximum.
    fn visible_rows(&self, area: egui::Rect) -> usize {
        self.panel_rows()
            .len()
            .min(self.room_rows(area))
            .min(self.max_rows)
    }

    /// The rows fading out under the listed ones, while there is room.
    fn ghost_rows(&self, area: egui::Rect) -> usize {
        self.ghosts
            .len()
            .min(self.room_rows(area).saturating_sub(self.visible_rows(area)))
    }

    /// Where the panel is: the top of `area`, as tall as its rows need.
    fn panel_rect(&self, area: egui::Rect) -> egui::Rect {
        let rows = self.visible_rows(area) + self.ghost_rows(area);
        let height = self.style.header_height + 2.0 + rows as f32 * self.style.row_height;
        egui::Rect::from_min_size(area.min, egui::vec2(area.width(), height))
    }

    /// Notes the events that were listed and no longer are (pushed past the
    /// maximum by newer ones, or expired): they fade out for a moment under
    /// the last row. Returns whether one is still fading out.
    fn track_ghosts(&mut self) -> bool {
        let key = |row: &NearbyRow| (row.system.clone(), row.text.clone());
        let listed: Vec<_> = self.rows.iter().take(self.max_rows).map(key).collect();
        for row in std::mem::take(&mut self.listed_before) {
            let gone = !listed.contains(&key(&row));
            if gone && !self.ghosts.iter().any(|ghost| key(&ghost.row) == key(&row)) {
                self.ghosts.push(Ghost {
                    row,
                    since: Instant::now(),
                });
            }
        }
        self.ghosts.retain(|ghost| {
            !listed.contains(&key(&ghost.row)) && ghost.since.elapsed() < self.style.fade_out
        });
        self.listed_before = self.rows.iter().take(self.max_rows).cloned().collect();
        !self.ghosts.is_empty()
    }

    /// How far a leaving row has faded out, 0 (just left) to 1 (gone).
    fn fade_out_progress(&self, since: Instant) -> f32 {
        (since.elapsed().as_secs_f32() / self.style.fade_out.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// The window parts that are painted: only these are shown, the rest of
    /// the window is cut away.
    fn shown_parts(&self, area: egui::Rect) -> Vec<egui::Rect> {
        vec![self.panel_rect(area)]
    }

    /// `m:ss` since the event.
    fn age_text(age_secs: u64) -> String {
        format!("{}:{:02}", age_secs / 60, age_secs % 60)
    }

    /// A color of the panel with the opacity of the setting.
    fn with_opacity((r, g, b): (u8, u8, u8), alpha: f32) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(
            r,
            g,
            b,
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }

    /// Where the middle of the capitals and digits of `font` is, measured down
    /// from the top of a line of text: what to center a line on so it looks
    /// centered (the line box itself is taller than the glyphs).
    fn cap_center(painter: &egui::Painter, font: egui::FontId) -> f32 {
        let galley = painter.layout_no_wrap(String::from("0"), font, egui::Color32::WHITE);
        galley
            .rows
            .first()
            .map(|row| row.visuals.mesh_bounds.center().y)
            .filter(|y| y.is_finite() && *y > 0.0)
            .unwrap_or(galley.size().y / 2.0)
    }

    /// One line of text, cut with an ellipsis to `width`.
    fn text_galley(
        painter: &egui::Painter,
        text: &str,
        font: egui::FontId,
        color: egui::Color32,
        width: f32,
    ) -> std::sync::Arc<egui::Galley> {
        let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width.max(1.0));
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('\u{2026}');
        painter.layout_job(job)
    }

    /// The header's title: the character(s) the panel is for (the panel's name
    /// while there is none) and, while positioning without events, that the
    /// rows are an example.
    fn header_title(&self) -> String {
        let mut parts = vec![if self.characters.is_empty() {
            self.texts.nearby.clone()
        } else {
            self.characters.clone()
        }];
        if self.is_example() {
            parts.push(self.texts.example.clone());
        }
        parts.join(" · ")
    }

    /// A new event fading in: `row` drawn with an opacity of `progress`, 0
    /// (just arrived, invisible) to 1 (fully shown).
    fn paint_row_fading_in(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        row: &NearbyRow,
        progress: f32,
    ) {
        self.paint_row_faded(painter, rect, row, progress);
    }

    /// An event leaving, fading out: `row` drawn with an opacity of
    /// `1 - progress`, `progress` 0 (just left, fully shown) to 1 (gone).
    fn paint_row_fading_out(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        row: &NearbyRow,
        progress: f32,
    ) {
        self.paint_row_faded(painter, rect, row, 1.0 - progress);
    }

    /// `row` drawn with `opacity` (0 to 1).
    fn paint_row_faded(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        row: &NearbyRow,
        opacity: f32,
    ) {
        let mut faded = painter.clone();
        faded.multiply_opacity(opacity.clamp(0.0, 1.0));
        self.paint_row(&faded, rect, row);
    }

    /// One event of the panel, in `rect`.
    fn paint_row(&self, painter: &egui::Painter, rect: egui::Rect, row: &NearbyRow) {
        let (stripe, color, tinted) = self.style.jump_colors(row.jumps);
        if tinted {
            painter.rect_filled(
                rect,
                0.0,
                Self::with_opacity((70, 20, 24), self.paint_opacity() * 0.7),
            );
        }
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            0.0,
            stripe,
        );
        let mid = rect.center().y;
        painter.text(
            egui::pos2(rect.left() + 9.0, mid),
            egui::Align2::LEFT_CENTER,
            format!("{}j", row.jumps),
            Self::font_bold(11.0),
            color,
        );
        let system = Self::text_galley(
            painter,
            &row.system,
            Self::font_bold(11.5),
            egui::Color32::from_rgb(238, 241, 245),
            64.0,
        );
        painter.galley(
            egui::pos2(rect.left() + 34.0, mid - system.size().y / 2.0),
            system,
            egui::Color32::WHITE,
        );
        let age = Self::age_text(row.age_secs);
        let age_width = 30.0;
        painter.text(
            egui::pos2(rect.right() - 6.0, mid),
            egui::Align2::RIGHT_CENTER,
            age,
            Self::font(10.5),
            egui::Color32::from_rgb(125, 137, 155),
        );
        let text_left = rect.left() + 102.0;
        let summary = Self::text_galley(
            painter,
            &row.text,
            Self::font(11.0),
            egui::Color32::from_rgb(181, 189, 201),
            rect.right() - age_width - 6.0 - text_left,
        );
        painter.galley(
            egui::pos2(text_left, mid - summary.size().y / 2.0),
            summary,
            egui::Color32::WHITE,
        );
    }

    /// The content of the area: the events panel, with an example while
    /// positioning with no events. The header always shows the padlock, also
    /// when locked and empty (`lock_hovered` is for its hover color).
    fn paint_content(&self, ui: &egui::Ui, area: egui::Rect, lock_hovered: bool) {
        let rows = self.panel_rows();
        let panel = self.panel_rect(area);
        // everything stays inside the panel: a new row slides in from its edge
        let clipped = ui.painter().with_clip_rect(panel);
        let painter = &clipped;
        let border = Self::with_opacity(
            self.style.panel_border,
            (self.paint_opacity() + 0.2).min(1.0),
        );
        painter.rect_filled(
            panel,
            0.0,
            Self::with_opacity(self.style.panel_color, self.paint_opacity()),
        );
        let header = egui::Rect::from_min_size(
            panel.min,
            egui::vec2(panel.width(), self.style.header_height),
        );
        painter.rect_filled(
            header,
            0.0,
            Self::with_opacity(self.style.header_color, self.paint_opacity()),
        );
        let lock = self.style.lock_button(panel);
        let lock_color = if lock_hovered {
            egui::Color32::from_rgb(140, 220, 140)
        } else if self.locked {
            egui::Color32::from_gray(150)
        } else {
            egui::Color32::from_rgb(224, 129, 58)
        };
        self.paint_padlock(painter, lock.center(), lock_color, self.locked);
        // The header's texts are centered on their capitals and digits, not on
        // the line box, which also holds room for descenders and sits high.
        let text_center = Self::cap_center(painter, Self::font(10.5));
        let mut title_right = lock.left() - 4.0;
        if !self.is_example() {
            let count = self.active.to_string();
            let count_galley = painter.layout_no_wrap(
                count,
                Self::font(10.5),
                egui::Color32::from_rgb(224, 129, 58),
            );
            title_right -= count_galley.size().x + 6.0;
            painter.galley(
                egui::pos2(
                    lock.left() - 2.0 - count_galley.size().x,
                    header.center().y - text_center,
                ),
                count_galley,
                egui::Color32::WHITE,
            );
        }
        let title = Self::text_galley(
            painter,
            &self.header_title().to_uppercase(),
            Self::font(10.5),
            egui::Color32::from_rgb(170, 180, 196),
            title_right - (header.left() + 8.0),
        );
        painter.galley(
            egui::pos2(header.left() + 8.0, header.center().y - text_center),
            title,
            egui::Color32::WHITE,
        );

        let mut top = header.bottom();
        for row in rows.iter().take(self.visible_rows(area)) {
            let rect = egui::Rect::from_min_size(
                egui::pos2(panel.left(), top),
                egui::vec2(panel.width(), self.style.row_height),
            );
            // a new event fades in
            let progress = self.enter_progress(row);
            if progress < 1.0 {
                self.paint_row_fading_in(&clipped, rect, row, progress);
            } else {
                self.paint_row(&clipped, rect, row);
            }
            top += self.style.row_height;
        }
        // the events pushed out of the panel fade out below the last row
        for ghost in self.ghosts.iter().take(self.ghost_rows(area)) {
            let rect = egui::Rect::from_min_size(
                egui::pos2(panel.left(), top),
                egui::vec2(panel.width(), self.style.row_height),
            );
            let progress = self.fade_out_progress(ghost.since);
            self.paint_row_fading_out(&clipped, rect, &ghost.row, progress);
            top += self.style.row_height;
        }
        painter.rect_stroke(
            panel,
            0.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        if !self.locked {
            // the edge that is dragged to change the width
            let grip = self.style.grip_rect(panel);
            painter.line_segment(
                [
                    egui::pos2(grip.center().x, grip.top() + 4.0),
                    egui::pos2(grip.center().x, grip.bottom() - 4.0),
                ],
                egui::Stroke::new(1.5, egui::Color32::from_rgb(224, 129, 58)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_closed_and_toggles() {
        let mut overlay = Overlays::new();
        assert!(!overlay.is_open());
        overlay.toggle();
        assert!(overlay.is_open());
        overlay.toggle();
        assert!(!overlay.is_open());
    }

    #[test]
    fn a_saved_place_is_restored_and_a_move_is_handed_over_once() {
        let position = OverlayPosition {
            anchor: OverlayAnchor::BottomLeft,
            offset_x: 12,
            offset_y: 34,
            width: 260,
            height: 200,
            reference_height: Some(1080),
        };
        let mut window = OverlayState::for_window(7, 0, Some(position));
        assert!(window.placed);
        assert_eq!(window.take_position(), None);
        window.moved = true;
        assert_eq!(window.take_position(), Some(position));
        assert_eq!(window.take_position(), None);
    }

    #[test]
    fn a_window_that_goes_away_keeps_where_it_was_left() {
        let mut overlays = Overlays::new();
        overlays.open = true;
        let mut window = OverlayState::for_window(7, 0, None);
        window.moved = true;
        overlays.windows.push((7, window));
        overlays.drop_windows(|key| key == 7);
        assert!(overlays.windows.is_empty());
        let positions = overlays.take_positions();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].0, 7);
        assert!(overlays.take_positions().is_empty());
    }

    #[test]
    fn dragging_the_right_edge_resizes_the_window_also_without_the_game() {
        let ctx = egui::Context::default();
        let mut overlay = OverlayState::new();
        assert!(overlay.game.is_none());
        overlay.resize_to(&ctx, 400.0, 1.0);
        assert_eq!(overlay.free_width, Some(400.0));
        assert!(overlay.moved);
        // the place keeps it too, in real pixels
        let position = overlay.take_position().expect("moved");
        assert_eq!(position.width, 400);

        // within the limits of the style
        overlay.resize_to(&ctx, 5000.0, 1.0);
        assert_eq!(overlay.free_width, Some(overlay.style.max_width));
        overlay.resize_to(&ctx, 10.0, 1.0);
        assert_eq!(overlay.free_width, Some(overlay.style.min_width));
    }

    #[test]
    fn the_width_given_without_the_game_goes_with_the_place_when_it_shows_up() {
        let ctx = egui::Context::default();
        let mut overlay = OverlayState::new();
        overlay.resize_to(&ctx, 420.0, 1.0);

        let game = ScreenRect::new(0, 0, 1920, 1080);
        overlay.place(&game, 1.0);
        overlay.set_placement_width(&game, 420);
        let target = overlay.placement.rect_in(&game);
        assert_eq!(target.width, 420);
    }

    #[test]
    fn dragging_the_right_edge_with_the_game_changes_the_width_of_the_place() {
        let ctx = egui::Context::default();
        let mut overlay = OverlayState::new();
        let game = ScreenRect::new(0, 0, 1920, 1080);
        overlay.game = Some(GameWindow {
            rect: game,
            focused: true,
        });
        overlay.place(&game, 1.0);
        // away from the right edge of the game, where it could not grow
        overlay.placement.move_to(&game, 100, 80);
        let target = overlay.placement.rect_in(&game);
        overlay.last_target = Some(target);

        overlay.resize_to(&ctx, 350.0, 1.0);
        let resized = overlay.placement.rect_in(&game);
        assert_eq!(resized.width, 350);
        // the top-left corner stayed where it was
        assert_eq!((resized.x, resized.y), (target.x, target.y));
        assert_eq!(overlay.last_target.map(|t| t.width), Some(350));
        assert!(overlay.free_width.is_none());
        assert!(overlay.moved);
    }

    #[test]
    fn each_window_has_its_own_title_and_starting_place() {
        let first = OverlayState::for_window(7, 0, None);
        let second = OverlayState::for_window(8, 1, None);
        assert_ne!(first.title, second.title);
        assert_ne!(first.viewport_id(), second.viewport_id());
        assert_eq!(OverlayState::new().title, OverlayState::TITLE);
        assert!(
            OverlayStyle::default().slot_offset(1, 1.0)
                > OverlayStyle::default().slot_offset(0, 1.0)
        );
    }

    #[test]
    fn there_is_no_warning_without_a_game() {
        let mut overlay = OverlayState::new();
        overlay.check_exclusive_full_screen();
        assert_eq!(overlay.take_warning(), None);
    }

    fn row(jumps: u8, system: &str) -> NearbyRow {
        NearbyRow {
            jumps,
            system: system.to_string(),
            text: String::from("2 in gate"),
            age_secs: 5,
        }
    }

    fn area() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 24.0), egui::vec2(260.0, 136.0))
    }

    // ---- Snapshots (egui_kittest) ----
    //
    // The overlay is painted, not made of widgets, so it is checked by its
    // image: `tests/snapshots/overlay_*.png`, compared within the tolerance
    // of `kittest.toml`. To accept a change of look, run the tests with
    // `UPDATE_SNAPSHOTS=true` and review the new images. Where there is no
    // GPU adapter to render with (a headless CI) the check is skipped, with a
    // note in the output; any other failure fails the test.

    /// English texts, whatever the language other tests left in use.
    fn fixed_texts() -> OverlayTexts {
        OverlayTexts {
            nearby: String::from("Nearby"),
            example: String::from("example"),
            example_hostiles: String::from("+4 Loki, Sabre, Huginn"),
            example_kill: String::from("Kill 412M · Ishtar"),
            example_gate: String::from("2 at the gate"),
            example_system: format!("System {}", OverlayTexts::NAME),
        }
    }

    /// An overlay window as `setup` leaves it, with fixed texts.
    fn overlay_for_snapshot(setup: impl FnOnce(&mut OverlayState)) -> OverlayState {
        let mut overlay = OverlayState::new();
        overlay.texts = fixed_texts();
        setup(&mut overlay);
        overlay
    }

    /// A harness of `size` points, at `pixels_per_point`, that draws with
    /// Telescope's fonts.
    fn snapshot_harness<S: 'static>(
        size: egui::Vec2,
        pixels_per_point: f32,
        draw: impl FnMut(&mut egui::Ui, &mut S) + 'static,
        state: S,
    ) -> egui_kittest::Harness<'static, S> {
        egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(pixels_per_point)
            .with_theme(egui::Theme::Dark)
            .with_context_setup(|ctx| ctx.set_fonts(crate::app::TelescopeApp::font_definitions()))
            .build_ui_state(draw, state)
    }

    /// Runs `draw` on a `Ui` that covers the whole harness, without the
    /// margins of the panel the harness puts its content in: the overlay's
    /// window draws from its very edges.
    fn whole_surface(ui: &mut egui::Ui, draw: impl FnOnce(&mut egui::Ui, egui::Rect)) {
        let full = ui.ctx().content_rect();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
        child.set_clip_rect(full);
        draw(&mut child, full);
    }

    /// Compares the harness's image with `name`; skipped (with a note) when
    /// nothing can render it.
    fn check_snapshot<S>(harness: &mut egui_kittest::Harness<'_, S>, name: &str) {
        harness.run_steps(2);
        match harness.try_snapshot(name) {
            Ok(()) => {}
            Err(egui_kittest::SnapshotError::RenderError { err }) => {
                eprintln!("snapshot {name} skipped: nothing to render it with ({err})");
            }
            Err(error) => panic!("snapshot {name}: {error}"),
        }
    }

    /// The panel of `overlay`, as its window draws it.
    fn panel_snapshot(overlay: OverlayState, name: &str) {
        let size = overlay.style.size();
        let mut harness = snapshot_harness(
            size,
            1.0,
            |ui, overlay: &mut OverlayState| {
                whole_surface(ui, |ui, area| overlay.paint_content(ui, area, false));
            },
            overlay,
        );
        check_snapshot(&mut harness, name);
    }

    #[test]
    fn snapshot_the_example_while_positioning() {
        panel_snapshot(
            overlay_for_snapshot(|overlay| overlay.max_rows = 5),
            "overlay_example",
        );
    }

    #[test]
    fn snapshot_a_locked_overlay_with_events_at_every_distance() {
        panel_snapshot(
            overlay_for_snapshot(|overlay| {
                overlay.locked = true;
                overlay.characters = String::from("Pilot Seven");
                overlay.rows = (1..=7)
                    .map(|jumps| NearbyRow {
                        jumps,
                        system: format!("System {jumps}"),
                        text: String::from("2 in gate, a Sabre"),
                        age_secs: u64::from(jumps) * 23,
                    })
                    .collect();
                overlay.active = 9;
            }),
            "overlay_events",
        );
    }

    #[test]
    fn snapshot_a_locked_overlay_without_events() {
        panel_snapshot(
            overlay_for_snapshot(|overlay| {
                overlay.locked = true;
                overlay.characters = String::from("Pilot Seven");
            }),
            "overlay_locked_empty",
        );
    }

    /// One row of `overlay`, as `draw` paints it at `progress`.
    fn row_snapshot(
        name: &str,
        progress: f32,
        draw: fn(&OverlayState, &egui::Painter, egui::Rect, &NearbyRow, f32),
    ) {
        let overlay = overlay_for_snapshot(|_| {});
        let size = egui::vec2(overlay.style.width, overlay.style.row_height);
        let row = NearbyRow {
            jumps: 1,
            system: String::from("System A"),
            text: String::from("+4 Loki, Sabre, Huginn"),
            age_secs: 12,
        };
        // a single row is small: three times the size to see its details
        let mut harness = snapshot_harness(
            size,
            3.0,
            move |ui, overlay: &mut OverlayState| {
                whole_surface(ui, |ui, rect| {
                    let painter = ui.painter().with_clip_rect(rect);
                    // over the panel, as in the overlay
                    painter.rect_filled(
                        rect,
                        0.0,
                        OverlayState::with_opacity(overlay.style.panel_color, 1.0),
                    );
                    draw(overlay, &painter, rect, &row, progress);
                });
            },
            overlay,
        );
        check_snapshot(&mut harness, name);
    }

    #[test]
    fn snapshot_a_new_event_halfway_faded_in() {
        row_snapshot(
            "overlay_row_fading_in",
            0.5,
            OverlayState::paint_row_fading_in,
        );
    }

    #[test]
    fn snapshot_an_event_halfway_faded_out() {
        row_snapshot(
            "overlay_row_fading_out",
            0.5,
            OverlayState::paint_row_fading_out,
        );
    }

    #[test]
    fn the_panel_is_as_tall_as_its_rows_and_fits_the_area() {
        let mut overlay = OverlayState::new();
        overlay.rows = vec![row(1, "Tama"), row(2, "Kedama")];
        let panel = overlay.panel_rect(area());
        assert_eq!(panel.height(), 22.0 + 2.0 + 2.0 * 22.0);
        assert_eq!(panel.min, area().min);

        overlay.rows = (0..20).map(|id| row(3, &format!("S{id}"))).collect();
        assert_eq!(overlay.visible_rows(area()), 5);
        assert!(overlay.panel_rect(area()).height() <= area().height());
    }

    #[test]
    fn only_the_header_is_shown_when_locked_without_events() {
        let mut overlay = OverlayState::new();
        overlay.locked = true;
        let parts = overlay.shown_parts(area());
        assert_eq!(parts, vec![overlay.panel_rect(area())]);
        assert_eq!(parts[0].height(), overlay.style.header_height + 2.0);

        // positioning shows the example in its place
        overlay.locked = false;
        assert_eq!(
            overlay.shown_parts(area()),
            vec![overlay.panel_rect(area())]
        );

        overlay.rows = vec![row(1, "Tama")];
        assert_eq!(
            overlay.shown_parts(area()),
            vec![overlay.panel_rect(area())]
        );
    }

    #[test]
    fn positioning_without_events_shows_an_example_and_locking_hides_it() {
        let mut overlay = OverlayState::new();
        assert!(overlay.is_example());
        // as many example rows as the maximum, as far as the window fits
        assert_eq!(overlay.panel_rows().len(), overlay.max_rows);
        assert_eq!(overlay.visible_rows(area()), 5);

        // real events replace the example
        overlay.rows = vec![row(1, "Tama")];
        assert!(!overlay.is_example());
        assert_eq!(overlay.panel_rows().len(), 1);

        // locked and empty: nothing to list
        overlay.rows.clear();
        overlay.locked = true;
        assert!(!overlay.is_example());
        assert!(overlay.panel_rows().is_empty());
    }

    #[test]
    fn rows_are_colored_by_distance_and_only_the_closest_are_tinted() {
        for jumps in 1..=7 {
            // a vivid accent
            let stripe = OverlayStyle::default().jump_colors(jumps).0;
            let brightest = [stripe.r(), stripe.g(), stripe.b()].into_iter().max();
            assert_eq!(brightest, Some(255), "{jumps}");
        }
        assert!(OverlayStyle::default().jump_colors(0).2);
        assert!(OverlayStyle::default().jump_colors(1).2);
        assert!(!OverlayStyle::default().jump_colors(2).2);
        assert!(!OverlayStyle::default().jump_colors(7).2);
        // every distance up to seven jumps has a color of its own
        for jumps in 1..7 {
            assert_ne!(
                OverlayStyle::default().jump_colors(jumps).0,
                OverlayStyle::default().jump_colors(jumps + 1).0,
                "{jumps}"
            );
        }
        // further than that stays blue
        assert_eq!(
            OverlayStyle::default().jump_colors(7).0,
            OverlayStyle::default().jump_colors(9).0
        );
    }

    #[test]
    fn colors_go_from_red_to_yellow_to_green_to_blue() {
        assert_eq!(OverlayStyle::default().jump_hue(0), 0.0);
        assert_eq!(OverlayStyle::default().jump_hue(1), 0.0);
        assert_eq!(OverlayStyle::default().jump_hue(3), 60.0);
        assert_eq!(OverlayStyle::default().jump_hue(5), 120.0);
        assert_eq!(OverlayStyle::default().jump_hue(7), 210.0);
        assert_eq!(OverlayStyle::default().jump_hue(20), 210.0);
        // never going back as the distance grows
        for jumps in 1..7 {
            assert!(
                OverlayStyle::default().jump_hue(jumps)
                    < OverlayStyle::default().jump_hue(jumps + 1)
            );
        }
        // red first, blue last
        let red = OverlayStyle::default().jump_colors(1).0;
        let blue = OverlayStyle::default().jump_colors(7).0;
        assert!(red.r() > red.g() && red.r() > red.b());
        assert!(blue.b() > blue.r() && blue.b() > blue.g());
    }

    #[test]
    fn the_panel_grows_only_up_to_the_configured_rows() {
        let mut overlay = OverlayState::new();
        overlay.rows = (0..8).map(|id| row(3, &format!("S{id}"))).collect();
        let tall = egui::Rect::from_min_size(egui::Pos2::ZERO, overlay.style.size());
        assert_eq!(overlay.visible_rows(tall), 8);
        overlay.max_rows = 3;
        assert_eq!(overlay.visible_rows(tall), 3);
    }

    #[test]
    fn the_header_names_the_characters() {
        let mut overlay = OverlayState::new();
        overlay.locked = true;
        overlay.characters = String::from("Ana, Luis");
        assert!(overlay.header_title().ends_with("Ana, Luis"));
        overlay.locked = false;
        assert!(overlay.header_title().contains("Ana, Luis"));
    }

    #[test]
    fn the_oldest_events_leave_the_queue_first_whatever_the_order() {
        let aged = |jumps: u8, system: &str, age_secs: u64| NearbyRow {
            age_secs,
            ..row(jumps, system)
        };
        let events = vec![
            aged(1, "old-close", 90),
            aged(5, "new-far", 5),
            aged(2, "mid", 40),
        ];
        // two places: the oldest is out, even though it is the closest
        let mut rows = events.clone();
        arrange_rows(&mut rows, 2, OverlayOrder::Jumps);
        let names: Vec<&str> = rows.iter().map(|row| row.system.as_str()).collect();
        assert_eq!(names, ["mid", "new-far"]);
        let mut rows = events;
        arrange_rows(&mut rows, 2, OverlayOrder::Age);
        let names: Vec<&str> = rows.iter().map(|row| row.system.as_str()).collect();
        assert_eq!(names, ["new-far", "mid"]);
    }

    #[test]
    fn the_example_shows_as_many_rows_as_the_maximum_with_made_up_names() {
        let mut overlay = OverlayState::new();
        overlay.max_rows = 5;
        let tall = egui::Rect::from_min_size(egui::Pos2::ZERO, overlay.style.size());
        assert!(overlay.is_example());
        assert_eq!(overlay.panel_rows().len(), 5);
        assert_eq!(overlay.visible_rows(tall), 5);
        for real in ["Tama", "Kedama", "Amamake"] {
            assert!(overlay.panel_rows().iter().all(|row| row.system != real));
        }
    }

    #[test]
    fn a_row_that_leaves_the_list_fades_out_under_it_for_a_while() {
        let mut overlay = OverlayState::new();
        overlay.max_rows = 2;
        overlay.rows = vec![row(1, "A"), row(2, "B")];
        assert!(!overlay.track_ghosts());
        // a newer event pushes the oldest one out of the two places
        overlay.rows = vec![row(3, "C"), row(1, "A")];
        assert!(overlay.track_ghosts());
        assert_eq!(overlay.ghosts.len(), 1);
        assert_eq!(overlay.ghosts[0].row.system, "B");
        // it is drawn under the listed rows while there is room
        let tall = egui::Rect::from_min_size(egui::Pos2::ZERO, overlay.style.size());
        assert_eq!(overlay.ghost_rows(tall), 1);
        assert_eq!(
            overlay.panel_rect(tall).height(),
            overlay.style.header_height + 2.0 + 3.0 * overlay.style.row_height
        );
        // and is forgotten once it has faded
        overlay.ghosts[0].since = Instant::now() - overlay.style.fade_out;
        assert!(!overlay.track_ghosts());
        assert!(overlay.ghosts.is_empty());
    }

    #[test]
    fn rows_already_there_do_not_fade_in_but_new_ones_do() {
        let mut overlay = OverlayState::new();
        overlay.rows = vec![row(1, "A")];
        assert!(!overlay.track_new_rows());
        assert_eq!(overlay.enter_progress(&row(1, "A")), 1.0);
        overlay.rows.push(row(2, "B"));
        assert!(overlay.track_new_rows());
        assert!(overlay.enter_progress(&row(2, "B")) < 1.0);
    }

    #[test]
    fn rows_are_sorted_by_age_or_by_jumps() {
        let aged = |jumps: u8, system: &str, age_secs: u64| NearbyRow {
            age_secs,
            ..row(jumps, system)
        };
        let mut rows = vec![aged(3, "C", 5), aged(1, "A", 40), aged(1, "B", 10)];
        sort_rows(&mut rows, OverlayOrder::Age);
        let names: Vec<&str> = rows.iter().map(|row| row.system.as_str()).collect();
        assert_eq!(names, ["C", "B", "A"]);
        sort_rows(&mut rows, OverlayOrder::Jumps);
        let names: Vec<&str> = rows.iter().map(|row| row.system.as_str()).collect();
        assert_eq!(names, ["B", "A", "C"]);
    }

    #[test]
    fn the_overlay_depends_on_the_client_and_not_on_the_focus() {
        let game = |focused| {
            Some(GameWindow {
                rect: ScreenRect::new(0, 0, 100, 100),
                focused,
            })
        };
        // the client is there: shown, in front or not
        assert!(OverlayState::should_show(game(true), true));
        assert!(OverlayState::should_show(game(false), true));
        assert!(OverlayState::should_show(game(false), false));
        // the client is gone: only the overlay being positioned stays
        assert!(!OverlayState::should_show(None, true));
        assert!(OverlayState::should_show(None, false));
    }

    #[test]
    fn a_character_overlay_follows_the_client_of_its_character() {
        let mut overlay = OverlayState::for_window(7, 0, None);
        overlay.follow_character("Alice Pilot");
        assert!(overlay.tracker.matches_title("EVE - Alice Pilot"));
        assert!(!overlay.tracker.matches_title("EVE - Bob Pilot"));
        assert!(!overlay.tracker.matches_title("EVE"));
        // asking again for the same character keeps the poll it has
        overlay.last_poll = Some(Instant::now());
        overlay.follow_character("Alice Pilot");
        assert!(overlay.last_poll.is_some());
        // the overlay of all the characters follows any client
        assert!(OverlayState::new().tracker.matches_title("EVE - Bob Pilot"));
    }

    #[test]
    fn ages_are_minutes_and_seconds() {
        assert_eq!(OverlayState::age_text(5), "0:05");
        assert_eq!(OverlayState::age_text(125), "2:05");
    }

    #[test]
    fn the_opacity_scales_the_panel_alpha() {
        assert_eq!(OverlayState::with_opacity((12, 16, 23), 1.0).a(), 255);
        assert_eq!(OverlayState::with_opacity((12, 16, 23), 0.0).a(), 0);
        assert_eq!(OverlayState::with_opacity((12, 16, 23), 2.0).a(), 255);
        let half = OverlayState::with_opacity((12, 16, 23), 0.5).a();
        assert!((126..=129).contains(&half), "{half}");
    }

    #[test]
    fn points_and_pixels_round_trip() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let pixels = OverlayState::to_pixels(260.0, ppp);
            let points = OverlayState::to_points(pixels, ppp);
            assert!((points - 260.0).abs() < 1.0 / ppp, "{ppp}: {points}");
        }
    }

    #[test]
    fn the_first_placement_is_calibrated_to_the_game_window() {
        let mut overlay = OverlayState::new();
        let game = ScreenRect::new(0, 0, 1920, 1080);
        overlay.place(&game, 1.0);
        assert!(overlay.placed);
        assert_eq!(overlay.placement.reference_height(), Some(1080));
        assert_eq!(
            overlay.placement.rect_in(&game),
            ScreenRect::new(
                1920 - 260 - 40,
                80,
                260,
                OverlayStyle::default().size().y as u32
            )
        );
    }
}
