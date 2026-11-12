//! The silenced systems of the Sound page: the systems whose reports never
//! sound the alarm, however close they are (a trade hub, say). A text field
//! with suggestions ([`egui::CompletionPopup`]) adds a system by its name, and
//! every silenced system is a row with a button that takes it off. The maps and
//! the overlay go on showing what is reported there: only the sound is
//! silenced.

use crate::app::TelescopeApp;
use eframe::egui;
use egui::text_edit::Suggestion;
use egui_panels::{Palette, Section, Theme, Variant};
use sde::objects::Universe;

/// Most suggestions the popup lists at once.
const SUGGESTIONS: usize = 8;

/// Fewest letters typed before the popup suggests anything: with one letter
/// it would list a thousand systems.
const MIN_LETTERS: usize = 2;

/// The names of the systems of the universe in lower case and sorted, so that
/// the ones a typed prefix leads to are found without going through all of
/// them every frame.
#[derive(Default)]
pub(crate) struct SystemIndex {
    /// How many systems the index was built from: it is built again when the
    /// universe has another number (a new SDE was loaded).
    built_from: usize,
    /// `(name in lower case, id)`, sorted.
    names: Vec<(String, u32)>,
}

impl SystemIndex {
    /// Builds the index when the universe changed since it was.
    pub(crate) fn refresh(&mut self, universe: &Universe) {
        if self.built_from == universe.solar_systems.len() && !self.names.is_empty() {
            return;
        }
        self.names = universe
            .solar_systems
            .values()
            .map(|system| (system.name.to_lowercase(), system.id))
            .collect();
        self.names.sort_unstable();
        self.built_from = universe.solar_systems.len();
    }

    /// The ids of the systems whose name starts with `word`, in name order.
    fn starting_with(&self, word: &str) -> impl Iterator<Item = u32> + '_ {
        let word = word.trim().to_lowercase();
        let first = self
            .names
            .partition_point(|(name, _)| name.as_str() < word.as_str());
        self.names[first..]
            .iter()
            .take_while(move |(name, _)| name.starts_with(&word))
            .map(|(_, id)| *id)
    }

    /// The system called exactly `name`, whatever the case.
    fn find(&self, name: &str) -> Option<u32> {
        let name = name.trim().to_lowercase();
        let at = self
            .names
            .binary_search_by(|(candidate, _)| candidate.as_str().cmp(name.as_str()))
            .ok()?;
        Some(self.names[at].1)
    }

    /// What the popup offers for the text typed so far: the systems that are
    /// not silenced already and start with it, each with its region.
    fn suggestions(&self, word: &str, silenced: &[u32], universe: &Universe) -> Vec<Suggestion> {
        if word.trim().chars().count() < MIN_LETTERS {
            return Vec::new();
        }
        self.starting_with(word)
            .filter(|id| silenced.binary_search(id).is_err())
            .take(SUGGESTIONS)
            .filter_map(|id| {
                let system = universe.solar_systems.get(&id)?;
                let suggestion = Suggestion::new(system.name.clone());
                Some(match universe.regions.get(&system.region) {
                    Some(region) => suggestion.description(region.name.clone()),
                    None => suggestion,
                })
            })
            .collect()
    }
}

/// One silenced system as a tag: its name, the region dimmed after it and the
/// button that takes it off. It looks like [`egui_panels::chip`], which has no
/// button.
struct SystemChip {
    name: String,
    region: Option<String>,
}

impl SystemChip {
    /// Space between the tags, and between the parts of one.
    const GAP: f32 = 6.0;
    /// Side of the button that takes the system off.
    const CROSS: f32 = 18.0;
    /// What the frame of a tag takes besides its content: the margins, left
    /// and right, and the border.
    const FRAME: f32 = 14.0;

    /// The tag of `system_id`: `#id` while the SDE is not loaded.
    fn of(system_id: u32, universe: &Universe) -> Self {
        let system = universe.solar_systems.get(&system_id);
        Self {
            name: system.map_or_else(|| format!("#{system_id}"), |system| system.name.clone()),
            region: system
                .and_then(|system| universe.regions.get(&system.region))
                .map(|region| region.name.clone()),
        }
    }

    /// The text after the name.
    fn region_text(&self) -> Option<String> {
        self.region.as_ref().map(|region| format!("· {region}"))
    }

    /// How wide the tag is, to know how many fit in a line.
    fn width(&self, ui: &egui::Ui, font: &egui::FontId) -> f32 {
        let measure = |text: String| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), egui::Color32::WHITE)
                .size()
                .x
        };
        let mut width = measure(self.name.clone());
        if let Some(region) = self.region_text() {
            width += Self::GAP + measure(region);
        }
        width + Self::GAP + Self::CROSS + Self::FRAME
    }

    /// Draws the tag. Returns whether its button was clicked.
    fn show(&self, ui: &mut egui::Ui, theme: &Theme, palette: &Palette) -> bool {
        let mut removed = false;
        egui::Frame::new()
            .fill(palette.badge_fill)
            .stroke(egui::Stroke::new(1.0, palette.card_stroke))
            .corner_radius(egui::CornerRadius::same(theme.radius.saturating_sub(1)))
            .inner_margin(egui::Margin {
                left: 10,
                right: 2,
                top: 2,
                bottom: 2,
            })
            .show(ui, |ui| {
                // A tag is never squeezed to a column of letters.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.spacing_mut().item_spacing.x = Self::GAP;
                ui.label(
                    egui::RichText::new(&self.name)
                        .size(theme.small_size)
                        .color(palette.text),
                );
                if let Some(region) = self.region_text() {
                    ui.label(
                        egui::RichText::new(region)
                            .size(theme.small_size)
                            .color(palette.muted_text),
                    );
                }
                let cross = egui::Button::new(
                    egui::RichText::new("\u{00d7}")
                        .size(theme.small_size + 4.0)
                        .color(palette.muted_text),
                )
                .frame(false)
                .min_size(egui::vec2(Self::CROSS, Self::CROSS));
                removed = ui
                    .add(cross)
                    .on_hover_text(t!("settings.alerts.silenced_remove"))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked();
            });
        removed
    }
}

impl TelescopeApp {
    /// The field that adds a system, how many are silenced and, under them,
    /// the silenced systems as tags.
    pub(crate) fn show_silenced_section(&mut self, ui: &mut egui::Ui) {
        self.settings_ui.system_index.refresh(&self.universe);
        Section::new(&t!("settings.alerts.silenced_title"))
            .description(&t!("settings.alerts.silenced_description"))
            .show(ui, |ui| {
                egui_panels::form(ui, |form| {
                    form.row(t!("settings.alerts.silenced_add"), |ui| {
                        self.show_system_search(ui);
                        let count = self.settings.get_silenced_systems().len();
                        egui_panels::badge(
                            ui,
                            &t!("settings.alerts.silenced_count", count = count),
                        );
                        if count > 1
                            && egui_panels::button(
                                ui,
                                t!("settings.alerts.silenced_clear"),
                                Variant::Ghost,
                            )
                            .clicked()
                        {
                            self.settings.clear_silenced_systems();
                        }
                    });
                    form.note(|ui| {
                        ui.weak(t!("settings.alerts.silenced_help"));
                    });
                    form.note(|ui| self.show_silenced_chips(ui));
                });
            });
    }

    /// The text field with the suggestions: accepting one silences the system
    /// and clears the field.
    fn show_system_search(&mut self, ui: &mut egui::Ui) {
        let width = 260.0_f32.min(ui.available_width());
        let output = {
            let universe = &self.universe;
            let index = &self.settings_ui.system_index;
            let silenced = self.settings.get_silenced_systems();
            let field = egui::TextEdit::singleline(&mut self.settings_ui.silenced_search)
                .hint_text(t!("settings.alerts.silenced_hint"))
                .desired_width(width);
            // The whole text is one word, since a name can have spaces
            // ("Tash-Murkon Prime").
            egui::CompletionPopup::new(ui.make_persistent_id("silenced_system_search"))
                .word_boundary(|_| false)
                .align(egui::RectAlign::BOTTOM_START)
                .show(ui, field, |query| {
                    index.suggestions(query.word, silenced, universe)
                })
        };
        if let Some(accepted) = output.accepted {
            if let Some(id) = self.settings_ui.system_index.find(&accepted.insert) {
                self.settings.silence_system(id);
            }
            self.settings_ui.silenced_search.clear();
        }
    }

    /// The silenced systems as tags that flow over as many lines as the width
    /// needs (a tag that does not fit in what is left of a line starts the
    /// next one, whole), or what to expect while there are none.
    fn show_silenced_chips(&mut self, ui: &mut egui::Ui) {
        let silenced = self.settings.get_silenced_systems().to_vec();
        if silenced.is_empty() {
            ui.weak(t!("settings.alerts.silenced_empty"));
            return;
        }
        let theme = Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let font = egui::FontId::proportional(theme.small_size);
        let chips: Vec<(u32, SystemChip)> = silenced
            .iter()
            .map(|id| (*id, SystemChip::of(*id, &self.universe)))
            .collect();
        let widths: Vec<f32> = chips
            .iter()
            .map(|(_, chip)| chip.width(ui, &font))
            .collect();
        let available = ui.available_width();
        let mut removed = None;
        let mut start = 0;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = SystemChip::GAP;
            while start < chips.len() {
                let mut end = start;
                let mut used = 0.0;
                while end < chips.len() {
                    let next = used + widths[end] + if end > start { SystemChip::GAP } else { 0.0 };
                    if end > start && next > available {
                        break;
                    }
                    used = next;
                    end += 1;
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SystemChip::GAP;
                    for (id, chip) in &chips[start..end] {
                        if chip.show(ui, &theme, &palette) {
                            removed = Some(*id);
                        }
                    }
                });
                start = end;
            }
        });
        if let Some(id) = removed {
            self.settings.unsilence_system(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sde::objects::{Region, SolarSystem};

    /// A universe with the given `(id, name)` systems, all in region 1
    /// ("The Forge").
    impl SystemIndex {
        fn of(systems: &[(u32, &str)]) -> (Self, Universe) {
            let mut universe = Universe::new(1.0);
            let mut region = Region::new();
            region.id = 1;
            region.name = String::from("The Forge");
            universe.regions.insert(1, region);
            for (id, name) in systems {
                let mut system = SolarSystem::new(1.0);
                system.id = *id;
                system.name = (*name).to_owned();
                system.region = 1;
                universe.solar_systems.insert(*id, system);
            }
            let mut index = Self::default();
            index.refresh(&universe);
            (index, universe)
        }
    }

    fn names(suggestions: &[Suggestion]) -> Vec<&str> {
        suggestions
            .iter()
            .map(|suggestion| suggestion.insert.as_str())
            .collect()
    }

    #[test]
    fn a_prefix_finds_the_systems_that_start_with_it_in_any_case() {
        let (index, universe) = SystemIndex::of(&[
            (1, "Jita"),
            (2, "Jite"),
            (3, "Perimeter"),
            (4, "Tash-Murkon Prime"),
        ]);
        assert_eq!(
            names(&index.suggestions("ji", &[], &universe)),
            ["Jita", "Jite"]
        );
        assert_eq!(names(&index.suggestions("JITA", &[], &universe)), ["Jita"]);
        assert_eq!(
            names(&index.suggestions("tash-murkon p", &[], &universe)),
            ["Tash-Murkon Prime"],
            "a name with spaces is one word"
        );
        assert!(index.suggestions("zz", &[], &universe).is_empty());
    }

    #[test]
    fn nothing_is_suggested_for_less_than_two_letters() {
        let (index, universe) = SystemIndex::of(&[(1, "Jita")]);
        assert!(index.suggestions("", &[], &universe).is_empty());
        assert!(index.suggestions("j", &[], &universe).is_empty());
        assert!(index.suggestions("  ", &[], &universe).is_empty());
    }

    #[test]
    fn a_silenced_system_is_not_suggested_again() {
        let (index, universe) = SystemIndex::of(&[(1, "Jita"), (2, "Jite")]);
        assert_eq!(names(&index.suggestions("ji", &[1], &universe)), ["Jite"]);
    }

    #[test]
    fn the_popup_lists_only_a_few_suggestions() {
        let systems: Vec<(u32, String)> = (1..=20).map(|id| (id, format!("Aa-{id:03}"))).collect();
        let borrowed: Vec<(u32, &str)> = systems
            .iter()
            .map(|(id, name)| (*id, name.as_str()))
            .collect();
        let (index, universe) = SystemIndex::of(&borrowed);
        let suggestions = index.suggestions("aa", &[], &universe);
        assert_eq!(suggestions.len(), SUGGESTIONS);
    }

    #[test]
    fn a_system_is_found_by_its_exact_name_in_any_case() {
        let (index, _) = SystemIndex::of(&[(30_000_142, "Jita"), (30_002_187, "Amarr")]);
        assert_eq!(index.find("Jita"), Some(30_000_142));
        assert_eq!(index.find("  amarr "), Some(30_002_187));
        assert_eq!(index.find("Jit"), None);
    }

    #[test]
    fn the_index_is_built_again_when_the_universe_changes() {
        let (mut index, mut universe) = SystemIndex::of(&[(1, "Jita")]);
        let mut system = SolarSystem::new(1.0);
        system.id = 2;
        system.name = String::from("Jite");
        universe.solar_systems.insert(2, system);
        assert_eq!(index.find("Jite"), None);
        index.refresh(&universe);
        assert_eq!(index.find("Jite"), Some(2));
    }

    #[test]
    fn a_tag_names_the_system_and_its_region_or_falls_back_to_the_id() {
        let (_, universe) = SystemIndex::of(&[(30_000_142, "Jita")]);
        let chip = SystemChip::of(30_000_142, &universe);
        assert_eq!(chip.name, "Jita");
        assert_eq!(chip.region_text().as_deref(), Some("· The Forge"));
        let unknown = SystemChip::of(7, &universe);
        assert_eq!(unknown.name, "#7");
        assert_eq!(unknown.region_text(), None);
    }
}
