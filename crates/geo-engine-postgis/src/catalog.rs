//! Source locality: which LOGICAL sources (the uris plans use) are also resident in the database, and where.
//!
//! This is the mapping `LogicalDataset -> PhysicalSource[]` in its minimal form (ADR-013): the plan keeps naming the logical
//! source; the catalog says "a copy of it lives in `schema.relation`, with this geometry column, SRID, CRS and columns".
//! A plan whose sources are not all resident cannot run on PostGIS - the importing of a file into the database is never a query
//! cost and never a planner candidate.
//!
//! Every identifier comes from here (validated once, quoted when used); no user string is ever an identifier.

use crate::crs::{parse_crs, srid_for};
use geo_types::{Crs, FieldType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgColType {
    Int,
    Float,
    Text,
    Bool,
    /// Present in the relation but of a type the adapter does not lower comparisons on.
    Other,
}

impl PgColType {
    /// The plan-side type this column corresponds to (the Arrow type the connector produces).
    pub fn field_type(self) -> FieldType {
        match self {
            PgColType::Int => FieldType::Int64,
            PgColType::Float => FieldType::Float64,
            PgColType::Text => FieldType::Utf8,
            PgColType::Bool => FieldType::Bool,
            PgColType::Other => FieldType::Other("other".into()),
        }
    }
}

/// One resident source. `columns` lists the non-geometry columns the engine may select.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostGisSource {
    /// The uri as written in plans (authorized by the access guard like any other source).
    pub logical_uri: String,
    pub schema: String,
    pub relation: String,
    pub geometry_column: String,
    /// `OGC:CRS84` or `EPSG:<code>` - must equal the CRS the plan's source declares.
    pub crs: String,
    /// SRID of the stored geometries; must equal `srid_for(crs)`.
    pub srid: i32,
    pub columns: BTreeMap<String, PgColType>,
    #[serde(default)]
    pub row_estimate: Option<u64>,
    /// A GiST index exists on the geometry column (bbox filters can use it). Recorded after `CREATE INDEX` + `ANALYZE`.
    #[serde(default)]
    pub spatially_indexed: bool,
    /// `[min_x, min_y, max_x, max_y]` in the source's CRS when the relation holds only the rows that intersect this region (a clipped
    /// copy). A partial copy must never answer a question it cannot answer completely: the lowering refuses any plan that reads this
    /// source other than through a `BboxFilter` whose window lies inside the coverage. `None` = the relation holds the whole source.
    #[serde(default)]
    pub coverage: Option<[f64; 4]>,
    /// Columns of the relation that hold the file bbox COVERING box (the flattened GeoParquet `bbox` struct). When present,
    /// `BboxFilter` is "covering overlaps the window", exactly like the engines that read the file.
    #[serde(default)]
    pub covering: Option<CoveringCols>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoveringCols {
    pub xmin: String,
    pub ymin: String,
    pub xmax: String,
    pub ymax: String,
}

/// `[A-Za-z_][A-Za-z0-9_]{0,62}`: PostgreSQL identifiers we are willing to quote.
pub fn valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.len() <= 63
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl PostGisSource {
    pub fn validate(&self) -> Result<(), String> {
        if self.logical_uri.is_empty() || self.logical_uri.len() > 512 {
            return Err("logical_uri must be 1..512 characters".into());
        }
        for (what, v) in [("schema", &self.schema), ("relation", &self.relation), ("geometry_column", &self.geometry_column)] {
            if !valid_identifier(v) {
                return Err(format!("{what} {v:?} is not a plain identifier"));
            }
        }
        for c in self.columns.keys() {
            if !valid_identifier(c) {
                return Err(format!("column {c:?} is not a plain identifier"));
            }
        }
        if self.columns.contains_key(&self.geometry_column) {
            return Err("the geometry column must not be listed among the plain columns".into());
        }
        if let Some(c) = &self.covering {
            for n in [&c.xmin, &c.ymin, &c.xmax, &c.ymax] {
                if !valid_identifier(n) {
                    return Err(format!("covering column {n:?} is not a plain identifier"));
                }
            }
        }
        if let Some(c) = self.coverage {
            if !c.iter().all(|v| v.is_finite()) || c[0] >= c[2] || c[1] >= c[3] {
                return Err("coverage must be [min_x, min_y, max_x, max_y] with min < max".into());
            }
        }
        let crs = parse_crs(&self.crs)?;
        if srid_for(&crs)? != self.srid {
            return Err(format!("srid {} does not match crs {} (expected {})", self.srid, self.crs, srid_for(&crs)?));
        }
        Ok(())
    }

    pub fn crs_value(&self) -> Crs {
        parse_crs(&self.crs).unwrap_or(Crs::Undefined)
    }

    /// Fully qualified, quoted relation name.
    pub fn qualified(&self) -> String {
        format!("\"{}\".\"{}\"", self.schema, self.relation)
    }

    pub fn load_file(path: &std::path::Path) -> Result<Vec<PostGisSource>, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read the PostGIS sources file: {e}"))?;
        let list: Vec<PostGisSource> = serde_json::from_str(&text).map_err(|e| format!("malformed PostGIS sources file: {e}"))?;
        PostGisCatalog::new(list.clone())?;
        Ok(list)
    }
}

/// The same file has several spellings: the access guard hands out the CANONICAL path (on Windows `\\?\C:\data\x.parquet`), a
/// configuration naturally says `C:\data\x.parquet` or `C:/data/x.parquet`. Found by the first live run (a plan carried the
/// canonical form while the catalog was keyed by the plain one: "source not resident"). Lookups compare the normalized form.
pub fn normalize_uri(uri: &str) -> String {
    let u = uri.strip_prefix("\\\\?\\").unwrap_or(uri).replace('\\', "/");
    if cfg!(windows) {
        u.to_lowercase()
    } else {
        u
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostGisCatalog {
    by_uri: BTreeMap<String, PostGisSource>,
}

impl PostGisCatalog {
    pub fn new(sources: Vec<PostGisSource>) -> Result<Self, String> {
        let mut by_uri = BTreeMap::new();
        for s in sources {
            s.validate().map_err(|e| format!("{}: {e}", s.logical_uri))?;
            if by_uri.insert(normalize_uri(&s.logical_uri), s.clone()).is_some() {
                return Err(format!("{}: listed twice", s.logical_uri));
            }
        }
        Ok(PostGisCatalog { by_uri })
    }

    pub fn get(&self, logical_uri: &str) -> Option<&PostGisSource> {
        self.by_uri.get(&normalize_uri(logical_uri))
    }

    pub fn len(&self) -> usize {
        self.by_uri.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_uri.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &PostGisSource> {
        self.by_uri.values()
    }
}

#[cfg(test)]
pub(crate) fn test_source(uri: &str, relation: &str) -> PostGisSource {
    PostGisSource {
        logical_uri: uri.into(),
        schema: "geofabric".into(),
        relation: relation.into(),
        geometry_column: "geometry".into(),
        crs: "OGC:CRS84".into(),
        srid: 4326,
        columns: [("id", PgColType::Int), ("class", PgColType::Text), ("height", PgColType::Float)].into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        row_estimate: Some(1000),
        spatially_indexed: true,
        coverage: None,
        covering: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_plain_or_rejected() {
        for ok in ["buildings", "_x1", "A_b_9"] {
            assert!(valid_identifier(ok), "{ok}");
        }
        for bad in ["", "1a", "a b", "a\"b", "a;drop", "a-b", &"x".repeat(64)] {
            assert!(!valid_identifier(bad), "{bad}");
        }
    }

    #[test]
    fn the_canonical_and_the_plain_spelling_of_a_path_are_the_same_source() {
        let c = PostGisCatalog::new(vec![test_source("C:\\Data\\Places.parquet", "places")]).unwrap();
        // forward slashes and the verbatim prefix of the canonical form name the same file
        assert!(c.get("C:/Data/Places.parquet").is_some());
        assert!(c.get("\\\\?\\C:\\Data\\Places.parquet").is_some());
        assert!(c.get("C:\\Data\\Other.parquet").is_none());
        if cfg!(windows) {
            assert!(c.get("\\\\?\\c:\\data\\places.parquet").is_some(), "case-insensitive on Windows");
            let dup = PostGisCatalog::new(vec![test_source("C:\\a\\x.parquet", "x"), test_source("\\\\?\\C:\\A\\X.parquet", "y")]);
            assert!(dup.unwrap_err().contains("twice"), "two spellings of one file are a duplicate");
        }
    }

    #[test]
    fn catalog_validates_crs_srid_and_identifiers() {
        assert!(PostGisCatalog::new(vec![test_source("a.parquet", "buildings")]).is_ok());
        let mut s = test_source("a.parquet", "buildings");
        s.srid = 3857;
        assert!(PostGisCatalog::new(vec![s]).unwrap_err().contains("does not match"));
        let mut s = test_source("a.parquet", "bad\"name");
        s.relation = "bad\"name".into();
        assert!(PostGisCatalog::new(vec![s]).is_err());
        let a = test_source("a.parquet", "x");
        assert!(PostGisCatalog::new(vec![a.clone(), a]).unwrap_err().contains("twice"));
    }
}
