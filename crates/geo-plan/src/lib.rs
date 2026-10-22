//! Canonical request -> PlanGraph (RFC-004, second layer).
//!
//! `geo-lowering` defines what a PlanGraph MEANS when it becomes SQL. This crate defines what a USER REQUEST means as a PlanGraph: which
//! nodes a viewport query, an attribute filter, a count/group, a thinned render query or a metric neighbour query produce. Before it existed the
//! gateway (Rust) and the static browser build (TypeScript) each built plans from requests, so the same user action could produce two
//! different PlanGraphs before lowering even started. Now there is one builder: the gateway calls it natively, the static build calls the same
//! code compiled to WebAssembly (`geo-lowering`'s wasm module exports it), and parity is a golden-fixture test, not a hope.
//!
//! * The request model ([`PlanRequest`]) carries intent only: source, viewport, filters, sample, limit, grouping, and the optional `with` source of
//!   a neighbour/join question. No engine, no SQL, no dialect, no engine choice (the planner chooses engines; an engine *hint* is a planner input
//!   that the caller sets on the plan afterwards).
//! * Validation happens BEFORE any node is built ([`PlanError`] is structured); an invalid request never becomes an invalid PlanGraph that a lower
//!   layer has to reject.
//! * A request may legitimately produce a plan the executing environment cannot run (a browser cannot run `Transform` + `SpatialSemiJoin`): the
//!   plan is still built exactly and the capability layer refuses it, never an approximation here.
//! * Output is deterministic: node ids are content hashes, maps are ordered, floats use Rust's shortest round-trip form.

use geo_ir::{AggExpr, AggFunc, CompareOp, Expr, GeometryLiteral, JoinType, Literal, Node, Plan, SortKey, SpatialPredicate};
use geo_types::{local_metric_crs, BoundingBox, Crs, Distance, DistanceUnit, FieldType, SpatialSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Version of the request schema (independent of the IR version: changing how a request is *written* is not a change of PlanGraph semantics).
pub const REQUEST_SCHEMA_VERSION: u32 = 1;

/// Same cap as the gateway's response cap (a request can never ask for more rows than the system will ever return).
pub const MAX_FEATURES: u64 = 100_000;
/// `near` distances above this are refused (a local UTM zone stays accurate; larger questions are not neighbour queries).
pub const MAX_NEAR_DISTANCE_M: f64 = 5_000.0;
/// Rows of the grouped result (`count_by`, `join_count_by`).
const TOP_GROUPS: u64 = 50;
const TOP_JOIN_GROUPS: u64 = 200;
/// Smallest metres per degree of latitude on the WGS 84 ellipsoid (at the equator): the conservative direction for a degree margin.
const METERS_PER_DEGREE_LAT_MIN: f64 = 110_574.0;
/// Metres per degree of longitude on the equator (the maximum); it shrinks with cos(latitude).
const METERS_PER_DEGREE_LON_EQUATOR: f64 = 111_320.0;

// ------------------------------------------------------------------------------------------------------------------ request model

/// An attribute filter: `eq|ne|lt|le|gt|ge` use `value`; `in` uses `values`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilterRequest {
    pub column: String,
    pub op: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub values: Vec<Value>,
}

/// The second source of a neighbour (`near`, `near_count`) or join (`join_count_by`) question.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WithRequest {
    pub source: String,
    #[serde(default)]
    pub filter: Option<FilterRequest>,
    #[serde(default)]
    pub distance_m: Option<f64>,
    #[serde(default)]
    pub group_by: Option<String>,
}

/// What the user asked for. Unknown JSON fields are ignored on purpose: the same object also carries transport fields (engine hint, browser flag,
/// client state) that belong to the planner, not to the plan.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanRequest {
    /// `features | count | count_by | intersects_features | near | near_count | join_count_by`
    pub kind: String,
    pub source: String,
    /// viewport `[xmin, ymin, xmax, ymax]` in `crs`
    pub bbox: [f64; 4],
    pub crs: String,
    #[serde(default)]
    pub filter: Option<FilterRequest>,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default, rename = "with")]
    pub with_request: Option<WithRequest>,
    #[serde(default)]
    pub limit: Option<u64>,
    /// RENDERING only: keep the rows whose `md5(id) mod sample_modulus = 0` (power of two, at most 2^20). Never valid for counts or aggregates:
    /// analytics always see the full logical result (ADR-012).
    #[serde(default)]
    pub sample_modulus: Option<u32>,
    #[serde(default)]
    pub request_schema_version: Option<u32>,
}

// ------------------------------------------------------------------------------------------------------------------ errors

/// Why a request cannot become a PlanGraph. The code is the contract (UIs map it to a friendly message); the message is for developers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnknownDataset,
    InvalidField,
    InvalidFilter,
    InvalidBbox,
    InvalidCrs,
    InvalidLimit,
    InvalidRequest,
    UnsupportedRequest,
    MetricCrsUnavailable,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct PlanError {
    pub code: ErrorCode,
    pub message: String,
}

impl PlanError {
    fn new(code: ErrorCode, message: impl Into<String>) -> PlanError {
        PlanError { code, message: message.into() }
    }

    /// The HTTP status a gateway answers with.
    pub fn http_status(&self) -> u16 {
        match self.code {
            ErrorCode::UnknownDataset => 404,
            ErrorCode::InvalidField | ErrorCode::InvalidCrs | ErrorCode::MetricCrsUnavailable => 422,
            _ => 400,
        }
    }

    /// The legacy gateway `error` string (kept so existing API clients and tests keep working); `code` is the structured contract.
    pub fn legacy_error(&self) -> &'static str {
        match self.code {
            ErrorCode::UnknownDataset => "no_such_source",
            ErrorCode::InvalidField => "invalid_field",
            ErrorCode::InvalidCrs => "crs_mismatch",
            ErrorCode::MetricCrsUnavailable => "metric_crs_unavailable",
            _ => "bad_request",
        }
    }
}

type Res<T> = Result<T, PlanError>;

fn invalid_request<T>(m: impl Into<String>) -> Res<T> {
    Err(PlanError::new(ErrorCode::InvalidRequest, m))
}

// ------------------------------------------------------------------------------------------------------------------ builder

/// Source nodes by the dataset id used in the request (the caller resolved and authorized them: the gateway through its access guard and the
/// Parquet footer, the static build from its published manifest).
pub type Sources = BTreeMap<String, Node>;

fn source_info(node: &Node) -> Res<(String, Crs, &SpatialSchema)> {
    match node {
        Node::Source { schema, .. } => {
            let g = schema.geometry_columns.first().ok_or_else(|| PlanError::new(ErrorCode::UnsupportedRequest, "source has no geometry column"))?;
            Ok((g.name.clone(), g.crs.clone(), schema))
        }
        _ => Err(PlanError::new(ErrorCode::InvalidRequest, "dataset did not resolve to a source node")),
    }
}

fn literal(v: &Value) -> Res<Literal> {
    Ok(match v {
        Value::Bool(b) => Literal::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Literal::Int(i),
            None => Literal::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Literal::Str(s.clone()),
        Value::Null => Literal::Null,
        _ => return Err(PlanError::new(ErrorCode::InvalidFilter, "filter value must be a scalar")),
    })
}

fn has_column(schema: &SpatialSchema, name: &str) -> bool {
    schema.fields.iter().any(|f| f.name == name)
}

/// Validate a filter against the source's schema, then apply it. Column and operator problems are reported here, not by a lower layer.
fn apply_filter(node: Node, filter: Option<&FilterRequest>, schema: &SpatialSchema, dataset: &str) -> Res<Node> {
    let Some(f) = filter else { return Ok(node) };
    if !has_column(schema, &f.column) {
        return Err(PlanError::new(ErrorCode::InvalidField, format!("filter column '{}' is not a column of dataset '{dataset}'", f.column)));
    }
    if schema.geometry(&f.column).is_some() {
        return Err(PlanError::new(ErrorCode::InvalidFilter, format!("filter column '{}' is a geometry column", f.column)));
    }
    let predicate = if f.op == "in" {
        if f.values.is_empty() {
            return Err(PlanError::new(ErrorCode::InvalidFilter, "filter op 'in' needs a non-empty values list"));
        }
        let values = f.values.iter().map(literal).collect::<Res<Vec<_>>>()?;
        Expr::In { column: f.column.clone(), values }
    } else {
        let op = match f.op.as_str() {
            "eq" => CompareOp::Eq,
            "ne" => CompareOp::Ne,
            "lt" => CompareOp::Lt,
            "le" => CompareOp::Le,
            "gt" => CompareOp::Gt,
            "ge" => CompareOp::Ge,
            _ => return Err(PlanError::new(ErrorCode::InvalidFilter, "unknown filter op")),
        };
        Expr::Compare { column: f.column.clone(), op, value: literal(&f.value)? }
    };
    Ok(Node::Filter { input: Box::new(node), predicate })
}

/// Deterministic render thinning (ADR-012): row-returning kinds only.
fn apply_sample(node: Node, req: &PlanRequest, schema: &SpatialSchema) -> Res<Node> {
    let Some(m) = req.sample_modulus else { return Ok(node) };
    if !matches!(req.kind.as_str(), "features" | "intersects_features" | "near") {
        return invalid_request("sample_modulus is only valid for row-returning kinds (features, intersects_features, near): counts and aggregates always use the full result");
    }
    if m == 0 || !m.is_power_of_two() || m > (1 << 20) {
        return invalid_request("sample_modulus must be a power of two between 1 and 1048576");
    }
    if m == 1 {
        return Ok(node);
    }
    if !schema.fields.iter().any(|f| f.name == "id" && schema.geometry(&f.name).is_none()) {
        return invalid_request("sampling needs an `id` column in the source");
    }
    Ok(Node::Filter { input: Box::new(node), predicate: Expr::HashBucket { column: "id".into(), modulus: m, remainder: 0 } })
}

fn viewport(req: &PlanRequest, source_crs: &Crs) -> Res<BoundingBox> {
    let crs: Crs = req.crs.parse().map_err(|_| PlanError::new(ErrorCode::InvalidCrs, "bad crs"))?;
    if crs != *source_crs {
        return Err(PlanError::new(
            ErrorCode::InvalidCrs,
            format!("viewport CRS {crs} differs from source CRS {source_crs}; explicit transform not available in v0"),
        ));
    }
    BoundingBox::new(req.bbox[0], req.bbox[1], req.bbox[2], req.bbox[3], crs).map_err(|e| PlanError::new(ErrorCode::InvalidBbox, e.to_string()))
}

fn count_all(input: Node) -> Node {
    Node::Aggregate { input: Box::new(input), group_by: vec![], aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }] }
}

/// Build the PlanGraph of `req`. `sources` holds the Source node of `req.source` and, for `near`/`near_count`/`join_count_by`, of `req.with.source`.
pub fn build_plan(req: &PlanRequest, sources: &Sources) -> Res<Plan> {
    if let Some(v) = req.request_schema_version {
        if v != REQUEST_SCHEMA_VERSION {
            return invalid_request(format!("request_schema_version {v} is not supported (this builder speaks {REQUEST_SCHEMA_VERSION})"));
        }
    }
    let two_sources = matches!(req.kind.as_str(), "near" | "near_count" | "join_count_by");
    match (&req.with_request, two_sources) {
        (None, true) => return invalid_request("with is required for this kind"),
        (Some(_), false) => return invalid_request("with is only valid for near and join_count_by"),
        _ => {}
    }
    if !["features", "count", "count_by", "intersects_features", "near", "near_count", "join_count_by"].contains(&req.kind.as_str()) {
        return Err(PlanError::new(ErrorCode::UnsupportedRequest, "unknown kind"));
    }
    if let Some(l) = req.limit {
        if l == 0 {
            return Err(PlanError::new(ErrorCode::InvalidLimit, "limit must be at least 1"));
        }
    }
    let src = sources.get(&req.source).ok_or_else(|| PlanError::new(ErrorCode::UnknownDataset, req.source.clone()))?;
    if two_sources {
        return build_two_source(req, src, sources);
    }
    let (geom_col, crs_src, schema) = source_info(src)?;
    let bb = viewport(req, &crs_src)?;
    let mut node = match req.kind.as_str() {
        "intersects_features" => {
            let (a, b, c, d) = (bb.min_x(), bb.min_y(), bb.max_x(), bb.max_y());
            Node::Intersects {
                input: Box::new(src.clone()),
                geometry_column: geom_col.clone(),
                with: GeometryLiteral { wkt: format!("POLYGON(({a} {b}, {c} {b}, {c} {d}, {a} {d}, {a} {b}))"), crs: bb.crs().clone() },
            }
        }
        _ => Node::BboxFilter { input: Box::new(src.clone()), geometry_column: geom_col.clone(), bbox: bb },
    };
    node = apply_filter(node, req.filter.as_ref(), schema, &req.source)?;
    node = apply_sample(node, req, schema)?;
    let limit = req.limit.unwrap_or(20_000).min(MAX_FEATURES);
    node = match req.kind.as_str() {
        "count" => count_all(node),
        "count_by" => {
            let g = req.group_by.clone().ok_or_else(|| PlanError::new(ErrorCode::InvalidRequest, "group_by required"))?;
            if !has_column(schema, &g) {
                return Err(PlanError::new(ErrorCode::InvalidField, format!("group_by column '{g}' is not a column of dataset '{}'", req.source)));
            }
            Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate {
                        input: Box::new(node),
                        group_by: vec![g.clone()],
                        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                    }),
                    // the group key breaks ties: two engines must cut the top 50 at the same place
                    keys: vec![SortKey { column: "count".into(), descending: true }, SortKey { column: g.clone(), descending: false }],
                }),
                n: TOP_GROUPS,
            }
        }
        _ => {
            let schema_now = node.output_schema().map_err(|e| PlanError::new(ErrorCode::InvalidRequest, e.to_string()))?;
            let cols: Vec<String> = schema_now
                .fields
                .iter()
                .filter(|f| !matches!(&f.data_type, FieldType::Other(t) if t != "geometry"))
                .map(|f| f.name.clone())
                .collect();
            Node::Limit { input: Box::new(Node::Project { input: Box::new(node), columns: cols }), n: limit }
        }
    };
    Ok(Plan::new(node))
}

fn lon_lat(crs: &Crs) -> bool {
    matches!(crs, Crs::OgcCrs84 | Crs::Epsg(4326))
}

/// `near` / `near_count` (a metric neighbour query: Transform + SpatialSemiJoin) and `join_count_by` (a spatial join + grouped count).
fn build_two_source(req: &PlanRequest, left: &Node, sources: &Sources) -> Res<Plan> {
    let with = req.with_request.as_ref().expect("checked by the caller");
    let right = sources.get(&with.source).ok_or_else(|| PlanError::new(ErrorCode::UnknownDataset, with.source.clone()))?;
    let (left_geometry, left_crs, left_schema) = source_info(left)?;
    let (right_geometry, right_crs, right_schema) = source_info(right)?;
    let bbox = viewport(req, &left_crs)?;
    if right_crs != left_crs {
        return Err(PlanError::new(
            ErrorCode::InvalidCrs,
            format!("left source CRS {left_crs} differs from right source CRS {right_crs}; explicit transform not available in v0"),
        ));
    }
    match req.kind.as_str() {
        "near" | "near_count" => {
            if !lon_lat(&left_crs) {
                return invalid_request("near measures real metres: it needs a longitude/latitude source (OGC:CRS84 or EPSG:4326)");
            }
            let distance_m = with.distance_m.ok_or_else(|| PlanError::new(ErrorCode::InvalidRequest, "with.distance_m is required for near"))?;
            if !distance_m.is_finite() || !(0.0 < distance_m && distance_m <= MAX_NEAR_DISTANCE_M) {
                return invalid_request("with.distance_m must be greater than 0 and at most 5000");
            }
            let distance = Distance::new(distance_m, DistanceUnit::Meter).map_err(|e| PlanError::new(ErrorCode::InvalidRequest, e.to_string()))?;

            // Right-hand candidates: the viewport grown by the distance, IN DEGREES. This is only a pruning prefilter (a superset); the decision
            // "within 200 m" is taken in the metric CRS below. The margins are the conservative (larger) degree equivalents of the distance, so no
            // true neighbour is pruned.
            let lat_max = bbox.max_y().abs().max(bbox.min_y().abs()).min(89.0) + distance_m / METERS_PER_DEGREE_LAT_MIN;
            let dlat = distance_m / METERS_PER_DEGREE_LAT_MIN * 1.02;
            let dlon = distance_m / (METERS_PER_DEGREE_LON_EQUATOR * lat_max.to_radians().cos().max(0.01)) * 1.02;
            let expanded = BoundingBox::new(
                (bbox.min_x() - dlon).max(-180.0),
                (bbox.min_y() - dlat).max(-90.0),
                (bbox.max_x() + dlon).min(180.0),
                (bbox.max_y() + dlat).min(90.0),
                left_crs.clone(),
            )
            .map_err(|e| PlanError::new(ErrorCode::InvalidBbox, e.to_string()))?;
            // The metric CRS comes from the area the plan touches: refused (not guessed) when no single zone is accurate enough.
            let metric = local_metric_crs(&expanded)
                .map_err(|e| PlanError::new(ErrorCode::MetricCrsUnavailable, format!("cannot measure {distance_m} m here: {e}")))?;

            let l = apply_filter(Node::BboxFilter { input: Box::new(left.clone()), geometry_column: left_geometry.clone(), bbox }, req.filter.as_ref(), left_schema, &req.source)?;
            let l = apply_sample(l, req, left_schema)?;
            let left_columns: Vec<String> = left_schema.fields.iter().filter(|f| f.name != "bbox").map(|f| f.name.clone()).collect();
            // the original geometry stays (the result is drawn in lon/lat); a metric COPY of it is used for the predicate
            let metric_column = "geometry_metric".to_string();
            let l = Node::Transform {
                input: Box::new(l),
                geometry_column: left_geometry.clone(),
                input_crs: left_crs.clone(),
                output_crs: metric.clone(),
                output_column: metric_column.clone(),
            };
            let r = apply_filter(
                Node::BboxFilter { input: Box::new(right.clone()), geometry_column: right_geometry.clone(), bbox: expanded },
                with.filter.as_ref(),
                right_schema,
                &with.source,
            )?;
            let r = Node::Transform {
                input: Box::new(Node::Project { input: Box::new(r), columns: vec![right_geometry.clone()] }),
                geometry_column: right_geometry.clone(),
                input_crs: right_crs,
                output_crs: metric,
                output_column: right_geometry.clone(),
            };
            let semi = Node::SpatialSemiJoin {
                left: Box::new(l),
                right: Box::new(r),
                left_column: metric_column,
                right_column: right_geometry,
                predicate: SpatialPredicate::WithinDistance { distance },
            };
            if req.kind == "near_count" {
                // each building once: the semi join cannot multiply rows, so the count is the number of buildings
                return Ok(Plan::new(count_all(semi)));
            }
            let limit = req.limit.unwrap_or(20_000).min(MAX_FEATURES);
            Ok(Plan::new(Node::Limit { input: Box::new(Node::Project { input: Box::new(semi), columns: left_columns }), n: limit }))
        }
        _ => {
            let group_by = with.group_by.as_ref().ok_or_else(|| PlanError::new(ErrorCode::InvalidRequest, "with.group_by is required for join_count_by"))?;
            match right_schema.fields.iter().find(|f| f.name == *group_by) {
                Some(f) if f.data_type == FieldType::Utf8 => {}
                Some(_) => return invalid_request("with.group_by must name a utf8 column of the right source"),
                None => return Err(PlanError::new(ErrorCode::InvalidField, "with.group_by is not a column of the right source")),
            }
            let l = apply_filter(Node::BboxFilter { input: Box::new(left.clone()), geometry_column: left_geometry.clone(), bbox: bbox.clone() }, req.filter.as_ref(), left_schema, &req.source)?;
            let l = Node::Project { input: Box::new(l), columns: vec![left_geometry.clone()] };
            let r = apply_filter(Node::BboxFilter { input: Box::new(right.clone()), geometry_column: right_geometry.clone(), bbox }, with.filter.as_ref(), right_schema, &with.source)?;
            let r = Node::Project { input: Box::new(r), columns: vec![right_geometry.clone(), group_by.clone()] };
            let joined = Node::SpatialJoin {
                left: Box::new(l),
                right: Box::new(r),
                left_column: left_geometry,
                right_column: right_geometry,
                predicate: SpatialPredicate::Intersects,
                join_type: JoinType::Inner,
                right_prefix: "r_".into(),
            };
            Ok(Plan::new(Node::Limit {
                input: Box::new(Node::Sort {
                    input: Box::new(Node::Aggregate {
                        input: Box::new(joined),
                        group_by: vec![format!("r_{group_by}")],
                        aggregates: vec![AggExpr { func: AggFunc::Count, column: None, alias: "count".into() }],
                    }),
                    keys: vec![SortKey { column: "count".into(), descending: true }, SortKey { column: format!("r_{group_by}"), descending: false }],
                }),
                n: TOP_JOIN_GROUPS,
            }))
        }
    }
}

/// The request as the wire carries it (a JSON object) -> PlanGraph JSON, or the structured error. This is the whole interface of the WebAssembly
/// build and of anything else that does not link Rust types.
pub fn build_plan_json(request: &str, sources: &str) -> Result<String, PlanError> {
    let req: PlanRequest = serde_json::from_str(request).map_err(|e| PlanError::new(ErrorCode::InvalidRequest, format!("bad request: {e}")))?;
    let sources: Sources = serde_json::from_str(sources).map_err(|e| PlanError::new(ErrorCode::InvalidRequest, format!("bad sources: {e}")))?;
    Ok(build_plan(&req, &sources)?.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo_types::{Field, GeometryColumn, GeometryType};

    fn src() -> Node {
        Node::Source {
            uri: "places.parquet".into(),
            format: geo_ir::SourceFormat::GeoParquet,
            schema: SpatialSchema {
                fields: vec![
                    Field { name: "id".into(), data_type: FieldType::Utf8 },
                    Field { name: "category".into(), data_type: FieldType::Utf8 },
                    Field { name: "geometry".into(), data_type: FieldType::Other("geometry".into()) },
                    Field { name: "bbox".into(), data_type: FieldType::Other("struct".into()) },
                ],
                geometry_columns: vec![GeometryColumn { name: "geometry".into(), geometry_type: GeometryType::Point, crs: Crs::OgcCrs84 }],
            },
            statistics: Default::default(),
        }
    }

    fn req(kind: &str) -> PlanRequest {
        serde_json::from_value(serde_json::json!({"kind": kind, "source": "p", "bbox": [106.0, 10.0, 107.0, 11.0], "crs": "OGC:CRS84"})).unwrap()
    }

    fn sources() -> Sources {
        Sources::from([("p".to_string(), src())])
    }

    #[test]
    fn same_request_same_plan_bytes() {
        let a = build_plan(&req("features"), &sources()).unwrap().to_json();
        let b = build_plan(&req("features"), &sources()).unwrap().to_json();
        assert_eq!(a, b);
    }

    #[test]
    fn errors_are_structured_and_found_before_any_node_is_built() {
        let mut r = req("features");
        r.filter = Some(FilterRequest { column: "nope".into(), op: "eq".into(), value: Value::from("x"), values: vec![] });
        let e = build_plan(&r, &sources()).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidField);
        assert_eq!(e.http_status(), 422);
        assert_eq!(build_plan(&req("teleport"), &sources()).unwrap_err().code, ErrorCode::UnsupportedRequest);
        let mut r = req("features");
        r.source = "ghost".into();
        assert_eq!(build_plan(&r, &sources()).unwrap_err().code, ErrorCode::UnknownDataset);
        let mut r = req("features");
        r.bbox = [2.0, 0.0, 1.0, 1.0];
        assert_eq!(build_plan(&r, &sources()).unwrap_err().code, ErrorCode::InvalidBbox);
        let mut r = req("features");
        r.crs = "EPSG:32648".into();
        assert_eq!(build_plan(&r, &sources()).unwrap_err().code, ErrorCode::InvalidCrs);
        let mut r = req("features");
        r.limit = Some(0);
        assert_eq!(build_plan(&r, &sources()).unwrap_err().code, ErrorCode::InvalidLimit);
        let mut r = req("features");
        r.request_schema_version = Some(99);
        assert_eq!(build_plan(&r, &sources()).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn json_interface_never_panics_on_garbage() {
        for bad in ["", "null", "[]", "{\"kind\":1}", "{", "\u{0}"] {
            assert!(build_plan_json(bad, "{}").is_err(), "{bad:?}");
        }
        assert!(build_plan_json("{\"kind\":\"features\",\"source\":\"p\",\"bbox\":[1,2,3,4],\"crs\":\"OGC:CRS84\"}", "not json").is_err());
    }

    #[test]
    fn a_neighbour_query_is_built_exactly_even_though_a_browser_cannot_run_it() {
        let mut s = sources();
        s.insert("r".into(), src());
        let r: PlanRequest = serde_json::from_value(serde_json::json!({"kind": "near_count", "source": "p", "bbox": [106.0, 10.0, 106.1, 10.1], "crs": "OGC:CRS84", "with": {"source": "r", "distance_m": 200}})).unwrap();
        let plan = build_plan(&r, &s).unwrap();
        let kinds: Vec<&str> = plan.nodes.values().map(|n| n.kind().name()).collect();
        assert!(kinds.contains(&"spatial_semi_join") && kinds.contains(&"transform"), "{kinds:?}");
    }
}
