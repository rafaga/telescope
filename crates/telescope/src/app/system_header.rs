//! The header of a node's tooltip on the universe map: one line with the
//! name of the system, its security status and its region.
//!
//! The universe map no longer writes the name of the node under the pointer:
//! the tooltip says it, with what the map does not show. The three facts of
//! every system are read from `sde.db` once, when the map loads its data
//! (through the `sde` crate, which opens the database and closes it at once:
//! the SDE updater replaces the file while Telescope runs), so hovering reads
//! no file.

use super::wormhole_links::{Folded, Hub};
use egui::{Color32, RichText, Ui};
use sde::SdeManager;
use sde::objects::SystemSummary;
use std::collections::HashMap;
use std::path::Path;

/// What the header of a system says.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SystemHeader {
    /// The name of the system.
    pub name: String,
    /// The name of its region.
    pub region: String,
    /// The true security status the SDE gives (-1.0 to 1.0).
    pub security: f64,
    /// The wormhole class of the system (see `SolarSystem::wormhole_class`).
    pub wormhole_class: Option<u32>,
    /// Whether the status written beside the name is the security even though
    /// the system has a class: Thera, and only Thera (it keeps the color of
    /// its class).
    pub security_shown: bool,
}

impl SystemHeader {
    /// The green of the wormholes: their lines, and the systems of wormhole space
    /// that have no class of 1 to 6.
    pub(crate) const WORMHOLE_COLOR: Color32 = Color32::from_rgb(0x4d, 0xb6, 0xac);

    /// The class of a wormhole system as it is written, `C3`: classes 1 to 6,
    /// 12 (Thera) and 13 to 18. The 7, 8 and 9 of a region of k-space are
    /// not wormhole classes.
    pub(crate) fn class_text(&self) -> Option<String> {
        self.wormhole_class
            .filter(|class| matches!(class, 1..=6 | 12..=18))
            .map(|class| format!("C{class}"))
    }

    /// The color of a wormhole system: blue for C1 and C2, orange for C3 and
    /// C4, red for C5 and C6 (the harder the class, the warmer), and the green
    /// of the wormholes for the others (Thera, shattered, drifters). `None`
    /// for a system that is not of wormhole space.
    pub(crate) fn class_color(&self) -> Option<Color32> {
        self.class_text()?;
        Some(match self.wormhole_class? {
            1 | 2 => Color32::from_rgb(0x4a, 0x9e, 0xff),
            3 | 4 => Color32::from_rgb(0xf2, 0x9b, 0x2e),
            5 | 6 => Color32::from_rgb(0xe5, 0x48, 0x48),
            _ => Self::WORMHOLE_COLOR,
        })
    }

    /// What is written beside the name: the class of a wormhole system, the
    /// security status of any other, and of Thera.
    pub(crate) fn status_text(&self) -> String {
        if self.security_shown {
            return self.security_text();
        }
        self.class_text().unwrap_or_else(|| self.security_text())
    }

    /// The color of [`Self::status_text`].
    pub(crate) fn status_color(&self) -> Color32 {
        if self.security_shown {
            return self.security_color();
        }
        self.class_color().unwrap_or_else(|| self.security_color())
    }

    /// The security status as the game rounds it for display: one decimal,
    /// and never "-0.0".
    fn shown_security(&self) -> f64 {
        let rounded = (self.security * 10.0).round() / 10.0;
        if rounded.abs() < 0.05 { 0.0 } else { rounded }
    }

    /// The security status as it is written: `0.4`, `-0.7`.
    pub(crate) fn security_text(&self) -> String {
        format!("{:.1}", self.shown_security())
    }

    /// The color the game gives to the security status, from cyan at 1.0
    /// through yellow at 0.5 to red at 0.0 and below.
    pub(crate) fn security_color(&self) -> Color32 {
        let tenths = (self.shown_security() * 10.0).round().clamp(0.0, 10.0) as u8;
        let (red, green, blue) = match tenths {
            10 => (0x2F, 0xEF, 0xEF),
            9 => (0x48, 0xF0, 0xC0),
            8 => (0x00, 0xEF, 0x47),
            7 => (0x00, 0xF0, 0x00),
            6 => (0x8F, 0xEF, 0x2F),
            5 => (0xEF, 0xEF, 0x00),
            4 => (0xD7, 0x77, 0x00),
            3 => (0xF0, 0x60, 0x00),
            2 => (0xF0, 0x48, 0x00),
            1 => (0xD7, 0x30, 0x00),
            _ => (0xF0, 0x00, 0x00),
        };
        Color32::from_rgb(red, green, blue)
    }

    /// The kind of system it is, to count the wormholes that lead to it:
    /// its class (`C3`) in wormhole space, `HS`, `LS` or `NS` in k-space. The
    /// number puts the kinds in order: the classes first, then the three
    /// of k-space from the safest.
    pub(crate) fn kind(&self) -> (u32, String) {
        if let (Some(class), Some(text)) = (self.wormhole_class, self.class_text()) {
            return (class, text);
        }
        let rounded = self.shown_security();
        if rounded >= 0.5 {
            (100, String::from("HS"))
        } else if rounded > 0.0 {
            (101, String::from("LS"))
        } else {
            (102, String::from("NS"))
        }
    }

    /// The color of [`Self::kind`]: the one of the class, or the one of the
    /// security status of k-space.
    pub(crate) fn kind_color(&self) -> Color32 {
        self.class_color().unwrap_or_else(|| self.security_color())
    }

    /// One line: the name, the security status and, beside it, the region.
    pub(crate) fn show(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(&self.name).strong());
            ui.label(
                RichText::new(self.status_text())
                    .color(self.status_color())
                    .strong(),
            );
            ui.label(RichText::new(&self.region).weak());
        });
    }
}

/// The header of every system of the SDE.
#[derive(Default)]
pub(crate) struct SystemHeaders {
    systems: HashMap<usize, SystemHeader>,
    /// Nodes that are drawn for a system but have an id of their own (the
    /// nodes of the wormholes), by the id of the system they stand for.
    aliases: HashMap<usize, usize>,
    /// The nodes that stand for all the wormholes of a hub, by the number of
    /// wormholes. They write that number beside the name.
    folded: HashMap<usize, usize>,
}

impl SystemHeaders {
    /// Reads the headers from the SDE at `path`, through the `sde` crate.
    /// Empty if the database can't be read (first run, or while the updater
    /// rebuilds it).
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(manager) = SdeManager::new(path, 1.0) else {
            return Self::default();
        };
        match manager.get_system_summaries() {
            Ok(summaries) => Self::from_summaries(summaries),
            Err(_) => Self::default(),
        }
    }

    /// The headers of `summaries`, the systems that the SDE has with their region.
    fn from_summaries(summaries: HashMap<u32, SystemSummary>) -> Self {
        let systems = summaries
            .into_iter()
            .filter_map(|(id, summary)| {
                let id = usize::try_from(id).ok()?;
                Some((
                    id,
                    SystemHeader {
                        name: summary.name,
                        region: summary.region_name,
                        security: summary.security,
                        wormhole_class: summary.wormhole_class,
                        security_shown: id == Hub::THERA_SYSTEM,
                    },
                ))
            })
            .collect();
        Self {
            systems,
            aliases: HashMap::new(),
            folded: HashMap::new(),
        }
    }

    /// The header of `system_id`, if the SDE knows it. The id of a node that
    /// stands for a system (see [`Self::set_aliases`]) gets the header of
    /// that system.
    pub(crate) fn get(&self, system_id: usize) -> Option<&SystemHeader> {
        let system_id = self.aliases.get(&system_id).copied().unwrap_or(system_id);
        self.systems.get(&system_id)
    }

    /// The system that the node `id` stands for: itself unless it is the node
    /// of a wormhole (see [`Self::set_aliases`]).
    pub(crate) fn system_of(&self, id: usize) -> usize {
        self.aliases.get(&id).copied().unwrap_or(id)
    }

    /// What is written beside the name of the node `id`: the number of
    /// wormholes it gathers if it is a folded node, or the status of its
    /// system, with its color.
    pub(crate) fn status_of(&self, id: usize) -> Option<(String, Color32)> {
        if let Some(count) = self.folded.get(&id) {
            return Some((format!("×{count}"), SystemHeader::WORMHOLE_COLOR));
        }
        self.get(id)
            .map(|header| (header.status_text(), header.status_color()))
    }

    /// How many wormholes the node `id` gathers, if it is a folded one.
    pub(crate) fn folded_count(&self, id: usize) -> Option<usize> {
        self.folded.get(&id).copied()
    }

    /// Replaces the folded nodes: `(node id, number of wormholes)`.
    pub(crate) fn set_folded(&mut self, folded: impl IntoIterator<Item = (usize, usize)>) {
        self.folded = folded.into_iter().collect();
    }

    /// The wormholes of a folded node by the kind of system on the other
    /// side, in order: `(kind, color, how many)`. A system the SDE does not
    /// know counts as `?`, last.
    pub(crate) fn kinds_of(&self, wormholes: &[Folded]) -> Vec<(String, Color32, usize)> {
        let mut kinds: Vec<((u32, String), Color32, usize)> = Vec::new();
        for wormhole in wormholes {
            let (kind, color) = match self.systems.get(&wormhole.system) {
                Some(header) => (header.kind(), header.kind_color()),
                None => ((u32::MAX, String::from("?")), Color32::GRAY),
            };
            match kinds.iter_mut().find(|(known, _, _)| *known == kind) {
                Some((_, _, count)) => *count += 1,
                None => kinds.push((kind, color, 1)),
            }
        }
        kinds.sort_by(|a, b| a.0.cmp(&b.0));
        kinds
            .into_iter()
            .map(|((_, text), color, count)| (text, color, count))
            .collect()
    }

    /// Replaces the nodes that stand for a system: `(node id, system id)`.
    pub(crate) fn set_aliases(&mut self, aliases: impl IntoIterator<Item = (usize, usize)>) {
        self.aliases = aliases.into_iter().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(security: f64) -> SystemHeader {
        SystemHeader {
            name: String::from("Tama"),
            region: String::from("The Citadel"),
            security,
            wormhole_class: None,
            security_shown: false,
        }
    }

    #[test]
    fn the_security_is_written_with_one_decimal_like_the_game() {
        assert_eq!(header(0.45).security_text(), "0.5");
        assert_eq!(header(0.3512).security_text(), "0.4");
        assert_eq!(header(1.0).security_text(), "1.0");
        assert_eq!(header(-0.7241).security_text(), "-0.7");
        assert_eq!(header(-0.01).security_text(), "0.0");
    }

    #[test]
    fn the_color_follows_the_rounded_security() {
        assert_eq!(
            header(1.0).security_color(),
            Color32::from_rgb(0x2F, 0xEF, 0xEF)
        );
        assert_eq!(
            header(0.5).security_color(),
            Color32::from_rgb(0xEF, 0xEF, 0x00)
        );
        assert_eq!(
            header(0.44).security_color(),
            Color32::from_rgb(0xD7, 0x77, 0x00)
        );
        assert_eq!(
            header(0.0).security_color(),
            Color32::from_rgb(0xF0, 0x00, 0x00)
        );
        assert_eq!(
            header(-0.9).security_color(),
            Color32::from_rgb(0xF0, 0x00, 0x00)
        );
    }

    #[test]
    fn a_wormhole_system_writes_its_class_instead_of_the_security() {
        let mut wormhole = header(-0.99);
        assert_eq!(wormhole.status_text(), "-1.0");
        wormhole.wormhole_class = Some(3);
        assert_eq!(wormhole.status_text(), "C3");
        assert_eq!(wormhole.status_color(), Color32::from_rgb(0xf2, 0x9b, 0x2e));
        wormhole.wormhole_class = Some(12);
        assert_eq!(wormhole.status_text(), "C12");
        assert_eq!(wormhole.status_color(), SystemHeader::WORMHOLE_COLOR);
        // The classes of k-space are not shown.
        wormhole.wormhole_class = Some(9);
        assert_eq!(wormhole.status_text(), "-1.0");
        assert_eq!(wormhole.status_color(), wormhole.security_color());
    }

    #[test]
    fn thera_writes_its_security_and_keeps_the_color_of_its_class() {
        let summary = |wormhole_class| SystemSummary {
            name: String::from("Thera"),
            region_id: 11_000_031,
            region_name: String::from("G-R00031"),
            security: -0.99,
            wormhole_class,
        };
        let headers = SystemHeaders::from_summaries(HashMap::from([
            (31_000_005, summary(Some(12))),
            (31_000_006, summary(Some(12))),
        ]));
        let thera = headers.get(31_000_005).unwrap();
        assert_eq!(thera.status_text(), "-1.0");
        assert_eq!(thera.status_color(), thera.security_color());
        assert_eq!(thera.class_color(), Some(SystemHeader::WORMHOLE_COLOR));
        // Any other system of wormhole space writes its class.
        assert_eq!(headers.get(31_000_006).unwrap().status_text(), "C12");
    }

    #[test]
    fn the_classes_are_blue_orange_and_red_by_pairs() {
        let color = |class| {
            let mut wormhole = header(-0.99);
            wormhole.wormhole_class = Some(class);
            wormhole.class_color()
        };
        assert_eq!(color(1), color(2));
        assert_eq!(color(3), color(4));
        assert_eq!(color(5), color(6));
        assert_ne!(color(2), color(3));
        assert_ne!(color(4), color(5));
        assert_eq!(color(1), Some(Color32::from_rgb(0x4a, 0x9e, 0xff)));
        assert_eq!(color(6), Some(Color32::from_rgb(0xe5, 0x48, 0x48)));
        assert_eq!(color(13), Some(SystemHeader::WORMHOLE_COLOR));
        assert_eq!(color(9), None);
    }

    #[test]
    fn a_node_that_stands_for_a_system_has_its_header() {
        let mut headers = SystemHeaders::default();
        assert_eq!(headers.system_of(900_000_001), 900_000_001);
        headers.systems.insert(30_000_142, header(0.9));
        assert!(headers.get(900_000_001).is_none());
        headers.set_aliases([(900_000_001, 30_000_142)]);
        assert_eq!(headers.get(900_000_001).unwrap().name, "Tama");
        assert_eq!(headers.system_of(900_000_001), 30_000_142);
        headers.set_aliases([]);
        assert!(headers.get(900_000_001).is_none());
    }

    #[test]
    fn a_database_that_cannot_be_read_gives_no_headers() {
        let headers = SystemHeaders::load(Path::new("/no/such/sde.db"));
        assert!(headers.get(30000142).is_none());
    }

    #[test]
    fn the_headers_come_from_the_summaries_of_the_systems() {
        let summary = |name: &str, region: &str, security, wormhole_class| SystemSummary {
            name: String::from(name),
            region_id: 1,
            region_name: String::from(region),
            security,
            wormhole_class,
        };
        let headers = SystemHeaders::from_summaries(HashMap::from([
            (30002813, summary("Tama", "The Citadel", 0.4, Some(7))),
            (31000100, summary("J214534", "A-R00001", -0.99, Some(1))),
        ]));
        let tama = headers.get(30002813).unwrap();
        assert_eq!(tama.name, "Tama");
        assert_eq!(tama.region, "The Citadel");
        assert_eq!(tama.security, 0.4);
        // The class of a region of k-space is not the one of a wormhole.
        assert_eq!(tama.class_text(), None);
        let wormhole = headers.get(31000100).unwrap();
        assert_eq!(wormhole.region, "A-R00001");
        assert_eq!(wormhole.class_text().as_deref(), Some("C1"));
        assert!(headers.get(1).is_none());
    }

    #[test]
    fn the_headers_of_the_real_sde_have_what_the_tooltips_say() {
        let Some(path) = std::env::var_os("SDE_TEST_DB") else {
            return;
        };
        let headers = SystemHeaders::load(Path::new(&path));
        let thera = headers.get(31_000_005).unwrap();
        assert_eq!(thera.region, "G-R00031");
        assert_eq!(thera.class_text().as_deref(), Some("C12"));
        assert_eq!(headers.get(30_000_142).unwrap().name, "Jita");
    }

    #[test]
    fn a_folded_node_writes_how_many_wormholes_it_gathers() {
        let mut headers = SystemHeaders::default();
        headers.systems.insert(30_002_086, header(0.4));
        headers.set_aliases([(900_000_001, 30_002_086)]);
        let (text, _) = headers.status_of(900_000_001).unwrap();
        assert_eq!(text, "0.4");
        headers.set_folded([(900_000_001, 17)]);
        assert_eq!(headers.folded_count(900_000_001), Some(17));
        let (text, color) = headers.status_of(900_000_001).unwrap();
        assert_eq!(text, "×17");
        assert_eq!(color, SystemHeader::WORMHOLE_COLOR);
        headers.set_folded([]);
        assert_eq!(headers.folded_count(900_000_001), None);
    }

    #[test]
    fn the_kind_of_a_system_is_its_class_or_the_band_of_its_security() {
        let mut system = header(0.9);
        assert_eq!(system.kind(), (100, String::from("HS")));
        system.security = 0.4;
        assert_eq!(system.kind(), (101, String::from("LS")));
        system.security = -0.3;
        assert_eq!(system.kind(), (102, String::from("NS")));
        system.wormhole_class = Some(4);
        assert_eq!(system.kind(), (4, String::from("C4")));
        assert_eq!(system.kind_color(), system.class_color().unwrap());
        system.wormhole_class = Some(9);
        assert_eq!(system.kind(), (102, String::from("NS")));
    }

    #[test]
    fn the_wormholes_of_a_folded_node_are_counted_by_kind_in_order() {
        let mut headers = SystemHeaders::default();
        let mut c3 = header(-0.99);
        c3.wormhole_class = Some(3);
        let mut c5 = header(-0.99);
        c5.wormhole_class = Some(5);
        headers.systems.insert(1, c3.clone());
        headers.systems.insert(2, c3);
        headers.systems.insert(3, c5);
        headers.systems.insert(4, header(0.9));
        let wormhole = |system| Folded {
            system,
            label: String::new(),
        };
        let kinds = headers.kinds_of(&[
            wormhole(4),
            wormhole(3),
            wormhole(1),
            wormhole(99),
            wormhole(2),
        ]);
        let texts: Vec<(&str, usize)> = kinds
            .iter()
            .map(|(text, _, count)| (text.as_str(), *count))
            .collect();
        assert_eq!(texts, vec![("C3", 2), ("C5", 1), ("HS", 1), ("?", 1)]);
    }

    #[test]
    fn the_folded_node_of_a_hub_has_the_header_of_the_hub() {
        use crate::app::settings::HubDisplay;
        use crate::app::wormhole_links::{
            LabelSpace, NodeSize, WormholeLayer, WormholeMarkerStyle,
        };
        use sde::links::{Link, LinkLayer, Wormhole};
        let summary = SystemSummary {
            name: String::from("Turnur"),
            region_id: 10_000_042,
            region_name: String::from("Metropolis"),
            security: 0.3,
            wormhole_class: None,
        };
        let mut headers = SystemHeaders::from_summaries(HashMap::from([(30_002_086, summary)]));
        let wormhole = Wormhole {
            out_signature: String::from("AAA-111"),
            in_signature: None,
            expires_at: None,
        };
        let mut links = LinkLayer::new();
        links.replace(vec![Link::wormhole(30_002_086, 31_000_100, wormhole)]);
        let mut layer = WormholeLayer::new(WormholeMarkerStyle::default());
        layer.set_display(HubDisplay::Collapsed, 6);
        let systems = HashMap::from([(30_002_086_usize, [0.0_f32, 0.0])]);
        let markers = layer.markers(
            &links,
            chrono::Utc::now(),
            &systems,
            &|_| None,
            NodeSize {
                width: 90.0,
                height: 35.0,
            },
            LabelSpace {
                measure: &|_| [0.0, 0.0],
            },
        );
        let id = markers[0].id;
        let mut map = egui_map::map::Map::new();
        layer.show(&mut map, markers, 1, std::time::Instant::now());
        headers.set_aliases(layer.aliases());
        let header = headers.get(id).expect("the folded node has a header");
        assert_eq!(header.name, "Turnur");
        assert_eq!(header.region, "Metropolis");
    }
}
