//! The controls of a form row are named after the row's label, the way an
//! HTML `<label>` names its input: a screen reader (and a test) finds them by
//! that text.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_panels::{Slider, form, label_with_row, switch};

#[test]
fn the_controls_of_a_form_row_are_named_after_its_label() {
    let mut harness = Harness::new_ui_state(
        |ui, (on, value, choice): &mut (bool, f32, usize)| {
            form(ui, |form| {
                form.row("Play a sound", |ui| {
                    switch(ui, on);
                });
                form.row("Volume", |ui| {
                    Slider::new(value, 0.0..=100.0).show(ui);
                });
                form.row("Sound", |ui| {
                    let combo = egui::ComboBox::from_id_salt("sound")
                        .selected_text(choice.to_string())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(choice, 0, "0");
                            ui.selectable_value(choice, 1, "1");
                        });
                    label_with_row(ui, &combo.response);
                });
            });
        },
        (false, 50.0, 0),
    );
    // the harness fails if any control has no accessible name
    harness.run();

    harness.get_by_label("Play a sound").click();
    harness.run();
    assert!(harness.state().0, "the switch was found by its row's label");
    assert!(harness.query_by_label("Volume").is_some());
    assert!(harness.query_by_label("Sound").is_some());
}

#[test]
fn outside_a_row_nothing_is_named() {
    let mut harness = Harness::builder()
        .with_accessibility_check(false)
        .build_ui_state(
            |ui, on: &mut bool| {
                ui.label("Play a sound");
                switch(ui, on);
            },
            false,
        );
    harness.run();
    // the label is a plain label: no control is named after it
    let switches = harness
        .query_all_by_label("Play a sound")
        .filter(|node| node.accesskit_node().role() == egui::accesskit::Role::CheckBox)
        .count();
    assert_eq!(switches, 0);
}
