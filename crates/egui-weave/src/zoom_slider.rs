//! A slider for the zoom of a [`CanvasView`], to put in a toolbar.

use egui::{Pos2, Response, Slider, Ui, Widget};

use crate::CanvasView;

/// A slider of the zoom, in percent and on a logarithmic scale, that keeps
/// the point of the screen at `anchor` where it is (the center of the canvas,
/// usually).
pub struct ZoomSlider<'a> {
    view: &'a mut CanvasView,
    anchor: Pos2,
    width: f32,
}

impl<'a> ZoomSlider<'a> {
    /// A slider of the zoom of `view`, around the point `anchor` of the screen.
    #[must_use]
    pub fn new(view: &'a mut CanvasView, anchor: Pos2) -> Self {
        Self {
            view,
            anchor,
            width: 100.0,
        }
    }

    /// The width of the track, in points (100 by default).
    #[must_use]
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl Widget for ZoomSlider<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let min = self.view.min_zoom() * 100.0;
        let max = self.view.max_zoom() * 100.0;
        let mut percent = self.view.zoom() * 100.0;
        let response = ui
            .scope(|ui| {
                ui.spacing_mut().slider_width = self.width;
                ui.add(
                    Slider::new(&mut percent, min..=max)
                        .logarithmic(true)
                        .suffix(" %")
                        .max_decimals(0),
                )
            })
            .inner;
        if response.changed() {
            self.view.zoom_to(percent / 100.0, self.anchor);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    /// What the tests share.
    struct Fixture;

    impl Fixture {
        fn harness() -> Harness<'static, CanvasView> {
            Harness::builder()
                .with_accessibility_check(false)
                .with_size(egui::vec2(300.0, 60.0))
                .build_ui_state(
                    |ui, view: &mut CanvasView| {
                        ui.add(ZoomSlider::new(view, Pos2::new(100.0, 100.0)));
                    },
                    CanvasView::new(),
                )
        }
    }

    #[test]
    fn the_slider_starts_at_the_zoom_of_the_view() {
        let mut harness = Fixture::harness();
        harness.run();
        assert_eq!(harness.state().zoom_percent(), 100);
    }
}
