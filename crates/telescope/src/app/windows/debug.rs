//! The debug window, one tab per tool: search the SDE by name and act on a
//! result (center the maps on it, optionally emitting a notification), run
//! chat-log lines through the whole intel pipeline, detection and dispatch
//! (typed or from a few presets),
//! test alerts and log notifications, preview node animations, move a
//! character's marker, and a read-only view of the app's internal state.
//!
//! The window is drawn with `egui-panels`, like the Settings: a side
//! navigation that groups the tabs by what they are for (find, simulate,
//! inspect) and, on each page, a header and sections with their controls in
//! aligned rows. Each control lives in the tab it belongs to.

use crate::app::TelescopeApp;
use crate::app::intel::input::InputEvent;
use crate::app::messages::CharacterSync;
use crate::app::messages::MapSync;
use crate::app::messages::Message;
use crate::app::messages::NodeEffect;
use crate::app::messages::Target;
use crate::app::messages::Type;
use eframe::egui;
use eframe::egui::RichText;
use egui_extras::Column;
use egui_extras::TableBuilder;
use egui_tiles::{Behavior, TileId, Tree, UiResponse};
use sde::SdeManager;
use std::sync::Arc;
use webb::esi::{SCHEMA_VERSION, SchemaStatus};
use webb::map_alerts::{AlertSummary, IntelAlert};
use zkill::guard::Endpoint;

/// The system named in the presets when none is picked in the search.
const DEFAULT_PRESET_SYSTEM: &str = "Jita";

/// Chat lines for the Intel tab, one per kind of message the rules know:
/// `(button label, hover text, line with `{system}` for the system name)`.
const INTEL_PRESETS: [(&str, &str, &str); 4] = [
    ("Clear report", "A system reported clear", "{system} clr"),
    (
        "Hostiles",
        "A pilot count and ship names in a system",
        "{system} +3 Rifter Punisher",
    ),
    (
        "Query",
        "A question about a system's status",
        "{system} status?",
    ),
    (
        "Chatter",
        "A line with no system, no ship and no keyword",
        "o7 fly safe everyone",
    ),
];

/// A tab of the debug window.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum DebugTab {
    #[default]
    Search,
    Intel,
    Alerts,
    Animations,
    Characters,
    ZKillboard,
    State,
}

impl DebugTab {
    /// The tabs of the window, in order.
    fn all() -> Vec<DebugTab> {
        vec![
            DebugTab::Search,
            DebugTab::Intel,
            DebugTab::Alerts,
            DebugTab::Animations,
            DebugTab::Characters,
            DebugTab::ZKillboard,
            DebugTab::State,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            DebugTab::Search => "Search",
            DebugTab::Intel => "Intel",
            DebugTab::Alerts => "Alerts & log",
            DebugTab::Animations => "Animations",
            DebugTab::Characters => "Characters",
            DebugTab::ZKillboard => "zKillboard",
            DebugTab::State => "State",
        }
    }
}

/// State of the Debug window.
pub(crate) struct DebugState {
    /// The tabs of the window, laid out like the maps' tabs.
    tree: Tree<DebugTab>,
    intel_channel: String,
    intel_author: String,
    intel_text: String,
    /// `intel_text` is already a full chat-log line (`[ ts ] Author > text`).
    intel_raw: bool,
    /// Only list the rules the line triggers; don't feed it to the pipeline.
    intel_dry_run: bool,
    intel_result: Option<String>,
    effect: NodeEffect,
    move_character: Option<i64>,
    /// Show the (internal) node ids, read-only, in the rules editor.
    pub(crate) show_node_ids: bool,
    /// How many waiting requests the zKillboard tab lists.
    queue_rows: usize,
}

/// Lays out the tabs of the Debug window. A page needs the whole app, which
/// the tree cannot lend, so the pane only reserves the room and says which tab
/// is on show; [`TelescopeApp::open_debug_menu`] draws the page there.
#[derive(Default)]
struct DebugBehavior {
    shown: Option<(DebugTab, egui::Rect)>,
}

impl Behavior<DebugTab> for DebugBehavior {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut DebugTab) -> UiResponse {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, egui::Sense::hover());
        self.shown = Some((*pane, rect));
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &DebugTab) -> egui::WidgetText {
        pane.label().into()
    }
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            tree: Tree::new_tabs(egui::Id::unique("debug_tabs"), DebugTab::all()),
            intel_channel: String::new(),
            intel_author: String::from("Debug"),
            intel_text: String::new(),
            intel_raw: false,
            intel_dry_run: false,
            intel_result: None,
            effect: NodeEffect::default(),
            move_character: None,
            show_node_ids: false,
            queue_rows: 30,
        }
    }
}

/// Label and control pairs of a Debug tab, in a plain egui `Grid`.
struct DebugForm<'a> {
    ui: &'a mut egui::Ui,
}

impl DebugForm<'_> {
    fn show(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut DebugForm<'_>)) {
        egui::Grid::new(id).num_columns(2).show(ui, |ui| {
            add_rows(&mut DebugForm { ui });
        });
    }

    fn row<R>(&mut self, label: &str, add_control: impl FnOnce(&mut egui::Ui) -> R) {
        self.ui.label(label);
        add_control(self.ui);
        self.ui.end_row();
    }

    fn row_hint<R>(
        &mut self,
        label: &str,
        hint: &str,
        add_control: impl FnOnce(&mut egui::Ui) -> R,
    ) {
        self.ui.label(label).on_hover_text(hint);
        add_control(self.ui);
        self.ui.end_row();
    }

    /// A row with no label: the buttons and notes under the controls.
    fn note(&mut self, add_content: impl FnOnce(&mut egui::Ui)) {
        self.ui.label("");
        add_content(self.ui);
        self.ui.end_row();
    }
}

impl TelescopeApp {
    /// The Debug window, a window of its own over the app's (like Settings):
    /// a tab for each group of controls. It lives as long as this is called
    /// every frame; its close button closes the menu (`open[1]`).
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn open_debug_menu(&mut self, ctx: &egui::Context) {
        let id = egui::ViewportId::from_hash_of("telescope.debug");
        let builder = egui::ViewportBuilder::default()
            .with_title("Debug Menu")
            .with_inner_size([780.0, 520.0])
            .with_min_inner_size([520.0, 320.0]);
        let closing = ctx.show_viewport_immediate(id, builder, |ui, _class| {
            egui::CentralPanel::default().show(ui, |ui| self.show_debug_screen(ui));
            ui.input(|input| input.viewport().close_requested())
        });
        if closing {
            self.open[1] = false;
        }
    }

    /// The tabs of the Debug window and the page of the one on show.
    fn show_debug_screen(&mut self, ui: &mut egui::Ui) {
        let selected = self.debug_selected_system();
        let mut behavior = DebugBehavior::default();
        self.debug.tree.ui(&mut behavior, ui);
        // The page of the tab on show, inside the room of its pane.
        if let Some((tab, rect)) = behavior.shown {
            let mut page = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            page.set_clip_rect(rect);
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(&mut page, |ui| match tab {
                    DebugTab::Search => self.debug_search_ui(ui, selected.as_ref()),
                    DebugTab::Intel => self.debug_intel_ui(ui, selected.as_ref()),
                    DebugTab::Alerts => self.debug_alerts_ui(ui),
                    DebugTab::Animations => self.debug_animations_ui(ui, selected.as_ref()),
                    DebugTab::Characters => {
                        self.debug_move_character_ui(ui, selected.as_ref());
                    }
                    DebugTab::ZKillboard => self.debug_zkillboard_ui(ui),
                    DebugTab::State => self.debug_state_ui(ui),
                });
        }
    }

    /// System picked in the search results, as `(id, name)`.
    fn debug_selected_system(&self) -> Option<(usize, String)> {
        let row = self.search_results.get(self.search_selected_row?)?;
        Some((usize::try_from(row.0).ok()?, row.1.clone()))
    }

    /// The target system the tabs act on: the one picked in Search.
    fn debug_target(ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        match selected {
            Some((_, name)) => {
                ui.label(format!("Target system: {name}"));
            }
            None => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "No target system: pick one in Search first",
                );
            }
        }
    }

    /// Searches the SDE by name; clicking a result centers the maps on it and
    /// makes it the target of the other tabs.
    fn debug_search_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        ui.label(
            RichText::new(
                "Find a system by name. The result you pick is the target of the other tabs.",
            )
            .weak(),
        );
        egui::CollapsingHeader::new("System")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_1", |form| {
                    form.row("Name", |ui| {
                        let response = ui.text_edit_singleline(&mut self.search_text);
                        if response.changed() {
                            if self.search_text.len() >= 3 {
                                let sde = SdeManager::new(
                                    self.settings.get_sde(),
                                    self.settings.get_factor(),
                                );
                                match sde.and_then(|s| {
                                    s.get_system_id(self.search_text.clone().to_lowercase())
                                }) {
                                    Ok(system_results) => {
                                        // Preselect the first result, so the other tabs
                                        // (animations, move character, intel presets)
                                        // have a target right away.
                                        self.search_selected_row =
                                            (!system_results.is_empty()).then_some(0);
                                        self.search_results = system_results;
                                    }
                                    Err(t_error) => {
                                        self.task_msg.spawn(Message::GenericNotification((
                                            Type::Error,
                                            String::from("sde"),
                                            String::from("get_system_id"),
                                            t_error.to_string(),
                                        )));
                                    }
                                }
                            }
                            if self.search_text.is_empty() {
                                self.search_results.clear();
                                self.search_selected_row = None;
                            }
                        }
                    });
                    form.row_hint(
                        "Pulse an alert",
                        "Also send an alert and a tooltip to the node when a result is clicked",
                        |ui| ui.checkbox(&mut self.emit_notification, ""),
                    );
                    form.note(|ui| {
                        ui.horizontal(|ui| {
                            Self::debug_target(ui, selected);
                            if ui.button("Clear").clicked() {
                                self.search_text.clear();
                                self.search_results.clear();
                                self.search_selected_row = None;
                            }
                        });
                    });
                });
            });
        egui::CollapsingHeader::new("Results")
            .default_open(true)
            .show(ui, |ui| {
                ui.push_id("search_table", |ui| {
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::auto())
                        .column(Column::remainder())
                        .min_scrolled_height(0.0)
                        .max_scroll_height(220.0);

                    table = table.sense(egui::Sense::click());
                    table
                        .header(20.0, |mut header| {
                            header.col(|ui| {
                                ui.strong("System");
                            });
                            header.col(|ui| {
                                ui.strong("Region");
                            });
                        })
                        .body(|mut body| {
                            for row_index in 0..self.search_results.len() {
                                body.row(18.00, |mut row| {
                                    row.set_selected(false);
                                    if let Some(selected_row) = self.search_selected_row
                                        && row_index == selected_row
                                    {
                                        row.set_selected(true);
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].1).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let system_id = self.search_results[row_index].0;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                system_id.try_into().unwrap(),
                                                Target::System,
                                            )));
                                            if self.emit_notification {
                                                let _result =
                                                    tx_map.send(MapSync::SystemAlert(debug_alert(
                                                        system_id.try_into().unwrap(),
                                                        self.settings.get_alert_duration(),
                                                    )));
                                                let _result = tx_map.send(MapSync::SystemTooltip(
                                                    debug_alert(
                                                        system_id.try_into().unwrap(),
                                                        self.settings.get_alert_duration(),
                                                    ),
                                                ));
                                            }
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let system_id = self.search_results[row_index].0;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            system_id.try_into().unwrap(),
                                            Target::System,
                                        )));
                                        if self.emit_notification {
                                            let _result =
                                                tx_map.send(MapSync::SystemAlert(debug_alert(
                                                    system_id.try_into().unwrap(),
                                                    self.settings.get_alert_duration(),
                                                )));
                                            let _result =
                                                tx_map.send(MapSync::SystemTooltip(debug_alert(
                                                    system_id.try_into().unwrap(),
                                                    self.settings.get_alert_duration(),
                                                )));
                                        }
                                    }
                                    let col_data = row.col(|ui| {
                                        if ui.label(&self.search_results[row_index].3).clicked() {
                                            self.search_selected_row = Some(row_index);
                                            let tx_map = Arc::clone(&self.map_msg.0);
                                            let region_id = self.search_results[row_index].2;
                                            let _result = tx_map.send(MapSync::CenterOn((
                                                region_id.try_into().unwrap(),
                                                Target::Region,
                                            )));
                                        }
                                    });
                                    if col_data.1.clicked() {
                                        let tx_map = Arc::clone(&self.map_msg.0);
                                        let region_id = self.search_results[row_index].2;
                                        let _result = tx_map.send(MapSync::CenterOn((
                                            region_id.try_into().unwrap(),
                                            Target::Region,
                                        )));
                                    }
                                    if row.response().clicked() {
                                        self.search_selected_row = Some(row_index);
                                    }
                                });
                            }
                            if self.search_results.is_empty() {
                                body.row(18.00, |mut row| {
                                    row.col(|ui| {
                                        ui.label("No result(s)");
                                    });
                                    row.col(|_ui| {});
                                });
                            }
                        });
                });
            });
    }

    /// Runs a chat line through the pattern engine as if it had just been
    /// read from an intel log, so rules and alerts can be tested. The preset
    /// buttons fill the text with a line of each kind of message.
    fn debug_intel_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        let system = selected.map_or(DEFAULT_PRESET_SYSTEM, |(_, name)| name.as_str());
        ui.label(RichText::new("Run a chat line through detection and dispatch, as if the reader had just found it.").weak());
        egui::CollapsingHeader::new("Presets")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Each button fills the text with a line of that kind about the target system.").weak());
                Self::debug_target(ui, selected);
                ui.horizontal_wrapped(|ui| {
                    for (label, hover, template) in INTEL_PRESETS {
                        if ui.button(label)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            self.debug.intel_text = template.replace("{system}", system);
                            self.debug.intel_raw = false;
                        }
                    }
                });
            });
        let monitored = self.settings.get_cloned_monitored_channels();
        egui::CollapsingHeader::new("Line")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_2", |form| {
                    form.row("Channel", |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.debug.intel_channel)
                                    .hint_text("any channel")
                                    .desired_width(160.0),
                            );
                            egui::ComboBox::from_id_salt("debug_intel_channel")
                                .selected_text("monitored")
                                .show_ui(ui, |ui| {
                                    for channel in monitored.iter() {
                                        ui.selectable_value(
                                            &mut self.debug.intel_channel,
                                            channel.clone(),
                                            channel,
                                        );
                                    }
                                });
                        });
                    });
                    if !self.debug.intel_raw {
                        form.row("Author", |ui| {
                            ui.text_edit_singleline(&mut self.debug.intel_author);
                        });
                    }
                    form.row("Text", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.debug.intel_text)
                                .hint_text("e.g. Jita clr")
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
            });
        egui::CollapsingHeader::new("Send")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_3", |form| {
                    form.row_hint(
                        "Raw chat-log line",
                        "The text is already `[ YYYY.MM.DD HH:MM:SS ] Author > message`",
                        |ui| ui.checkbox(&mut self.debug.intel_raw, ""),
                    );
                    form.row_hint(
                        "Dry run",
                        "Only list the rules the line triggers. Without it the line is also fed to \
                     the pipeline: map pulse, sound, tooltip and log fire as for a real line.",
                        |ui| ui.checkbox(&mut self.debug.intel_dry_run, ""),
                    );
                    form.note(|ui| {
                        let can_send = !self.debug.intel_text.trim().is_empty();
                        let send = ui.add_enabled_ui(can_send, |ui| ui.button("Send")).inner;
                        if send.clicked() {
                            let line = if self.debug.intel_raw {
                                self.debug.intel_text.trim().to_string()
                            } else {
                                format!(
                                    "[ {} ] {} > {}",
                                    chrono::Utc::now().format("%Y.%m.%d %H:%M:%S"),
                                    self.debug.intel_author.trim(),
                                    self.debug.intel_text.trim()
                                )
                            };
                            let channel = match self.debug.intel_channel.trim() {
                                "" => "Debug",
                                channel => channel,
                            }
                            .to_string();
                            let rules = self.parse_intel_data(&channel, &line);
                            // Unless it is a dry run, the same input the reader feeds:
                            // detection, then dispatch (map pulse, sound, tooltip, log),
                            // so the alerts really fire.
                            let sent = if self.debug.intel_dry_run {
                                Ok(false)
                            } else {
                                match webb::rules::IntelLine::parse_or_assume(
                                    &line,
                                    chrono::Utc::now(),
                                ) {
                                    Some(parsed) => self
                                        .intel_input
                                        .try_send(InputEvent {
                                            source: String::from("Debug"),
                                            channel,
                                            kind: webb::rules::InputKind::ChatLog,
                                            line: parsed.0,
                                            facts: webb::event::Facts::default(),
                                            time_origin: parsed.1,
                                        })
                                        .map(|()| true)
                                        .map_err(|error| error.to_string()),
                                    None => Err(String::from("the line isn't in chat-log format")),
                                }
                            };
                            let matched = if rules.is_empty() {
                                String::from("no rule matched")
                            } else {
                                format!("matched: {}", rules.join(", "))
                            };
                            self.debug.intel_result = Some(match sent {
                                Ok(true) => format!("Sent to the pipeline; {matched}."),
                                Ok(false) => format!("Dry run; {matched}."),
                                Err(error) => {
                                    format!("Not sent to the pipeline ({error}); {matched}.")
                                }
                            });
                        }
                    });
                });
                if let Some(result) = &self.debug.intel_result {
                    ui.label(result);
                }
            });
    }

    fn debug_alerts_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Try the alert sound and the notifications of the log.").weak());
        egui::CollapsingHeader::new("Sound")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_4", |form| {
                    form.row("Alert sound", |ui| {
                        ui.label(
                            RichText::new(
                                self.settings.get_alert_sound_path().display().to_string(),
                            )
                            .weak(),
                        );
                    });
                    form.note(|ui| {
                        if ui.button("Play alert sound").clicked() {
                            self.audio.play_alarm(&self.settings.get_alert_sound_path());
                        }
                    });
                });
            });
        egui::CollapsingHeader::new("Log")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("One notification of each kind, to see how the log shows them.")
                        .weak(),
                );
                if ui.button("Send test notifications").clicked() {
                    for (kind, name) in [
                        (Type::Debug, "Debug"),
                        (Type::Info, "Info"),
                        (Type::Warning, "Warning"),
                        (Type::Error, "Error"),
                    ] {
                        self.task_msg.spawn(Message::GenericNotification((
                            kind,
                            String::from("Debug"),
                            String::from("test"),
                            format!("{name} test notification"),
                        )));
                    }
                }
            });
    }

    fn debug_animations_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        ui.label(RichText::new("Preview the node effects on the target system.").weak());
        egui::CollapsingHeader::new("Effect")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Lasting effects run until \"Clear effects\". The map's node template decides \
                 how each effect is drawn.").weak());
                DebugForm::show(ui, "debug_form_5", |form| {
                    form.row("Effect", |ui| {
                        egui::ComboBox::from_id_salt("debug_node_effect")
                            .selected_text(self.debug.effect.label())
                            .show_ui(ui, |ui| {
                                for effect in NodeEffect::ALL {
                                    ui.selectable_value(
                                        &mut self.debug.effect,
                                        effect,
                                        effect.label(),
                                    );
                                }
                            });
                    });
                    form.row("Target", |ui| Self::debug_target(ui, selected));
                    form.note(|ui| {
                        let play = ui
                            .add_enabled_ui(selected.is_some(), |ui| {
                                ui.button(match selected {
                                        Some((_, name)) => format!("Play on {name}"),
                                        None => String::from("Play"),
                                    })
                            })
                            .inner
                            .on_disabled_hover_text("Pick a system in Search first");
                        if play.clicked()
                            && let Some((system_id, _)) = selected
                        {
                            let _ = self
                                .map_msg
                                .0
                                .send(MapSync::NodeEffect((*system_id, self.debug.effect)));
                        }
                    });
                });
            });
    }

    /// Puts a character's marker on the selected system without it moving in
    /// the game (the real location comes back with "Restore").
    fn debug_move_character_ui(&mut self, ui: &mut egui::Ui, selected: Option<&(usize, String)>) {
        ui.label(
            RichText::new(
                "Put a character's marker on the target system without moving it in the game.",
            )
            .weak(),
        );
        if self.esi.characters.is_empty() {
            egui::CollapsingHeader::new("Marker")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label("No linked characters.");
                });
            return;
        }
        if self
            .debug
            .move_character
            .is_none_or(|id| !self.esi.characters.iter().any(|c| c.id == id))
        {
            self.debug.move_character = self.esi.characters.first().map(|c| c.id);
        }
        let current = self
            .esi
            .characters
            .iter()
            .find(|c| Some(c.id) == self.debug.move_character)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        egui::CollapsingHeader::new("Marker")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_6", |form| {
                    form.row("Character", |ui| {
                        egui::ComboBox::from_id_salt("debug_move_character")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for character in &self.esi.characters {
                                    ui.selectable_value(
                                        &mut self.debug.move_character,
                                        Some(character.id),
                                        &character.name,
                                    );
                                }
                            });
                    });
                    form.row("Target", |ui| Self::debug_target(ui, selected));
                    form.note(|ui| {
                        let Some(id) = self.debug.move_character else {
                            return;
                        };
                        ui.horizontal(|ui| {
                            let move_to = ui
                                .add_enabled_ui(selected.is_some(), |ui| {
                                    ui.button(match selected {
                                        Some((_, name)) => format!("Move to {name}"),
                                        None => String::from("Move to…"),
                                    })
                                })
                                .inner
                                .on_disabled_hover_text("Pick a system in Search first");
                            if move_to.clicked()
                                && let Some((system_id, _)) = selected
                                && let Ok(system) = i64::try_from(*system_id)
                            {
                                self.update_player_location(id, system);
                            }
                            if ui
                                .button("Restore")
                                .on_hover_text(
                                    "Ask the location watchdog for the character's real location",
                                )
                                .clicked()
                                && let Some(sender) = &self.char_msg
                            {
                                let _ = sender.try_send(CharacterSync::Add(id));
                            }
                        });
                    });
                });
            });
    }

    /// How the zKillboard source walks the feed: where it is, how far behind
    /// it can be, and what it has skipped. The counting works on sequence
    /// numbers, so there is no need to know beforehand how many killmails a
    /// round will bring: the walk reads `n`, `n + 1`, ... until a 404.
    fn debug_zkillboard_ui(&mut self, ui: &mut egui::Ui) {
        // The numbers move by themselves.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        let status = self.intel_zkillboard.status();
        let walk = &status.diagnostics;
        let number =
            |value: Option<u64>| value.map_or_else(|| String::from("-"), |n| n.to_string());
        // `sequence.json` is refreshed every 51 killmails: the difference is a
        // lower bound of what is pending, not a count.
        let behind = match (walk.next_sequence, walk.latest_sequence) {
            (Some(next), Some(latest)) => latest.saturating_sub(next).to_string(),
            _ => String::from("-"),
        };
        let wait = walk.next_delay.map_or_else(
            || String::from("-"),
            |d| format!("{:.1} s", d.as_secs_f32()),
        );
        let last_error = status
            .last_error
            .clone()
            .unwrap_or_else(|| String::from("-"));
        let feed = zkill::feed::FeedConfig::default().url;
        ui.label(RichText::new("How the source walks the R2Z2 feed: where it is, how far behind and what it skipped.").weak());
        egui::CollapsingHeader::new("Source")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_7", |form| {
                    form.row("State", |ui| ui.label(format!("{:?}", status.state)));
                    form.row("Asked by the rules", |ui| {
                        ui.label(self.intel_zkillboard.is_wanted().to_string())
                    });
                    form.row("Paused by the settings", |ui| {
                        ui.label((!self.settings.get_zkillboard().enabled).to_string())
                    });
                    form.row("Last error", |ui| ui.label(last_error));
                });
            });
        egui::CollapsingHeader::new("Walk")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(RichText::new("A round reads n, n + 1, ... until a 404.").weak());
                DebugForm::show(ui, "debug_form_8", |form| {
                    form.row("Next sequence to read", |ui| {
                        ui.label(number(walk.next_sequence))
                    });
                    form.row("Latest in sequence.json", |ui| {
                        ui.label(number(walk.latest_sequence))
                    });
                    form.row_hint(
                        "Behind by at least",
                        "sequence.json is refreshed only every 51 killmails, so this is a lower \
                         bound, not a count.",
                        |ui| ui.label(behind),
                    );
                    form.row("Caught up with the end", |ui| {
                        ui.label(walk.caught_up.to_string())
                    });
                    form.row("Holes skipped", |ui| {
                        ui.label(walk.holes_skipped.to_string())
                    });
                    form.row("Resyncs after failures", |ui| {
                        ui.label(walk.resyncs.to_string())
                    });
                    form.row("Rounds", |ui| ui.label(walk.polls.to_string()));
                    form.row("Wait before the next round", |ui| ui.label(wait));
                });
            });
        egui::CollapsingHeader::new("Traffic")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_9", |form| {
                    form.row("Sent to the rules", |ui| {
                        ui.label(status.announced.to_string())
                    });
                    form.row("Left out by the filters", |ui| {
                        ui.label(status.filtered.to_string())
                    });
                });
            });
        egui::CollapsingHeader::new("Endpoints")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_10", |form| {
                    form.row("Pointer", |ui| ui.label(format!("{feed}/sequence.json")));
                    form.row("Killmail", |ui| {
                        ui.label(format!("{feed}/{{sequence}}.json"))
                    });
                });
            });
        egui::CollapsingHeader::new("Request queue")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "The requests waiting in the lane of the query API: the counts of the \
                         last day, tooltips first (after their hover delay), then the plan \
                         around the characters by priority. The time is the lane played \
                         forward at its pace, starting from the wait the guard asks for.",
                    )
                    .weak(),
                );
                let queue = self.intel_zkillboard.day_queue(self.debug.queue_rows);
                let api = Endpoint::Api;
                let r2z2 = Endpoint::R2z2;
                let wait_api = self.intel_zkillboard.request_wait(api);
                let wait_r2z2 = self.intel_zkillboard.request_wait(r2z2);
                let sent_api = self.intel_zkillboard.requests_sent(api);
                let sent_r2z2 = self.intel_zkillboard.requests_sent(r2z2);
                let clock = |wait: std::time::Duration| {
                    let seconds = wait.as_secs();
                    if seconds >= 3600 {
                        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
                    } else if seconds >= 60 {
                        format!("{}m {:02}s", seconds / 60, seconds % 60)
                    } else {
                        format!("{seconds} s")
                    }
                };
                let guard_text = |wait: Option<std::time::Duration>| match wait {
                    Some(wait) => format!("keeping away for {}", clock(wait)),
                    None => String::from("free"),
                };
                DebugForm::show(ui, "debug_queue_summary", |form| {
                    form.row("Waiting (API / R2Z2)", |ui| {
                        ui.label(format!(
                            "{} / {}",
                            queue.total,
                            self.intel_zkillboard.requests_waiting(r2z2)
                        ))
                    });
                    form.row("Requests sent (API / R2Z2)", |ui| {
                        ui.label(format!("{sent_api} / {sent_r2z2}"))
                    });
                    form.row("Guard (API)", |ui| ui.label(guard_text(wait_api)));
                    form.row("Guard (R2Z2)", |ui| ui.label(guard_text(wait_r2z2)));
                    form.row("Rows shown", |ui| {
                        ui.add(egui::DragValue::new(&mut self.debug.queue_rows).range(5..=500))
                    });
                });
                if queue.requests.is_empty() {
                    ui.label("Nothing is waiting.");
                    return;
                }
                egui::Grid::new("debug_queue")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("In").weak());
                        ui.label(RichText::new("For").weak());
                        ui.label(RichText::new("System").weak());
                        ui.end_row();
                        for request in &queue.requests {
                            ui.label(clock(request.eta));
                            ui.label(request.kind.label());
                            ui.label(
                                self.intel_zkillboard
                                    .system_name(request.system_id)
                                    .unwrap_or_else(|| request.system_id.to_string()),
                            );
                            ui.end_row();
                        }
                    });
                if queue.total > queue.requests.len() {
                    ui.label(
                        RichText::new(format!(
                            "... and {} more",
                            queue.total - queue.requests.len()
                        ))
                        .weak(),
                    );
                }
            });
    }

    fn debug_state_ui(&mut self, ui: &mut egui::Ui) {
        let watchdog = match &self.char_msg {
            Some(sender) if !sender.is_closed() => "running",
            _ => "stopped",
        };
        let schema = match self.esi.schema_status {
            Some(SchemaStatus::Created) => String::from("created this run"),
            Some(SchemaStatus::Migrated(from)) => format!("migrated from {from} this run"),
            Some(SchemaStatus::UpToDate) => String::from("up to date"),
            Some(SchemaStatus::Newer(version)) => format!("newer ({version})"),
            None => String::from("could not be opened"),
        };
        ui.label(RichText::new("A read-only view of the application's internal state.").weak());
        egui::CollapsingHeader::new("Application")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_11", |form| {
                    form.row("Location watchdog", |ui| ui.label(watchdog));
                    form.row("Player DB schema", |ui| {
                        ui.label(format!("v{SCHEMA_VERSION}, {schema}"))
                    });
                    form.row("Log entries", |ui| {
                        ui.label(self.app_messages.len().to_string())
                    });
                });
            });
        egui::CollapsingHeader::new("Locations")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_12", |form| {
                    form.row("Player DB", |ui| {
                        ui.label(self.settings.get_db().display().to_string())
                    });
                    form.row("SDE", |ui| {
                        ui.label(self.settings.get_sde().display().to_string())
                    });
                    form.row("Intel directory", |ui| {
                        ui.label(self.settings.get_intel().display().to_string())
                    });
                    form.row("Monitored channels", |ui| {
                        ui.label(self.settings.get_cloned_monitored_channels().join(", "))
                    });
                });
            });
        egui::CollapsingHeader::new("Linked characters")
            .default_open(true)
            .show(ui, |ui| {
                if self.esi.characters.is_empty() {
                    ui.label("No linked characters.");
                    return;
                }
                egui::Grid::new("debug_characters")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Character").weak());
                        ui.label(RichText::new("Location").weak());
                        ui.label(RichText::new("Token expires").weak());
                        ui.end_row();
                        for character in &self.esi.characters {
                            ui.label(format!("{} ({})", character.name, character.id));
                            ui.label(character.location.to_string());
                            let token = match self.esi.auth.get(&character.id) {
                                Some(auth) if !auth.refresh_token.is_empty() => auth
                                    .expiration
                                    .map_or(String::from("unknown"), |expiration| {
                                        expiration.format("%Y-%m-%d %H:%M:%S UTC").to_string()
                                    }),
                                _ => String::from("no token (link it again)"),
                            };
                            ui.label(token);
                            ui.end_row();
                        }
                    });
            });
        egui::CollapsingHeader::new("Developer options")
            .default_open(true)
            .show(ui, |ui| {
                DebugForm::show(ui, "debug_form_13", |form| {
                    form.row_hint(
                        "Node ids in the rules editor",
                        "Show the internal node ids, read-only, in the rules editor",
                        |ui| ui.checkbox(&mut self.debug.show_node_ids, ""),
                    );
                });
            });
    }
}

/// A made-up intel alert on `system_id`, sent by "Emit notification" to
/// preview the node effect and its tooltip line.
fn debug_alert(system_id: usize, duration: std::time::Duration) -> IntelAlert {
    let text = "Debug alert";
    let summary = AlertSummary {
        leftover: String::from(text),
        ..AlertSummary::default()
    };
    IntelAlert::new(
        system_id,
        std::time::Instant::now(),
        duration,
        text,
        summary,
        true,
    )
}
