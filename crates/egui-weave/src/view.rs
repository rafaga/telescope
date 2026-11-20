//! Where the canvas is looked at from: pan and zoom, and the conversion
//! between the coordinates of the screen and the ones of the canvas.
//!
//! Nothing here draws. A point of the canvas is on the screen at
//! `canvas * zoom + offset`, with the zoom the same in both axes.

use egui::{Pos2, Rect, Vec2};

/// The pan and the zoom of a canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasView {
    zoom: f32,
    offset: Vec2,
    min_zoom: f32,
    max_zoom: f32,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self::new()
    }
}

impl CanvasView {
    /// A view at 100 %, with the origin of the canvas at the origin of the
    /// screen and a zoom between 25 % and 300 %.
    #[must_use]
    pub fn new() -> Self {
        Self {
            zoom: 1.0,
            offset: Vec2::ZERO,
            min_zoom: 0.25,
            max_zoom: 3.0,
        }
    }

    /// The least and the most the view zooms, as factors (1.0 is 100 %). The
    /// zoom is brought into the range at once.
    #[must_use]
    pub fn zoom_range(mut self, min: f32, max: f32) -> Self {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        self.min_zoom = min.max(f32::EPSILON);
        self.max_zoom = max.max(self.min_zoom);
        self.zoom = self.zoom.clamp(self.min_zoom, self.max_zoom);
        self
    }

    /// The least the view zooms, as a factor.
    #[must_use]
    pub fn min_zoom(&self) -> f32 {
        self.min_zoom
    }

    /// The most the view zooms, as a factor.
    #[must_use]
    pub fn max_zoom(&self) -> f32 {
        self.max_zoom
    }

    /// Takes the pan and the zoom from the part of the canvas (`scene`) that
    /// a surface shows in `viewport`, as `egui::Scene` keeps them. Ignored
    /// when either is empty or not finite.
    pub fn set_from_scene_rect(&mut self, scene: Rect, viewport: Rect) {
        let zoom = viewport.width() / scene.width();
        if !zoom.is_finite() || zoom <= 0.0 || !scene.min.x.is_finite() || !scene.min.y.is_finite()
        {
            return;
        }
        self.zoom = zoom.clamp(self.min_zoom, self.max_zoom);
        self.offset = viewport.min.to_vec2() - scene.min.to_vec2() * self.zoom;
    }

    /// The zoom as a factor (1.0 is 100 %).
    #[must_use]
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The zoom as a percentage, rounded.
    #[must_use]
    pub fn zoom_percent(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }

    /// Where the origin of the canvas is on the screen.
    #[must_use]
    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    /// The point of the screen that shows the point `pos` of the canvas.
    #[must_use]
    pub fn canvas_to_screen(&self, pos: Pos2) -> Pos2 {
        Pos2::new(
            pos.x * self.zoom + self.offset.x,
            pos.y * self.zoom + self.offset.y,
        )
    }

    /// The point of the canvas that the point `pos` of the screen shows.
    #[must_use]
    pub fn screen_to_canvas(&self, pos: Pos2) -> Pos2 {
        Pos2::new(
            (pos.x - self.offset.x) / self.zoom,
            (pos.y - self.offset.y) / self.zoom,
        )
    }

    /// The part of the canvas that shows on the screen in `viewport`.
    #[must_use]
    pub fn visible_rect(&self, viewport: Rect) -> Rect {
        Rect::from_min_max(
            self.screen_to_canvas(viewport.min),
            self.screen_to_canvas(viewport.max),
        )
    }

    /// Moves the canvas on the screen by `delta` points.
    pub fn pan_by(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    /// Multiplies the zoom by `factor`, keeping the point of the canvas that
    /// is under `anchor` (a point of the screen) where it is. The zoom stays
    /// in its range. Returns whether the zoom changed.
    pub fn zoom_by(&mut self, factor: f32, anchor: Pos2) -> bool {
        if !factor.is_finite() || factor <= 0.0 {
            return false;
        }
        self.zoom_to(self.zoom * factor, anchor)
    }

    /// Sets the zoom to `zoom` (1.0 is 100 %), keeping the point of the
    /// canvas that is under `anchor` where it is. Returns whether the zoom
    /// changed.
    pub fn zoom_to(&mut self, zoom: f32, anchor: Pos2) -> bool {
        if !zoom.is_finite() {
            return false;
        }
        let zoom = zoom.clamp(self.min_zoom, self.max_zoom);
        if (zoom - self.zoom).abs() <= f32::EPSILON {
            return false;
        }
        let ratio = zoom / self.zoom;
        let anchor = anchor.to_vec2();
        self.offset = anchor - (anchor - self.offset) * ratio;
        self.zoom = zoom;
        true
    }

    /// Sets the zoom to a percentage, around `anchor` (see
    /// [`zoom_to`](Self::zoom_to)).
    pub fn set_zoom_percent(&mut self, percent: u32, anchor: Pos2) -> bool {
        self.zoom_to(percent as f32 / 100.0, anchor)
    }

    /// Zooms in by a step of a quarter, around `anchor`.
    pub fn zoom_in(&mut self, anchor: Pos2) -> bool {
        self.zoom_by(1.25, anchor)
    }

    /// Zooms out by a step of a quarter, around `anchor`.
    pub fn zoom_out(&mut self, anchor: Pos2) -> bool {
        self.zoom_by(0.8, anchor)
    }

    /// Shows all of `content` (a rectangle of the canvas) inside `viewport`
    /// (a rectangle of the screen), centered and with `margin` points free
    /// on each side. It never zooms in past 100 %, so a small graph is not
    /// blown up. An empty `content` only centers the origin of the canvas.
    pub fn fit(&mut self, content: Rect, viewport: Rect, margin: f32) {
        let free = Vec2::new(
            (viewport.width() - 2.0 * margin).max(1.0),
            (viewport.height() - 2.0 * margin).max(1.0),
        );
        let zoom = if content.width() > 0.0 && content.height() > 0.0 {
            (free.x / content.width())
                .min(free.y / content.height())
                .min(1.0)
        } else {
            1.0
        };
        self.zoom = zoom.clamp(self.min_zoom, self.max_zoom);
        self.offset = viewport.center().to_vec2() - content.center().to_vec2() * self.zoom;
    }

    /// Moves the canvas so that `pos` (a point of the canvas) is at the center
    /// of `viewport`, without changing the zoom.
    pub fn center_on(&mut self, pos: Pos2, viewport: Rect) {
        self.offset = viewport.center().to_vec2() - pos.to_vec2() * self.zoom;
    }

    /// Keeps the view within `content` plus a margin of `margin` times the
    /// size of what is in view on each side, so that the user can scroll a
    /// bit past what is drawn but not lose it. When the view is larger than
    /// that area on an axis, the area is centered on that axis.
    pub fn limit_to(&mut self, content: Rect, viewport: Rect, margin: f32) {
        let visible = self.visible_rect(viewport);
        let bounds = content.expand2(visible.size() * margin.max(0.0));
        let shift = Vec2::new(
            Self::shift_into(visible.min.x, visible.max.x, bounds.min.x, bounds.max.x),
            Self::shift_into(visible.min.y, visible.max.y, bounds.min.y, bounds.max.y),
        );
        self.offset -= shift * self.zoom;
    }

    /// How far to move the span `min..max` to have it inside `lo..hi`; when
    /// it does not fit, to have it centered there.
    fn shift_into(min: f32, max: f32, lo: f32, hi: f32) -> f32 {
        if max - min >= hi - lo {
            (lo + hi) / 2.0 - (min + max) / 2.0
        } else if min < lo {
            lo - min
        } else if max > hi {
            hi - max
        } else {
            0.0
        }
    }

    /// Goes back to 100 % with the origin of the canvas at the origin of the
    /// screen.
    pub fn reset(&mut self) {
        self.zoom = 1.0_f32.clamp(self.min_zoom, self.max_zoom);
        self.offset = Vec2::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the tests share.
    struct Fixture;

    impl Fixture {
        fn close(a: f32, b: f32) -> bool {
            (a - b).abs() < 1e-3
        }

        fn same(a: Pos2, b: Pos2) -> bool {
            Self::close(a.x, b.x) && Self::close(a.y, b.y)
        }

        fn viewport() -> Rect {
            Rect::from_min_max(Pos2::new(100.0, 50.0), Pos2::new(900.0, 650.0))
        }
    }

    #[test]
    fn a_new_view_is_at_100_percent_over_the_origin() {
        let view = CanvasView::new();
        assert_eq!(view.zoom_percent(), 100);
        assert_eq!(view.offset(), Vec2::ZERO);
        let pos = Pos2::new(12.0, -7.0);
        assert_eq!(view.canvas_to_screen(pos), pos);
    }

    #[test]
    fn screen_and_canvas_coordinates_go_back_and_forth() {
        let mut view = CanvasView::new();
        view.pan_by(Vec2::new(30.0, -20.0));
        view.zoom_to(2.0, Pos2::new(400.0, 300.0));
        let canvas = Pos2::new(123.0, 45.0);
        let back = view.screen_to_canvas(view.canvas_to_screen(canvas));
        assert!(Fixture::same(back, canvas));
    }

    #[test]
    fn panning_moves_the_canvas_by_that_much() {
        let mut view = CanvasView::new();
        view.pan_by(Vec2::new(10.0, 5.0));
        assert_eq!(
            view.canvas_to_screen(Pos2::new(1.0, 1.0)),
            Pos2::new(11.0, 6.0)
        );
    }

    #[test]
    fn zooming_keeps_the_point_under_the_anchor() {
        let mut view = CanvasView::new();
        view.pan_by(Vec2::new(40.0, 10.0));
        let anchor = Pos2::new(300.0, 200.0);
        let under = view.screen_to_canvas(anchor);
        assert!(view.zoom_in(anchor));
        assert!(Fixture::same(view.screen_to_canvas(anchor), under));
        assert!(view.zoom_out(anchor));
        assert!(view.zoom_out(anchor));
        assert!(Fixture::same(view.screen_to_canvas(anchor), under));
    }

    #[test]
    fn the_zoom_stays_in_its_range_and_tells_when_it_did_not_move() {
        let mut view = CanvasView::new().zoom_range(0.5, 2.0);
        let anchor = Pos2::ZERO;
        assert!(view.zoom_to(10.0, anchor));
        assert!(Fixture::close(view.zoom(), 2.0));
        assert!(!view.zoom_in(anchor));
        assert!(view.zoom_to(0.01, anchor));
        assert!(Fixture::close(view.zoom(), 0.5));
        assert!(!view.zoom_out(anchor));
    }

    #[test]
    fn a_zoom_that_is_not_a_number_is_ignored() {
        let mut view = CanvasView::new();
        assert!(!view.zoom_by(f32::NAN, Pos2::ZERO));
        assert!(!view.zoom_by(0.0, Pos2::ZERO));
        assert!(!view.zoom_by(-1.0, Pos2::ZERO));
        assert!(!view.zoom_to(f32::INFINITY, Pos2::ZERO));
        assert_eq!(view, CanvasView::new());
    }

    #[test]
    fn the_zoom_range_brings_the_zoom_into_it() {
        let view = CanvasView::new().zoom_range(2.0, 4.0);
        assert!(Fixture::close(view.zoom(), 2.0));
        // the ends in the wrong order are put right
        let view = CanvasView::new().zoom_range(3.0, 0.5);
        assert!(Fixture::close(view.zoom(), 1.0));
    }

    #[test]
    fn the_percent_is_set_and_rounded() {
        let mut view = CanvasView::new();
        assert!(view.set_zoom_percent(150, Pos2::ZERO));
        assert_eq!(view.zoom_percent(), 150);
        assert!(!view.set_zoom_percent(150, Pos2::ZERO));
        view.set_zoom_percent(33, Pos2::ZERO);
        assert_eq!(view.zoom_percent(), 33);
    }

    #[test]
    fn fit_shows_all_the_content_centered_with_a_margin() {
        let mut view = CanvasView::new();
        let viewport = Fixture::viewport();
        // wider than the viewport: it has to zoom out
        let content = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1400.0, 400.0));
        view.fit(content, viewport, 50.0);
        assert!(Fixture::close(view.zoom(), 0.5));
        let shown = view.canvas_to_screen(content.center());
        assert!(Fixture::same(shown, viewport.center()));
        let left = view.canvas_to_screen(content.min).x;
        let right = view.canvas_to_screen(content.max).x;
        assert!(left >= viewport.min.x + 50.0 - 1e-3);
        assert!(right <= viewport.max.x - 50.0 + 1e-3);
    }

    #[test]
    fn fit_does_not_blow_up_a_small_graph() {
        let mut view = CanvasView::new();
        let viewport = Fixture::viewport();
        let content = Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(110.0, 60.0));
        view.fit(content, viewport, 20.0);
        assert!(Fixture::close(view.zoom(), 1.0));
        assert!(Fixture::same(
            view.canvas_to_screen(content.center()),
            viewport.center()
        ));
    }

    #[test]
    fn fit_of_nothing_centers_the_origin() {
        let mut view = CanvasView::new();
        let viewport = Fixture::viewport();
        view.fit(Rect::from_min_max(Pos2::ZERO, Pos2::ZERO), viewport, 20.0);
        assert!(Fixture::close(view.zoom(), 1.0));
        assert!(Fixture::same(
            view.canvas_to_screen(Pos2::ZERO),
            viewport.center()
        ));
    }

    #[test]
    fn fit_in_a_viewport_smaller_than_its_margin_does_not_break() {
        let mut view = CanvasView::new();
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0));
        let content = Rect::from_min_max(Pos2::ZERO, Pos2::new(500.0, 500.0));
        view.fit(content, viewport, 50.0);
        assert!(view.zoom().is_finite());
        assert!(view.zoom() >= 0.25);
    }

    #[test]
    fn the_visible_rect_follows_pan_and_zoom() {
        let mut view = CanvasView::new();
        let viewport = Fixture::viewport();
        let visible = view.visible_rect(viewport);
        assert_eq!(visible, viewport);
        view.zoom_to(2.0, Pos2::ZERO);
        let visible = view.visible_rect(viewport);
        assert!(Fixture::close(visible.width(), viewport.width() / 2.0));
        assert!(Fixture::close(visible.height(), viewport.height() / 2.0));
    }

    #[test]
    fn the_view_is_taken_back_from_the_scene_it_was_given() {
        let mut view = CanvasView::new();
        view.pan_by(Vec2::new(30.0, -10.0));
        view.zoom_to(1.5, Pos2::new(200.0, 100.0));
        let viewport = Fixture::viewport();
        let scene = view.visible_rect(viewport);
        let mut other = CanvasView::new();
        other.set_from_scene_rect(scene, viewport);
        assert!(Fixture::close(other.zoom(), view.zoom()));
        assert!(Fixture::close(other.offset().x, view.offset().x));
        assert!(Fixture::close(other.offset().y, view.offset().y));
    }

    #[test]
    fn an_empty_or_broken_scene_is_ignored() {
        let mut view = CanvasView::new();
        let viewport = Fixture::viewport();
        view.set_from_scene_rect(Rect::from_min_max(Pos2::ZERO, Pos2::ZERO), viewport);
        view.set_from_scene_rect(
            Rect::from_min_max(Pos2::new(f32::NAN, 0.0), Pos2::new(10.0, 10.0)),
            viewport,
        );
        assert_eq!(view, CanvasView::new());
    }

    #[test]
    fn center_on_puts_a_point_in_the_middle_of_the_viewport() {
        let mut view = CanvasView::new();
        view.zoom_to(2.0, Pos2::ZERO);
        let viewport = Fixture::viewport();
        view.center_on(Pos2::new(123.0, -45.0), viewport);
        assert!(Fixture::same(
            view.canvas_to_screen(Pos2::new(123.0, -45.0)),
            viewport.center()
        ));
        assert!(Fixture::close(view.zoom(), 2.0));
    }

    #[test]
    fn reset_goes_back_to_100_percent_over_the_origin() {
        let mut view = CanvasView::new();
        view.pan_by(Vec2::new(500.0, 500.0));
        view.zoom_to(2.5, Pos2::new(10.0, 10.0));
        view.reset();
        assert_eq!(view.zoom_percent(), 100);
        assert_eq!(view.offset(), Vec2::ZERO);
    }

    #[test]
    fn the_view_can_go_past_the_content_by_the_margin_but_not_further() {
        let content = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 1000.0));
        let viewport = Fixture::viewport();
        let mut view = CanvasView::new();
        view.center_on(Pos2::new(50000.0, 500.0), viewport);
        view.limit_to(content, viewport, 0.5);
        let visible = view.visible_rect(viewport);
        assert!(Fixture::close(visible.max.x, 1000.0 + 800.0 * 0.5));
    }

    #[test]
    fn a_view_inside_the_limits_is_left_alone() {
        let content = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 1000.0));
        let viewport = Fixture::viewport();
        let mut view = CanvasView::new();
        view.center_on(Pos2::new(500.0, 500.0), viewport);
        let before = view;
        view.limit_to(content, viewport, 0.5);
        assert_eq!(view, before);
    }

    #[test]
    fn a_view_larger_than_the_limits_is_centered_on_the_content() {
        let content = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let viewport = Fixture::viewport();
        let mut view = CanvasView::new();
        view.center_on(Pos2::new(900.0, -400.0), viewport);
        view.limit_to(content, viewport, 0.0);
        let center = view.visible_rect(viewport).center();
        assert!(Fixture::same(center, Pos2::new(50.0, 50.0)));
    }
}
