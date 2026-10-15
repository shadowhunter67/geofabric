//! Canonical lowering of a PlanGraph to **browser-dialect DuckDB SQL** (RFC-004, step 2).
//!
//! The browser engine (DuckDB-Wasm, no spatial extension) used to be the only engine whose lowering existed in just
//! one language (`web/src/engine/sqlgen.ts`), next to the server lowering in `geo-engine-duckdb/src/sql.rs`. Two
//! hand-written lowerings drifted once already (typed `GEOMETRY(crs)` columns could not be streamed out of the
//! browser). This module is the *reference* for the browser dialect: golden fixtures
//! (`tests/fixtures/lowering/*.json`) are generated from it and checked by BOTH `cargo test` and the TypeScript tests,
//! so the TypeScript twin cannot drift silently. Migration step 3 (not done): the gateway ships this SQL to the client
//! and `sqlgen.ts` is deleted.
//!
//! Dialect rules: no spatial functions; `BboxFilter` only through the bbox covering columns; the geometry column is
//! converted to WKB at the scan (`ST_AsWKB`); one `NOT MATERIALIZED` CTE per node; every identifier quoted; literals
//! escaped or numeric; one extra row is fetched (`LIMIT max_rows + 1`) so the caller can detect "result too large".

use geo_executor::EngineError;
use geo_ir::{AggFunc, CompareOp, Expr, Literal, NodeId, Plan, PlanNode};
use std::collections::{BTreeMap, HashMap};

/// What the client knows about one source: its bbox covering columns (dotted paths: `bbox.xmin`). File names are
/// assigned by the lowering (`gf_src_0.parquet`, ... in order of first use); the client may rename them.
#[derive(Debug, Clone)]
pub struct BrowserSource {
    pub covering: Option<[String; 4]>,
}

fn q_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn q_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn num(v: f64) -> Result<String, EngineError> {
    if !v.is_finite() {
        return Err(EngineError::InvalidPlan(format!("non-finite number {v}")));
    }
    Ok(if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") })
}

fn lit(l: &Literal) -> Result<String, EngineError> {
    Ok(match l {
        Literal::Int(i) => {
            if i.abs() > (1i64 << 53) - 1 {
                return Err(EngineError::InvalidPlan("integer literal out of the safe range".into()));
            }
            i.to_string()
        }
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

#[derive(Clone)]
struct Out {
    name: String,
    /// SQL expressions of the covering (xmin, ymin, xmax, ymax) while still valid at this node.
    covering: Option<[String; 4]>,
    covering_geom: Option<String>,
    covering_top: Option<String>,
}

struct Lowering<'a> {
    plan: &'a Plan,
    sources: &'a BTreeMap<String, BrowserSource>,
    memo: HashMap<NodeId, Out>,
    ctes: Vec<(String, String)>,
    files: Vec<String>,
}

impl Lowering<'_> {
    fn push(&mut self, sql: String) -> String {
        let n = format!("n{}", self.ctes.len());
        self.ctes.push((n.clone(), sql));
        n
    }

    fn visit(&mut self, id: &NodeId, stack: &mut Vec<NodeId>) -> Result<Out, EngineError> {
        if let Some(o) = self.memo.get(id) {
            return Ok(o.clone());
        }
        if stack.contains(id) {
            return Err(EngineError::InvalidPlan(format!("cycle at node {}", id.0)));
        }
        let node = self.plan.nodes.get(id).ok_or_else(|| EngineError::InvalidPlan(format!("missing node {}", id.0)))?.clone();
        stack.push(id.clone());
        let out = self.node(&node, stack)?;
        stack.pop();
        self.memo.insert(id.clone(), out.clone());
        Ok(out)
    }

    fn node(&mut self, n: &PlanNode, stack: &mut Vec<NodeId>) -> Result<Out, EngineError> {
        Ok(match n {
            PlanNode::Source { uri, schema, .. } => {
                let b = self.sources.get(uri).ok_or_else(|| EngineError::InvalidPlan("source is not bound to an authorized URL".into()))?.clone();
                let idx = match self.files.iter().position(|f| f == uri) {
                    Some(i) => i,
                    None => {
                        self.files.push(uri.clone());
                        self.files.len() - 1
                    }
                };
                let geom = schema.geometry_columns.first().map(|g| g.name.clone());
                let read = format!("read_parquet({})", q_str(&format!("gf_src_{idx}.parquet")));
                let name = self.push(match &geom {
                    Some(g) => format!("SELECT * REPLACE (ST_AsWKB({0}) AS {0}) FROM {read}", q_ident(g)),
                    None => format!("SELECT * FROM {read}"),
                });
                let col = |p: &str| p.split('.').map(q_ident).collect::<Vec<_>>().join(".");
                Out {
                    name,
                    covering: b.covering.as_ref().map(|c| [col(&c[0]), col(&c[1]), col(&c[2]), col(&c[3])]),
                    covering_geom: geom,
                    covering_top: b.covering.as_ref().map(|c| c[0].split('.').next().unwrap_or("").to_string()),
                }
            }
            PlanNode::Filter { input, predicate } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT * FROM {} WHERE {}", q_ident(&i.name), expr(predicate)?));
                Out { name, ..i }
            }
            PlanNode::BboxFilter { input, geometry_column, bbox } => {
                let i = self.visit(input, stack)?;
                let Some(cov) = i.covering.clone().filter(|_| i.covering_geom.as_deref() == Some(geometry_column.as_str())) else {
                    return Err(EngineError::Unsupported("bbox filter needs a bbox covering column (the browser has no spatial extension)".into()));
                };
                let (x0, y0, x1, y1) = (num(bbox.min_x())?, num(bbox.min_y())?, num(bbox.max_x())?, num(bbox.max_y())?);
                let pred = format!("{} <= {x1} AND {} >= {x0} AND {} <= {y1} AND {} >= {y0}", cov[0], cov[2], cov[1], cov[3]);
                let name = self.push(format!("SELECT * FROM {} WHERE {pred}", q_ident(&i.name)));
                Out { name, ..i }
            }
            PlanNode::Aggregate { input, group_by, aggregates } => {
                let i = self.visit(input, stack)?;
                let mut sel: Vec<String> = group_by.iter().map(|g| q_ident(g)).collect();
                for a in aggregates {
                    let f = match (&a.func, &a.column) {
                        (AggFunc::Count, None) => "COUNT(*)".to_string(),
                        (AggFunc::Count, Some(c)) => format!("COUNT({})", q_ident(c)),
                        (_, None) => return Err(EngineError::InvalidPlan("aggregate needs a column".into())),
                        (func, Some(c)) => format!(
                            "{}({})",
                            match func {
                                AggFunc::Sum => "SUM",
                                AggFunc::Min => "MIN",
                                AggFunc::Max => "MAX",
                                AggFunc::Avg => "AVG",
                                AggFunc::Count => "COUNT",
                            },
                            q_ident(c)
                        ),
                    };
                    sel.push(format!("{f} AS {}", q_ident(&a.alias)));
                }
                let mut sql = format!("SELECT {} FROM {}", sel.join(", "), q_ident(&i.name));
                if !group_by.is_empty() {
                    sql.push_str(&format!(" GROUP BY {}", group_by.iter().map(|g| q_ident(g)).collect::<Vec<_>>().join(", ")));
                }
                Out { name: self.push(sql), covering: None, covering_geom: None, covering_top: None }
            }
            PlanNode::Project { input, columns } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT {} FROM {}", columns.iter().map(|c| q_ident(c)).collect::<Vec<_>>().join(", "), q_ident(&i.name)));
                let keep = i.covering_top.as_ref().is_some_and(|t| columns.contains(t)) && i.covering_geom.as_ref().is_some_and(|g| columns.contains(g));
                if keep { Out { name, ..i } } else { Out { name, covering: None, covering_geom: None, covering_top: None } }
            }
            PlanNode::Limit { input, n } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT * FROM {} LIMIT {n}", q_ident(&i.name)));
                Out { name, ..i }
            }
            PlanNode::Sort { input, keys } => {
                let i = self.visit(input, stack)?;
                if keys.is_empty() {
                    return Err(EngineError::InvalidPlan("sort without keys".into()));
                }
                let ob = keys.iter().map(|k| format!("{}{}", q_ident(&k.column), if k.descending { " DESC" } else { "" })).collect::<Vec<_>>().join(", ");
                let name = self.push(format!("SELECT * FROM {} ORDER BY {ob}", q_ident(&i.name)));
                Out { name, ..i }
            }
            other => return Err(EngineError::Unsupported(format!("operation '{}' is not supported by the browser engine", other.kind().name()))),
        })
    }
}

/// Lower `plan` for DuckDB-Wasm. Returns the SQL and the source URIs it reads; source `i` is file `gf_src_{i}.parquet`.
pub fn lower_browser(plan: &Plan, sources: &BTreeMap<String, BrowserSource>, max_rows: u64) -> Result<(String, Vec<String>), EngineError> {
    let mut l = Lowering { plan, sources, memo: HashMap::new(), ctes: Vec::new(), files: Vec::new() };
    let root = l.visit(&plan.root.clone(), &mut Vec::new())?;
    let with = l.ctes.iter().map(|(n, s)| format!("{} AS NOT MATERIALIZED ({})", q_ident(n), s)).collect::<Vec<_>>().join(",\n");
    Ok((format!("WITH {with}\nSELECT * FROM {} LIMIT {}", q_ident(&root.name), max_rows + 1), l.files))
}
