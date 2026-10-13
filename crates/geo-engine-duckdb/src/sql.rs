//! Logical IR -> DuckDB SQL. Private to the adapter; the IR never sees SQL.
//!
//! Safety: every identifier is quoted, every literal is escaped or numeric,
//! file paths are validated against the allow-list, WKT literals are
//! restricted to a safe character set. No user string is concatenated raw.

use geo_executor::{EngineError, ExecutionContext};
use geo_ir::{AggFunc, CompareOp, Expr, JoinType, Literal, Node};
use geo_source_geoparquet::{wkt_envelope, GeoParquetSource};
use geo_types::{AuthorizedSource, Crs, SpatialSchema};
use std::path::Path;

pub(crate) fn q_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(crate) fn q_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn num(v: f64) -> Result<String, EngineError> {
    if v.is_finite() {
        Ok(format!("{v:?}"))
    } else {
        Err(EngineError::InvalidPlan(format!("non-finite number {v}")))
    }
}

fn lit(l: &Literal) -> Result<String, EngineError> {
    Ok(match l {
        Literal::Int(i) => i.to_string(),
        Literal::Float(f) => num(*f)?,
        Literal::Str(s) => q_str(s),
        Literal::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Literal::Null => "NULL".to_string(),
    })
}

fn expr(e: &Expr) -> Result<String, EngineError> {
    Ok(match e {
        Expr::Compare { column, op, value } => {
            let o = match op {
                CompareOp::Eq => "=",
                CompareOp::Ne => "<>",
                CompareOp::Lt => "<",
                CompareOp::Le => "<=",
                CompareOp::Gt => ">",
                CompareOp::Ge => ">=",
            };
            format!("{} {o} {}", q_ident(column), lit(value)?)
        }
        Expr::In { column, values } => {
            if values.is_empty() {
                return Err(EngineError::InvalidPlan("empty IN list".into()));
            }
            let v: Result<Vec<_>, _> = values.iter().map(lit).collect();
            format!("{} IN ({})", q_ident(column), v?.join(", "))
        }
        Expr::IsNull { column } => format!("{} IS NULL", q_ident(column)),
        Expr::And { exprs } | Expr::Or { exprs } => {
            if exprs.is_empty() {
                return Err(EngineError::InvalidPlan("empty AND/OR".into()));
            }
            let sep = if matches!(e, Expr::And { .. }) { " AND " } else { " OR " };
            let parts: Result<Vec<_>, _> = exprs.iter().map(|x| expr(x).map(|s| format!("({s})"))).collect();
            parts?.join(sep)
        }
        Expr::Not { expr: inner } => format!("NOT ({})", expr(inner)?),
    })
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
    ctx: &'a ExecutionContext,
    spatial: bool,
    ctes: Vec<(String, String)>,
    sources: Vec<AuthorizedSource>,
}

pub(crate) fn compile(root: &Node, ctx: &ExecutionContext, spatial: bool) -> Result<Compiled, EngineError> {
    let mut t = Translator { ctx, spatial, ctes: Vec::new(), sources: Vec::new() };
    let out = t.node(root)?;
    let with = t.ctes.iter().map(|(n, s)| format!("{} AS ({})", q_ident(n), s)).collect::<Vec<_>>().join(",\n");
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
    let sql = format!("WITH {with}\nSELECT {select} FROM {} LIMIT {}", q_ident(&out.name), ctx.max_rows.saturating_add(1));
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

    fn schema_of(&self, n: &Node) -> Result<SpatialSchema, EngineError> {
        n.output_schema().map_err(|e| EngineError::InvalidPlan(e.to_string()))
    }

    fn keep_covering(&self, c: &Option<CoveringRef>, schema: &SpatialSchema) -> Option<CoveringRef> {
        c.clone()
            .filter(|c| schema.fields.iter().any(|f| f.name == c.top_level) && schema.geometry(&c.geom).is_some())
    }

    fn node(&mut self, n: &Node) -> Result<Out, EngineError> {
        match n {
            Node::Source { uri, .. } => {
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
                Ok(Out { name, schema: self.schema_of(n)?, covering })
            }
            Node::Filter { input, predicate } => {
                let i = self.node(input)?;
                let name = self.push(format!("SELECT * FROM {} WHERE {}", q_ident(&i.name), expr(predicate)?));
                Ok(Out { name, ..i })
            }
            Node::BboxFilter { input, geometry_column, bbox } => {
                let i = self.node(input)?;
                let (x0, y0, x1, y1) =
                    (num(bbox.min_x())?, num(bbox.min_y())?, num(bbox.max_x())?, num(bbox.max_y())?);
                let pred = match i.covering.as_ref().filter(|c| c.geom == *geometry_column && c.crs == *bbox.crs()) {
                    Some(c) => format!(
                        "{} <= {x1} AND {} >= {x0} AND {} <= {y1} AND {} >= {y0}",
                        c.cols[0], c.cols[2], c.cols[1], c.cols[3]
                    ),
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
            Node::Intersects { input, geometry_column, with } => {
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
            Node::Buffer {
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
                let schema = self.schema_of(n)?;
                Ok(Out { name, covering: None, schema })
            }
            Node::SpatialJoin { left, right, left_column, right_column, join_type, right_prefix, .. } => {
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
                Ok(Out { name, covering: None, schema: self.schema_of(n)? })
            }
            Node::Aggregate { input, group_by, aggregates } => {
                let i = self.node(input)?;
                let mut sel: Vec<String> = group_by.iter().map(|g| q_ident(g)).collect();
                for a in aggregates {
                    let f = match (a.func, &a.column) {
                        (AggFunc::Count, None) => "COUNT(*)".to_string(),
                        (AggFunc::Count, Some(c)) => format!("COUNT({})", q_ident(c)),
                        (func, Some(c)) => {
                            let fname = match func {
                                AggFunc::Sum => "SUM",
                                AggFunc::Min => "MIN",
                                AggFunc::Max => "MAX",
                                AggFunc::Avg => "AVG",
                                AggFunc::Count => unreachable!(),
                            };
                            format!("{fname}({})", q_ident(c))
                        }
                        (_, None) => return Err(EngineError::InvalidPlan("aggregate needs a column".into())),
                    };
                    sel.push(format!("{f} AS {}", q_ident(&a.alias)));
                }
                let mut sql = format!("SELECT {} FROM {}", sel.join(", "), q_ident(&i.name));
                if !group_by.is_empty() {
                    sql += &format!(" GROUP BY {}", group_by.iter().map(|g| q_ident(g)).collect::<Vec<_>>().join(", "));
                }
                let name = self.push(sql);
                Ok(Out { name, covering: None, schema: self.schema_of(n)? })
            }
            Node::Project { input, columns } => {
                let i = self.node(input)?;
                let sel = columns.iter().map(|c| q_ident(c)).collect::<Vec<_>>().join(", ");
                let name = self.push(format!("SELECT {sel} FROM {}", q_ident(&i.name)));
                let schema = self.schema_of(n)?;
                let covering = self.keep_covering(&i.covering, &schema);
                Ok(Out { name, covering, schema })
            }
            Node::Limit { input, n: lim } => {
                let i = self.node(input)?;
                let name = self.push(format!("SELECT * FROM {} LIMIT {lim}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
            Node::Sort { input, keys } => {
                let i = self.node(input)?;
                if keys.is_empty() {
                    return Err(EngineError::InvalidPlan("sort without keys".into()));
                }
                let ob = keys
                    .iter()
                    .map(|k| format!("{}{}", q_ident(&k.column), if k.descending { " DESC" } else { "" }))
                    .collect::<Vec<_>>()
                    .join(", ");
                let name = self.push(format!("SELECT * FROM {} ORDER BY {ob}", q_ident(&i.name)));
                Ok(Out { name, ..i })
            }
        }
    }
}
