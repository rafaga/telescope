//! The header of a node's tooltip on the universe map: one line with the
//! name of the system, its security status and its region.
//!
//! The universe map no longer writes the name of the node under the pointer:
//! the tooltip says it, with what the map does not show. The three facts of
//! every system are read from `sde.db` once, when the map loads its data
//! (through the `sde` crate, which opens the database and closes it at once:
//! the SDE updater replaces the file while Telescope runs), so hovering reads
//! no file.

use egui::{Color32, RichText, Ui};
use sde::SdeManager;
use sde::objects::Universe;
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
}

impl SystemHeader {
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

    /// One line: the name, the security status and, beside it, the region.
    pub(crate) fn show(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(&self.name).strong());
            ui.label(
                RichText::new(self.security_text())
                    .color(self.security_color())
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
}

impl SystemHeaders {
    /// Reads the headers from the SDE at `path`, through the `sde` crate.
    /// Empty if the database can't be read (first run, or while the updater
    /// rebuilds it).
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(mut manager) = SdeManager::new(path, 1.0) else {
            return Self::default();
        };
        if manager.get_universe().is_err() {
            return Self::default();
        }
        Self::from_universe(&manager.universe)
    }

    /// The headers of the systems of `universe`; the region of a system the
    /// universe has no region for is left empty.
    fn from_universe(universe: &Universe) -> Self {
        let systems = universe
            .solar_systems
            .iter()
            .filter_map(|(id, system)| {
                let region = universe
                    .regions
                    .get(&system.region)
                    .map(|region| region.name.clone())
                    .unwrap_or_default();
                Some((
                    usize::try_from(*id).ok()?,
                    SystemHeader {
                        name: system.name.clone(),
                        region,
                        security: system.security,
                    },
                ))
            })
            .collect();
        Self { systems }
    }

    /// The header of `system_id`, if the SDE knows it.
    pub(crate) fn get(&self, system_id: usize) -> Option<&SystemHeader> {
        self.systems.get(&system_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sde::objects::{Region, SolarSystem};

    fn header(security: f64) -> SystemHeader {
        SystemHeader {
            name: String::from("Tama"),
            region: String::from("The Citadel"),
            security,
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
    fn a_database_that_cannot_be_read_gives_no_headers() {
        let headers = SystemHeaders::load(Path::new("/no/such/sde.db"));
        assert!(headers.get(30000142).is_none());
    }

    #[test]
    fn the_headers_come_from_the_systems_and_regions_of_the_universe() {
        let mut universe = Universe::new(1.0);
        universe.regions.insert(
            10000033,
            Region {
                id: 10000033,
                name: String::from("The Citadel"),
                ..Region::new()
            },
        );
        universe.solar_systems.insert(
            30002813,
            SolarSystem {
                id: 30002813,
                name: String::from("Tama"),
                region: 10000033,
                security: 0.4,
                ..SolarSystem::new(1.0)
            },
        );
        let headers = SystemHeaders::from_universe(&universe);
        assert_eq!(headers.get(30002813), Some(&header(0.4)));
        assert!(headers.get(1).is_none());
    }
}
