//! Experimental overlay: a transparent, borderless window
//! that stays above the game and follows the EVE client window.
//!
//! It shows the events near the linked characters (see
//! `intel::nearby`): a small semi-transparent panel with one row per event, the
//! closest and newest first. With nothing to show it draws nothing when locked,
//! and an example while it is being positioned. Nothing is
//! injected into the game; see `native_tools::overlay`.

use eframe::egui;
use native_tools::overlay::{
    Anchor, FullScreenState, GameWindow, GameWindowTracker, OverlayError, OverlayPlacement,
    OverlayWindow, ScreenRect,
};
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

    /// The events near the linked characters, as the widget lists them: newest
    /// first, with the system's name, and only those still within the alert
    /// duration.
    pub(crate) fn nearby_rows(&self) -> Vec<NearbyRow> {
        let now = std::time::Instant::now();
        self.alarm_shared
            .nearby
            .recent(now, self.settings.get_alert_duration())
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
            .collect()
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
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayState {
    const TITLE: &'static str = "Telescope Overlay";
    /// Size of the placeholder, in points.
    const SIZE: egui::Vec2 = egui::vec2(260.0, 160.0);
    const TITLE_BAR_HEIGHT: f32 = 24.0;
    const LOCK_BUTTON_WIDTH: f32 = 40.0;
    const BAR_COLOR: egui::Color32 = egui::Color32::from_rgb(48, 54, 74);
    /// How often the game window is looked up.
    const POLL: Duration = Duration::from_millis(250);
    /// Tries to apply the native settings (the window does not exist on the
    /// first frames).
    const NATIVE_ATTEMPTS: u32 = 30;
    /// Pixels the window may differ from its target before it counts as moved
    /// by the user.
    const MOVE_TOLERANCE: i32 = 1;
    const EXCLUSIVE_WARNING: &'static str = "EVE is in exclusive full screen, so the overlay cannot be shown over it. Switch EVE to windowed or borderless full screen mode.";

    pub(crate) fn new() -> Self {
        // the window must take mouse input, otherwise it cannot be dragged
        let mut native = OverlayWindow::new();
        native.set_click_through(false);
        Self {
            open: false,
            locked: false,
            region: None,
            tracker: GameWindowTracker::default(),
            native,
            placement: OverlayPlacement::new(Anchor::TopRight, 40, 80, 260, 160),
            placed: false,
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
        }
    }

    fn viewport_id() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("telescope_overlay")
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
        }
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
        self.region = None;
        self.native_applied = false;
        self.native_attempts = 0;
        self.last_game_rect = None;
        self.last_target = None;
        self.overlay_focused = false;
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

    /// Draws the overlay window, when it is open. Called every frame by the
    /// app; it also keeps a slow repaint going to follow the game window.
    pub(crate) fn show(&mut self, ctx: &egui::Context, rows: &[NearbyRow], opacity: f32) {
        if !self.open {
            return;
        }
        if self.rows != rows {
            self.rows = rows.to_vec();
        }
        self.opacity = opacity.clamp(0.0, 1.0);
        ctx.request_repaint_after(Self::POLL);
        self.poll_game(ctx);

        let mut builder = egui::ViewportBuilder::default()
            .with_title(Self::TITLE)
            .with_inner_size(Self::SIZE)
            .with_decorations(false)
            .with_transparent(true)
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
        ctx.show_viewport_immediate(Self::viewport_id(), builder, |ui, _class| {
            self.draw(ui);
        });
    }

    /// Looks for the game window (a few times a second) and moves the overlay
    /// when the game moved or was resized.
    fn poll_game(&mut self, ctx: &egui::Context) {
        if self
            .last_poll
            .is_some_and(|last| last.elapsed() < Self::POLL)
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
        if self.last_game_rect == Some(game.rect) {
            return;
        }
        self.last_game_rect = Some(game.rect);
        let target = self.placement.rect_in(&game.rect);
        self.last_target = Some(target);
        let id = Self::viewport_id();
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
    fn update_visibility(&mut self, ctx: &egui::Context) {
        let own_focus =
            ctx.input(|input| input.viewport().focused.unwrap_or(false)) || self.overlay_focused;
        let visible = self.game.is_none_or(|game| game.focused || own_focus);
        if visible != self.visible {
            self.visible = visible;
            ctx.send_viewport_cmd_to(Self::viewport_id(), egui::ViewportCommand::Visible(visible));
        }
    }

    /// First placement for a game window: the top-right corner, scaled to
    /// this screen from now on (`calibrate`).
    fn place(&mut self, game: &ScreenRect, pixels_per_point: f32) {
        self.placement = OverlayPlacement::new(
            Anchor::TopRight,
            Self::to_pixels(40.0, pixels_per_point),
            Self::to_pixels(80.0, pixels_per_point),
            Self::to_pixels(Self::SIZE.x, pixels_per_point).max(1) as u32,
            Self::to_pixels(Self::SIZE.y, pixels_per_point).max(1) as u32,
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
        if (x - last.x).abs() <= Self::MOVE_TOLERANCE && (y - last.y).abs() <= Self::MOVE_TOLERANCE
        {
            return;
        }
        self.placement.move_to(&game.rect, x, y);
        self.last_target = Some(ScreenRect::new(x, y, last.width, last.height));
    }

    /// Applies the native settings once the window exists.
    fn apply_native(&mut self) {
        if self.native_applied || self.native_attempts >= Self::NATIVE_ATTEMPTS {
            return;
        }
        self.native_attempts += 1;
        match self.native.apply_by_title(Self::TITLE) {
            Ok(()) => self.native_applied = true,
            Err(OverlayError::Unsupported) => {
                tracing::warn!("the overlay's native settings are not supported here");
                self.native_applied = true;
            }
            Err(error) if self.native_attempts >= Self::NATIVE_ATTEMPTS => {
                tracing::warn!(%error, "could not apply the overlay's native settings");
            }
            Err(_) => {}
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
        let ppp = Self::pixels_per_point(&ctx);
        if let Some(outer) = ctx.input(|input| input.viewport().outer_rect) {
            self.track_drag(outer, ppp);
        }

        let rect = ui.max_rect();
        let bar =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), Self::TITLE_BAR_HEIGHT));
        let close = egui::Rect::from_min_size(
            egui::pos2(bar.right() - Self::TITLE_BAR_HEIGHT, bar.top()),
            egui::vec2(Self::TITLE_BAR_HEIGHT, Self::TITLE_BAR_HEIGHT),
        );
        let lock = egui::Rect::from_min_max(
            egui::pos2(close.left() - Self::LOCK_BUTTON_WIDTH, bar.top()),
            egui::pos2(close.left(), bar.bottom()),
        );
        // the part that stays in the same place in both modes: where the
        // indicators go
        let area = egui::Rect::from_min_max(egui::pos2(rect.left(), bar.bottom()), rect.max);

        // Only what is painted is shown: the window has no background and no
        // border. The rest is cut away natively, so this needs no
        // transparent swapchain.
        let mut shown = self.shown_parts(area);
        if !self.locked {
            shown.push(bar);
        }
        self.apply_region(rect, &shown, ppp);
        if self.locked {
            self.paint_content(ui, area);
            return;
        }

        // positioning mode: pressing anywhere but on the buttons moves the
        // window; the native drag has to start right at the press
        let pressed = ui.input(|input| {
            input.pointer.primary_pressed()
                && input.pointer.interact_pos().is_some_and(|pos| {
                    rect.contains(pos) && !close.contains(pos) && !lock.contains(pos)
                })
        });
        if pressed {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let close_response = ui.interact(
            close,
            egui::Id::unique("overlay_close"),
            egui::Sense::click(),
        );
        let lock_response =
            ui.interact(lock, egui::Id::unique("overlay_lock"), egui::Sense::click());
        if close_response.clicked() {
            self.close();
            return;
        }
        if lock_response.clicked() {
            self.set_locked(true);
            return;
        }

        let painter = ui.painter();
        // an opaque title bar
        painter.rect_filled(
            bar,
            egui::CornerRadius {
                nw: 6,
                ne: 6,
                sw: 0,
                se: 0,
            },
            Self::BAR_COLOR,
        );
        painter.text(
            egui::pos2(bar.left() + 8.0, bar.center().y),
            egui::Align2::LEFT_CENTER,
            Self::TITLE,
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(225),
        );
        let hover_color = |response: &egui::Response, hover: egui::Color32| {
            if response.hovered() {
                hover
            } else {
                egui::Color32::from_gray(200)
            }
        };
        Self::paint_padlock(
            painter,
            lock.center(),
            hover_color(&lock_response, egui::Color32::from_rgb(140, 220, 140)),
        );
        painter.text(
            close.center(),
            egui::Align2::CENTER_CENTER,
            "x",
            egui::FontId::proportional(13.0),
            hover_color(&close_response, egui::Color32::from_rgb(255, 120, 120)),
        );
        self.paint_content(ui, area);
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
            .set_visible_region_by_title(Self::TITLE, &region)
        {
            Ok(()) | Err(OverlayError::Unsupported) => self.region = Some(region),
            // the window is not ready yet: the next frame tries again
            Err(_) => {}
        }
    }

    /// A closed padlock drawn with shapes (no icon font needed), centered on
    /// `center`.
    fn paint_padlock(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
        let body =
            egui::Rect::from_center_size(center + egui::vec2(0.0, 2.5), egui::vec2(11.0, 8.0));
        painter.rect_filled(body, 1.5, color);
        // the shackle: a half circle over the body
        let radius = 3.5;
        let base = egui::pos2(center.x, body.top());
        let shackle: Vec<egui::Pos2> = (0..=12)
            .map(|step| {
                let angle = std::f32::consts::PI * step as f32 / 12.0;
                egui::pos2(base.x - radius * angle.cos(), base.y - radius * angle.sin())
            })
            .collect();
        painter.add(egui::Shape::line(shackle, egui::Stroke::new(1.8, color)));
        painter.circle_filled(body.center(), 1.2, Self::BAR_COLOR);
    }

    const ROW_HEIGHT: f32 = 22.0;
    const HEADER_HEIGHT: f32 = 22.0;
    const PANEL_COLOR: (u8, u8, u8) = (12, 16, 23);
    const HEADER_COLOR: (u8, u8, u8) = (24, 32, 46);
    const PANEL_BORDER: (u8, u8, u8) = (70, 90, 120);

    /// Whether the panel shows made-up rows: while positioning with no real
    /// event, so the user sees what the overlay looks like.
    fn is_example(&self) -> bool {
        self.rows.is_empty() && !self.locked
    }

    /// The rows the panel lists: the events, or the example while positioning
    /// without any.
    fn panel_rows(&self) -> std::borrow::Cow<'_, [NearbyRow]> {
        if self.is_example() {
            std::borrow::Cow::Owned(Self::example_rows())
        } else {
            std::borrow::Cow::Borrowed(&self.rows)
        }
    }

    /// Three made-up events at different distances.
    fn example_rows() -> Vec<NearbyRow> {
        vec![
            NearbyRow {
                jumps: 1,
                system: String::from("Tama"),
                text: t!("overlay.example_hostiles").to_string(),
                age_secs: 12,
            },
            NearbyRow {
                jumps: 2,
                system: String::from("Kedama"),
                text: t!("overlay.example_kill").to_string(),
                age_secs: 48,
            },
            NearbyRow {
                jumps: 3,
                system: String::from("Amamake"),
                text: t!("overlay.example_gate").to_string(),
                age_secs: 125,
            },
        ]
    }

    /// The rows that fit in `area`, below the header.
    fn visible_rows(&self, area: egui::Rect) -> usize {
        let room = ((area.height() - Self::HEADER_HEIGHT - 2.0) / Self::ROW_HEIGHT).floor();
        self.panel_rows().len().min(room.max(0.0) as usize)
    }

    /// Where the panel is: the top of `area`, as tall as its rows need.
    fn panel_rect(&self, area: egui::Rect) -> egui::Rect {
        let height = Self::HEADER_HEIGHT + 2.0 + self.visible_rows(area) as f32 * Self::ROW_HEIGHT;
        egui::Rect::from_min_size(area.min, egui::vec2(area.width(), height))
    }

    /// The window parts that are painted, besides the title bar: only these
    /// are shown, the rest of the window is cut away.
    fn shown_parts(&self, area: egui::Rect) -> Vec<egui::Rect> {
        if self.rows.is_empty() && self.locked {
            // nothing to show: a single pixel stands for "no region"
            return vec![egui::Rect::from_min_size(area.min, egui::vec2(1.0, 1.0))];
        }
        vec![self.panel_rect(area)]
    }

    /// The colors of a row by distance: its stripe, its text, and whether its
    /// background is tinted (the closest ones). The distance is always written
    /// too, so the color never carries it alone.
    fn jump_colors(jumps: u8) -> (egui::Color32, egui::Color32, bool) {
        match jumps {
            0 | 1 => (
                egui::Color32::from_rgb(210, 56, 44),
                egui::Color32::from_rgb(255, 106, 92),
                true,
            ),
            2 => (
                egui::Color32::from_rgb(224, 129, 58),
                egui::Color32::from_rgb(240, 160, 90),
                false,
            ),
            _ => (
                egui::Color32::from_rgb(201, 178, 74),
                egui::Color32::from_rgb(220, 198, 92),
                false,
            ),
        }
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

    /// One line of text, cut with an ellipsis to `width`.
    fn text_galley(
        painter: &egui::Painter,
        text: &str,
        size: f32,
        color: egui::Color32,
        width: f32,
    ) -> std::sync::Arc<egui::Galley> {
        let mut job = egui::text::LayoutJob::simple(
            text.to_string(),
            egui::FontId::proportional(size),
            color,
            width.max(1.0),
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('\u{2026}');
        painter.layout_job(job)
    }

    /// The content of the area: the events panel, with an example while
    /// positioning with no events (nothing at all when locked and empty).
    fn paint_content(&self, ui: &egui::Ui, area: egui::Rect) {
        if self.rows.is_empty() && self.locked {
            return;
        }
        let rows = self.panel_rows();
        let painter = ui.painter();
        let panel = self.panel_rect(area);
        let border = Self::with_opacity(Self::PANEL_BORDER, (self.opacity + 0.2).min(1.0));
        painter.rect_filled(
            panel,
            0.0,
            Self::with_opacity(Self::PANEL_COLOR, self.opacity),
        );
        let header =
            egui::Rect::from_min_size(panel.min, egui::vec2(panel.width(), Self::HEADER_HEIGHT));
        painter.rect_filled(
            header,
            0.0,
            Self::with_opacity(Self::HEADER_COLOR, self.opacity),
        );
        let title = if self.is_example() {
            format!("{} · {}", t!("overlay.nearby"), t!("overlay.example"))
        } else {
            t!("overlay.nearby").to_string()
        }
        .to_uppercase();
        painter.text(
            egui::pos2(header.left() + 8.0, header.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(10.5),
            egui::Color32::from_rgb(170, 180, 196),
        );
        if !self.is_example() {
            painter.text(
                egui::pos2(header.right() - 8.0, header.center().y),
                egui::Align2::RIGHT_CENTER,
                rows.len().to_string(),
                egui::FontId::proportional(10.5),
                egui::Color32::from_rgb(224, 129, 58),
            );
        }

        let mut top = header.bottom();
        for row in rows.iter().take(self.visible_rows(area)) {
            let rect = egui::Rect::from_min_size(
                egui::pos2(panel.left(), top),
                egui::vec2(panel.width(), Self::ROW_HEIGHT),
            );
            let (stripe, color, tinted) = Self::jump_colors(row.jumps);
            if tinted {
                painter.rect_filled(
                    rect,
                    0.0,
                    Self::with_opacity((70, 20, 24), self.opacity * 0.7),
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
                egui::FontId::proportional(11.0),
                color,
            );
            let system = Self::text_galley(
                painter,
                &row.system,
                11.5,
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
                egui::FontId::proportional(10.5),
                egui::Color32::from_rgb(125, 137, 155),
            );
            let text_left = rect.left() + 102.0;
            let summary = Self::text_galley(
                painter,
                &row.text,
                11.0,
                egui::Color32::from_rgb(181, 189, 201),
                rect.right() - age_width - 6.0 - text_left,
            );
            painter.galley(
                egui::pos2(text_left, mid - summary.size().y / 2.0),
                summary,
                egui::Color32::WHITE,
            );
            top += Self::ROW_HEIGHT;
        }
        painter.rect_stroke(
            panel,
            0.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_starts_closed_and_toggles() {
        let mut overlay = OverlayState::new();
        assert!(!overlay.is_open());
        overlay.toggle();
        assert!(overlay.is_open());
        overlay.toggle();
        assert!(!overlay.is_open());
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
    fn nothing_is_painted_when_locked_without_events() {
        let mut overlay = OverlayState::new();
        overlay.locked = true;
        let parts = overlay.shown_parts(area());
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].size(), egui::vec2(1.0, 1.0));

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
        assert_eq!(overlay.panel_rows().len(), 3);
        assert_eq!(overlay.visible_rows(area()), 3);

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
    fn rows_are_colored_by_distance_and_the_closest_are_tinted() {
        assert!(OverlayState::jump_colors(0).2);
        assert!(OverlayState::jump_colors(1).2);
        assert!(!OverlayState::jump_colors(2).2);
        assert_ne!(
            OverlayState::jump_colors(1).0,
            OverlayState::jump_colors(2).0
        );
        assert_ne!(
            OverlayState::jump_colors(2).0,
            OverlayState::jump_colors(3).0
        );
        assert_eq!(
            OverlayState::jump_colors(3).0,
            OverlayState::jump_colors(9).0
        );
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
            ScreenRect::new(1920 - 260 - 40, 80, 260, 160)
        );
    }
}
