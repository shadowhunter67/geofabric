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

#[derive(Debug, Clone, Default)]
pub struct PostGisCatalog {
    by_uri: BTreeMap<String, PostGisSource>,
}

impl PostGisCatalog {
    pub fn new(sources: Vec<PostGisSource>) -> Result<Self, String> {
        let mut by_uri = BTreeMap::new();
        for s in sources {
            s.validate().map_err(|e| format!("{}: {e}", s.logical_uri))?;
            if by_uri.insert(s.logical_uri.clone(), s.clone()).is_some() {
                return Err(format!("{}: listed twice", s.logical_uri));
            }
        }
        Ok(PostGisCatalog { by_uri })
    }

    pub fn get(&self, logical_uri: &str) -> Option<&PostGisSource> {
        self.by_uri.get(logical_uri)
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
