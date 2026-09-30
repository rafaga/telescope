//! Layout, navigation and widget behaviour beyond the basic clicks of
//! `interaction.rs`, on a headless egui context.

use egui::{
    Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Visuals, pos2, vec2,
};
use egui_panels::{
    Action, ActionBar, Draft, NavGroup, NavItem, Palette, PathPicker, SettingsLayout, SideNav,
    StatusKind, Theme, Variant, badge, badges, button, chip, dialog_frame, divider, form, notice,
    page, page_header, page_header_with, progress_steps, progress_steps_with_notes, segmented,
    status, stepper, switch, tile_grid,
};
use std::cell::Cell;

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
        output.textures_delta.clear();
    }

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

/// Draws once with `draw` and returns whatever it measured.
fn measure<R>(mut draw: impl FnMut(&mut Ui) -> R) -> R {
    let harness = Harness::new();
    let mut result = None;
    harness.frame(Vec::new(), &mut |ui| result = Some(draw(ui)));
    result.expect("the frame ran")
}

// ---- SideNav ----------------------------------------------------------

fn two_groups() -> [NavGroup<u8>; 2] {
    [
        NavGroup::new(
            "First",
            vec![NavItem::new(0, "One"), NavItem::new(1, "Two")],
        ),
        NavGroup::untitled(vec![NavItem::new(2, "Three")]),
    ]
}

#[test]
fn nav_builders_fill_the_fields() {
    let item = NavItem::new(7, "Sound").icon("\u{266a}").dirty(true);
    assert_eq!(item.id, 7);
    assert_eq!(item.label, "Sound");
    assert_eq!(item.icon.as_deref(), Some("\u{266a}"));
    assert!(item.dirty);

    let plain = NavItem::new(1, "Plain");
    assert!(plain.icon.is_none());
    assert!(!plain.dirty);

    assert_eq!(
        NavGroup::new("Pages", vec![plain.clone()]).title.as_deref(),
        Some("Pages")
    );
    assert!(NavGroup::untitled(vec![plain]).title.is_none());
}

#[test]
fn any_dirty_looks_through_every_group() {
    let mut groups = two_groups();
    assert!(!SideNav::new(&groups).any_dirty());
    groups[1].items[0].dirty = true;
    assert!(SideNav::new(&groups).any_dirty());
    assert!(!SideNav::<u8>::new(&[]).any_dirty());
}

#[test]
fn clicking_the_selected_entry_changes_nothing() {
    let harness = Harness::new();
    let groups = two_groups();
    let mut selected = 0u8;
    let mut changed = false;
    let rect = Cell::new(Rect::NOTHING);
    harness.click(
        // The captioned group starts with the selected entry, right under
        // its caption: the top of the column is the first entry area.
        || pos2(rect.get().center().x, rect.get().top() + 40.0),
        &mut |ui| {
            let response = SideNav::new(&groups).show(ui, &mut selected);
            rect.set(response.rect);
            changed |= response.changed();
        },
    );
    assert_eq!(selected, 0);
    assert!(!changed);
}

#[test]
fn clicking_an_entry_of_another_group_selects_it_and_marks_the_response_changed() {
    let harness = Harness::new();
    let groups = two_groups();
    let mut selected = 0u8;
    let mut changed = false;
    let rect = Cell::new(Rect::NOTHING);
    harness.click(
        // "Three" is the last entry, at the bottom of the column.
        || pos2(rect.get().center().x, rect.get().bottom() - 6.0),
        &mut |ui| {
            let response = SideNav::new(&groups).show(ui, &mut selected);
            rect.set(response.rect);
            changed |= response.changed();
        },
    );
    assert_eq!(selected, 2);
    assert!(changed);
}

#[test]
fn the_title_and_subtitle_make_the_nav_taller() {
    let groups = two_groups();
    let plain = measure(|ui| SideNav::new(&groups).show(ui, &mut 0).rect.height());
    let titled = measure(|ui| {
        SideNav::new(&groups)
            .title("Settings")
            .show(ui, &mut 0)
            .rect
            .height()
    });
    let subtitled = measure(|ui| {
        SideNav::new(&groups)
            .title("Settings")
            .subtitle("v1.0")
            .show(ui, &mut 0)
            .rect
            .height()
    });
    assert!(titled > plain);
    assert!(subtitled > titled);
}

#[test]
fn a_captioned_group_takes_more_room_than_an_untitled_one() {
    let items = || vec![NavItem::new(0, "One"), NavItem::new(1, "Two")];
    let captioned = [NavGroup::new("Pages", items())];
    let untitled = [NavGroup::untitled(items())];
    let with = measure(|ui| SideNav::new(&captioned).show(ui, &mut 0).rect.height());
    let without = measure(|ui| SideNav::new(&untitled).show(ui, &mut 0).rect.height());
    assert!(with > without);
}

#[test]
fn a_long_label_does_not_widen_its_entry() {
    let groups = [NavGroup::untitled(vec![
        NavItem::new(0, "x".repeat(200))
            .icon("\u{2605}")
            .dirty(true),
    ])];
    let width = measure(|ui| {
        ui.allocate_ui(vec2(160.0, 100.0), |ui| {
            SideNav::new(&groups).show(ui, &mut 0).rect.width()
        })
        .inner
    });
    assert!(width <= 160.5, "{width}");
}

// ---- ActionBar --------------------------------------------------------

/// The action a click at `x` (relative to the bar's rect) triggers.
fn bar_click(x_from_right: f32) -> Option<Action> {
    let harness = Harness::new();
    let rect = Cell::new(Rect::NOTHING);
    let mut action = None;
    harness.click(
        || pos2(rect.get().right() - x_from_right, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| ActionBar::new("Cancel", "Apply", "Accept").show(ui));
            rect.set(inner.response.rect);
            action = action.or(inner.inner);
        },
    );
    action
}

#[test]
fn the_action_bar_buttons_sit_right_to_left_as_accept_apply_cancel() {
    // Scan from the right end: the first hits are Accept, then Apply, then
    // Cancel, each in its own stretch.
    let hits: Vec<Option<Action>> = (0..40).map(|i| bar_click(4.0 + i as f32 * 8.0)).collect();
    let first = |wanted| hits.iter().position(|hit| *hit == Some(wanted));
    let accept = first(Action::Accept).expect("Accept is clickable");
    let apply = first(Action::Apply).expect("Apply is clickable");
    let cancel = first(Action::Cancel).expect("Cancel is clickable");
    assert!(
        accept < apply && apply < cancel,
        "{accept} {apply} {cancel}"
    );
}

#[test]
fn a_click_on_the_pending_message_triggers_no_action() {
    let harness = Harness::new();
    let rect = Cell::new(Rect::NOTHING);
    let mut action = None;
    harness.click(
        || pos2(rect.get().left() + 6.0, rect.get().center().y),
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
    assert_eq!(action, None);
}

#[test]
fn nothing_clicked_means_no_action() {
    let action = measure(|ui| ActionBar::new("Cancel", "Apply", "Accept").show(ui));
    assert_eq!(action, None);
}

#[test]
fn every_message_variant_of_the_bar_draws_at_the_same_height() {
    let height = |bar: fn() -> ActionBar<'static>| {
        measure(|ui| {
            let before = ui.min_rect().height();
            let _ = bar().show(ui);
            ui.min_rect().height() - before
        })
    };
    let plain = height(|| ActionBar::new("Cancel", "Apply", "Accept"));
    let saved = height(|| ActionBar::new("Cancel", "Apply", "Accept").saved_message("All saved"));
    let pending = height(|| ActionBar::new("Cancel", "Apply", "Accept").pending(Some("Unsaved")));
    assert!((plain - saved).abs() < 0.5 && (plain - pending).abs() < 0.5);
}

// ---- Buttons and controls --------------------------------------------

#[test]
fn every_button_variant_reports_its_click() {
    for variant in [
        Variant::Primary,
        Variant::Secondary,
        Variant::Ghost,
        Variant::Danger,
    ] {
        let harness = Harness::new();
        let rect = Cell::new(Rect::NOTHING);
        let mut clicked = false;
        harness.click(|| rect.get().center(), &mut |ui| {
            let response = button(ui, "Go", variant);
            rect.set(response.rect);
            clicked |= response.clicked();
        });
        assert!(clicked, "{variant:?}");
    }
}

#[test]
fn a_button_is_as_tall_as_a_control() {
    let height = measure(|ui| button(ui, "Go", Variant::Primary).rect.height());
    assert!(
        (height - Theme::default().control_height).abs() < 0.5,
        "{height}"
    );
}

#[test]
fn a_switch_flips_back_on_a_second_click() {
    let harness = Harness::new();
    let mut on = false;
    let rect = Cell::new(Rect::NOTHING);
    let mut draw = |ui: &mut Ui| rect.set(switch(ui, &mut on).rect);
    harness.click(|| rect.get().center(), &mut draw);
    harness.click(|| rect.get().center(), &mut draw);
    assert!(!on);
}

#[test]
fn a_disabled_switch_ignores_clicks() {
    let harness = Harness::new();
    let mut on = false;
    let rect = Cell::new(Rect::NOTHING);
    harness.click(|| rect.get().center(), &mut |ui| {
        ui.add_enabled_ui(false, |ui| rect.set(switch(ui, &mut on).rect));
    });
    assert!(!on);
}

#[test]
fn segmented_keeps_the_value_when_the_current_option_is_clicked() {
    let harness = Harness::new();
    let mut value = 1u8;
    let mut changed = false;
    let rect = Cell::new(Rect::NOTHING);
    harness.click(
        || pos2(rect.get().left() + 4.0, rect.get().center().y),
        &mut |ui| {
            let response = segmented(ui, &mut value, &[(1, "One"), (2, "Two")]);
            rect.set(response.rect);
            changed |= response.changed();
        },
    );
    assert_eq!(value, 1);
    assert!(!changed);
}

#[test]
fn segmented_picks_the_first_option_from_the_left_edge() {
    let harness = Harness::new();
    let mut value = 3u8;
    let rect = Cell::new(Rect::NOTHING);
    harness.click(
        || pos2(rect.get().left() + 4.0, rect.get().center().y),
        &mut |ui| rect.set(segmented(ui, &mut value, &[(1, "One"), (2, "Two"), (3, "Three")]).rect),
    );
    assert_eq!(value, 1);
}

#[test]
fn segmented_shrinks_to_the_room_it_has() {
    let options = [(1, "A rather long label"), (2, "Another long label")];
    let wide = measure(|ui| segmented(ui, &mut 1, &options).rect.width());
    let narrow = measure(|ui| {
        ui.allocate_ui(vec2(120.0, 60.0), |ui| {
            segmented(ui, &mut 1, &options).rect.width()
        })
        .inner
    });
    assert!(narrow < wide);
    assert!(narrow <= 120.5, "{narrow}");
}

#[test]
fn the_stepper_reports_nothing_for_the_current_step() {
    let harness = Harness::new();
    let rect = Cell::new(Rect::NOTHING);
    let mut clicked = None;
    harness.click(
        // The last step is the current one.
        || pos2(rect.get().right() - 4.0, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| stepper(ui, &["Sources", "Rules", "Alerts"], 2));
            rect.set(inner.response.rect);
            clicked = clicked.or(inner.inner);
        },
    );
    assert_eq!(clicked, None);
}

#[test]
fn the_path_picker_reset_button_is_the_rightmost_and_only_reports_reset() {
    let harness = Harness::new();
    let mut path = String::from("sde.db");
    let (mut reset, mut browse) = (false, false);
    let rect = Cell::new(Rect::NOTHING);
    harness.click(
        || pos2(rect.get().right() - 10.0, rect.get().center().y),
        &mut |ui| {
            let inner = ui.scope(|ui| {
                PathPicker::new(&mut path, "Browse")
                    .reset("Reset")
                    .hint("path")
                    .show(ui)
            });
            rect.set(inner.response.rect);
            reset |= inner.inner.reset;
            browse |= inner.inner.browse;
        },
    );
    assert!(reset);
    assert!(!browse);
    assert_eq!(path, "sde.db");
}

#[test]
fn a_path_picker_fills_the_width_and_is_one_control_tall() {
    let (width, height, available) = measure(|ui| {
        let available = ui.available_width();
        let rect = PathPicker::new(&mut String::new(), "Browse")
            .show(ui)
            .response
            .rect;
        (rect.width(), rect.height(), available)
    });
    assert!(width < available);
    assert!(height >= Theme::default().control_height - 0.5);
}

// ---- Grids, steps, badges, status ------------------------------------

#[test]
fn the_tile_grid_calls_each_item_once_and_wraps_by_columns() {
    let tops = measure(|ui| {
        let mut tops = vec![None; 7];
        tile_grid(ui, 3, 7, |ui, index| {
            assert!(tops[index].is_none(), "item {index} drawn twice");
            tops[index] = Some(ui.label(format!("item {index}")).rect.top());
        });
        tops
    });
    let tops: Vec<f32> = tops.into_iter().map(|top| top.expect("drawn")).collect();
    assert_eq!(tops[0], tops[1]);
    assert_eq!(tops[1], tops[2]);
    assert!(tops[3] > tops[2]);
    assert_eq!(tops[3], tops[5]);
    assert!(tops[6] > tops[5]);
}

#[test]
fn the_tile_grid_drops_columns_when_there_is_no_room() {
    let tops = measure(|ui| {
        ui.allocate_ui(vec2(200.0, 400.0), |ui| {
            let mut tops = Vec::new();
            tile_grid(ui, 3, 3, |ui, index| {
                tops.push(ui.label(format!("item {index}")).rect.top());
            });
            tops
        })
        .inner
    });
    assert!(
        tops[0] < tops[1] && tops[1] < tops[2],
        "one column: {tops:?}"
    );
}

#[test]
fn an_empty_tile_grid_draws_nothing() {
    let mut called = false;
    measure(|ui| tile_grid(ui, 3, 0, |_, _| called = true));
    assert!(!called);
}

#[test]
fn progress_steps_take_a_line_per_step() {
    let height =
        |steps: &[&str], current| measure(|ui| progress_steps(ui, steps, current).rect.height());
    let two = height(&["a", "b"], 0);
    let four = height(&["a", "b", "c", "d"], 1);
    assert!(four > two * 1.5, "{two} {four}");
    // The current step past the end means "all done".
    assert!(height(&["a", "b"], 5) > 0.0);
    assert_eq!(height(&[], 0), 0.0);
}

#[test]
fn progress_notes_do_not_add_lines() {
    let steps = ["Download", "Parse"];
    let plain = measure(|ui| progress_steps(ui, &steps, 1).rect.height());
    let notes = vec![String::from("12 MB"), String::new()];
    let with_notes = measure(|ui| {
        progress_steps_with_notes(ui, &steps, &notes, 1)
            .rect
            .height()
    });
    assert!((plain - with_notes).abs() < 0.5, "{plain} {with_notes}");
    // Fewer notes than steps is fine.
    measure(|ui| progress_steps_with_notes(ui, &steps, &notes[..1], 0));
}

#[test]
fn badges_wrap_onto_more_lines_in_a_narrow_column() {
    let labels: Vec<String> = (0..12).map(|n| format!("channel {n}")).collect();
    let height = |width: f32| {
        measure(|ui| {
            ui.allocate_ui(vec2(width, 400.0), |ui| {
                let before = ui.min_rect().height();
                badges(ui, &labels);
                ui.min_rect().height() - before
            })
            .inner
        })
    };
    assert!(height(200.0) > height(900.0) + 10.0);
}

#[test]
fn no_badges_take_no_room() {
    let height = measure(|ui| {
        let before = ui.min_rect().height();
        badges(ui, &[]);
        ui.min_rect().height() - before
    });
    assert!(height < 1.0, "{height}");
}

#[test]
fn a_secondary_text_widens_a_chip() {
    let plain = measure(|ui| chip(ui, "Delve", None).rect.width());
    let with_secondary = measure(|ui| chip(ui, "Delve", Some("2 pilots")).rect.width());
    let badge_width = measure(|ui| badge(ui, "Delve").rect.width());
    assert!(with_secondary > plain);
    assert_eq!(plain, badge_width);
}

#[test]
fn status_lines_and_notices_draw_for_every_kind() {
    for kind in [
        StatusKind::Ok,
        StatusKind::Warning,
        StatusKind::Error,
        StatusKind::Info,
    ] {
        let (line, block) = measure(|ui| {
            (
                status(ui, kind, "Everything went fine").rect,
                notice(ui, kind, "Everything went fine").rect,
            )
        });
        assert!(line.width() > 0.0 && line.height() > 0.0, "{kind:?}");
        assert!(block.height() >= line.height(), "{kind:?}");
    }
}

#[test]
fn a_longer_status_text_is_wider() {
    let short = measure(|ui| status(ui, StatusKind::Ok, "ok").rect.width());
    let long = measure(|ui| {
        status(ui, StatusKind::Ok, "everything is fine here")
            .rect
            .width()
    });
    assert!(long > short);
}

// ---- Layout -----------------------------------------------------------

#[test]
fn a_page_stops_growing_at_the_theme_maximum() {
    let harness = Harness::new();
    Theme::set(
        &harness.ctx,
        Theme {
            page_max_width: 400.0,
            ..Theme::default()
        },
    );
    let mut width = 0.0;
    harness.frame(Vec::new(), &mut |ui| {
        page(ui, |ui| width = ui.available_width());
    });
    assert!((width - 400.0).abs() < 0.5, "{width}");
}

#[test]
fn a_page_header_with_a_description_is_taller() {
    let bare = measure(|ui| page_header(ui, "Alerts", None).rect.height());
    let described = measure(|ui| page_header(ui, "Alerts", Some("What fires")).rect.height());
    assert!(described > bare);
}

#[test]
fn the_header_actions_sit_at_the_right_end_and_return_their_value() {
    let (value, action_rect, header) = measure(|ui| {
        let header = page(ui, |ui| {
            page_header_with(ui, "Rules", None, |ui| {
                let rect = ui.label("Add rule").rect;
                (7, rect)
            })
        });
        let (value, rect) = header.inner;
        (value, rect, header.response.rect)
    });
    assert_eq!(value, 7);
    assert!(
        action_rect.right() > header.right() - 20.0,
        "{action_rect:?} {header:?}"
    );
    assert!(action_rect.left() > header.center().x);
}

#[test]
fn a_section_without_title_or_description_has_no_heading() {
    type Make = fn() -> egui_panels::Section<'static>;
    let height =
        |make: Make| measure(|ui| make().show(ui, |ui| ui.label("x")).response.rect.height());
    let bare = height(|| egui_panels::Section::new(""));
    let titled = height(|| egui_panels::Section::new("Sound"));
    let described = height(|| egui_panels::Section::new("Sound").description("What it does"));
    assert!(bare < titled);
    assert!(titled < described);
    // Highlighting only changes the color.
    let highlighted = height(|| egui_panels::Section::new("Sound").highlighted(true));
    assert_eq!(titled, highlighted);
}

#[test]
fn a_divider_is_a_hairline_across_the_width() {
    let (rect, available) = measure(|ui| {
        let available = ui.available_width();
        (divider(ui).rect, available)
    });
    assert_eq!(rect.height(), 1.0);
    assert!((rect.width() - available).abs() < 0.5);
}

#[test]
fn form_rows_put_the_label_beside_the_control_when_there_is_room() {
    let (label_left, control_left) = measure(|ui| {
        ui.allocate_ui(vec2(900.0, 200.0), |ui| {
            let start = ui.cursor().left();
            let left = form(ui, |form| {
                form.row("Name", |ui| ui.label("control").rect.left())
            });
            (start, left)
        })
        .inner
    });
    assert!(control_left >= label_left + Theme::default().label_width - 0.5);
}

#[test]
fn form_rows_put_the_label_above_the_control_when_narrow() {
    let (start, control_left, control_top, label_top) = measure(|ui| {
        ui.allocate_ui(vec2(320.0, 300.0), |ui| {
            let start = ui.cursor().left();
            let top = ui.cursor().top();
            let (left, control_top) = form(ui, |form| {
                form.row("Name", |ui| {
                    let rect = ui.label("control").rect;
                    (rect.left(), rect.top())
                })
            });
            (start, left, control_top, top)
        })
        .inner
    });
    assert!((control_left - start).abs() < 0.5, "{control_left} {start}");
    assert!(control_top > label_top + 8.0, "{control_top} {label_top}");
}

#[test]
fn a_form_note_lines_up_with_the_controls_and_hints_do_not_move_rows() {
    let (row_left, note_left, hinted_left) = measure(|ui| {
        form(ui, |form| {
            let row = form.row("Name", |ui| ui.label("control").rect.left());
            let note = form.note(|ui| ui.label("note").rect.left());
            let hinted = form.row_hint("Other", "explains it", |ui| ui.label("c").rect.left());
            (row, note, hinted)
        })
    });
    assert_eq!(row_left, note_left);
    assert_eq!(row_left, hinted_left);
}

#[test]
fn a_form_row_is_at_least_one_control_high() {
    let height = measure(|ui| {
        form(ui, |form| {
            let before = form.ui().min_rect().height();
            form.row("Name", |ui| ui.label("x"));
            form.ui().min_rect().height() - before
        })
    });
    assert!(height >= Theme::default().control_height - 0.5, "{height}");
}

#[test]
fn the_dialog_frame_follows_the_theme_and_the_visuals() {
    let harness = Harness::new();
    harness.ctx.set_visuals(Visuals::dark());
    let frame = dialog_frame(&harness.ctx);
    assert_eq!(frame.stroke.width, 1.0);
    assert_eq!(frame.corner_radius.nw, Theme::default().radius);
    assert_eq!(frame.fill, harness.ctx.global_style().visuals.panel_fill);

    Theme::set(
        &harness.ctx,
        Theme {
            radius: 9,
            ..Theme::default()
        },
    );
    assert_eq!(dialog_frame(&harness.ctx).corner_radius.nw, 9);
}

// ---- SettingsLayout ---------------------------------------------------

#[test]
fn the_settings_layout_draws_the_selected_page_once_per_frame() {
    for scroll in [true, false] {
        let groups = two_groups();
        let mut selected = 1u8;
        let mut pages = Vec::new();
        let mut action = None;
        let harness = Harness::new();
        harness.frame(Vec::new(), &mut |ui| {
            pages.clear();
            action = SettingsLayout::new("settings").scroll(scroll).show(
                ui,
                SideNav::new(&groups).title("Settings"),
                ActionBar::new("Cancel", "Apply", "Accept"),
                &mut selected,
                |_, page| pages.push(*page),
            );
        });
        assert_eq!(pages, [1], "scroll = {scroll}");
        assert_eq!(action, None);
    }
}

#[test]
fn the_settings_layout_returns_the_bar_action() {
    let groups = two_groups();
    let mut selected = 0u8;
    let mut action = None;
    let harness = Harness::new();
    let mut draw = |ui: &mut Ui| {
        action = action.or(SettingsLayout::new("settings").show(
            ui,
            SideNav::new(&groups),
            ActionBar::new("Cancel", "Apply", "Accept"),
            &mut selected,
            |ui, _| {
                ui.label("page");
            },
        ));
    };
    // Accept is the rightmost button of the bottom bar.
    harness.click(|| pos2(1000.0 - 40.0, 700.0 - 30.0), &mut draw);
    assert_eq!(action, Some(Action::Accept));
}

// ---- Draft and theme --------------------------------------------------

#[test]
fn editing_a_draft_does_not_touch_the_saved_value() {
    let mut draft = Draft::new(vec![1, 2, 3]);
    draft.current_mut().push(4);
    assert_eq!(draft.saved(), &[1, 2, 3]);
    assert!(draft.is_dirty());

    draft.commit();
    assert!(!draft.is_dirty());
    assert!(!draft.differs(|v| v.len()));

    draft.current_mut().clear();
    assert!(draft.differs(|v| v.len()));
    draft.revert();
    assert_eq!(draft.current(), &[1, 2, 3, 4]);
}

#[test]
fn reverting_to_an_equal_value_is_not_dirty() {
    let mut draft = Draft::new(5);
    *draft.current_mut() = 9;
    *draft.current_mut() = 5;
    assert!(!draft.is_dirty());
}

#[test]
fn a_default_draft_starts_clean() {
    let draft: Draft<String> = Draft::default();
    assert!(!draft.is_dirty());
    assert_eq!(draft.current(), "");
}

#[test]
fn the_palette_follows_light_and_dark_visuals() {
    let light = Palette::from_visuals(&Visuals::light());
    let dark = Palette::from_visuals(&Visuals::dark());
    assert_ne!(light, dark);
    assert_ne!(light.text, light.muted_text);
    assert_ne!(dark.accent, dark.on_accent);
}
