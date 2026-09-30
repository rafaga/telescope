//! The Cancel / Apply / Accept bar at the bottom of a settings screen.

use crate::theme::Theme;
use crate::widgets::{Variant, button};
use egui::{Align, Label, Layout, RichText, Sense, Ui, vec2};

/// What the user asked for in an [`ActionBar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Discard the changes (and usually close the screen).
    Cancel,
    /// Save the changes and stay.
    Apply,
    /// Save the changes and close the screen.
    Accept,
}

/// The bottom bar of a settings screen: what is pending at the left, Cancel,
/// Apply and Accept at the right.
#[must_use = "call `show` to draw the bar"]
pub struct ActionBar<'a> {
    cancel: &'a str,
    apply: &'a str,
    accept: &'a str,
    pending: Option<&'a str>,
    saved: Option<&'a str>,
}

impl<'a> ActionBar<'a> {
    /// A bar with these three button labels.
    pub fn new(cancel: &'a str, apply: &'a str, accept: &'a str) -> Self {
        Self {
            cancel,
            apply,
            accept,
            pending: None,
            saved: None,
        }
    }

    /// The message shown, after a dot, while there are changes not applied
    /// ("Unsaved changes in Alerts, Maps"). `None` means everything is saved.
    pub fn pending(mut self, message: Option<&'a str>) -> Self {
        self.pending = message;
        self
    }

    /// The dimmed message shown when nothing is pending ("All saved").
    pub fn saved_message(mut self, message: &'a str) -> Self {
        self.saved = Some(message);
        self
    }

    /// Draws the bar; returns the button clicked, if any.
    pub fn show(self, ui: &mut Ui) -> Option<Action> {
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let mut action = None;
        ui.horizontal(|ui| {
            ui.set_min_height(theme.control_height + 16.0);
            // The buttons take their room at the right end first; the message
            // gets what is left and is cut short when there is little.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, self.accept, Variant::Primary).clicked() {
                    action = Some(Action::Accept);
                }
                if button(ui, self.apply, Variant::Secondary).clicked() {
                    action = Some(Action::Apply);
                }
                if button(ui, self.cancel, Variant::Ghost).clicked() {
                    action = Some(Action::Cancel);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    match self.pending {
                        Some(message) => {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 4.0, palette.dirty);
                            ui.add(
                                Label::new(RichText::new(message).color(palette.dirty)).truncate(),
                            );
                        }
                        None => {
                            if let Some(message) = self.saved {
                                ui.add(
                                    Label::new(RichText::new(message).color(palette.muted_text))
                                        .truncate(),
                                );
                            }
                        }
                    }
                });
            });
        });
        action
    }
}
