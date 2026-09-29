//! Components driven by simulated pointer input on a headless egui context.

use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, pos2, vec2};
use egui_panels::{
    Action, ActionBar, EntityCard, NavGroup, NavItem, PathPicker, SideNav, Slider, Variant, button,
    form, segmented, stepper, switch, tile,
};

/// A headless context with a fixed 1000 x 700 screen.
struct Harness {
    ctx: Context,
}

impl Harness {
    fn new() -> Self {
        Self {
            ctx: Context::default(),
        }
    }

    fn frame(&self, events: Vec<Event>, draw: &mut dyn FnMut(&mut Ui)) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))),
            events,
            ..RawInput::default()
        };
        let mut output = self.ctx.run_ui(input, |ui| draw(ui));
        // Nothing renders here: drop the font atlas updates.
        output.textures_delta.clear();
    }

    /// Draws a frame to lay things out, then presses and releases the
    /// primary button at `at` (the click registers on release).
    fn click(&self, at: impl Fn() -> Pos2, draw: &mut dyn FnMut(&mut Ui)) {
        self.frame(Vec::new(), draw);
        let pos = at();
        self.frame(vec![Event::PointerMoved(pos)], draw);
        self.frame(vec![button_event(pos, true)], draw);
        self.frame(vec![button_event(pos, false)], draw);
    }
}

fn button_event(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    }
}

#[test]
fn a_switch_flips_when_clicked() {
    let harness = Harness::new();
    let mut on = false;
    let mut rect = Rect::NOTHING;
    let mut changed = false;
    let rect_cell = std::cell::Cell::new(Rect::NOTHING);
    harness.click(|| rect_cell.get().center(), &mut |ui| {
        let response = switch(ui, &mut on);
        rect = response.rect;
        rect_cell.set(response.rect);
        changed |= response.changed();
    });
    assert!(rect.width() > 0.0);
    assert!(on);
    assert!(changed);
}

#[test]
fn a_segmented_control_picks_the_clicked_option() {
    let harness = Harness::new();
    let mut value = 1u8;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        // The last segment ends at the right edge.
        || pos2(rect.get().right() - 4.0, rect.get().center().y),
        &mut |ui| {
            rect.set(segmented(ui, &mut value, &[(1, "One"), (2, "Two"), (3, "Three")]).rect);
        },
    );
    assert_eq!(value, 3);
}

#[test]
fn a_tile_flips_when_clicked() {
    let harness = Harness::new();
    let mut on = true;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(|| rect.get().center(), &mut |ui| {
        rect.set(tile(ui, &mut on, "Local", Some("last line 2 min ago")).rect)
    });
    assert!(!on);
}

#[test]
fn the_side_nav_selects_the_clicked_page() {
    let harness = Harness::new();
    let groups = [NavGroup::new(
        "Pages",
        vec![
            NavItem::new(0, "General"),
            NavItem::new(1, "Sound").dirty(true),
        ],
    )];
    assert!(SideNav::new(&groups).any_dirty());
    let mut selected = 0;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        // The second entry is the bottom one.
        || pos2(rect.get().center().x, rect.get().bottom() - 6.0),
        &mut |ui| rect.set(SideNav::new(&groups).show(ui, &mut selected).rect),
    );
    assert_eq!(selected, 1);
}

#[test]
fn the_action_bar_reports_accept() {
    let harness = Harness::new();
    let mut action = None;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        // Accept is the rightmost button.
        || pos2(rect.get().right() - 12.0, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| {
                ActionBar::new("Cancel", "Apply", "Accept")
                    .pending(Some("Unsaved changes in Sound"))
                    .show(ui)
            });
            rect.set(inner.response.rect);
            action = action.or(inner.inner);
        },
    );
    assert_eq!(action, Some(Action::Accept));
}

#[test]
fn a_path_picker_reports_browse() {
    let harness = Harness::new();
    let mut path = String::from("sde.db");
    let mut browse = false;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        // Browse is the rightmost button when there is no Reset.
        || pos2(rect.get().right() - 10.0, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| PathPicker::new(&mut path, "Browse…").show(ui));
            rect.set(inner.response.rect);
            browse |= inner.inner.browse;
        },
    );
    assert!(browse);
}

#[test]
fn the_stepper_reports_the_clicked_step() {
    let harness = Harness::new();
    let mut clicked = None;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        || pos2(rect.get().left() + 12.0, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| stepper(ui, &["Sources", "Rules", "Alerts"], 2));
            rect.set(inner.response.rect);
            clicked = clicked.or(inner.inner);
        },
    );
    assert_eq!(clicked, Some(0));
}

#[test]
fn form_controls_line_up_whatever_the_label() {
    let harness = Harness::new();
    let mut lefts = Vec::new();
    harness.frame(Vec::new(), &mut |ui| {
        lefts.clear();
        form(ui, |form| {
            for label in ["A", "A much longer label"] {
                let left = form.row(label, |ui| ui.label("control").rect.left());
                lefts.push(left);
            }
        });
    });
    assert_eq!(lefts.len(), 2);
    assert_eq!(lefts[0], lefts[1]);
}

#[test]
fn a_slider_jumps_to_the_click_and_snaps_to_its_step() {
    let harness = Harness::new();
    let mut seconds = 10u32;
    let rect = std::cell::Cell::new(Rect::NOTHING);
    harness.click(
        // The right end of the rail is the maximum.
        || pos2(rect.get().right() - 1.0, rect.get().center().y),
        &mut |ui| {
            rect.set(
                Slider::new(&mut seconds, 10..=600)
                    .step(10.0)
                    .width(300.0)
                    .show(ui)
                    .rect,
            );
        },
    );
    assert_eq!(seconds, 600);
    assert_eq!(seconds % 10, 0);
}

#[test]
fn an_entity_card_leaves_its_actions_their_clicks() {
    // Clicking the action button must reach the button, not the card.
    let harness = Harness::new();
    let button_rect = std::cell::Cell::new(Rect::NOTHING);
    let (mut action_clicked, mut card_clicked) = (false, false);
    harness.click(|| button_rect.get().center(), &mut |ui| {
        let card = EntityCard::new("Kara Voss").line("Corp").show(ui, |ui| {
            let response = button(ui, "Unlink", Variant::Danger);
            button_rect.set(response.rect);
            response.clicked()
        });
        action_clicked |= card.inner;
        card_clicked |= card.response.clicked();
    });
    assert!(action_clicked);
    assert!(!card_clicked);

    // Clicking elsewhere on the card clicks the card.
    let harness = Harness::new();
    let card_rect = std::cell::Cell::new(Rect::NOTHING);
    let mut card_clicked = false;
    harness.click(
        || pos2(card_rect.get().left() + 8.0, card_rect.get().center().y),
        &mut |ui| {
            let card = EntityCard::new("Kara Voss")
                .show(ui, |ui| button(ui, "Unlink", Variant::Danger).clicked());
            card_rect.set(card.response.rect);
            card_clicked |= card.response.clicked();
        },
    );
    assert!(card_clicked);
}
