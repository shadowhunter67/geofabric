//! GeoParquet source adapter.
//!
//! Reads only the Parquet footer for planning (schema, `geo` metadata, row-group
//! statistics); data is read as Arrow record batches with column projection and
//! row-group selection. Nothing here converts to GeoJSON.
//!
//! CRS policy (CONSTITUTION 2), following the GeoParquet specification:
//! * `crs` key **absent**  -> `Crs::OgcCrs84` (the spec default; lon/lat),
//!   never silently EPSG:4326;
//! * `"crs": null`         -> `Crs::Undefined` (CRS-sensitive ops are rejected);
//! * `crs` object/string   -> parsed (`EPSG` id -> `Epsg`, `OGC:CRS84` -> `OgcCrs84`,
//!   anything else -> `Unknown`).
//!
//! Authorization: production code opens sources only through
//! [`GeoParquetSource::open_authorized`]; the [`geo_types::AuthorizedSource`]
//! argument proves the access guard ran before any I/O on the file.

mod analysis;
pub use analysis::{analyze_plan, wkt_envelope, SourceScan};

use arrow_schema::DataType;
use geo_ir::{Node, SourceFormat, SourceStats};
use geo_types::{AuthorizedSource, BoundingBox, Crs, Field, FieldType, GeometryColumn, GeometryType, SpatialSchema};
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ProjectionMask;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::statistics::Statistics;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parquet: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("not a GeoParquet file: {0}")]
    NotGeoParquet(String),
    #[error("unsupported geometry encoding '{0}' (only WKB)")]
    UnsupportedEncoding(String),
    #[error("unknown column '{0}'")]
    UnknownColumn(String),
    #[error("CRS mismatch: query {query} vs source {source_crs}; explicit transform required")]
    CrsMismatch { query: String, source_crs: String },
    #[error("source CRS is unknown ({0}); cannot prune spatially")]
    UnknownCrs(String),
    /// Opaque on purpose (no path, no reason): see `geo_types::AccessDenied`.
    #[error("source not permitted")]
    AccessDenied,
}

/// Where the per-row-group bbox comes from.
#[derive(Debug, Clone, Serialize)]
pub struct Covering {
    pub xmin: String,
    pub ymin: String,
    pub xmax: String,
    pub ymax: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RowGroupInfo {
    pub index: usize,
    pub rows: u64,
    pub compressed_bytes: u64,
    /// `[xmin, ymin, xmax, ymax]` from covering-column statistics, if present.
    pub bbox: Option<[f64; 4]>,
    /// Compressed bytes per root column.
    pub column_bytes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub path: String,
    pub file_size: u64,
    pub geometry_column: String,
    pub geometry_type: GeometryType,
    pub crs: Crs,
    pub file_bbox: Option<[f64; 4]>,
    pub covering: Option<Covering>,
    pub geoparquet_version: String,
    pub row_count: u64,
    pub row_groups: Vec<RowGroupInfo>,
    pub schema: SpatialSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PruneMethod {
    /// Row-group min/max of the bbox covering columns.
    CoveringStats,
    /// No covering column / statistics: nothing could be pruned.
    NoStatistics,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneReport {
    pub method: PruneMethod,
    pub total_row_groups: usize,
    pub kept: Vec<usize>,
    pub skipped: usize,
    pub total_rows: u64,
    pub kept_rows: u64,
    pub total_bytes: u64,
    pub kept_bytes: u64,
}

pub struct GeoParquetSource {
    path: PathBuf,
    info: SourceInfo,
}

impl GeoParquetSource {
    /// Open a source that has passed the access guard.
    pub fn open_authorized(src: &AuthorizedSource) -> Result<Self, SourceError> {
        Self::open(src.path())
    }

    /// Unchecked open: for tests and offline tooling only. Production code
    /// (planner, engines, gateway) must use [`Self::open_authorized`];
    /// `tools/arch_check.py` enforces this.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SourceError> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let file_size = file.metadata()?.len();
        let reader = SerializedFileReader::new(file)?;
        let md = reader.metadata();
        let fmd = md.file_metadata();

        let geo_json = fmd
            .key_value_metadata()
            .and_then(|kv| kv.iter().find(|k| k.key == "geo"))
            .and_then(|k| k.value.clone())
            .ok_or_else(|| SourceError::NotGeoParquet("missing 'geo' key-value metadata".into()))?;
        let geo: serde_json::Value =
            serde_json::from_str(&geo_json).map_err(|e| SourceError::NotGeoParquet(format!("bad geo json: {e}")))?;
        let primary = geo["primary_column"]
            .as_str()
            .ok_or_else(|| SourceError::NotGeoParquet("no primary_column".into()))?
            .to_string();
        let col = &geo["columns"][&primary];
        if col.is_null() {
            return Err(SourceError::NotGeoParquet(format!("primary column '{primary}' not described")));
        }
        let encoding = col["encoding"].as_str().unwrap_or("WKB");
        if !encoding.eq_ignore_ascii_case("wkb") {
            return Err(SourceError::UnsupportedEncoding(encoding.to_string()));
        }
        let crs = match col.get("crs") {
            None => Crs::OgcCrs84,
            Some(v) => parse_crs(v),
        };
        let geometry_type = parse_geometry_types(&col["geometry_types"]);
        let file_bbox = col["bbox"].as_array().and_then(|a| {
            let v: Vec<f64> = a.iter().filter_map(|x| x.as_f64()).collect();
            match v.len() {
                4 => Some([v[0], v[1], v[2], v[3]]),
                6 => Some([v[0], v[1], v[3], v[4]]),
                _ => None,
            }
        });
        let covering = parse_covering(&col["covering"]["bbox"]);

        // schema
        let arrow_schema = parquet::arrow::parquet_to_arrow_schema(fmd.schema_descr(), fmd.key_value_metadata())?;
        let mut fields = Vec::new();
        for f in arrow_schema.fields() {
            let ty = if *f.name() == primary {
                FieldType::Other("geometry".into())
            } else {
                map_type(f.data_type())
            };
            fields.push(Field { name: f.name().clone(), data_type: ty });
        }
        if !fields.iter().any(|f| f.name == primary) {
            return Err(SourceError::NotGeoParquet(format!("primary column '{primary}' not in schema")));
        }
        let schema = SpatialSchema {
            fields,
            geometry_columns: vec![GeometryColumn { name: primary.clone(), geometry_type, crs: crs.clone() }],
        };

        // row groups
        let mut row_groups = Vec::new();
        for i in 0..md.num_row_groups() {
            let rg = md.row_group(i);
            let mut column_bytes: BTreeMap<String, u64> = BTreeMap::new();
            let mut stats: BTreeMap<String, (f64, f64)> = BTreeMap::new();
            for c in rg.columns() {
                let path = c.column_path().string();
                let root = path.split('.').next().unwrap_or("").to_string();
                *column_bytes.entry(root).or_default() += c.compressed_size().max(0) as u64;
                if let Some(Statistics::Double(s)) = c.statistics() {
                    if let (Some(lo), Some(hi)) = (s.min_opt(), s.max_opt()) {
                        stats.insert(path, (*lo, *hi));
                    }
                }
            }
            let bbox = covering.as_ref().and_then(|cv| {
                let xmin = stats.get(&cv.xmin)?.0;
                let ymin = stats.get(&cv.ymin)?.0;
                let xmax = stats.get(&cv.xmax)?.1;
                let ymax = stats.get(&cv.ymax)?.1;
                Some([xmin, ymin, xmax, ymax])
            });
            row_groups.push(RowGroupInfo {
                index: i,
                rows: rg.num_rows().max(0) as u64,
                compressed_bytes: rg.compressed_size().max(0) as u64,
                bbox,
                column_bytes,
            });
        }
        let info = SourceInfo {
            path: path.to_string_lossy().into_owned(),
            file_size,
            geometry_column: primary,
            geometry_type,
            crs,
            file_bbox,
            covering,
            geoparquet_version: geo["version"].as_str().unwrap_or("unknown").to_string(),
            row_count: md.file_metadata().num_rows().max(0) as u64,
            row_groups,
            schema,
        };
        Ok(Self { path, info })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self) -> &SourceInfo {
        &self.info
    }

    /// Source statistics for the IR (`SourceStats`), no data read.
    pub fn stats(&self) -> SourceStats {
        let bbox = self
            .info
            .file_bbox
            .and_then(|b| BoundingBox::new(b[0], b[1], b[2], b[3], self.info.crs.clone()).ok());
        SourceStats {
            row_count: Some(self.info.row_count),
            total_bytes: Some(self.info.file_size),
            row_groups: Some(self.info.row_groups.len() as u32),
            bbox,
        }
    }

    /// Logical IR `Source` node for this file.
    pub fn to_ir_source(&self) -> Node {
        Node::Source {
            uri: self.info.path.clone(),
            format: SourceFormat::GeoParquet,
            schema: self.info.schema.clone(),
            statistics: self.stats(),
        }
    }

    /// Row groups whose covering bbox can intersect `bbox`. Conservative:
    /// groups without statistics are kept. CRS must match exactly.
    pub fn prune(&self, bbox: &BoundingBox) -> Result<PruneReport, SourceError> {
        if !self.info.crs.is_known() {
            return Err(SourceError::UnknownCrs(self.info.crs.to_string()));
        }
        if bbox.crs() != &self.info.crs {
            return Err(SourceError::CrsMismatch {
                query: bbox.crs().to_string(),
                source_crs: self.info.crs.to_string(),
            });
        }
        let mut kept = Vec::new();
        let mut kept_rows = 0;
        let mut kept_bytes = 0;
        let mut any_stats = false;
        for rg in &self.info.row_groups {
            let keep = match rg.bbox {
                Some(b) => {
                    any_stats = true;
                    b[0] <= bbox.max_x() && bbox.min_x() <= b[2] && b[1] <= bbox.max_y() && bbox.min_y() <= b[3]
                }
                None => true,
            };
            if keep {
                kept.push(rg.index);
                kept_rows += rg.rows;
                kept_bytes += rg.compressed_bytes;
            }
        }
        let total = self.info.row_groups.len();
        Ok(PruneReport {
            method: if any_stats { PruneMethod::CoveringStats } else { PruneMethod::NoStatistics },
            total_row_groups: total,
            skipped: total - kept.len(),
            kept,
            total_rows: self.info.row_count,
            kept_rows,
            total_bytes: self.info.row_groups.iter().map(|r| r.compressed_bytes).sum(),
            kept_bytes,
        })
    }

    /// Compressed bytes that reading `columns` of `row_groups` would fetch.
    pub fn estimate_bytes(&self, columns: Option<&[String]>, row_groups: &[usize]) -> u64 {
        row_groups
            .iter()
            .filter_map(|i| self.info.row_groups.get(*i))
            .map(|rg| match columns {
                None => rg.compressed_bytes,
                Some(cols) => cols.iter().map(|c| rg.column_bytes.get(c).copied().unwrap_or(0)).sum(),
            })
            .sum()
    }

    /// Arrow-native read with column projection and row-group selection.
    pub fn read(
        &self,
        columns: Option<&[String]>,
        row_groups: Option<&[usize]>,
        batch_size: usize,
    ) -> Result<ParquetRecordBatchReader, SourceError> {
        let mut b = ParquetRecordBatchReaderBuilder::try_new(File::open(&self.path)?)?;
        if let Some(cols) = columns {
            for c in cols {
                if !self.info.schema.fields.iter().any(|f| f.name == *c) {
                    return Err(SourceError::UnknownColumn(c.clone()));
                }
            }
            let mask = ProjectionMask::columns(b.parquet_schema(), cols.iter().map(|s| s.as_str()));
            b = b.with_projection(mask);
        }
        if let Some(rgs) = row_groups {
            b = b.with_row_groups(rgs.to_vec());
        }
        Ok(b.with_batch_size(batch_size).build()?)
    }
}

fn map_type(t: &DataType) -> FieldType {
    use DataType::*;
    match t {
        Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 => FieldType::Int64,
        Float16 | Float32 | Float64 => FieldType::Float64,
        Utf8 | LargeUtf8 | Utf8View => FieldType::Utf8,
        Boolean => FieldType::Bool,
        Timestamp(_, _) => FieldType::Timestamp,
        Struct(_) => FieldType::Other("struct".into()),
        other => FieldType::Other(format!("{other}")),
    }
}

fn parse_crs(v: &serde_json::Value) -> Crs {
    if v.is_null() {
        return Crs::Undefined;
    }
    if let Some(s) = v.as_str() {
        return s.parse().unwrap_or_else(|_| Crs::Unknown(s.to_string()));
    }
    let id = v.get("id").or_else(|| v.get("ids").and_then(|a| a.get(0)));
    if let Some(id) = id {
        let auth = id["authority"].as_str().unwrap_or("");
        let code = &id["code"];
        let code_s = code.as_u64().map(|c| c.to_string()).or_else(|| code.as_str().map(String::from));
        if let Some(c) = code_s {
            if auth.eq_ignore_ascii_case("EPSG") {
                if let Ok(n) = c.parse::<u32>() {
                    return Crs::Epsg(n);
                }
            }
            if auth.eq_ignore_ascii_case("OGC") && c.eq_ignore_ascii_case("CRS84") {
                return Crs::OgcCrs84;
            }
            return Crs::Unknown(format!("{auth}:{c}"));
        }
    }
    Crs::Unknown(v.to_string())
}

fn parse_geometry_types(v: &serde_json::Value) -> GeometryType {
    let names: Vec<String> = v
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| s.split(' ').next().unwrap_or("").to_string()).collect())
        .unwrap_or_default();
    let mut uniq = names.clone();
    uniq.sort();
    uniq.dedup();
    if uniq.len() != 1 {
        return GeometryType::Unknown;
    }
    match uniq[0].as_str() {
        "Point" => GeometryType::Point,
        "LineString" => GeometryType::LineString,
        "Polygon" => GeometryType::Polygon,
        "MultiPoint" => GeometryType::MultiPoint,
        "MultiLineString" => GeometryType::MultiLineString,
        "MultiPolygon" => GeometryType::MultiPolygon,
        "GeometryCollection" => GeometryType::GeometryCollection,
        _ => GeometryType::Unknown,
    }
}

fn parse_covering(v: &serde_json::Value) -> Option<Covering> {
    let p = |k: &str| -> Option<String> {
        let arr = v.get(k)?.as_array()?;
        Some(arr.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("."))
    };
    Some(Covering { xmin: p("xmin")?, ymin: p("ymin")?, xmax: p("xmax")?, ymax: p("ymax")? })
}
