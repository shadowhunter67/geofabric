//! PlanGraph -> PostgreSQL/PostGIS SQL + bind parameters. Private to the adapter; the IR never sees SQL.
//!
//! Safety, in this order of importance:
//! 1. **Values are never SQL text.** Every literal (filter values, bbox coordinates, WKT, limits, distances) is a bind parameter with
//!    an explicit cast (`$1::double precision`). The SQL text contains no user-supplied value at all.
//! 2. **Identifiers come from the catalog**, validated once (`[A-Za-z_][A-Za-z0-9_]*`). A column named by a plan (group_by,
//!    project, sort, filter...) must be a column the catalog/schema knows; anything else is refused, so a plan cannot smuggle an
//!    identifier. Identifiers are quoted anyway.
//! 3. Unsupported operations are refused with a reason, never approximated (no `geography`, no silent type coercion).
//!
//! Semantics kept identical to the DuckDB lowering (`geo-engine-duckdb/src/sql.rs`), including the places where PostgreSQL's
//! defaults differ: `NULLS LAST` for both directions (DuckDB's default; PostgreSQL's DESC default is NULLS FIRST), byte-order
//! collation (`COLLATE "C"`) for text sort keys, `ST_AsBinary(.., 'NDR')` for little-endian WKB, bbox filter =
//! closed-interval overlap (`&&`), `ST_DWithin` inclusive, semi-join as `EXISTS` (each left row at most once), transforms in
//! (lon, lat) / (x, y) order through the SRID mapping in [`crate::crs`].

use crate::catalog::{PgColType, PostGisCatalog, PostGisSource};
use crate::connector::Param;
use crate::crs::srid_for;
use geo_ir::{AggFunc, CompareOp, Expr, Literal, NodeId, Plan, PlanNode, SpatialPredicate, SourceFormat};
use geo_lowering::{LowerError as Canon, PredicateDialect};
use geo_types::{Crs, SpatialSchema};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LowerError {
    #[error("invalid plan: {0}")]
    Invalid(String),
    /// A feature this adapter does not implement (or refuses to approximate).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The plan names a source that is not resident in the database.
    #[error("source not resident in PostGIS: {0}")]
    NotResident(String),
}

impl From<Canon> for LowerError {
    fn from(e: Canon) -> Self {
        match e {
            Canon::Invalid(m) => LowerError::Invalid(m),
            Canon::Unsupported(m) => LowerError::Unsupported(m),
        }
    }
}

fn canon(e: LowerError) -> Canon {
    match e {
        LowerError::Invalid(m) => Canon::Invalid(m),
        LowerError::Unsupported(m) | LowerError::NotResident(m) => Canon::Unsupported(m),
    }
}

/// A lowered statement.
#[derive(Debug, Clone)]
pub struct Lowered {
    pub sql: String,
    pub params: Vec<Param>,
    /// Geometry columns of the result (WKB in the Arrow result) and their GeoFabric CRS.
    pub geometry_outputs: Vec<(String, Crs)>,
    /// Logical uris of the sources read (all of them resident).
    pub sources: Vec<String>,
    /// Columns of the plan's source schemas that the relation does not store (e.g. a GeoParquet `bbox` covering struct). They
    /// are dropped at the scan; using one later is an error, so the result differs from DuckDB's only by these absent columns.
    pub dropped_columns: Vec<String>,
}

pub(crate) fn q_ident(s: &str) -> String {
    geo_lowering::quote_ident(s)
}

#[derive(Clone, Debug)]
enum Col {
    /// geometry column with its GeoFabric CRS
    Geometry(Crs),
    Plain(PgColType),
}

#[derive(Clone)]
struct Out {
    name: String,
    schema: SpatialSchema,
    cols: BTreeMap<String, Col>,
    /// The file bbox COVERING columns travel with the rows as hidden columns `__cov_xmin..__cov_ymax` while the chain preserves them
    /// (Filter, BboxFilter, Intersects, Limit, Sort - exactly where the DuckDB lowering keeps its covering). `BboxFilter` then means
    /// "the covering box overlaps the window", the definition every other engine uses. Cleared by Transform, Project, Aggregate, joins.
    cov: Option<Cov>,
}

#[derive(Clone)]
struct Cov {
    geom: String,
    crs: Crs,
}

const COV: [&str; 4] = ["__cov_xmin", "__cov_ymin", "__cov_xmax", "__cov_ymax"];

struct Lower<'a> {
    plan: &'a Plan,
    catalog: &'a PostGisCatalog,
    schemas: BTreeMap<NodeId, SpatialSchema>,
    memo: HashMap<NodeId, Out>,
    ctes: Vec<(String, String)>,
    params: Vec<Param>,
    sources: BTreeSet<String>,
    dropped: BTreeSet<String>,
    /// > 0 while compiling the right-hand input of a semi join (see `BboxFilter`)
    inner: usize,
}

/// Lower `plan` for a database that holds the sources in `catalog`. `max_rows` caps the result (`max_rows + 1` rows are
/// requested so the caller can tell "exactly the cap" from "over the cap", as in the DuckDB engine).
pub fn lower(plan: &Plan, catalog: &PostGisCatalog, max_rows: u64) -> Result<Lowered, LowerError> {
    let schemas = plan.schemas().map_err(|e| LowerError::Invalid(e.to_string()))?;
    let mut l = Lower { plan, catalog, schemas, memo: HashMap::new(), ctes: Vec::new(), params: Vec::new(), sources: BTreeSet::new(), dropped: BTreeSet::new(), inner: 0 };
    let root = plan.root.clone();
    let out = l.node(&root)?;
    let with = l.ctes.iter().map(|(n, s)| format!("{} AS ({})", q_ident(n), s)).collect::<Vec<_>>().join(",\n");
    let geometry_outputs: Vec<(String, Crs)> = out
        .schema
        .fields
        .iter()
        .filter_map(|f| match out.cols.get(&f.name) {
            Some(Col::Geometry(crs)) => Some((f.name.clone(), crs.clone())),
            _ => None,
        })
        .collect();
    let select = out
        .schema
        .fields
        .iter()
        .filter(|f| out.cols.contains_key(&f.name))
        .map(|f| match out.cols.get(&f.name) {
            Some(Col::Geometry(_)) => format!("ST_AsBinary({0}, 'NDR') AS {0}", q_ident(&f.name)),
            _ => q_ident(&f.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if select.is_empty() {
        return Err(LowerError::Invalid("the plan produces no column that the relation stores".into()));
    }
    let limit = l.bind(Param::Int(i64::try_from(geo_lowering::row_cap(max_rows)).unwrap_or(i64::MAX)), "bigint");
    let sql = format!("WITH {with}\nSELECT {select} FROM {} LIMIT {limit}", q_ident(&out.name));
    Ok(Lowered { sql, params: l.params, geometry_outputs, sources: l.sources.into_iter().collect(), dropped_columns: l.dropped.into_iter().collect() })
}

/// PostgreSQL's spelling of predicate leaves: every literal is a bind parameter with an explicit cast, against the column's catalog type.
struct PgPredicates<'x, 'a> {
    low: &'x mut Lower<'a>,
    out: &'x Out,
}

impl PredicateDialect for PgPredicates<'_, '_> {
    fn compare(&mut self, column: &str, op: CompareOp, value: &Literal) -> Result<String, Canon> {
        let ty = Lower::plain_type(self.out, column).map_err(canon)?;
        let lit = self.low.literal(column, ty, value).map_err(canon)?;
        Ok(format!("{} {} {lit}", q_ident(column), geo_lowering::cmp_symbol(op)))
    }
    fn in_list(&mut self, column: &str, values: &[Literal]) -> Result<String, Canon> {
        let ty = Lower::plain_type(self.out, column).map_err(canon)?;
        let mut items = Vec::new();
        for v in values {
            items.push(self.low.literal(column, ty, v).map_err(canon)?);
        }
        Ok(format!("{} IN ({})", q_ident(column), items.join(", ")))
    }
    fn is_null(&mut self, column: &str) -> Result<String, Canon> {
        Lower::need_col(self.out, column, "filter").map_err(canon)?;
        Ok(format!("{} IS NULL", q_ident(column)))
    }
    fn hash_bucket(&mut self, _column: &str, _modulus: u32, _remainder: u32) -> Result<String, Canon> {
        Err(Canon::Unsupported(
            "hash_bucket (render sampling): DuckDB's md5_number_lower has no byte-exact PostgreSQL equivalent that is also portable; sampling stays on the other engines".into(),
        ))
    }
}

impl<'a> Lower<'a> {
    fn bind(&mut self, p: Param, cast: &str) -> String {
        self.params.push(p);
        format!("${}::{cast}", self.params.len())
    }

    fn float(&mut self, v: f64) -> Result<String, LowerError> {
        // the canonical literal rules (finite, inside the portable range) apply to a bound number as well as to an inline one
        geo_lowering::format_float(v)?;
        Ok(self.bind(Param::Float(v), "double precision"))
    }

    fn push(&mut self, sql: String) -> String {
        let name = format!("n{}", self.ctes.len());
        self.ctes.push((name.clone(), sql));
        name
    }

    fn schema_of(&self, id: &NodeId) -> Result<SpatialSchema, LowerError> {
        self.schemas.get(id).cloned().ok_or_else(|| LowerError::Invalid(format!("no schema for node {id}")))
    }

    /// Columns of `schema` (in schema order) that exist in `cols`, as a quoted select list, with optional per-column expressions.
    fn select_list(schema: &SpatialSchema, cols: &BTreeMap<String, Col>, prefix: &str, replace: &BTreeMap<String, String>) -> String {
        schema
            .fields
            .iter()
            .filter(|f| cols.contains_key(&f.name))
            .map(|f| match replace.get(&f.name) {
                Some(expr) => format!("{expr} AS {}", q_ident(&f.name)),
                None => format!("{prefix}{}", q_ident(&f.name)),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The select list of a node that passes its input through unchanged (hidden covering columns included).
    fn pass_list(i: &Out) -> String {
        let mut list = Self::select_list(&i.schema, &i.cols, "", &BTreeMap::new());
        if i.cov.is_some() {
            for c in COV {
                list += &format!(", {}", q_ident(c));
            }
        }
        list
    }

    fn need_col<'o>(out: &'o Out, name: &str, what: &str) -> Result<&'o Col, LowerError> {
        out.cols.get(name).ok_or_else(|| {
            LowerError::Unsupported(format!("{what}: column {name:?} is not stored in the PostGIS relation or not produced by the plan"))
        })
    }

    fn geometry_of(out: &Out, name: &str, what: &str) -> Result<Crs, LowerError> {
        match Self::need_col(out, name, what)? {
            Col::Geometry(crs) => Ok(crs.clone()),
            Col::Plain(_) => Err(LowerError::Invalid(format!("{what}: {name:?} is not a geometry column"))),
        }
    }

    fn literal(&mut self, column: &str, ty: PgColType, l: &Literal) -> Result<String, LowerError> {
        let mismatch = |what: &str| {
            LowerError::Unsupported(format!(
                "comparing {column:?} ({ty:?}) with a {what} literal needs a type coercion; PostgreSQL would refuse or differ from DuckDB, so the adapter does not guess"
            ))
        };
        Ok(match l {
            Literal::Null => "NULL".to_string(),
            Literal::Int(i) => match ty {
                PgColType::Int | PgColType::Float => self.bind(Param::Int(*i), "bigint"),
                _ => return Err(mismatch("integer")),
            },
            Literal::Float(f) => match ty {
                PgColType::Int | PgColType::Float => self.float(*f)?,
                _ => return Err(mismatch("float")),
            },
            Literal::Str(s) => match ty {
                PgColType::Text => self.bind(Param::Text(s.clone()), "text"),
                _ => return Err(mismatch("string")),
            },
            Literal::Bool(b) => match ty {
                PgColType::Bool => self.bind(Param::Bool(*b), "boolean"),
                _ => return Err(mismatch("boolean")),
            },
        })
    }

    fn plain_type(out: &Out, column: &str) -> Result<PgColType, LowerError> {
        match Self::need_col(out, column, "filter")? {
            Col::Plain(t) => Ok(*t),
            Col::Geometry(_) => Err(LowerError::Invalid(format!("{column:?} is a geometry column and cannot be compared with a literal"))),
        }
    }

    /// The predicate tree is canonical (`geo_lowering::predicate`); this dialect binds the leaves and refuses what PostgreSQL would
    /// coerce differently from DuckDB.
    fn expr(&mut self, out: &Out, e: &Expr) -> Result<String, LowerError> {
        Ok(geo_lowering::predicate(e, &mut PgPredicates { low: self, out })?)
    }

    fn node(&mut self, id: &NodeId) -> Result<Out, LowerError> {
        if let Some(done) = self.memo.get(id) {
            return Ok(done.clone());
        }
        let out = self.compile_node(id)?;
        self.memo.insert(id.clone(), out.clone());
        Ok(out)
    }

    fn resident(&self, uri: &str) -> Result<&'a PostGisSource, LowerError> {
        let catalog: &'a PostGisCatalog = self.catalog;
        catalog.get(uri).ok_or_else(|| LowerError::NotResident(uri.to_string()))
    }

    fn compile_node(&mut self, id: &NodeId) -> Result<Out, LowerError> {
        let plan = self.plan;
        let n = plan.nodes.get(id).ok_or_else(|| LowerError::Invalid(format!("missing node {id}")))?;
        match n {
            PlanNode::Source { uri, format, schema, .. } => {
                if *format != SourceFormat::GeoParquet {
                    return Err(LowerError::Unsupported(format!("source format {format:?}")));
                }
                let src = self.resident(uri)?;
                if let Some(cov) = src.coverage {
                    // a clipped copy answers only what lies inside its coverage: every consumer of this source must be a bbox filter
                    // on its geometry column whose window is inside the coverage, or the plan would silently see fewer rows than
                    // the logical source holds (the other engines see all of them)
                    let consumers: Vec<&PlanNode> = plan.nodes.values().filter(|n| n.inputs().contains(&id)).collect();
                    if consumers.is_empty() {
                        return Err(LowerError::Unsupported(format!("{uri}: the relation is a clipped copy (coverage {cov:?}); an unwindowed read of the whole source cannot be answered from it")));
                    }
                    for c in consumers {
                        match c {
                            PlanNode::BboxFilter { geometry_column, bbox, .. }
                                if *geometry_column == src.geometry_column
                                    && *bbox.crs() == src.crs_value()
                                    && bbox.min_x() >= cov[0]
                                    && bbox.min_y() >= cov[1]
                                    && bbox.max_x() <= cov[2]
                                    && bbox.max_y() <= cov[3] => {}
                            _ => {
                                return Err(LowerError::Unsupported(format!(
                                    "{uri}: the relation is a clipped copy (coverage {cov:?}); it can only be read through a bbox filter whose window lies inside the coverage"
                                )))
                            }
                        }
                    }
                }
                let g = schema
                    .geometry(&src.geometry_column)
                    .ok_or_else(|| LowerError::Unsupported(format!("{uri}: the plan's schema has no geometry column {:?}", src.geometry_column)))?;
                if g.crs != src.crs_value() {
                    return Err(LowerError::Unsupported(format!(
                        "{uri}: the plan declares CRS {} but the relation stores {} (OGC:CRS84 and EPSG:4326 are different CRSs at the semantic layer; no implicit cast)",
                        g.crs, src.crs
                    )));
                }
                let mut cols: BTreeMap<String, Col> = BTreeMap::new();
                for f in &schema.fields {
                    if f.name == src.geometry_column {
                        cols.insert(f.name.clone(), Col::Geometry(g.crs.clone()));
                    } else if let Some(t) = src.columns.get(&f.name) {
                        cols.insert(f.name.clone(), Col::Plain(*t));
                    } else {
                        self.dropped.insert(f.name.clone());
                    }
                }
                self.sources.insert(uri.clone());
                let mut list = Self::select_list(schema, &cols, "", &BTreeMap::new());
                let mut cov = None;
                if let Some(c) = &src.covering {
                    for (alias, col) in COV.iter().zip([&c.xmin, &c.ymin, &c.xmax, &c.ymax]) {
                        list += &format!(", {} AS {}", q_ident(col), q_ident(alias));
                    }
                    cov = Some(Cov { geom: src.geometry_column.clone(), crs: g.crs.clone() });
                }
                let name = self.push(format!("SELECT {list} FROM {}", src.qualified()));
                Ok(Out { name, schema: self.schema_of(id)?, cols, cov })
            }
            PlanNode::Filter { input, predicate } => {
                let i = self.node(input)?;
                let w = self.expr(&i, predicate)?;
                let list = Self::pass_list(&i);
                let name = self.push(format!("SELECT {list} FROM {} WHERE {w}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::BboxFilter { input, geometry_column, bbox } => {
                let i = self.node(input)?;
                let crs = Self::geometry_of(&i, geometry_column, "bbox filter")?;
                if &crs != bbox.crs() {
                    return Err(LowerError::Invalid(format!("bbox CRS {} differs from the geometry CRS {crs}", bbox.crs())));
                }
                let srid = srid_for(&crs).map_err(LowerError::Unsupported)?;
                let (x0, y0, x1, y1) = (self.float(bbox.min_x())?, self.float(bbox.min_y())?, self.float(bbox.max_x())?, self.float(bbox.max_y())?);
                let g = q_ident(geometry_column);
                let list = Self::pass_list(&i);
                let covered = i.cov.as_ref().is_some_and(|c| c.geom == *geometry_column && c.crs == crs);
                // The right-hand input of a semi join is probed once PER LEFT ROW. There the envelope `&&` (an index assist for a standalone
                // windowed scan) is a pessimization: on 3.8 M buildings the planner repeated the covering-index probe 41,150 times
                // (10.4 s); with only the exact comparisons, which decide the result anyway, the same plan took 0.9 s. Results are unchanged.
                let assist = self.inner == 0;
                let pred = if covered {
                    // The definition the other engines use: the file covering box overlaps the window, in exact double arithmetic.
                    // (Overture covering boxes are padded by up to ~1.6 m; `&&` on the geometry, whose float4 box differs again, gave
                    // 41,165 rows where DuckDB and the browser give 41,150 - found by the first live cross-check.) The envelope
                    // overlap first lets a functional GiST index on the covering envelope prune; the four comparisons decide.
                    let env = format!("ST_MakeEnvelope({}, {}, {}, {}, {srid})", q_ident(COV[0]), q_ident(COV[1]), q_ident(COV[2]), q_ident(COV[3]));
                    // the canonical BboxFilter: the covering box overlaps the window (geo_lowering::bbox_covering_overlap)
                    let cov_cols = [q_ident(COV[0]), q_ident(COV[1]), q_ident(COV[2]), q_ident(COV[3])];
                    let exact = geo_lowering::bbox_covering_overlap(&cov_cols, &[x0.clone(), y0.clone(), x1.clone(), y1.clone()]);
                    if assist { format!("{env} && ST_MakeEnvelope({x0}, {y0}, {x1}, {y1}, {srid}) AND {exact}") } else { exact }
                } else {
                    // No covering (or it was lost on the way): exact envelope overlap like DuckDB fallback; `&&` is the index-assisted,
                    // outward-rounded prefilter that can only add candidates, ST_Intersects on the exact envelope decides.
                    let exact = format!("ST_Intersects(ST_Envelope({g}), ST_MakeEnvelope({x0}, {y0}, {x1}, {y1}, {srid}))");
                    if assist { format!("{g} && ST_MakeEnvelope({x0}, {y0}, {x1}, {y1}, {srid}) AND {exact}") } else { exact }
                };
                let name = self.push(format!("SELECT {list} FROM {} WHERE {pred}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::Intersects { input, geometry_column, with } => {
                let i = self.node(input)?;
                let crs = Self::geometry_of(&i, geometry_column, "intersects")?;
                if crs != with.crs {
                    return Err(LowerError::Invalid(format!("literal CRS {} differs from the geometry CRS {crs}", with.crs)));
                }
                if with.wkt.len() > 1_000_000 {
                    return Err(LowerError::Invalid("WKT literal is too large".into()));
                }
                let srid = srid_for(&crs).map_err(LowerError::Unsupported)?;
                let w = self.bind(Param::Text(with.wkt.clone()), "text");
                let list = Self::pass_list(&i);
                let name = self.push(format!(
                    "SELECT {list} FROM {} WHERE ST_Intersects({}, ST_GeomFromText({w}, {srid}))",
                    q_ident(&i.name),
                    q_ident(geometry_column)
                ));
                Ok(Out { name, ..i })
            }
            PlanNode::Transform { input, geometry_column, input_crs, output_crs, output_column } => {
                let i = self.node(input)?;
                let crs = Self::geometry_of(&i, geometry_column, "transform")?;
                if &crs != input_crs {
                    return Err(LowerError::Invalid(format!("transform input CRS {input_crs} differs from the geometry CRS {crs}")));
                }
                let from = srid_for(input_crs).map_err(LowerError::Unsupported)?;
                let to = srid_for(output_crs).map_err(LowerError::Unsupported)?;
                // ST_Transform(g, srid): the stored coordinates are (lon, lat) / (x, y); no axis swap happens (crate::crs)
                let t = format!("ST_Transform(ST_SetSRID({}, {from}), {to})", q_ident(geometry_column));
                let schema = self.schema_of(id)?;
                let mut cols = i.cols.clone();
                cols.insert(output_column.clone(), Col::Geometry(output_crs.clone()));
                let mut replace = BTreeMap::new();
                replace.insert(output_column.clone(), t);
                let list = Self::select_list(&schema, &cols, "", &replace);
                let name = self.push(format!("SELECT {list} FROM {}", q_ident(&i.name)));
                Ok(Out { name, schema, cols, cov: None })
            }
            PlanNode::SpatialSemiJoin { left, right, left_column, right_column, predicate } => {
                let l = self.node(left)?;
                self.inner += 1;
                let r = self.node(right);
                self.inner -= 1;
                let r = r?;
                let lc = Self::geometry_of(&l, left_column, "semi join (left)")?;
                let rc = Self::geometry_of(&r, right_column, "semi join (right)")?;
                if lc != rc {
                    return Err(LowerError::Invalid(format!("semi join of different CRSs ({lc} vs {rc}); transform explicitly")));
                }
                let pred = match predicate {
                    SpatialPredicate::Intersects => format!("ST_Intersects(l.{}, r.{})", q_ident(left_column), q_ident(right_column)),
                    SpatialPredicate::WithinDistance { distance } => {
                        // metres, never degrees (canonical rule), and only in the plane of a local metric CRS
                        let d = geo_lowering::within_distance_meters(distance)?;
                        if !lc.is_local_metric() {
                            return Err(LowerError::Invalid(format!("metric within_distance on {lc}, which is not a local metric CRS (transform first)")));
                        }
                        let d = self.float(d)?;
                        format!("ST_DWithin(l.{}, r.{}, {d})", q_ident(left_column), q_ident(right_column))
                    }
                };
                // EXISTS: each left row at most once and no pair table (a join + DISTINCT would build one)
                let list = Self::select_list(&l.schema, &l.cols, "l.", &BTreeMap::new());
                // canonical form: EXISTS, each left row at most once
                let name = self.push(geo_lowering::semi_join_exists(&list, &q_ident(&l.name), &q_ident(&r.name), &pred));
                Ok(Out { name, schema: self.schema_of(id)?, cols: l.cols, cov: None })
            }
            PlanNode::Buffer { .. } => Err(LowerError::Unsupported("buffer (not needed since SpatialSemiJoin; not implemented for PostGIS)".into())),
            PlanNode::SpatialJoin { .. } => Err(LowerError::Unsupported("spatial join (deferred: pair-producing joins are not in PostGIS v0)".into())),
            PlanNode::Aggregate { input, group_by, aggregates } => {
                let i = self.node(input)?;
                let mut cols: BTreeMap<String, Col> = BTreeMap::new();
                let mut sel: Vec<String> = Vec::new();
                for g in group_by {
                    match Self::need_col(&i, g, "group by")? {
                        Col::Plain(t) => {
                            cols.insert(g.clone(), Col::Plain(*t));
                            sel.push(q_ident(g));
                        }
                        Col::Geometry(_) => return Err(LowerError::Invalid(format!("cannot group by the geometry column {g:?}"))),
                    }
                }
                for a in aggregates {
                    let col_ty = match &a.column {
                        Some(c) => match Self::need_col(&i, c, "aggregate")? {
                            Col::Plain(t) => Some(*t),
                            Col::Geometry(_) => return Err(LowerError::Invalid(format!("cannot aggregate the geometry column {c:?}"))),
                        },
                        None => None,
                    };
                    let call = || geo_lowering::aggregate_call(a.func, a.column.as_deref()).map_err(LowerError::from);
                    // the canonical call, plus the result cast PostgreSQL needs to give the same type as DuckDB (SUM of bigint is numeric)
                    let (expr, ty) = match (a.func, &a.column, col_ty) {
                        (AggFunc::Count, _, _) => (call()?, PgColType::Int),
                        (AggFunc::Sum, Some(_), Some(PgColType::Int)) => (format!("{}::bigint", call()?), PgColType::Int),
                        (AggFunc::Sum, Some(_), Some(PgColType::Float)) => (format!("{}::double precision", call()?), PgColType::Float),
                        (AggFunc::Avg, Some(_), Some(PgColType::Int | PgColType::Float)) => (format!("{}::double precision", call()?), PgColType::Float),
                        (AggFunc::Min | AggFunc::Max, Some(_), Some(t @ (PgColType::Int | PgColType::Float | PgColType::Text))) => (call()?, t),
                        (f, c, t) => return Err(LowerError::Unsupported(format!("aggregate {f:?} over {c:?} of type {t:?}"))),
                    };
                    cols.insert(a.alias.clone(), Col::Plain(ty));
                    sel.push(format!("{expr} AS {}", q_ident(&a.alias)));
                }
                let mut sql = format!("SELECT {} FROM {}", sel.join(", "), q_ident(&i.name));
                if !group_by.is_empty() {
                    sql += &format!(" GROUP BY {}", group_by.iter().map(|g| q_ident(g)).collect::<Vec<_>>().join(", "));
                }
                let name = self.push(sql);
                Ok(Out { name, schema: self.schema_of(id)?, cols, cov: None })
            }
            PlanNode::Project { input, columns } => {
                let i = self.node(input)?;
                let mut cols = BTreeMap::new();
                for c in columns {
                    cols.insert(c.clone(), Self::need_col(&i, c, "project")?.clone());
                }
                let sel = columns.iter().map(|c| q_ident(c)).collect::<Vec<_>>().join(", ");
                let name = self.push(format!("SELECT {sel} FROM {}", q_ident(&i.name)));
                Ok(Out { name, schema: self.schema_of(id)?, cols, cov: None })
            }
            PlanNode::Limit { input, n: lim } => {
                let i = self.node(input)?;
                let p = self.bind(Param::Int(i64::try_from(*lim).unwrap_or(i64::MAX)), "bigint");
                let list = Self::pass_list(&i);
                let name = self.push(format!("SELECT {list} FROM {} LIMIT {p}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::Sort { input, keys } => {
                let i = self.node(input)?;
                // the order itself is canonical (direction, NULLS LAST both ways); PostgreSQL only has to ask for byte-order text comparison,
                // because the database collation must not decide the order or ties
                for k in keys {
                    if let Col::Geometry(_) = Self::need_col(&i, &k.column, "sort")? {
                        return Err(LowerError::Invalid(format!("cannot sort by the geometry column {:?}", k.column)));
                    }
                }
                let is_text = |c: &str| matches!(i.cols.get(c), Some(Col::Plain(PgColType::Text)));
                let ob = geo_lowering::order_by(keys, &|c| if is_text(c) { Some(" COLLATE \"C\"") } else { None })?;
                let list = Self::pass_list(&i);
                let name = self.push(format!("SELECT {list} FROM {} ORDER BY {ob}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
        }
    }
}
