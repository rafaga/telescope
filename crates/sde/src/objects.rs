use egui_map::map::objects::RawPoint;
use std::collections::HashMap;
use std::convert::{From, TryInto};
use std::io::{Error as GenericError, ErrorKind};
use std::ops::{Add, Div, DivAssign, Mul, MulAssign, Sub};

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct EveRegionArea {
    pub region_id: i64,
    pub name: String,
    pub min: SdePoint,
    pub max: SdePoint,
}

impl Default for EveRegionArea {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

impl EveRegionArea {
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        EveRegionArea {
            region_id: 0,
            name: String::new(),
            min: SdePoint::default(),
            max: SdePoint::default(),
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct SdeLine {
    points: [SdePoint; 2],
}

impl SdeLine {
    pub fn new(a: SdePoint, b: SdePoint) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self { points: [a, b] }
    }

    pub fn distance(self) -> f32 {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let x = self.points[0].x - self.points[1].x;
        let y = self.points[0].y - self.points[1].y;
        let z = self.points[0].z - self.points[1].z;
        let value = (x.pow(2) + y.pow(2) + z.pow(2)) as f32;
        value.sqrt()
    }

    pub fn midpoint(self) -> SdePoint {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let x = (self.points[0].x + self.points[1].x) / 2;
        let y = (self.points[0].y + self.points[1].y) / 2;
        let z = (self.points[0].z + self.points[1].z) / 2;
        SdePoint::new(x, y, z)
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
// This can by any object or point with its associated metadata
/// Struct that contains coordinates to help calculate nearest point in space
/// 3d point coordinates that it is used in:
///
/// - SolarSystems
pub struct SdePoint {
    /// X coorddinate
    pub x: i64,
    /// Y coordinate
    pub y: i64,
    /// Z coordinate
    pub z: i64,
}

impl SdePoint {
    /// Creates a new Coordinates struct. ALl the coordinates are initialized.
    pub fn new(x: i64, y: i64, z: i64) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SdePoint { x, y, z }
    }

    pub fn to_rawpoint(self) -> RawPoint {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        RawPoint::new(self.x as f32, self.z as f32)
    }
}

impl Default for SdePoint {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new(0, 0, 0)
    }
}

impl From<[i64; 3]> for SdePoint {
    fn from(value: [i64; 3]) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self {
            x: value[0],
            y: value[1],
            z: value[2],
        }
    }
}

impl From<SdePoint> for [i64; 3] {
    fn from(val: SdePoint) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        [val.x, val.y, val.z]
    }
}

impl From<SdePoint> for [f64; 3] {
    fn from(val: SdePoint) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        [val.x as f64, val.y as f64, val.z as f64]
    }
}

impl TryInto<[f32; 2]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[f32; 2], <Self as TryInto<[f32; 2]>>::Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if self.x == 0 {
            Ok([self.y as f32, self.z as f32])
        } else if self.y == 0 {
            Ok([self.x as f32, self.z as f32])
        } else if self.z == 0 {
            Ok([self.x as f32, self.y as f32])
        } else {
            Err(GenericError::new(
                ErrorKind::NotFound,
                "projection pivot value not found, it is not possible to determine wich values to return.",
            ))
        }
    }
}

impl TryInto<[f32; 3]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[f32; 3], <Self as TryInto<[f32; 3]>>::Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if self.x > f32::MAX as i64
            || self.x < f32::MIN as i64
            || self.y > f32::MAX as i64
            || self.y < f32::MIN as i64
            || self.z > f32::MAX as i64
            || self.z < f32::MIN as i64
        {
            return Err(GenericError::new(ErrorKind::InvalidData, "Value Overflow"));
        }
        Ok([self.x as f32, self.y as f32, self.z as f32])
    }
}

impl TryInto<[i64; 2]> for SdePoint {
    type Error = GenericError;

    fn try_into(self) -> Result<[i64; 2], <Self as TryInto<[i64; 2]>>::Error> {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        if self.x > f32::MAX as i64
            || self.x < f32::MIN as i64
            || self.y > f32::MAX as i64
            || self.y < f32::MIN as i64
            || self.z > f32::MAX as i64
            || self.z < f32::MIN as i64
        {
            return Err(GenericError::new(ErrorKind::InvalidData, "Value Overflow"));
        }
        if self.x == 0 {
            Ok([self.y, self.z])
        } else if self.y == 0 {
            Ok([self.x, self.z])
        } else if self.z == 0 {
            Ok([self.x, self.y])
        } else {
            Err(GenericError::new(
                ErrorKind::NotFound,
                "projection pivot value not found, it is not possible to determine wich values to return.",
            ))
        }
    }
}

impl From<[f32; 3]> for SdePoint {
    fn from(value: [f32; 3]) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self {
            x: value[0].round() as i64,
            y: value[1].round() as i64,
            z: value[2].round() as i64,
        }
    }
}

impl DivAssign<isize> for SdePoint {
    fn div_assign(&mut self, rhs: isize) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<u64> for SdePoint {
    fn div_assign(&mut self, rhs: u64) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<i64> for SdePoint {
    fn div_assign(&mut self, rhs: i64) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x / rhs;
        self.y = self.y / rhs;
        self.z = self.z / rhs;
    }
}

impl DivAssign<i32> for SdePoint {
    fn div_assign(&mut self, rhs: i32) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x / rhs as i64;
        self.y = self.y / rhs as i64;
        self.z = self.z / rhs as i64;
    }
}

impl DivAssign<f32> for SdePoint {
    fn div_assign(&mut self, rhs: f32) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x / rhs.round() as i64;
        self.y = self.y / rhs.round() as i64;
        self.z = self.z / rhs.round() as i64;
    }
}

impl MulAssign<isize> for SdePoint {
    fn mul_assign(&mut self, rhs: isize) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<u64> for SdePoint {
    fn mul_assign(&mut self, rhs: u64) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<i64> for SdePoint {
    fn mul_assign(&mut self, rhs: i64) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x * rhs;
        self.y = self.y * rhs;
        self.z = self.z * rhs;
    }
}

impl MulAssign<i32> for SdePoint {
    fn mul_assign(&mut self, rhs: i32) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x * rhs as i64;
        self.y = self.y * rhs as i64;
        self.z = self.z * rhs as i64;
    }
}

impl MulAssign<f32> for SdePoint {
    fn mul_assign(&mut self, rhs: f32) {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        self.x = self.x * rhs.round() as i64;
        self.y = self.y * rhs.round() as i64;
        self.z = self.z * rhs.round() as i64;
    }
}

impl Mul<isize> for SdePoint {
    type Output = Self;
    fn mul(self, rhs: isize) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self {
            x: self.x * rhs as i64,
            y: self.y * rhs as i64,
            z: self.z * rhs as i64,
        }
    }
}

impl Div<isize> for SdePoint {
    type Output = Self;
    fn div(self, rhs: isize) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self {
            x: self.x / rhs as i64,
            y: self.y / rhs as i64,
            z: self.z / rhs as i64,
        }
    }
}

impl Add<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: SdePoint) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SdePoint {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub<SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: SdePoint) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SdePoint {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl Add<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn add(self, rhs: &SdePoint) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SdePoint {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub<&SdePoint> for SdePoint {
    type Output = SdePoint;
    fn sub(self, rhs: &SdePoint) -> Self::Output {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SdePoint {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

/// Abstraction for a Planet Moons. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Moon {
    /// Moon Identifier
    pub id: u32,
    /// Moon's Planet identifier
    pub planet: u32,
    /// The cardinal number of this moon in the planet
    pub index: u8,
    /// Moon's Solar System Identifier
    pub solar_system: u32,
}

impl Moon {
    /// Creates a new Moon Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Moon {
            id: 0,
            planet: 0,
            index: 0,
            solar_system: 0,
        }
    }
}

impl Default for Moon {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

/// Abstraction for a Planet. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Planet {
    /// Planet identifier
    pub id: u32,
    /// Planet's Solar System Idetifier
    pub solar_system: u32,
    /// The cardinal number of this planet in the solar system.
    pub index: u8,
}

impl Planet {
    /// Creates a new Planet Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Planet {
            id: 0,
            solar_system: 0,
            index: 0,
        }
    }
}

impl Default for Planet {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

/// Abstraction for a Solar System. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct SolarSystem {
    /// Solar System identifier
    pub id: u32,
    /// Solar System name
    pub name: String,
    /// Region identifier
    pub region: u32,
    /// Constellation identifier
    pub constellation: u32,
    /// Planet vector with Identifer numbers in their respective cardinal order
    pub planets: Vec<u32>,
    /// Vector with Solar system identifiers where this Solar system has connections via Stargates
    pub connections: Vec<u32>,
    /// Solar System 3D Coordinates
    pub real_coords: SdePoint,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
    /// The factor that we need to adjust the coordinates
    pub factor: i64,
}

impl SolarSystem {
    /// Creates a new Solar System Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: i64) -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        SolarSystem {
            id: 0,
            name: String::new(),
            region: 0,
            constellation: 0,
            planets: Vec::new(),
            connections: Vec::new(),
            real_coords: SdePoint::default(),
            projected_coords: SdePoint::default(),
            factor,
        }
    }

    /// this function that correct the original 2d coordinates using the correction factor
    pub fn coord2d_to_f64(self) -> [f64; 2] {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        [
            (self.projected_coords.x / self.factor) as f64,
            (self.real_coords.y / self.factor) as f64,
        ]
    }

    /// this function that correct the original 3d coordinates using the correction factor
    pub fn coord3d_to_f64(self) -> [f64; 3] {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        [
            (self.projected_coords.x / self.factor) as f64,
            (self.real_coords.y / self.factor) as f64,
            (self.real_coords.z / self.factor) as f64,
        ]
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new(1)
    }
}

/// Abstraction for a Constellation. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Constellation {
    /// Constellation Identifier
    pub id: u32,
    /// Constellation Name
    pub name: String,
    /// Region Identifier
    pub region: u32,
    /// Solar System vector with Identifer numbers included in the constellation
    pub solar_systems: Vec<u32>,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
}

impl Constellation {
    /// Creates a new Constellation Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Constellation {
            id: 0,
            name: String::new(),
            region: 0,
            solar_systems: Vec::new(),
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Constellation {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

/// Abstraction for a Region. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Region {
    /// Region Identifier
    pub id: u32,
    /// Region Name
    pub name: String,
    /// Vector with Region's Constellationm Identifiers
    pub constellations: Vec<u32>,
    /// Region 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
}

impl Region {
    /// Creates a new Region Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Region {
            id: 0,
            name: String::new(),
            constellations: Vec::new(),
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new()
    }
}

#[derive(Clone)]
/// Struct that contains everything in EVE Onoline Universe
///
/// - Regions
/// - Constellations
/// - SolarSystems
/// - Planets
/// - Moons
/// - and the object dictionaries
pub struct Universe {
    /// Region objects you can access the data with their Identfiers
    pub regions: HashMap<u32, Region>,
    /// Constellation objects you can access the data with their Identfiers
    pub constellations: HashMap<u32, Constellation>,
    /// Solarsystem objects you can access the data with their Identfiers
    pub solar_systems: HashMap<u32, SolarSystem>,
    /// Planet objects you can access the data with their Identfiers
    pub planets: HashMap<u32, Planet>,
    /// Moon objects you can access the data with their Identfiers
    pub moons: HashMap<u32, Moon>,
    /// Factor used to correct coordinates
    pub factor: i64,
    /// List of system connections
    pub connections: HashMap<String, SdeLine>,
}

impl Universe {
    /// Creates a new Universe Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: i64) -> Universe {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Universe {
            regions: HashMap::new(),
            constellations: HashMap::new(),
            solar_systems: HashMap::new(),
            planets: HashMap::new(),
            moons: HashMap::new(),
            factor,
            connections: HashMap::new(),
        }
    }
}

impl Default for Universe {
    fn default() -> Self {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // SdePoint
    // ---------------------------------------------------------------------

    #[test]
    fn sdepoint_new_sets_coordinates() {
        let point = SdePoint::new(10, -20, 30);
        assert_eq!(point.x, 10);
        assert_eq!(point.y, -20);
        assert_eq!(point.z, 30);
    }

    #[test]
    fn sdepoint_default_is_origin() {
        let point = SdePoint::default();
        assert_eq!(point.x, 0);
        assert_eq!(point.y, 0);
        assert_eq!(point.z, 0);
        assert_eq!(point, SdePoint::new(0, 0, 0));
    }

    #[test]
    fn sdepoint_from_i64_array() {
        let point = SdePoint::from([1, 2, 3]);
        assert_eq!(point, SdePoint::new(1, 2, 3));
    }

    #[test]
    fn sdepoint_from_f32_array_rounds_values() {
        let point = SdePoint::from([1.4, 1.5, -1.5]);
        // f32::round rounds half away from zero
        assert_eq!(point, SdePoint::new(1, 2, -2));
    }

    #[test]
    fn sdepoint_into_i64_array() {
        let values: [i64; 3] = SdePoint::new(7, 8, 9).into();
        assert_eq!(values, [7, 8, 9]);
    }

    #[test]
    fn sdepoint_into_f64_array() {
        let values: [f64; 3] = SdePoint::new(7, 8, 9).into();
        assert_eq!(values, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn sdepoint_to_rawpoint_uses_x_and_z() {
        let raw = SdePoint::new(11, 22, 33).to_rawpoint();
        assert_eq!(raw.components, [11.0, 33.0]);
    }

    #[test]
    fn sdepoint_try_into_f32_pair_pivot_on_x() {
        let result: [f32; 2] = SdePoint::new(0, 20, 30).try_into().unwrap();
        assert_eq!(result, [20.0, 30.0]);
    }

    #[test]
    fn sdepoint_try_into_f32_pair_pivot_on_y() {
        let result: [f32; 2] = SdePoint::new(10, 0, 30).try_into().unwrap();
        assert_eq!(result, [10.0, 30.0]);
    }

    #[test]
    fn sdepoint_try_into_f32_pair_pivot_on_z() {
        let result: [f32; 2] = SdePoint::new(10, 20, 0).try_into().unwrap();
        assert_eq!(result, [10.0, 20.0]);
    }

    #[test]
    fn sdepoint_try_into_f32_pair_fails_without_pivot() {
        let result: Result<[f32; 2], GenericError> = SdePoint::new(10, 20, 30).try_into();
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn sdepoint_try_into_f32_trio_converts_values() {
        let result: [f32; 3] = SdePoint::new(10, -20, 30).try_into().unwrap();
        assert_eq!(result, [10.0, -20.0, 30.0]);
    }

    #[test]
    fn sdepoint_try_into_f32_trio_never_overflows() {
        // The guard compares against `f32::MAX as i64`, and float-to-int casts
        // saturate in Rust, so the check can never trigger: even the most
        // extreme i64 values convert successfully.
        let result: Result<[f32; 3], GenericError> =
            SdePoint::new(i64::MAX, i64::MIN, 0).try_into();
        assert!(result.is_ok());
    }

    #[test]
    fn sdepoint_try_into_i64_pair_pivot_on_x() {
        let result: [i64; 2] = SdePoint::new(0, 20, 30).try_into().unwrap();
        assert_eq!(result, [20, 30]);
    }

    #[test]
    fn sdepoint_try_into_i64_pair_pivot_on_y() {
        let result: [i64; 2] = SdePoint::new(10, 0, 30).try_into().unwrap();
        assert_eq!(result, [10, 30]);
    }

    #[test]
    fn sdepoint_try_into_i64_pair_pivot_on_z() {
        let result: [i64; 2] = SdePoint::new(10, 20, 0).try_into().unwrap();
        assert_eq!(result, [10, 20]);
    }

    #[test]
    fn sdepoint_try_into_i64_pair_fails_without_pivot() {
        let result: Result<[i64; 2], GenericError> = SdePoint::new(10, 20, 30).try_into();
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn sdepoint_add_owned() {
        let sum = SdePoint::new(1, 2, 3) + SdePoint::new(10, 20, 30);
        assert_eq!(sum, SdePoint::new(11, 22, 33));
    }

    #[test]
    fn sdepoint_add_reference() {
        let sum = SdePoint::new(1, 2, 3) + &SdePoint::new(-1, -2, -3);
        assert_eq!(sum, SdePoint::new(0, 0, 0));
    }

    #[test]
    fn sdepoint_sub_owned() {
        let diff = SdePoint::new(10, 20, 30) - SdePoint::new(1, 2, 3);
        assert_eq!(diff, SdePoint::new(9, 18, 27));
    }

    #[test]
    fn sdepoint_sub_reference() {
        let diff = SdePoint::new(10, 20, 30) - &SdePoint::new(10, 20, 30);
        assert_eq!(diff, SdePoint::new(0, 0, 0));
    }

    #[test]
    fn sdepoint_mul_isize() {
        let product = SdePoint::new(1, -2, 3) * 3isize;
        assert_eq!(product, SdePoint::new(3, -6, 9));
    }

    #[test]
    fn sdepoint_div_isize_truncates() {
        let quotient = SdePoint::new(7, -7, 10) / 2isize;
        assert_eq!(quotient, SdePoint::new(3, -3, 5));
    }

    #[test]
    fn sdepoint_mul_assign_variants() {
        let mut point = SdePoint::new(1, 2, 3);
        point *= 2isize;
        assert_eq!(point, SdePoint::new(2, 4, 6));
        point *= 2u64;
        assert_eq!(point, SdePoint::new(4, 8, 12));
        point *= -1i64;
        assert_eq!(point, SdePoint::new(-4, -8, -12));
        point *= -1i32;
        assert_eq!(point, SdePoint::new(4, 8, 12));
    }

    #[test]
    fn sdepoint_mul_assign_f32_rounds_factor() {
        let mut point = SdePoint::new(1, 1, 1);
        point *= 2.5f32; // rounds to 3
        assert_eq!(point, SdePoint::new(3, 3, 3));
    }

    #[test]
    fn sdepoint_div_assign_variants() {
        let mut point = SdePoint::new(24, 48, 96);
        point /= 2isize;
        assert_eq!(point, SdePoint::new(12, 24, 48));
        point /= 2u64;
        assert_eq!(point, SdePoint::new(6, 12, 24));
        point /= 2i64;
        assert_eq!(point, SdePoint::new(3, 6, 12));
        point /= 3i32;
        assert_eq!(point, SdePoint::new(1, 2, 4));
    }

    #[test]
    fn sdepoint_div_assign_f32_rounds_divisor() {
        let mut point = SdePoint::new(10, 20, 30);
        point /= 2.4f32; // rounds to 2
        assert_eq!(point, SdePoint::new(5, 10, 15));
    }

    // ---------------------------------------------------------------------
    // SdeLine
    // ---------------------------------------------------------------------

    #[test]
    fn sdeline_distance_345_triangle() {
        let line = SdeLine::new(SdePoint::new(0, 0, 0), SdePoint::new(3, 4, 0));
        assert_eq!(line.distance(), 5.0);
    }

    #[test]
    fn sdeline_distance_zero_for_same_point() {
        let line = SdeLine::new(SdePoint::new(5, 5, 5), SdePoint::new(5, 5, 5));
        assert_eq!(line.distance(), 0.0);
    }

    #[test]
    fn sdeline_midpoint() {
        let line = SdeLine::new(SdePoint::new(0, 0, 0), SdePoint::new(4, 6, 8));
        assert_eq!(line.midpoint(), SdePoint::new(2, 3, 4));
    }

    #[test]
    fn sdeline_midpoint_truncates_odd_values() {
        let line = SdeLine::new(SdePoint::new(0, 0, 0), SdePoint::new(3, 3, 3));
        assert_eq!(line.midpoint(), SdePoint::new(1, 1, 1));
    }

    // ---------------------------------------------------------------------
    // EveRegionArea
    // ---------------------------------------------------------------------

    #[test]
    fn everegionarea_new_is_empty() {
        let area = EveRegionArea::new();
        assert_eq!(area.region_id, 0);
        assert_eq!(area.name, String::new());
        assert_eq!(area.min, SdePoint::default());
        assert_eq!(area.max, SdePoint::default());
        assert_eq!(area, EveRegionArea::default());
    }

    // ---------------------------------------------------------------------
    // Moon / Planet
    // ---------------------------------------------------------------------

    #[test]
    fn moon_new_is_zeroed() {
        let moon = Moon::new();
        assert_eq!(moon.id, 0);
        assert_eq!(moon.planet, 0);
        assert_eq!(moon.index, 0);
        assert_eq!(moon.solar_system, 0);
        assert_eq!(moon, Moon::default());
    }

    #[test]
    fn planet_new_is_zeroed() {
        let planet = Planet::new();
        assert_eq!(planet.id, 0);
        assert_eq!(planet.solar_system, 0);
        assert_eq!(planet.index, 0);
        assert_eq!(planet, Planet::default());
    }

    // ---------------------------------------------------------------------
    // SolarSystem
    // ---------------------------------------------------------------------

    #[test]
    fn solarsystem_new_initializes_with_factor() {
        let system = SolarSystem::new(1000);
        assert_eq!(system.id, 0);
        assert_eq!(system.name, String::new());
        assert_eq!(system.region, 0);
        assert_eq!(system.constellation, 0);
        assert!(system.planets.is_empty());
        assert!(system.connections.is_empty());
        assert_eq!(system.real_coords, SdePoint::default());
        assert_eq!(system.projected_coords, SdePoint::default());
        assert_eq!(system.factor, 1000);
    }

    #[test]
    fn solarsystem_default_factor_is_one() {
        assert_eq!(SolarSystem::default().factor, 1);
    }

    #[test]
    fn solarsystem_coord2d_divides_by_factor() {
        let mut system = SolarSystem::new(1000);
        system.projected_coords.x = 2000;
        system.real_coords.y = 4000;
        assert_eq!(system.coord2d_to_f64(), [2.0, 4.0]);
    }

    #[test]
    fn solarsystem_coord3d_divides_by_factor() {
        let mut system = SolarSystem::new(1000);
        system.projected_coords.x = 2000;
        system.real_coords.y = 4000;
        system.real_coords.z = 6000;
        assert_eq!(system.coord3d_to_f64(), [2.0, 4.0, 6.0]);
    }

    #[test]
    fn solarsystem_coords_use_integer_division() {
        // Coordinates are divided as integers before being cast to f64,
        // so any fractional part is truncated.
        let mut system = SolarSystem::new(1000);
        system.projected_coords.x = 1500;
        system.real_coords.y = 2500;
        system.real_coords.z = 3500;
        assert_eq!(system.clone().coord2d_to_f64(), [1.0, 2.0]);
        assert_eq!(system.coord3d_to_f64(), [1.0, 2.0, 3.0]);
    }

    // ---------------------------------------------------------------------
    // Constellation / Region / Universe
    // ---------------------------------------------------------------------

    #[test]
    fn constellation_new_is_empty() {
        let constellation = Constellation::new();
        assert_eq!(constellation.id, 0);
        assert_eq!(constellation.name, String::new());
        assert_eq!(constellation.region, 0);
        assert!(constellation.solar_systems.is_empty());
        assert_eq!(constellation.projected_coords, SdePoint::default());
        assert_eq!(constellation, Constellation::default());
    }

    #[test]
    fn region_new_is_empty() {
        let region = Region::new();
        assert_eq!(region.id, 0);
        assert_eq!(region.name, String::new());
        assert!(region.constellations.is_empty());
        assert_eq!(region.projected_coords, SdePoint::default());
        assert_eq!(region, Region::default());
    }

    #[test]
    fn universe_new_initializes_with_factor() {
        let universe = Universe::new(42);
        assert!(universe.regions.is_empty());
        assert!(universe.constellations.is_empty());
        assert!(universe.solar_systems.is_empty());
        assert!(universe.planets.is_empty());
        assert!(universe.moons.is_empty());
        assert!(universe.connections.is_empty());
        assert_eq!(universe.factor, 42);
    }

    #[test]
    fn universe_default_factor_is_one() {
        assert_eq!(Universe::default().factor, 1);
    }
}
