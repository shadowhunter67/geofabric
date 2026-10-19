//! Spatial density histogram of a GeoParquet source (estimator v3).
//!
//! Row-group statistics say *where a row group's rows may be* (their union bbox) but not *how they are spread*:
//! on clustered data a window inside a big row-group box can contain thousands of rows or none (measured: the
//! uniform-density estimate was off by 100-1000x on ~10 % of windows, ADR-010). A coarse 2D histogram of the rows'
//! bbox centres, built **once per file** by reading only the covering columns (a few percent of the file), answers
//! "how many rows lie in this window" within the resolution of the grid.
//!
//! The histogram is a property of the data (like a footer): it never sees a query or a result. It is cached next to
//! the data (`.gf-cache/`), keyed by file size and modification time.

use crate::colstats::{self, Accumulator, ColumnStats};
use crate::{GeoParquetSource, SourceError};
use arrow_array::cast::AsArray;
use arrow_array::Array;
use arrow_array::types::Float64Type;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

pub const DEFAULT_GRID: usize = 128;
/// Bump when the sidecar format or what it contains changes: older sidecars are rebuilt.
pub const HIST_VERSION: u32 = 3;
/// Sources with at most this many rows also get an exact [`RowIndex`].
pub const ROW_INDEX_MAX_ROWS: u64 = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialHistogram {
    #[serde(default)]
    pub version: u32,
    /// `[xmin, ymin, xmax, ymax]` of the grid.
    pub bbox: [f64; 4],
    pub nx: usize,
    pub ny: usize,
    /// Row counts by cell of the bbox centre, row-major (`iy * nx + ix`).
    pub counts: Vec<u32>,
    /// Rows counted (rows with a null/NaN bbox are not counted).
    pub counted: u64,
    pub total_rows: u64,
    /// Fingerprint of the file the histogram was built from.
    pub file_size: u64,
    pub modified_ms: u64,
    pub build_ms: u64,
    /// Value statistics of up to [`MAX_STAT_COLUMNS`] cheap scalar columns (predicate selectivity).
    #[serde(default)]
    pub columns: BTreeMap<String, ColumnStats>,
    /// Exact per-row boxes and geometry sizes of a SMALL source (<= [`ROW_INDEX_MAX_ROWS`] rows). A centre histogram cannot
    /// see an object much larger than the window (a province boundary contains the whole viewport: its centre is elsewhere), and
    /// the mean geometry size says nothing about those heavy rows. With the index the window count and the geometry payload are exact.
    #[serde(default)]
    pub row_index: Option<RowIndex>,
}

/// One entry per row: bbox and the WKB length of its geometry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RowIndex {
    pub xmin: Vec<f64>,
    pub ymin: Vec<f64>,
    pub xmax: Vec<f64>,
    pub ymax: Vec<f64>,
    pub geometry_bytes: Vec<u32>,
}

impl RowIndex {
    /// Rows whose bbox intersects `window` (the semantics of the bbox filter) and the total geometry bytes of those rows.
    pub fn window(&self, w: [f64; 4]) -> (u64, f64) {
        let (mut n, mut bytes) = (0u64, 0.0f64);
        for i in 0..self.xmin.len() {
            if self.xmin[i] <= w[2] && self.xmax[i] >= w[0] && self.ymin[i] <= w[3] && self.ymax[i] >= w[1] {
                n += 1;
                bytes += self.geometry_bytes[i] as f64;
            }
        }
        (n, bytes)
    }
}

/// Columns profiled per source (the cheapest ones by compressed size).
pub const MAX_STAT_COLUMNS: usize = 12;

impl SpatialHistogram {
    /// Build from the covering columns (`bbox.xmin ...`) of `src`. Errors when the source has no covering or no extent.
    pub fn build(src: &GeoParquetSource, nx: usize, ny: usize) -> Result<SpatialHistogram, SourceError> {
        let t0 = std::time::Instant::now();
        let info = src.info();
        let cov = info.covering.as_ref().ok_or_else(|| SourceError::NotGeoParquet("histogram needs a bbox covering column".into()))?;
        let split = |p: &str| -> (String, String) {
            let mut it = p.splitn(2, '.');
            (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
        };
        let ((top, xmin_f), (_, ymin_f), (_, xmax_f), (_, ymax_f)) = (split(&cov.xmin), split(&cov.ymin), split(&cov.xmax), split(&cov.ymax));
        // extent: union of row-group boxes (statistics), else the file bbox
        let mut ext: Option<[f64; 4]> = None;
        for rg in &info.row_groups {
            if let Some(b) = rg.bbox {
                ext = Some(match ext {
                    None => b,
                    Some(e) => [e[0].min(b[0]), e[1].min(b[1]), e[2].max(b[2]), e[3].max(b[3])],
                });
            }
        }
        let ext = ext.or(info.file_bbox).ok_or_else(|| SourceError::NotGeoParquet("no extent for the histogram".into()))?;
        let (w, h) = ((ext[2] - ext[0]).max(1e-12), (ext[3] - ext[1]).max(1e-12));
        let mut counts = vec![0u32; nx * ny];
        let mut counted = 0u64;
        let reader = src.read(Some(&[top.clone()]), None, 65_536)?;
        for batch in reader {
            let batch = batch.map_err(|e| SourceError::NotGeoParquet(format!("arrow: {e}")))?;
            let st = batch.column(0).as_struct_opt().ok_or_else(|| SourceError::NotGeoParquet("covering column is not a struct".into()))?;
            let get = |name: &str| st.column_by_name(name).map(|c| c.as_primitive_opt::<Float64Type>().cloned());
            let (Some(Some(x0)), Some(Some(y0)), Some(Some(x1)), Some(Some(y1))) = (get(&xmin_f), get(&ymin_f), get(&xmax_f), get(&ymax_f)) else {
                return Err(SourceError::NotGeoParquet("covering fields are not Float64".into()));
            };
            for i in 0..batch.num_rows() {
                if x0.is_null(i) || y0.is_null(i) || x1.is_null(i) || y1.is_null(i) {
                    continue;
                }
                let (cx, cy) = ((x0.value(i) + x1.value(i)) * 0.5, (y0.value(i) + y1.value(i)) * 0.5);
                if !cx.is_finite() || !cy.is_finite() {
                    continue;
                }
                let ix = (((cx - ext[0]) / w) * nx as f64).floor().clamp(0.0, nx as f64 - 1.0) as usize;
                let iy = (((cy - ext[1]) / h) * ny as f64).floor().clamp(0.0, ny as f64 - 1.0) as usize;
                counts[iy * nx + ix] = counts[iy * nx + ix].saturating_add(1);
                counted += 1;
            }
        }
        let columns = Self::profile_columns(src, &top)?;
        let row_index = if info.row_count <= ROW_INDEX_MAX_ROWS { Self::build_row_index(src, &top, [&xmin_f, &ymin_f, &xmax_f, &ymax_f])? } else { None };
        let md = std::fs::metadata(src.path()).ok();
        Ok(SpatialHistogram {
            version: HIST_VERSION,
            columns,
            row_index,
            bbox: ext,
            nx,
            ny,
            counts,
            counted,
            total_rows: info.row_count,
            file_size: info.file_size,
            modified_ms: md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64),
            build_ms: t0.elapsed().as_millis() as u64,
        })
    }

    /// Exact boxes + geometry sizes of every row (small sources only). `None` when the geometry column cannot be read as binary.
    fn build_row_index(src: &GeoParquetSource, top: &str, fields: [&String; 4]) -> Result<Option<RowIndex>, SourceError> {
        let geom = src.info().geometry_column.clone();
        let reader = src.read(Some(&[top.to_string(), geom]), None, 65_536)?;
        let mut idx = RowIndex::default();
        for batch in reader {
            let batch = batch.map_err(|e| SourceError::NotGeoParquet(format!("arrow: {e}")))?;
            let Some(st) = batch.column_by_name(top).and_then(|c| c.as_struct_opt()) else { return Ok(None) };
            let get = |name: &str| st.column_by_name(name).and_then(|c| c.as_primitive_opt::<Float64Type>().cloned());
            let (Some(x0), Some(y0), Some(x1), Some(y1)) = (get(fields[0]), get(fields[1]), get(fields[2]), get(fields[3])) else { return Ok(None) };
            let g = batch.column_by_name(&src.info().geometry_column).ok_or_else(|| SourceError::NotGeoParquet("geometry column missing".into()))?;
            let len = |i: usize| -> Option<u32> {
                if let Some(b) = g.as_binary_opt::<i32>() {
                    return Some(if b.is_null(i) { 0 } else { b.value(i).len() as u32 });
                }
                g.as_binary_opt::<i64>().map(|b| if b.is_null(i) { 0 } else { b.value(i).len() as u32 })
            };
            for i in 0..batch.num_rows() {
                if x0.is_null(i) || y0.is_null(i) || x1.is_null(i) || y1.is_null(i) {
                    continue;
                }
                let Some(l) = len(i) else { return Ok(None) };
                idx.xmin.push(x0.value(i));
                idx.ymin.push(y0.value(i));
                idx.xmax.push(x1.value(i));
                idx.ymax.push(y1.value(i));
                idx.geometry_bytes.push(l);
            }
        }
        Ok(Some(idx))
    }

    /// Value statistics of the cheapest scalar columns (never the geometry or the covering struct).
    fn profile_columns(src: &GeoParquetSource, covering_top: &str) -> Result<BTreeMap<String, ColumnStats>, SourceError> {
        use geo_types::FieldType;
        let info = src.info();
        let mut cand: Vec<(u64, String)> = info
            .schema
            .fields
            .iter()
            .filter(|f| f.name != info.geometry_column && f.name != covering_top && matches!(f.data_type, FieldType::Int64 | FieldType::Float64 | FieldType::Utf8 | FieldType::Bool))
            .map(|f| (info.row_groups.iter().map(|r| r.column_bytes.get(&f.name).copied().unwrap_or(0)).sum::<u64>(), f.name.clone()))
            .collect();
        cand.sort();
        cand.truncate(MAX_STAT_COLUMNS);
        let names: Vec<String> = cand.into_iter().map(|c| c.1).collect();
        if names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let reader = src.read(Some(&names), None, 65_536)?;
        let mut acc: BTreeMap<String, Accumulator> = BTreeMap::new();
        for batch in reader {
            let batch = batch.map_err(|e| SourceError::NotGeoParquet(format!("arrow: {e}")))?;
            for (i, field) in batch.schema().fields().iter().enumerate() {
                if !colstats::supported(field.data_type()) {
                    continue;
                }
                if !acc.contains_key(field.name().as_str()) {
                    if let Some(a) = Accumulator::new(field.data_type(), info.row_count) {
                        acc.insert(field.name().clone(), a);
                    }
                }
                if let Some(a) = acc.get_mut(field.name().as_str()) {
                    a.add(batch.column(i));
                }
            }
        }
        Ok(acc.into_iter().map(|(k, a)| (k, a.finish())).collect())
    }

    /// Selectivity of the filter predicates of a plan over this source.
    pub fn predicate_selectivity(&self, e: &geo_ir::Expr) -> colstats::Selectivity {
        colstats::selectivity(e, &self.columns)
    }

    /// Expected number of rows whose bbox centre lies in `window` (`[xmin, ymin, xmax, ymax]`): full cells count in
    /// full, partly covered cells proportionally to the overlapped area (uniform inside a cell).
    pub fn estimate(&self, window: [f64; 4]) -> f64 {
        let (w, h) = ((self.bbox[2] - self.bbox[0]).max(1e-12), (self.bbox[3] - self.bbox[1]).max(1e-12));
        let (cw, ch) = (w / self.nx as f64, h / self.ny as f64);
        let lo = |v: f64, o: f64, c: f64, n: usize| (((v - o) / c).floor().max(0.0) as usize).min(n - 1);
        let (ix0, ix1) = (lo(window[0], self.bbox[0], cw, self.nx), lo(window[2], self.bbox[0], cw, self.nx));
        let (iy0, iy1) = (lo(window[1], self.bbox[1], ch, self.ny), lo(window[3], self.bbox[1], ch, self.ny));
        if window[2] < self.bbox[0] || window[0] > self.bbox[2] || window[3] < self.bbox[1] || window[1] > self.bbox[3] {
            return 0.0;
        }
        let mut sum = 0.0;
        for iy in iy0..=iy1 {
            let (cy0, cy1) = (self.bbox[1] + iy as f64 * ch, self.bbox[1] + (iy + 1) as f64 * ch);
            let fy = ((window[3].min(cy1) - window[1].max(cy0)) / ch).clamp(0.0, 1.0);
            for ix in ix0..=ix1 {
                let (cx0, cx1) = (self.bbox[0] + ix as f64 * cw, self.bbox[0] + (ix + 1) as f64 * cw);
                let fx = ((window[2].min(cx1) - window[0].max(cx0)) / cw).clamp(0.0, 1.0);
                sum += self.counts[iy * self.nx + ix] as f64 * fx * fy;
            }
        }
        sum
    }

    /// Fraction of the counted rows in the busiest cell: a skew indicator for reports.
    pub fn peak_share(&self) -> f64 {
        self.counts.iter().copied().max().unwrap_or(0) as f64 / self.counted.max(1) as f64
    }

    /// Whether this histogram still describes the file (same size and modification time).
    pub fn matches(&self, src: &GeoParquetSource) -> bool {
        let md = std::fs::metadata(src.path()).ok();
        let mtime = md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
        self.version == HIST_VERSION && self.file_size == src.info().file_size && self.modified_ms == mtime
    }
}

/// Histograms by file path, shared between the gateway's background builder and the planner.
#[derive(Default)]
pub struct HistogramStore {
    inner: Mutex<HashMap<String, Arc<SpatialHistogram>>>,
}

impl std::fmt::Debug for HistogramStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HistogramStore({} sources)", self.inner.lock().map_or(0, |g| g.len()))
    }
}

impl HistogramStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn put(&self, path: &std::path::Path, h: SpatialHistogram) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(path.to_string_lossy().into_owned(), Arc::new(h));
        }
    }
    pub fn get(&self, path: &std::path::Path) -> Option<Arc<SpatialHistogram>> {
        self.inner.lock().ok()?.get(path.to_string_lossy().as_ref()).cloned()
    }
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }
    pub fn paths(&self) -> Vec<String> {
        self.inner.lock().map(|g| g.keys().cloned().collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(counts: Vec<u32>, nx: usize, ny: usize) -> SpatialHistogram {
        let counted = counts.iter().map(|c| *c as u64).sum();
        SpatialHistogram { version: HIST_VERSION, bbox: [0.0, 0.0, nx as f64, ny as f64], nx, ny, counts, counted, total_rows: counted, file_size: 0, modified_ms: 0, build_ms: 0, columns: Default::default(), row_index: None }
    }

    #[test]
    fn estimate_counts_full_and_partial_cells() {
        let h = hist(vec![10, 20, 30, 40], 2, 2); // cells (0,0)=10 (1,0)=20 (0,1)=30 (1,1)=40
        assert_eq!(h.estimate([0.0, 0.0, 2.0, 2.0]), 100.0);
        assert_eq!(h.estimate([0.0, 0.0, 1.0, 1.0]), 10.0);
        assert!((h.estimate([0.5, 0.5, 1.5, 1.5]) - (2.5 + 5.0 + 7.5 + 10.0)).abs() < 1e-9, "a window across four cells takes a quarter of each");
        assert_eq!(h.estimate([5.0, 5.0, 6.0, 6.0]), 0.0, "outside the grid");
        assert_eq!(h.estimate([-3.0, -3.0, 0.0, 0.0]), 0.0, "touching only the corner point");
    }

    #[test]
    fn empty_regions_are_empty() {
        let h = hist(vec![0, 0, 0, 1000], 2, 2);
        assert_eq!(h.estimate([0.0, 0.0, 1.0, 2.0]), 0.0);
        assert_eq!(h.estimate([1.0, 1.0, 2.0, 2.0]), 1000.0);
        assert!((h.peak_share() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn store_roundtrip() {
        let s = HistogramStore::new();
        let p = std::path::Path::new("a.parquet");
        assert!(s.get(p).is_none());
        s.put(p, hist(vec![1], 1, 1));
        assert_eq!(s.get(p).unwrap().counted, 1);
        assert_eq!(s.len(), 1);
    }
}
