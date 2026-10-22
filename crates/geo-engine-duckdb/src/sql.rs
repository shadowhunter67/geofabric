//! Logical IR -> DuckDB SQL. Private to the adapter; the IR never sees SQL.
//!
//! Safety: every identifier is quoted, every literal is escaped or numeric,
//! file paths are validated against the allow-list, WKT literals are
//! restricted to a safe character set. No user string is concatenated raw.

use geo_executor::{EngineError, ExecutionContext};
use geo_ir::{Expr, JoinType, NodeId, Plan, PlanNode, SpatialPredicate};
use geo_source_geoparquet::{wkt_envelope, GeoParquetSource};
use geo_types::{AuthorizedSource, Crs, SpatialSchema};
use std::path::Path;

pub(crate) fn q_ident(s: &str) -> String {
    geo_lowering::quote_ident(s)
}

pub(crate) fn q_str(s: &str) -> String {
    geo_lowering::quote_string(s)
}

/// Canonical lowering errors become this engine's taxonomy.
fn lowered<T>(r: Result<T, geo_lowering::LowerError>) -> Result<T, EngineError> {
    r.map_err(|e| match e {
        geo_lowering::LowerError::Invalid(m) => EngineError::InvalidPlan(m),
        geo_lowering::LowerError::Unsupported(m) => EngineError::Unsupported(m),
    })
}

fn num(v: f64) -> Result<String, EngineError> {
    lowered(geo_lowering::format_float(v))
}

/// The server dialect keeps 64-bit integers exact (no JavaScript limit); everything else about a predicate is the canonical tree.
fn expr(e: &Expr) -> Result<String, EngineError> {
    lowered(geo_lowering::predicate(e, &mut geo_lowering::DuckDbPredicates { safe_int_only: false }))
}

fn proj_name(c: &Crs) -> Result<String, EngineError> {
    lowered(geo_lowering::proj_name(c))
}

fn safe_wkt(w: &str) -> Result<(), EngineError> {
    if w.len() > 1_000_000 || !w.chars().all(|c| c.is_ascii_alphanumeric() || " (),.+-".contains(c)) {
        return Err(EngineError::InvalidPlan("WKT literal contains unsupported characters".into()));
    }
    Ok(())
}

#[derive(Clone)]
struct CoveringRef {
    geom: String,
    /// SQL expressions for xmin, ymin, xmax, ymax
    cols: [String; 4],
    top_level: String,
    crs: Crs,
}

#[derive(Clone)]
struct Out {
    name: String,
    schema: SpatialSchema,
    covering: Option<CoveringRef>,
}

pub(crate) struct Compiled {
    pub sql: String,
    /// Sources that passed the guard while compiling (used to sandbox DuckDB).
    pub sources: Vec<AuthorizedSource>,
    /// Geometry output columns and their CRS (from the IR schema), for Arrow metadata.
    pub geometry_columns: Vec<(String, Crs)>,
}

struct Translator<'a> {
    plan: &'a Plan,
    schemas: std::collections::BTreeMap<NodeId, SpatialSchema>,
    /// One CTE per plan node: a shared node is compiled exactly once and referenced by name.
    memo: std::collections::HashMap<NodeId, Out>,
    ctx: &'a ExecutionContext,
    spatial: bool,
    ctes: Vec<(String, String)>,
    sources: Vec<AuthorizedSource>,
}

pub(crate) fn compile(plan: &Plan, ctx: &ExecutionContext, spatial: bool) -> Result<Compiled, EngineError> {
    let schemas = plan.schemas().map_err(|e| EngineError::InvalidPlan(e.to_string()))?;
    let mut t = Translator { plan, schemas, memo: Default::default(), ctx, spatial, ctes: Vec::new(), sources: Vec::new() };
    let root = plan.root.clone();
    let out = t.node(&root)?;
    let with = t.ctes.iter().map(|(n, s)| format!("{} AS NOT MATERIALIZED ({})", q_ident(n), s)).collect::<Vec<_>>().join(",\n");
    let geometry_columns: Vec<(String, Crs)> =
        out.schema.geometry_columns.iter().map(|g| (g.name.clone(), g.crs.clone())).collect();
    if !geometry_columns.is_empty() && !spatial {
        return Err(EngineError::Unsupported("returning geometry requires the DuckDB spatial extension".into()));
    }
    // GEOMETRY -> WKB explicitly: the Arrow export of DuckDB's native GEOMETRY type is not
    // reliable through the Rust binding, and WKB + IR-declared CRS keeps the CRS explicit.
    let select = if geometry_columns.is_empty() {
        "*".to_string()
    } else {
        let r = geometry_columns
            .iter()
            .map(|(n, _)| format!("ST_AsWKB({0}) AS {0}", q_ident(n)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("* REPLACE ({r})")
    };
    let sql = format!("WITH {with}\nSELECT {select} FROM {} LIMIT {}", q_ident(&out.name), geo_lowering::row_cap(ctx.max_rows));
    Ok(Compiled { sql, sources: t.sources, geometry_columns })
}

pub(crate) fn sql_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_string()
}

impl Translator<'_> {
    fn need_spatial(&self, what: &str) -> Result<(), EngineError> {
        if self.spatial {
            Ok(())
        } else {
            Err(EngineError::Unsupported(format!("{what} requires the DuckDB spatial extension (not available)")))
        }
    }

    fn push(&mut self, sql: String) -> String {
        let name = format!("n{}", self.ctes.len());
        self.ctes.push((name.clone(), sql));
        name
    }

    fn schema_of(&self, id: &NodeId) -> Result<SpatialSchema, EngineError> {
        self.schemas.get(id).cloned().ok_or_else(|| EngineError::InvalidPlan(format!("no schema for node {id}")))
    }

    fn keep_covering(&self, c: &Option<CoveringRef>, schema: &SpatialSchema) -> Option<CoveringRef> {
        c.clone()
            .filter(|c| schema.fields.iter().any(|f| f.name == c.top_level) && schema.geometry(&c.geom).is_some())
    }

    fn node(&mut self, id: &NodeId) -> Result<Out, EngineError> {
        if let Some(done) = self.memo.get(id) {
            return Ok(done.clone());
        }
        let out = self.compile_node(id)?;
        self.memo.insert(id.clone(), out.clone());
        Ok(out)
    }

    fn compile_node(&mut self, id: &NodeId) -> Result<Out, EngineError> {
        let plan = self.plan;
        let n = plan.nodes.get(id).ok_or_else(|| EngineError::InvalidPlan(format!("missing node {id}")))?;
        match n {
            PlanNode::Source { uri, .. } => {
                let authorized = self.ctx.guard.authorize(uri).map_err(|_| EngineError::AccessDenied)?;
                let src = GeoParquetSource::open_authorized(&authorized).map_err(|e| EngineError::Engine(e.to_string()))?;
                let p = authorized.path().to_path_buf();
                self.sources.push(authorized);
                let info = src.info();
                let covering = info.covering.as_ref().map(|c| {
                    let sqlcol = |path: &str| path.split('.').map(q_ident).collect::<Vec<_>>().join(".");
                    CoveringRef {
                        geom: info.geometry_column.clone(),
                        cols: [sqlcol(&c.xmin), sqlcol(&c.ymin), sqlcol(&c.xmax), sqlcol(&c.ymax)],
                        top_level: c.xmin.split('.').next().unwrap_or("").to_string(),
                        crs: info.crs.clone(),
                    }
                });
                let name = self.push(format!("SELECT * FROM read_parquet({})", q_str(&sql_path(&p))));
                Ok(Out { name, schema: self.schema_of(id)?, covering })
            }
            PlanNode::Filter { input, predicate } => {
                let i = self.node(input)?;
                let name = self.push(format!("SELECT * FROM {} WHERE {}", q_ident(&i.name), expr(predicate)?));
                Ok(Out { name, ..i })
            }
            PlanNode::BboxFilter { input, geometry_column, bbox } => {
                let i = self.node(input)?;
                let (x0, y0, x1, y1) =
                    (num(bbox.min_x())?, num(bbox.min_y())?, num(bbox.max_x())?, num(bbox.max_y())?);
                let pred = match i.covering.as_ref().filter(|c| c.geom == *geometry_column && c.crs == *bbox.crs()) {
                    Some(c) => geo_lowering::bbox_covering_overlap(&c.cols, &[x0.clone(), y0.clone(), x1.clone(), y1.clone()]),
                    None => {
                        self.need_spatial("bbox filter without a bbox covering column")?;
                        format!(
                            "ST_Intersects(ST_Envelope({}), ST_MakeEnvelope({x0}, {y0}, {x1}, {y1}))",
                            q_ident(geometry_column)
                        )
                    }
                };
                let name = self.push(format!("SELECT * FROM {} WHERE {pred}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::Intersects { input, geometry_column, with } => {
                self.need_spatial("intersects")?;
                safe_wkt(&with.wkt)?;
                let i = self.node(input)?;
                let g = q_ident(geometry_column);
                let mut pred = format!("ST_Intersects({g}, ST_GeomFromText({}))", q_str(&with.wkt));
                // Safe prefilter on covering columns => row-group pruning in the scan.
                if let (Some(c), Some(e)) = (
                    i.covering.as_ref().filter(|c| c.geom == *geometry_column && c.crs == with.crs),
                    wkt_envelope(&with.wkt, &with.crs),
                ) {
                    pred = format!(
                        "{} <= {} AND {} >= {} AND {} <= {} AND {} >= {} AND {pred}",
                        c.cols[0],
                        num(e.max_x())?,
                        c.cols[2],
                        num(e.min_x())?,
                        c.cols[1],
                        num(e.max_y())?,
                        c.cols[3],
                        num(e.min_y())?
                    );
                }
                let name = self.push(format!("SELECT * FROM {} WHERE {pred}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::Buffer {
                input, geometry_column, distance, input_crs, output_crs, error_tolerance, output_column,
            } => {
                self.need_spatial("buffer")?;
                if input_crs != output_crs {
                    return Err(EngineError::Unsupported("buffer with CRS change (needs explicit Transform node)".into()));
                }
                if error_tolerance.is_some() {
                    return Err(EngineError::Unsupported("buffer error_tolerance".into()));
                }
                // v0 assumption: the validator only accepts projected CRSs from a table of
                // meter-based systems, so metres == CRS units.
                let d = if distance.unit().is_angular() {
                    distance.value()
                } else {
                    distance.to_meters().map_err(|e| EngineError::InvalidPlan(e.to_string()))?
                };
                let i = self.node(input)?;
                let g = q_ident(geometry_column);
                let sql = if output_column == geometry_column {
                    format!("SELECT * REPLACE (ST_Buffer({g}, {}) AS {g}) FROM {}", num(d)?, q_ident(&i.name))
                } else {
                    format!("SELECT *, ST_Buffer({g}, {}) AS {} FROM {}", num(d)?, q_ident(output_column), q_ident(&i.name))
                };
                let name = self.push(sql);
                let schema = self.schema_of(id)?;
                Ok(Out { name, covering: None, schema })
            }
            PlanNode::Transform { input, geometry_column, input_crs, output_crs, output_column } => {
                self.need_spatial("transform")?;
                let i = self.node(input)?;
                let g = q_ident(geometry_column);
                let t = format!(
                    "ST_Transform({g}, {}, {}, always_xy := true)",
                    q_str(&proj_name(input_crs)?),
                    q_str(&proj_name(output_crs)?)
                );
                let sql = if output_column == geometry_column {
                    format!("SELECT * REPLACE ({t} AS {g}) FROM {}", q_ident(&i.name))
                } else {
                    format!("SELECT *, {t} AS {} FROM {}", q_ident(output_column), q_ident(&i.name))
                };
                let name = self.push(sql);
                Ok(Out { name, covering: None, schema: self.schema_of(id)? })
            }
            PlanNode::SpatialSemiJoin { left, right, left_column, right_column, predicate } => {
                self.need_spatial("spatial semi join")?;
                let l = self.node(left)?;
                let r = self.node(right)?;
                let (lc, rc) = (q_ident(left_column), q_ident(right_column));
                let pred = match predicate {
                    SpatialPredicate::Intersects => format!("ST_Intersects(l.{lc}, r.{rc})"),
                    SpatialPredicate::WithinDistance { distance } => {
                        // metres in the plane of a local metric CRS (the validator guarantees both); never degrees
                        let d = lowered(geo_lowering::within_distance_meters(distance))?;
                        format!("ST_DWithin(l.{lc}, r.{rc}, {})", num(d)?)
                    }
                };
                // canonical form: EXISTS, each left row at most once, no pair table
                let sql = geo_lowering::semi_join_exists("l.*", &q_ident(&l.name), &q_ident(&r.name), &pred);
                let name = self.push(sql);
                Ok(Out { name, covering: None, schema: self.schema_of(id)? })
            }
            PlanNode::SpatialJoin { left, right, left_column, right_column, join_type, right_prefix, .. } => {
                self.need_spatial("spatial join")?;
                let l = self.node(left)?;
                let r = self.node(right)?;
                let mut cols: Vec<String> =
                    l.schema.fields.iter().map(|f| format!("l.{0} AS {0}", q_ident(&f.name))).collect();
                cols.extend(r.schema.fields.iter().map(|f| {
                    format!("r.{} AS {}", q_ident(&f.name), q_ident(&format!("{right_prefix}{}", f.name)))
                }));
                let jt = match join_type {
                    JoinType::Inner => "INNER",
                    JoinType::Left => "LEFT",
                };
                let sql = format!(
                    "SELECT {} FROM {} l {jt} JOIN {} r ON ST_Intersects(l.{}, r.{})",
                    cols.join(", "),
                    q_ident(&l.name),
                    q_ident(&r.name),
                    q_ident(left_column),
                    q_ident(right_column)
                );
                let name = self.push(sql);
                Ok(Out { name, covering: None, schema: self.schema_of(id)? })
            }
            PlanNode::Aggregate { input, group_by, aggregates } => {
                let i = self.node(input)?;
                let mut sel: Vec<String> = group_by.iter().map(|g| q_ident(g)).collect();
                for a in aggregates {
                    sel.push(format!("{} AS {}", lowered(geo_lowering::aggregate_call(a.func, a.column.as_deref()))?, q_ident(&a.alias)));
                }
                let mut sql = format!("SELECT {} FROM {}", sel.join(", "), q_ident(&i.name));
                if !group_by.is_empty() {
                    sql += &format!(" GROUP BY {}", group_by.iter().map(|g| q_ident(g)).collect::<Vec<_>>().join(", "));
                }
                let name = self.push(sql);
                Ok(Out { name, covering: None, schema: self.schema_of(id)? })
            }
            PlanNode::Project { input, columns } => {
                let i = self.node(input)?;
                let sel = columns.iter().map(|c| q_ident(c)).collect::<Vec<_>>().join(", ");
                let name = self.push(format!("SELECT {sel} FROM {}", q_ident(&i.name)));
                let schema = self.schema_of(id)?;
                let covering = self.keep_covering(&i.covering, &schema);
                Ok(Out { name, covering, schema })
            }
            PlanNode::Limit { input, n: lim } => {
                let i = self.node(input)?;
                let name = self.push(format!("SELECT * FROM {} LIMIT {lim}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            PlanNode::Sort { input, keys } => {
                let i = self.node(input)?;
                // canonical: explicit direction, NULLS LAST both ways; DuckDB compares VARCHAR byte-wise, so no collation clause
                let ob = lowered(geo_lowering::order_by(keys, &|_| None))?;
                let name = self.push(format!("SELECT * FROM {} ORDER BY {ob}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
        }
    }
}
