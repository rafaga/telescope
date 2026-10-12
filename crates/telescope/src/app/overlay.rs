//! Experimental overlay (feature `overlay`): a transparent, borderless window
//! that stays above the game and follows the EVE client window.
//!
//! For now it only shows a placeholder: a rectangle with a title bar to drag it
//! and a struck-through body, to try the native behaviour on Windows, macOS and
//! Linux (X11). Nothing is injected into the game; see `native_tools::overlay`.

use eframe::egui;
use native_tools::overlay::{
    Anchor, FullScreenState, GameWindow, GameWindowTracker, OverlayError, OverlayPlacement, OverlayWindow,
    ScreenRect,
};
use std::time::{Duration, Instant};

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

    pub(crate) fn is_locked(&self) -> bool {
        self.locked
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
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
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
        let bar = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), Self::TITLE_BAR_HEIGHT),
        );
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
        let mut shown = Self::indicator_rects(area);
        if !self.locked {
            shown.push(bar);
        }
        self.apply_region(rect, &shown, ppp);
        if self.locked {
            self.paint_area(ui, area);
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
        let close_response =
            ui.interact(close, egui::Id::new("overlay_close"), egui::Sense::click());
        let lock_response =
            ui.interact(lock, egui::Id::new("overlay_lock"), egui::Sense::click());
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
        self.paint_area(ui, area);
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
        match self.native.set_visible_region_by_title(Self::TITLE, &region) {
            Ok(()) | Err(OverlayError::Unsupported) => self.region = Some(region),
            // the window is not ready yet: the next frame tries again
            Err(_) => {}
        }
    }

    /// A closed padlock drawn with shapes (no icon font needed), centered on
    /// `center`.
    fn paint_padlock(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
        let body = egui::Rect::from_center_size(
            center + egui::vec2(0.0, 2.5),
            egui::vec2(11.0, 8.0),
        );
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

    /// The rectangle outline of the placeholder, and the stroke it is drawn
    /// with.
    fn indicator_outline(area: egui::Rect) -> egui::Rect {
        area.shrink(14.0)
    }

    const STROKE: f32 = 2.0;

    /// The window parts the placeholder covers: only these are shown, so the
    /// area between the strokes is as transparent as the rest of the window.
    /// The diagonals are cut in small steps along their length.
    fn indicator_rects(area: egui::Rect) -> Vec<egui::Rect> {
        let inner = Self::indicator_outline(area);
        let half = Self::STROKE;
        let mut rects = vec![
            egui::Rect::from_min_max(
                inner.left_top() - egui::vec2(half, half),
                egui::pos2(inner.right() + half, inner.top() + half),
            ),
            egui::Rect::from_min_max(
                egui::pos2(inner.left() - half, inner.bottom() - half),
                inner.right_bottom() + egui::vec2(half, half),
            ),
            egui::Rect::from_min_max(
                inner.left_top() - egui::vec2(half, half),
                egui::pos2(inner.left() + half, inner.bottom() + half),
            ),
            egui::Rect::from_min_max(
                egui::pos2(inner.right() - half, inner.top() - half),
                inner.right_bottom() + egui::vec2(half, half),
            ),
        ];
        let step = 2.0;
        let steps = (inner.width() / step).ceil() as usize;
        let slope = inner.height() / inner.width();
        for index in 0..steps {
            let x0 = inner.left() + index as f32 * step;
            let x1 = (x0 + step).min(inner.right());
            let down = |x: f32| inner.top() + (x - inner.left()) * slope;
            let up = |x: f32| inner.bottom() - (x - inner.left()) * slope;
            for (y0, y1) in [(down(x0), down(x1)), (up(x0), up(x1))] {
                rects.push(egui::Rect::from_min_max(
                    egui::pos2(x0, y0.min(y1) - half),
                    egui::pos2(x1, y0.max(y1) + half),
                ));
            }
        }
        rects
    }

    /// The placeholder: a crossed-out area, painted opaque on no background.
    fn paint_area(&self, ui: &egui::Ui, area: egui::Rect) {
        let painter = ui.painter();
        let inner = Self::indicator_outline(area);
        let stroke = egui::Stroke::new(Self::STROKE, egui::Color32::from_rgb(200, 90, 90));
        painter.line_segment([inner.left_top(), inner.right_bottom()], stroke);
        painter.line_segment([inner.right_top(), inner.left_bottom()], stroke);
        painter.rect_stroke(inner, 0.0, stroke, egui::StrokeKind::Inside);
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
