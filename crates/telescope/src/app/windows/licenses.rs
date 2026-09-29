//! The "Third-party licenses" window (Help menu): every crate Telescope is
//! built with and the other works it ships (fonts, EVE Online's static data),
//! with their license and its text.
//!
//! The crates come from `build.rs`, which lists them with `cargo metadata`
//! and reads the license files of their packages into `$OUT_DIR/licenses.rs`,
//! so the list always matches `Cargo.lock`.

use crate::app::TelescopeApp;
use eframe::egui::{self, RichText};
use egui_panels::{StatusKind, Variant};

/// A crate of the build, written by `build.rs`.
pub(crate) struct LicensedPackage {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
    /// SPDX expression, e.g. `MIT OR Apache-2.0`.
    pub(crate) license: &'static str,
    pub(crate) repository: &'static str,
    /// Index of its license files in `TEXTS`.
    pub(crate) text: usize,
    /// The package ships no license file: `text` is the standard text of
    /// its licenses (`licenses/`, from the SPDX license list).
    pub(crate) standard: bool,
}

mod generated {
    use super::LicensedPackage;
    include!(concat!(env!("OUT_DIR"), "/licenses.rs"));
}

/// The SIL Open Font License, as shipped with Noto Sans CJK.
const OFL: &str = include_str!("../../../../../assets/NotoSansCJK-LICENSE.txt");

/// The proprietary notice CCP's Developer License Agreement (section 7.1,
/// "Proprietary Notices") requires for EVE's game data, word for word, and
/// where the agreement is. The notice's year is CCP's own.
const CCP_NOTICE: &str = "© 2014 CCP hf. All rights reserved. \"EVE\", \"EVE Online\", \"CCP\", \
and all related logos and images are trademarks or registered trademarks of CCP hf.\n\n\
EVE Online's static data export (SDE) and the data read through ESI are used under CCP's \
Developer License Agreement: https://developers.eveonline.com/license-agreement";

/// One work listed in the window.
struct Entry {
    name: String,
    version: String,
    license: String,
    repository: String,
    text: String,
    /// Shipped with Telescope (fonts, data) rather than a crate.
    bundled: bool,
    /// See [`LicensedPackage::standard`].
    standard: bool,
}

/// Width of the list at the left.
const LIST_WIDTH: f32 = 290.0;

/// State of the window while it is open.
pub(crate) struct LicensesWindow {
    entries: Vec<Entry>,
    filter: String,
    selected: usize,
}

impl Default for LicensesWindow {
    fn default() -> Self {
        let mut entries = vec![
            Entry {
                name: String::from("EVE Online static data"),
                version: String::new(),
                license: String::from("CCP Developer License Agreement"),
                repository: String::from("https://developers.eveonline.com/license-agreement"),
                text: CCP_NOTICE.to_owned(),
                bundled: true,
                standard: false,
            },
            Entry {
                name: String::from("Noto Sans CJK"),
                version: String::new(),
                license: String::from("OFL-1.1"),
                repository: String::from("https://github.com/notofonts/noto-cjk"),
                text: format!("© 2014-2021 Adobe (http://www.adobe.com/).\n\n{OFL}"),
                bundled: true,
                standard: false,
            },
            Entry {
                name: String::from("Fira Sans"),
                version: String::new(),
                license: String::from("OFL-1.1"),
                repository: String::from("https://github.com/mozilla/Fira"),
                text: format!(
                    "Digitized data copyright 2012-2016, The Mozilla Foundation and Telefonica S.A.\n\n{OFL}"
                ),
                bundled: true,
                standard: false,
            },
        ];
        entries.extend(generated::PACKAGES.iter().map(|package| {
            Entry {
                name: package.name.to_owned(),
                version: package.version.to_owned(),
                license: package.license.to_owned(),
                repository: package.repository.to_owned(),
                text: generated::TEXTS
                    .get(package.text)
                    .copied()
                    .unwrap_or_default()
                    .to_owned(),
                bundled: false,
                standard: package.standard,
            }
        }));
        Self {
            entries,
            filter: String::new(),
            selected: 0,
        }
    }
}

impl LicensesWindow {
    /// The entries whose name or license contains the filter.
    fn visible(&self) -> Vec<usize> {
        let filter = self.filter.trim().to_lowercase();
        (0..self.entries.len())
            .filter(|index| {
                let entry = &self.entries[*index];
                filter.is_empty()
                    || entry.name.to_lowercase().contains(&filter)
                    || entry.license.to_lowercase().contains(&filter)
            })
            .collect()
    }

    fn show_list(&mut self, ui: &mut egui::Ui) {
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(t!("licenses.search"))
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, theme.control_height)),
        );
        let visible = self.visible();
        egui::ScrollArea::vertical()
            .id_salt("licenses_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for index in visible {
                    let entry = &self.entries[index];
                    let selected = index == self.selected;
                    let fill = if selected {
                        palette.card_fill_selected
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let response = egui::Frame::new()
                        .fill(fill)
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&entry.name).color(palette.strong_text));
                                if !entry.version.is_empty() {
                                    ui.label(
                                        RichText::new(&entry.version)
                                            .size(theme.small_size)
                                            .color(palette.muted_text),
                                    );
                                }
                            });
                            ui.label(
                                RichText::new(if entry.license.is_empty() {
                                    t!("licenses.unknown").into_owned()
                                } else {
                                    entry.license.clone()
                                })
                                .size(theme.small_size)
                                .color(palette.muted_text),
                            );
                        })
                        .response
                        .interact(egui::Sense::click());
                    if response.hovered() && !selected {
                        ui.painter().rect_stroke(
                            response.rect,
                            4,
                            egui::Stroke::new(1.0, palette.card_stroke),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if response.clicked() {
                        self.selected = index;
                    }
                }
            });
    }

    fn show_details(&self, ui: &mut egui::Ui) {
        let theme = egui_panels::Theme::get(ui.ctx());
        let palette = theme.palette(ui.visuals());
        let Some(entry) = self.entries.get(self.selected) else {
            return;
        };
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&entry.name)
                        .size(theme.section_title_size + 2.0)
                        .color(palette.strong_text),
                );
                if !entry.version.is_empty() {
                    egui_panels::badge(ui, &entry.version);
                }
                if entry.bundled {
                    egui_panels::badge(ui, &t!("licenses.bundled"));
                }
            });
            ui.horizontal_wrapped(|ui| {
                if !entry.license.is_empty() {
                    egui_panels::chip(ui, &entry.license, None);
                }
                if !entry.repository.is_empty() && ui.link(&entry.repository).clicked() {
                    let _ = open::that(&entry.repository);
                }
            });
        });
        ui.add_space(theme.section_spacing);
        if entry.standard && !entry.text.is_empty() {
            egui_panels::status(ui, StatusKind::Info, &t!("licenses.standard_text"));
        }
        if entry.text.is_empty() {
            egui_panels::status(
                ui,
                StatusKind::Info,
                &t!("licenses.no_text", license = entry.license),
            );
            return;
        }
        egui::Frame::new()
            .fill(palette.card_fill)
            .stroke(egui::Stroke::new(1.0, palette.card_stroke))
            .corner_radius(egui::CornerRadius::same(theme.radius))
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("licenses_text", self.selected))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&entry.text)
                                    .monospace()
                                    .size(theme.small_size)
                                    .color(palette.text),
                            )
                            .wrap(),
                        );
                    });
            });
    }
}

impl TelescopeApp {
    /// Opens the "Third-party licenses" window.
    pub(crate) fn open_licenses(&mut self) {
        if self.licenses.is_none() {
            self.licenses = Some(LicensesWindow::default());
        }
    }

    /// Draws the "Third-party licenses" window while it is open.
    #[tracing::instrument(skip(self, ctx))]
    pub(crate) fn show_licenses_window(&mut self, ctx: &egui::Context) {
        let Some(window) = self.licenses.as_mut() else {
            return;
        };
        let mut open = true;
        let mut close = false;
        egui::Window::new(t!("licenses.title"))
            .id(egui::Id::new("licenses_window"))
            .open(&mut open)
            .default_size([860.0, 560.0])
            .min_size([600.0, 360.0])
            .collapsible(false)
            .show(ctx, |ui| {
                let theme = egui_panels::Theme::get(ui.ctx());
                let palette = theme.palette(ui.visuals());
                ui.spacing_mut().item_spacing.y = theme.section_spacing;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t!("licenses.description", count = window.entries.len()))
                            .color(palette.muted_text),
                    );
                });
                let height = ui.available_height() - theme.control_height - theme.section_spacing;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.allocate_ui_with_layout(
                        egui::vec2(LIST_WIDTH, height),
                        egui::Layout::top_down_justified(egui::Align::Min),
                        |ui| {
                            ui.set_min_height(height);
                            window.show_list(ui);
                        },
                    );
                    ui.separator();
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_height(height);
                            window.show_details(ui);
                        },
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if egui_panels::button(ui, t!("licenses.close"), Variant::Secondary).clicked() {
                        close = true;
                    }
                });
            });
        if !open || close {
            self.licenses = None;
        }
    }
}
