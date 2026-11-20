//! The components in a headless egui: what they report when they are used.

use egui::accesskit::Role;
use egui_ide::{
    PropertyDescriptor, PropertyGrid, PropertyKind, PropertyModel, PropertyValue, Toolbar,
    ToolbarIcon, ToolbarPanel, Toolbox, ToolboxEntry,
};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

#[test]
fn a_click_on_an_entry_gives_its_item_back() {
    let mut harness = Harness::new_ui_state(
        |ui, picked: &mut Option<u8>| {
            let entries = [
                ToolboxEntry::new(1_u8, "Circle"),
                ToolboxEntry::new(2, "Square"),
            ];
            if let Some(item) = Toolbox::new("tools").group("Shapes", entries).show(ui) {
                *picked = Some(item);
            }
        },
        None,
    );
    harness.run();
    harness.get_by_label("Square").click();
    harness.run();
    assert_eq!(*harness.state(), Some(2));
}

#[test]
fn a_disabled_entry_is_not_picked() {
    let mut harness = Harness::new_ui_state(
        |ui, picked: &mut Option<u8>| {
            let entries = [ToolboxEntry::new(1_u8, "Circle").enabled(false)];
            if let Some(item) = Toolbox::new("tools").group("Shapes", entries).show(ui) {
                *picked = Some(item);
            }
        },
        None,
    );
    harness.run();
    harness.get_by_label("Circle").click();
    harness.run();
    assert_eq!(*harness.state(), None);
}

#[test]
fn the_property_grid_reports_what_changed() {
    let mut harness = Harness::builder()
        .with_accessibility_check(false)
        .build_ui_state(
            |ui, (on, changed): &mut (bool, bool)| {
                *changed |= PropertyGrid::new("properties").show(ui, |grid| {
                    grid.group("General", |rows| {
                        rows.read_only("Id", "node_1");
                        rows.row("Enabled", |ui| ui.checkbox(on, "on").changed());
                    });
                });
            },
            (false, false),
        );
    harness.run();
    assert!(!harness.state().1);
    harness.get_by_label("on").click();
    harness.run();
    assert!(harness.state().0);
    assert!(harness.state().1);
}

#[test]
fn a_toggle_flips_its_flag() {
    let mut harness = Harness::new_ui_state(
        |ui, on: &mut bool| {
            Toolbar::show(ui, |bar| {
                bar.toggle(on, "Grid");
            });
        },
        false,
    );
    harness.run();
    harness.get_by_label("Grid").click();
    harness.run();
    assert!(*harness.state());
}

#[test]
fn the_grid_shows_a_model_and_names_its_properties() {
    use egui_ide::{PropertyDescriptor, PropertyKind, PropertyModel};

    let mut harness = Harness::builder()
        .with_accessibility_check(false)
        .build_ui_state(
            |ui, model: &mut PropertyModel| {
                PropertyGrid::new("model").show_model(ui, model);
            },
            {
                let mut model = PropertyModel::new();
                model.group("General", |group| {
                    group
                        .property(PropertyDescriptor::new("name", PropertyKind::Text).label("Name"))
                        .property(
                            PropertyDescriptor::new("on", PropertyKind::Bool).label("Enabled"),
                        );
                });
                model
            },
        );
    harness.run();
    harness.get_by_label("Name");
    // the label of the row and the dropdown of the flag share the name
    harness.get_by_role_and_label(Role::ComboBox, "Enabled");
}

#[test]
fn an_icon_button_is_found_by_its_tip_and_clicked() {
    let mut harness = Harness::new_ui_state(
        |ui, clicks: &mut u8| {
            Toolbar::panel(ui, |bar| {
                bar.group("File", |bar| {
                    if bar.icon_button(ToolbarIcon::Done, "Done").clicked() {
                        *clicks += 1;
                    }
                });
            });
        },
        0,
    );
    harness.run();
    harness.get_by_label("Done").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}

#[test]
fn an_icon_toggle_flips_its_flag_and_a_combo_picks_an_option() {
    let mut harness = Harness::new_ui_state(
        |ui, (on, pick): &mut (bool, usize)| {
            Toolbar::panel(ui, |bar| {
                bar.icon_toggle(on, ToolbarIcon::Toolbox, "Toolbox");
                bar.combo("zoom", pick, &["50%", "100%", "200%"], 64.0, "Zoom");
            });
        },
        (false, 1),
    );
    harness.run();
    harness.get_by_label("Toolbox").click();
    harness.run();
    assert!(harness.state().0);
    assert_eq!(harness.state().1, 1);
}

#[test]
fn a_panel_of_toolbars_lays_out_its_groups() {
    let mut harness = Harness::new_ui_state(
        |ui, clicks: &mut u8| {
            ToolbarPanel::new("bars")
                .group("file", "File", |bar| {
                    if bar.icon_button(ToolbarIcon::Done, "Done").clicked() {
                        *clicks += 1;
                    }
                })
                .group("view", "View", |bar| {
                    bar.icon_text_button(ToolbarIcon::FitView, "Fit", "Fit the view");
                    bar.separator();
                    bar.split_button(Some(ToolbarIcon::Play), "Test", |ui| {
                        ui.label("Test all");
                    });
                })
                .show(ui);
        },
        0,
    );
    harness.run();
    harness.get_by_label("Fit");
    harness.get_by_label("Test");
    harness.get_by_label("Done").click();
    harness.run();
    assert_eq!(*harness.state(), 1);
}

#[test]
fn the_order_of_a_panel_can_be_saved_and_put_back() {
    let mut harness = Harness::new_ui_state(
        |ui, _: &mut ()| {
            ToolbarPanel::new("saved")
                .group("a", "A", |bar| {
                    bar.icon_button(ToolbarIcon::Done, "First");
                })
                .group("b", "B", |bar| {
                    bar.icon_button(ToolbarIcon::Clear, "Second");
                })
                .show(ui);
        },
        (),
    );
    harness.run();
    let ctx = harness.ctx.clone();
    assert_eq!(ToolbarPanel::order(&ctx, "saved"), ["a", "b"]);
    ToolbarPanel::set_order(&ctx, "saved", vec!["b".to_owned(), "a".to_owned()]);
    harness.run();
    assert_eq!(ToolbarPanel::order(&ctx, "saved"), ["b", "a"]);
}

#[test]
fn an_action_property_reports_its_press() {
    let mut model = PropertyModel::new();
    model.group("Actions", |group| {
        group.property(
            PropertyDescriptor::new("run", PropertyKind::Action("Run".to_owned())).label("Test"),
        );
    });
    let mut harness = Harness::new_ui_state(
        |ui, model: &mut PropertyModel| {
            PropertyGrid::new("props").show_model(ui, model);
        },
        model,
    );
    harness.run();
    harness.get_by_label("Run").click();
    harness.run();
    let pressed = harness.state_mut().take_activated();
    assert_eq!(pressed.len(), 1);
    assert_eq!(pressed[0].as_str(), "run");
}

#[test]
fn a_flag_shows_true_or_false_in_a_dropdown() {
    let mut model = PropertyModel::new();
    model.group("General", |group| {
        group.property(
            PropertyDescriptor::new("on", PropertyKind::Bool)
                .label("Enabled")
                .default_to(PropertyValue::Bool(true)),
        );
    });
    let mut harness = Harness::new_ui_state(
        |ui, model: &mut PropertyModel| {
            PropertyGrid::new("props").show_model(ui, model);
        },
        model,
    );
    harness.run();
    let dropdown = harness.get_by_role_and_label(Role::ComboBox, "Enabled");
    assert_eq!(dropdown.accesskit_node().value(), Some("True".to_owned()));
}
