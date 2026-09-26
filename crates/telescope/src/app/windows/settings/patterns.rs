//! Settings page "Patterns": add, edit, delete and enable/disable the rules
//! declared in `patterns.toml`, then save the file and reload the matching
//! engine.
//!
//! The page keeps its own in-memory copy of the rules ([`PatternsEditor`]),
//! loaded the first time it is shown. Changes are written to `patterns.toml`
//! -- and the engine reloaded -- only when Save is pressed; validation also
//! happens then, so a rule that does not compile never reaches the file.

use crate::app::TelescopeApp;
use crate::app::messages::{Message, Type};
use crate::app_dirs::PATTERNS_FILE;
use eframe::egui::{self, Button, Color32, FontId, RichText};
use egui_extras::{Column, TableBuilder};
use webb::patterns::{
    ActionConfig, DictionaryActionConfig, DictionaryRuleConfig, IntelCategory, PatternConfig,
    PatternEngine, PatternRuleConfig,
};
use webb::rules::RulesConfig;
use std::path::Path;

/// In-memory editing state of the Patterns page.
#[derive(Default)]
pub(crate) struct PatternsEditor {
    /// The rules being edited; `None` until the page is first shown, or when
    /// the file could not be read.
    config: Option<PatternConfig>,
    /// Which rule the form edits: `(is_dictionary, index)`.
    selected: Option<(bool, usize)>,
    /// Whether the in-memory rules differ from the file.
    dirty: bool,
    /// Errors of the last load or save attempt, shown above the table.
    errors: Vec<String>,
}

impl PatternsEditor {
    /// Loads the rules from disk the first time the page is shown.
    fn ensure_loaded(&mut self) {
        if self.config.is_none() && self.errors.is_empty() {
            self.reload();
        }
    }

    /// (Re)loads the rules from `patterns.toml`, discarding unsaved edits.
    fn reload(&mut self) {
        self.errors.clear();
        self.selected = None;
        self.dirty = false;
        match PatternConfig::load(Path::new(PATTERNS_FILE)) {
            Ok(config) => self.config = Some(config),
            Err(error) => {
                self.config = None;
                self.errors.push(error.to_string());
            }
        }
    }

    /// Appends a new regex rule and selects it.
    fn add_pattern(&mut self) {
        let Some(config) = self.config.as_mut() else {
            return;
        };
        let id = unique_id(config, "new_pattern");
        config.patterns.push(PatternRuleConfig {
            id,
            pattern: String::new(),
            case_insensitive: false,
            channels: Vec::new(),
            enabled: true,
            action: ActionConfig::Notify,
            category: None,
        });
        self.selected = Some((false, config.patterns.len() - 1));
        self.dirty = true;
    }

    /// Appends a new dictionary rule and selects it.
    fn add_dictionary(&mut self) {
        let Some(config) = self.config.as_mut() else {
            return;
        };
        let id = unique_id(config, "new_dictionary");
        config.dictionaries.push(DictionaryRuleConfig {
            id,
            words: Vec::new(),
            case_insensitive: false,
            channels: Vec::new(),
            enabled: true,
            action: DictionaryActionConfig::Notify,
            category: None,
        });
        self.selected = Some((true, config.dictionaries.len() - 1));
        self.dirty = true;
    }

    /// Removes the selected rule.
    fn delete_selected(&mut self) {
        let Some((is_dict, index)) = self.selected else {
            return;
        };
        let Some(config) = self.config.as_mut() else {
            return;
        };
        if is_dict {
            if index < config.dictionaries.len() {
                config.dictionaries.remove(index);
            }
        } else if index < config.patterns.len() {
            config.patterns.remove(index);
        }
        self.selected = None;
        self.dirty = true;
    }

    /// The table of every rule (patterns first, then dictionaries).
    fn table(&mut self, ui: &mut egui::Ui) {
        let Some(config) = self.config.as_mut() else {
            return;
        };
        let pattern_count = config.patterns.len();
        let total = pattern_count + config.dictionaries.len();
        let mut selected = self.selected;
        let mut dirty = self.dirty;
        ui.push_id("patterns_table", |ui| {
            TableBuilder::new(ui)
                .column(Column::exact(24.0))
                .column(Column::exact(170.0))
                .column(Column::exact(80.0))
                .column(Column::remainder())
                .striped(true)
                .vscroll(true)
                .max_scroll_height(180.0)
                .body(|mut body| {
                    for index in 0..total {
                        let is_dict = index >= pattern_count;
                        let local = if is_dict {
                            index - pattern_count
                        } else {
                            index
                        };
                        let row_selected = selected == Some((is_dict, local));
                        body.row(18.0, |mut row| {
                            row.col(|ui| {
                                let mut enabled = if is_dict {
                                    config.dictionaries[local].enabled
                                } else {
                                    config.patterns[local].enabled
                                };
                                if ui.checkbox(&mut enabled, "").changed() {
                                    if is_dict {
                                        config.dictionaries[local].enabled = enabled;
                                    } else {
                                        config.patterns[local].enabled = enabled;
                                    }
                                    dirty = true;
                                }
                            });
                            row.col(|ui| {
                                let id = if is_dict {
                                    &config.dictionaries[local].id
                                } else {
                                    &config.patterns[local].id
                                };
                                if ui.selectable_label(row_selected, id).clicked() {
                                    selected = Some((is_dict, local));
                                }
                            });
                            row.col(|ui| {
                                let kind = if is_dict {
                                    t!("settings.patterns.kind_dictionary")
                                } else {
                                    t!("settings.patterns.kind_pattern")
                                };
                                ui.label(kind);
                            });
                            row.col(|ui| {
                                if is_dict {
                                    let rule = &config.dictionaries[local];
                                    let summary = format!(
                                        "{} · {}",
                                        action_label_dict(&rule.action),
                                        category_label(rule.category)
                                    );
                                    ui.label(summary)
                                        .on_hover_text(format!("{} words", rule.words.len()));
                                } else {
                                    let rule = &config.patterns[local];
                                    let summary = format!(
                                        "{} · {}",
                                        action_label_pattern(&rule.action),
                                        category_label(rule.category)
                                    );
                                    ui.label(summary).on_hover_text(&rule.pattern);
                                }
                            });
                        });
                    }
                });
        });
        self.selected = selected;
        self.dirty = dirty;
    }

    /// The edit form for the selected rule.
    fn form(&mut self, ui: &mut egui::Ui) {
        let Some((is_dict, index)) = self.selected else {
            ui.separator();
            ui.label(t!("settings.patterns.no_selection"));
            return;
        };
        let Some(config) = self.config.as_mut() else {
            return;
        };
        let mut dirty = self.dirty;
        let mut delete = false;
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("settings.patterns.edit")).strong());
            if ui.button(t!("settings.patterns.delete")).clicked() {
                delete = true;
            }
        });
        if is_dict {
            let Some(rule) = config.dictionaries.get_mut(index) else {
                return;
            };
            dirty |= dictionary_form(ui, rule);
        } else {
            let Some(rule) = config.patterns.get_mut(index) else {
                return;
            };
            dirty |= pattern_form(ui, rule);
        }
        self.dirty = dirty;
        if delete {
            self.delete_selected();
        }
    }
}

impl TelescopeApp {
    pub(super) fn show_patterns_page(&mut self, ui: &mut egui::Ui) {
        self.patterns_editor.ensure_loaded();

        ui.label(RichText::new(t!("settings.patterns.heading")).font(FontId::proportional(20.0)));
        ui.label(t!("settings.patterns.help"));

        let mut reload = false;
        let mut save = false;
        {
            let editor = &mut self.patterns_editor;
            ui.horizontal(|ui| {
                if ui.button(t!("settings.patterns.add_pattern")).clicked() {
                    editor.add_pattern();
                }
                if ui.button(t!("settings.patterns.add_dictionary")).clicked() {
                    editor.add_dictionary();
                }
                if ui.button(t!("settings.patterns.reload")).clicked() {
                    reload = true;
                }
                save = ui
                    .add_enabled(editor.dirty, Button::new(t!("settings.patterns.save")))
                    .clicked();
                if editor.dirty {
                    ui.colored_label(Color32::YELLOW, t!("settings.unsaved"));
                }
            });
            for error in &editor.errors {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
            editor.table(ui);
            editor.form(ui);
        }

        if reload {
            self.patterns_editor.reload();
        }
        if save {
            self.save_patterns();
        }
    }

    /// Validates the edited rules, writes `patterns.toml` and reloads the
    /// engine. On any error nothing is written and the errors are shown on the
    /// page, so the file always holds a set of rules that compiles.
    pub(crate) fn save_patterns(&mut self) {
        let Some(config) = self.patterns_editor.config.clone() else {
            return;
        };

        let (_engine, rule_errors) = match PatternEngine::from_config(&config) {
            Ok(built) => built,
            Err(error) => {
                self.patterns_editor.errors = vec![error.to_string()];
                return;
            }
        };
        if !rule_errors.is_empty() {
            self.patterns_editor.errors = rule_errors.iter().map(|e| e.to_string()).collect();
            return;
        }

        if let Err(error) = config.save(Path::new(PATTERNS_FILE)) {
            self.patterns_editor.errors = vec![error.to_string()];
            return;
        }

        // The database is the source of truth now: translate the edited
        // patterns into the three-class model, persist it and reload the
        // live engine/router.
        let rules = RulesConfig::from_pattern_config(&config);
        self.apply_rules(rules);
        self.patterns_editor.errors.clear();
        self.patterns_editor.dirty = false;
        self.task_msg.spawn(Message::GenericNotification((
            Type::Info,
            String::from("PatternEngine"),
            String::from("save_patterns"),
            format!("{} was saved and the rules reloaded", PATTERNS_FILE),
        )));
    }
}

/// Unique id for a new rule, derived from `base` and the ids already in use.
fn unique_id(config: &PatternConfig, base: &str) -> String {
    let taken = |id: &str| {
        config.patterns.iter().any(|rule| rule.id == id)
            || config.dictionaries.iter().any(|rule| rule.id == id)
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|id| !taken(id))
        .unwrap()
}

/// The edit fields of a regex rule. Returns whether anything changed.
fn pattern_form(ui: &mut egui::Ui, rule: &mut PatternRuleConfig) -> bool {
    let mut changed = false;
    egui::Grid::new("pattern_form")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(t!("settings.patterns.id"));
            changed |= ui.text_edit_singleline(&mut rule.id).changed();
            ui.end_row();

            ui.label(t!("settings.patterns.pattern"));
            changed |= ui.text_edit_multiline(&mut rule.pattern).changed();
            ui.end_row();

            ui.label(t!("settings.patterns.channels"));
            changed |= channels_edit(ui, &mut rule.channels);
            ui.end_row();
        });
    changed |= ui
        .checkbox(
            &mut rule.case_insensitive,
            t!("settings.patterns.case_insensitive"),
        )
        .changed();
    changed |= action_edit_pattern(ui, &mut rule.action);
    changed |= category_edit(ui, &mut rule.category, true);
    changed
}

/// The edit fields of a dictionary rule. Returns whether anything changed.
fn dictionary_form(ui: &mut egui::Ui, rule: &mut DictionaryRuleConfig) -> bool {
    let mut changed = false;
    egui::Grid::new("dictionary_form")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label(t!("settings.patterns.id"));
            changed |= ui.text_edit_singleline(&mut rule.id).changed();
            ui.end_row();

            ui.label(t!("settings.patterns.words"));
            let mut words = rule.words.join("\n");
            if ui.text_edit_multiline(&mut words).changed() {
                rule.words = words
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
                changed = true;
            }
            ui.end_row();

            ui.label(t!("settings.patterns.channels"));
            changed |= channels_edit(ui, &mut rule.channels);
            ui.end_row();
        });
    changed |= ui
        .checkbox(
            &mut rule.case_insensitive,
            t!("settings.patterns.case_insensitive"),
        )
        .changed();
    changed |= action_edit_dictionary(ui, &mut rule.action);
    changed |= category_edit(ui, &mut rule.category, false);
    changed
}

/// Comma-separated channel filter. Returns whether anything changed.
fn channels_edit(ui: &mut egui::Ui, channels: &mut Vec<String>) -> bool {
    let mut text = channels.join(", ");
    if ui
        .text_edit_singleline(&mut text)
        .on_hover_text(t!("settings.patterns.channels_hint"))
        .changed()
    {
        *channels = text
            .split(',')
            .map(|channel| channel.trim().to_string())
            .filter(|channel| !channel.is_empty())
            .collect();
        return true;
    }
    false
}

/// Action editor for a regex rule. Returns whether anything changed.
fn action_edit_pattern(ui: &mut egui::Ui, action: &mut ActionConfig) -> bool {
    let mut choice = match action {
        ActionConfig::Notify => 0,
        ActionConfig::MapAlert { .. } => 1,
        ActionConfig::Ignore => 2,
    };
    let before = choice;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.action"));
        egui::ComboBox::from_id_salt("pattern_action")
            .selected_text(action_label_pattern(action))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, 0, t!("settings.patterns.action_notify"));
                ui.selectable_value(&mut choice, 1, t!("settings.patterns.action_map_alert"));
                ui.selectable_value(&mut choice, 2, t!("settings.patterns.action_ignore"));
            });
    });
    let mut changed = choice != before;
    match choice {
        0 => {
            if !matches!(action, ActionConfig::Notify) {
                *action = ActionConfig::Notify;
                changed = true;
            }
        }
        1 => {
            let group = match action {
                ActionConfig::MapAlert { system_group } => system_group.clone(),
                _ => String::from("system"),
            };
            if !matches!(action, ActionConfig::MapAlert { .. }) {
                *action = ActionConfig::MapAlert {
                    system_group: group,
                };
                changed = true;
            }
        }
        _ => {
            if !matches!(action, ActionConfig::Ignore) {
                *action = ActionConfig::Ignore;
                changed = true;
            }
        }
    }
    if let ActionConfig::MapAlert { system_group } = action {
        ui.horizontal(|ui| {
            ui.label(t!("settings.patterns.system_group"));
            changed |= ui.text_edit_singleline(system_group).changed();
        });
    }
    changed
}

/// Action editor for a dictionary rule. Returns whether anything changed.
fn action_edit_dictionary(ui: &mut egui::Ui, action: &mut DictionaryActionConfig) -> bool {
    let mut choice = match action {
        DictionaryActionConfig::Notify => 0,
        DictionaryActionConfig::MapAlert => 1,
    };
    let before = choice;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.action"));
        egui::ComboBox::from_id_salt("dictionary_action")
            .selected_text(action_label_dict(action))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, 0, t!("settings.patterns.action_notify"));
                ui.selectable_value(&mut choice, 1, t!("settings.patterns.action_map_alert"));
            });
    });
    if choice != before {
        *action = if choice == 0 {
            DictionaryActionConfig::Notify
        } else {
            DictionaryActionConfig::MapAlert
        };
        return true;
    }
    false
}

/// Category editor. `allow_count` is false for dictionaries, which have no
/// capture groups and so cannot be a `count`.
fn category_edit(
    ui: &mut egui::Ui,
    category: &mut Option<IntelCategory>,
    allow_count: bool,
) -> bool {
    let mut choice = *category;
    let before = choice;
    ui.horizontal(|ui| {
        ui.label(t!("settings.patterns.category"));
        egui::ComboBox::from_id_salt("pattern_category")
            .selected_text(category_label(*category))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, None, t!("settings.patterns.category_none"));
                ui.selectable_value(
                    &mut choice,
                    Some(IntelCategory::Ship),
                    t!("settings.patterns.category_ship"),
                );
                if allow_count {
                    ui.selectable_value(
                        &mut choice,
                        Some(IntelCategory::Count),
                        t!("settings.patterns.category_count"),
                    );
                }
                ui.selectable_value(
                    &mut choice,
                    Some(IntelCategory::Clear),
                    t!("settings.patterns.category_clear"),
                );
                ui.selectable_value(
                    &mut choice,
                    Some(IntelCategory::Keyword),
                    t!("settings.patterns.category_keyword"),
                );
                ui.selectable_value(
                    &mut choice,
                    Some(IntelCategory::Query),
                    t!("settings.patterns.category_query"),
                );
            });
    });
    if choice != before {
        *category = choice;
        return true;
    }
    false
}

fn action_label_pattern(action: &ActionConfig) -> String {
    match action {
        ActionConfig::Notify => t!("settings.patterns.action_notify"),
        ActionConfig::MapAlert { .. } => t!("settings.patterns.action_map_alert"),
        ActionConfig::Ignore => t!("settings.patterns.action_ignore"),
    }
    .into_owned()
}

fn action_label_dict(action: &DictionaryActionConfig) -> String {
    match action {
        DictionaryActionConfig::Notify => t!("settings.patterns.action_notify"),
        DictionaryActionConfig::MapAlert => t!("settings.patterns.action_map_alert"),
    }
    .into_owned()
}

fn category_label(category: Option<IntelCategory>) -> String {
    match category {
        None => t!("settings.patterns.category_none"),
        Some(IntelCategory::Ship) => t!("settings.patterns.category_ship"),
        Some(IntelCategory::Count) => t!("settings.patterns.category_count"),
        Some(IntelCategory::Clear) => t!("settings.patterns.category_clear"),
        Some(IntelCategory::Keyword) => t!("settings.patterns.category_keyword"),
        Some(IntelCategory::Query) => t!("settings.patterns.category_query"),
    }
    .into_owned()
}
