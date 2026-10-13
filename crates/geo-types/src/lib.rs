//! Explicit CRS, unit and schema value types.
//!
//! Invariants (CONSTITUTION 2, 3): there are no `Default` impls for CRS or
//! units, and quantities cannot be built without a unit.

mod access;
pub use access::{AccessDenied, AuthorizedSource, RootedAccessGuard, SourceAccessGuard};

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Errors from constructing or validating value types.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TypeError {
    #[error("value must be finite, got {0}")]
    NotFinite(f64),
    #[error("value must be non-negative, got {0}")]
    Negative(f64),
    #[error("invalid bounding box: {0}")]
    InvalidBBox(String),
    #[error("cannot convert angular unit (degree) to a linear measure without a CRS-aware transform")]
    AngularUnit,
    #[error("empty CRS string")]
    EmptyCrs,
    #[error("invalid schema: {0}")]
    InvalidSchema(String),
}

/// A coordinate reference system. Always explicit; never defaulted.
///
/// `OgcCrs84` (lon/lat WGS 84) is deliberately **not** equal to `Epsg(4326)`
/// (lat/lon axis order in the EPSG definition): converting between them is an
/// explicit decision, never implicit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Crs {
    Epsg(u32),
    /// OGC:CRS84. GeoParquet's meaning of an *absent* `crs` key.
    OgcCrs84,
    /// The data declares that its CRS is undefined (GeoParquet `"crs": null`).
    /// CRS-sensitive operations must reject it.
    Undefined,
    /// A CRS we cannot interpret (WKT, PROJJSON without a usable id, custom
    /// string...). May be carried around; CRS-sensitive operations reject it.
    Unknown(String),
}

impl Crs {
    /// Known = we can reason about it (EPSG code or CRS84).
    pub fn is_known(&self) -> bool {
        matches!(self, Crs::Epsg(_) | Crs::OgcCrs84)
    }

    pub fn epsg(&self) -> Option<u32> {
        match self {
            Crs::Epsg(c) => Some(*c),
            _ => None,
        }
    }

    /// `Some(true)` geographic (angular units), `Some(false)` projected
    /// (linear units), `None` when we do not know. v0 uses a tiny built-in
    /// table; real lookup belongs to a PROJ-backed layer later.
    pub fn is_geographic(&self) -> Option<bool> {
        match self {
            Crs::OgcCrs84 => Some(true),
            Crs::Epsg(c) => match c {
                4326 | 4269 | 4258 | 4490 => Some(true),
                3857 | 3405 | 32648 | 32649 => Some(false),
                _ => None,
            },
            _ => None,
        }
    }
}

impl fmt::Display for Crs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Crs::Epsg(c) => write!(f, "EPSG:{c}"),
            Crs::OgcCrs84 => f.write_str("OGC:CRS84"),
            Crs::Undefined => f.write_str("undefined"),
            Crs::Unknown(s) => f.write_str(s),
        }
    }
}

impl FromStr for Crs {
    type Err = TypeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t.is_empty() {
            return Err(TypeError::EmptyCrs);
        }
        let lower = t.to_ascii_lowercase();
        if matches!(lower.as_str(), "ogc:crs84" | "crs84" | "urn:ogc:def:crs:ogc:1.3:crs84" | "http://www.opengis.net/def/crs/ogc/1.3/crs84") {
            return Ok(Crs::OgcCrs84);
        }
        if lower == "undefined" {
            return Ok(Crs::Undefined);
        }
        if let Some((prefix, code)) = t.split_once(':') {
            if prefix.eq_ignore_ascii_case("epsg") {
                if let Ok(c) = code.trim().parse::<u32>() {
                    return Ok(Crs::Epsg(c));
                }
            }
        }
        Ok(Crs::Unknown(t.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryType {
    Point,
    LineString,
    Polygon,
    MultiPoint,
    MultiLineString,
    MultiPolygon,
    GeometryCollection,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceUnit {
    Meter,
    Kilometer,
    Foot,
    /// Angular unit. Never equal to a linear measure.
    Degree,
}

impl DistanceUnit {
    pub fn is_angular(self) -> bool {
        matches!(self, DistanceUnit::Degree)
    }

    /// Linear factor to meters; `None` for angular units.
    pub fn to_meters_factor(self) -> Option<f64> {
        match self {
            DistanceUnit::Meter => Some(1.0),
            DistanceUnit::Kilometer => Some(1000.0),
            DistanceUnit::Foot => Some(0.3048),
            DistanceUnit::Degree => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaUnit {
    SquareMeter,
    SquareKilometer,
    Hectare,
    SquareFoot,
}

impl AreaUnit {
    pub fn to_square_meters_factor(self) -> f64 {
        match self {
            AreaUnit::SquareMeter => 1.0,
            AreaUnit::SquareKilometer => 1_000_000.0,
            AreaUnit::Hectare => 10_000.0,
            AreaUnit::SquareFoot => 0.092_903_04,
        }
    }
}

fn check_measure(v: f64) -> Result<f64, TypeError> {
    if !v.is_finite() {
        Err(TypeError::NotFinite(v))
    } else if v < 0.0 {
        Err(TypeError::Negative(v))
    } else {
        Ok(v)
    }
}

/// A non-negative finite distance with a mandatory unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawDistance")]
pub struct Distance {
    value: f64,
    unit: DistanceUnit,
}

#[derive(Deserialize)]
struct RawDistance {
    value: f64,
    unit: DistanceUnit,
}

impl TryFrom<RawDistance> for Distance {
    type Error = TypeError;
    fn try_from(r: RawDistance) -> Result<Self, TypeError> {
        Distance::new(r.value, r.unit)
    }
}

impl Distance {
    pub fn new(value: f64, unit: DistanceUnit) -> Result<Self, TypeError> {
        Ok(Self { value: check_measure(value)?, unit })
    }
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn unit(&self) -> DistanceUnit {
        self.unit
    }
    /// Errors for angular units: 1 degree is never 1 meter.
    pub fn to_meters(&self) -> Result<f64, TypeError> {
        self.unit
            .to_meters_factor()
            .map(|f| f * self.value)
            .ok_or(TypeError::AngularUnit)
    }
}

/// A non-negative finite area with a mandatory unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawArea")]
pub struct Area {
    value: f64,
    unit: AreaUnit,
}

#[derive(Deserialize)]
struct RawArea {
    value: f64,
    unit: AreaUnit,
}

impl TryFrom<RawArea> for Area {
    type Error = TypeError;
    fn try_from(r: RawArea) -> Result<Self, TypeError> {
        Area::new(r.value, r.unit)
    }
}

impl Area {
    pub fn new(value: f64, unit: AreaUnit) -> Result<Self, TypeError> {
        Ok(Self { value: check_measure(value)?, unit })
    }
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn unit(&self) -> AreaUnit {
        self.unit
    }
    pub fn to_square_meters(&self) -> f64 {
        self.value * self.unit.to_square_meters_factor()
    }
}

/// Axis-aligned box in an explicit CRS. v0: no antimeridian wrapping
/// (`min_x <= max_x` is required).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawBBox")]
pub struct BoundingBox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    crs: Crs,
}

#[derive(Deserialize)]
struct RawBBox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    crs: Crs,
}

impl TryFrom<RawBBox> for BoundingBox {
    type Error = TypeError;
    fn try_from(r: RawBBox) -> Result<Self, TypeError> {
        BoundingBox::new(r.min_x, r.min_y, r.max_x, r.max_y, r.crs)
    }
}

impl BoundingBox {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64, crs: Crs) -> Result<Self, TypeError> {
        for v in [min_x, min_y, max_x, max_y] {
            if !v.is_finite() {
                return Err(TypeError::NotFinite(v));
            }
        }
        if min_x > max_x || min_y > max_y {
            return Err(TypeError::InvalidBBox("min must be <= max".into()));
        }
        Ok(Self { min_x, min_y, max_x, max_y, crs })
    }
    pub fn min_x(&self) -> f64 {
        self.min_x
    }
    pub fn min_y(&self) -> f64 {
        self.min_y
    }
    pub fn max_x(&self) -> f64 {
        self.max_x
    }
    pub fn max_y(&self) -> f64 {
        self.max_y
    }
    pub fn crs(&self) -> &Crs {
        &self.crs
    }

    /// Errors on CRS mismatch (an explicit transform is required first).
    pub fn intersects(&self, other: &BoundingBox) -> Result<bool, TypeError> {
        if self.crs != other.crs {
            return Err(TypeError::InvalidBBox(format!(
                "CRS mismatch: {} vs {} (explicit transform required)",
                self.crs, other.crs
            )));
        }
        Ok(self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Int64,
    Float64,
    Utf8,
    Bool,
    Timestamp,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub data_type: FieldType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GeometryColumn {
    pub name: String,
    pub geometry_type: GeometryType,
    pub crs: Crs,
}

/// Schema of a spatial dataset. `fields` lists all columns (including the
/// geometry column(s), whose `FieldType` is `Other("geometry")` by convention).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SpatialSchema {
    pub fields: Vec<Field>,
    pub geometry_columns: Vec<GeometryColumn>,
}

impl SpatialSchema {
    pub fn validate(&self) -> Result<(), TypeError> {
        let mut seen = std::collections::HashSet::new();
        for f in &self.fields {
            if !seen.insert(f.name.as_str()) {
                return Err(TypeError::InvalidSchema(format!("duplicate field '{}'", f.name)));
            }
        }
        for g in &self.geometry_columns {
            if !seen.contains(g.name.as_str()) {
                return Err(TypeError::InvalidSchema(format!(
                    "geometry column '{}' not in fields",
                    g.name
                )));
            }
        }
        Ok(())
    }

    pub fn geometry(&self, name: &str) -> Option<&GeometryColumn> {
        self.geometry_columns.iter().find(|g| g.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn rt<T: Serialize + DeserializeOwned + PartialEq + fmt::Debug>(v: T) {
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&s).unwrap(), v);
    }

    #[test]
    fn crs_parse_and_display() {
        assert_eq!("epsg:4326".parse::<Crs>().unwrap(), Crs::Epsg(4326));
        assert_eq!(Crs::Epsg(3857).to_string(), "EPSG:3857");
        assert_eq!("PROJCS[\"x\"]".parse::<Crs>().unwrap(), Crs::Unknown("PROJCS[\"x\"]".into()));
        assert_eq!("EPSG:abc".parse::<Crs>().unwrap(), Crs::Unknown("EPSG:abc".into()));
        assert_eq!("".parse::<Crs>(), Err(TypeError::EmptyCrs));
        assert!(!Crs::Unknown("x".into()).is_known());
        assert_eq!("OGC:CRS84".parse::<Crs>().unwrap(), Crs::OgcCrs84);
        assert_eq!("urn:ogc:def:crs:OGC:1.3:CRS84".parse::<Crs>().unwrap(), Crs::OgcCrs84);
        assert_eq!("undefined".parse::<Crs>().unwrap(), Crs::Undefined);
        assert!(Crs::OgcCrs84.is_known() && !Crs::Undefined.is_known());
        assert_ne!(Crs::OgcCrs84, Crs::Epsg(4326), "axis order differs; never implicitly equal");
        assert_eq!(Crs::OgcCrs84.is_geographic(), Some(true));
        assert_eq!(Crs::Undefined.is_geographic(), None);
        for c in [Crs::OgcCrs84, Crs::Undefined, Crs::Epsg(4326)] {
            assert_eq!(c.to_string().parse::<Crs>().unwrap(), c, "display/parse roundtrip");
        }
        assert_eq!(Crs::Epsg(4326).is_geographic(), Some(true));
        assert_eq!(Crs::Epsg(3857).is_geographic(), Some(false));
        assert_eq!(Crs::Epsg(99999).is_geographic(), None);
        assert_eq!(Crs::Unknown("x".into()).is_geographic(), None);
    }

    #[test]
    fn distance_validation() {
        assert!(Distance::new(f64::NAN, DistanceUnit::Meter).is_err());
        assert!(Distance::new(f64::INFINITY, DistanceUnit::Meter).is_err());
        assert!(Distance::new(-1.0, DistanceUnit::Meter).is_err());
        assert_eq!(Distance::new(2.0, DistanceUnit::Kilometer).unwrap().to_meters().unwrap(), 2000.0);
        assert_eq!(Distance::new(1.0, DistanceUnit::Degree).unwrap().to_meters(), Err(TypeError::AngularUnit));
        assert!(DistanceUnit::Degree.is_angular());
    }

    #[test]
    fn deserialization_is_validated_and_unit_required() {
        assert!(serde_json::from_str::<Distance>(r#"{"value":-1.0,"unit":"meter"}"#).is_err());
        assert!(serde_json::from_str::<Distance>(r#"{"value":1.0}"#).is_err());
        assert!(serde_json::from_str::<Area>(r#"{"value":-1.0,"unit":"hectare"}"#).is_err());
        assert!(serde_json::from_str::<BoundingBox>(
            r#"{"min_x":5,"min_y":0,"max_x":1,"max_y":1,"crs":{"epsg":4326}}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<BoundingBox>(r#"{"min_x":0,"min_y":0,"max_x":1,"max_y":1}"#).is_err(),
            "crs is mandatory"
        );
    }

    #[test]
    fn area_conversion() {
        assert_eq!(Area::new(1.0, AreaUnit::Hectare).unwrap().to_square_meters(), 10_000.0);
        assert!(Area::new(f64::NAN, AreaUnit::Hectare).is_err());
    }

    #[test]
    fn bbox_rules() {
        let a = BoundingBox::new(0.0, 0.0, 2.0, 2.0, Crs::Epsg(4326)).unwrap();
        let b = BoundingBox::new(1.0, 1.0, 3.0, 3.0, Crs::Epsg(4326)).unwrap();
        let c = BoundingBox::new(1.0, 1.0, 3.0, 3.0, Crs::Epsg(3857)).unwrap();
        assert!(a.intersects(&b).unwrap());
        assert!(a.intersects(&c).is_err());
        assert!(BoundingBox::new(2.0, 0.0, 1.0, 1.0, Crs::Epsg(4326)).is_err());
        assert!(BoundingBox::new(f64::NAN, 0.0, 1.0, 1.0, Crs::Epsg(4326)).is_err());
    }

    #[test]
    fn schema_validation() {
        let mut s = SpatialSchema {
            fields: vec![
                Field { name: "id".into(), data_type: FieldType::Int64 },
                Field { name: "geom".into(), data_type: FieldType::Other("geometry".into()) },
            ],
            geometry_columns: vec![GeometryColumn {
                name: "geom".into(),
                geometry_type: GeometryType::Polygon,
                crs: Crs::Epsg(4326),
            }],
        };
        assert!(s.validate().is_ok());
        assert!(s.geometry("geom").is_some());
        s.fields.push(Field { name: "id".into(), data_type: FieldType::Utf8 });
        assert!(s.validate().is_err());
        s.fields.pop();
        s.geometry_columns[0].name = "nope".into();
        assert!(s.validate().is_err());
    }

    #[test]
    fn serde_roundtrips() {
        rt(Crs::Epsg(4326));
        rt(Crs::Unknown("x".into()));
        rt(Crs::OgcCrs84);
        rt(Crs::Undefined);
        rt(GeometryType::MultiPolygon);
        rt(Distance::new(3.5, DistanceUnit::Foot).unwrap());
        rt(Area::new(3.5, AreaUnit::Hectare).unwrap());
        rt(BoundingBox::new(0.0, 0.0, 1.0, 1.0, Crs::Epsg(3857)).unwrap());
        rt(SpatialSchema::default());
        rt(FieldType::Other("z".into()));
    }
}
