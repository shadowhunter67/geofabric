//! The browser dialect (DuckDB-Wasm, no spatial extension), built on the canonical semantics of the crate root.
//!
//! Dialect rules: no spatial functions; `BboxFilter` only through the bbox covering columns; the geometry column is converted to WKB at
//! the scan (`ST_AsWKB`); one `NOT MATERIALIZED` CTE per node; every identifier quoted; literals escaped or numeric (integers inside
//! JavaScript's exact range); one extra row is requested (`LIMIT max_rows + 1`). Operations that need the spatial extension
//! (`Intersects`, `Buffer`, `Transform`, joins) are refused with a reason: the planner never routes them here and the executor is the
//! safety net.
//!
//! This is the ONLY lowering the web client runs: it is compiled to WebAssembly ([`crate::wasm`]) and called by the page, so the
//! browser no longer has its own TypeScript compiler of the plan (`sqlgen.ts` was deleted, RFC-004 step 3).

use crate::{
    aggregate_call, bbox_covering_overlap, format_float, order_by, predicate, quote_ident, quote_path, quote_string, row_cap, DuckDbPredicates, LowerError,
};
use geo_ir::{NodeId, Plan, PlanNode};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

/// What the client knows about one source: its bbox covering columns (dotted paths: `bbox.xmin`). File names are assigned by the
/// lowering (`gf_src_0.parquet`, ... in order of first use); the client may rename them.
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserSource {
    pub covering: Option<[String; 4]>,
}

#[derive(Clone)]
struct Out {
    name: String,
    /// SQL expressions of the covering (xmin, ymin, xmax, ymax) while still valid at this node.
    covering: Option<[String; 4]>,
    covering_geom: Option<String>,
    covering_top: Option<String>,
}

impl Out {
    fn without_covering(name: String) -> Out {
        Out { name, covering: None, covering_geom: None, covering_top: None }
    }
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

    fn visit(&mut self, id: &NodeId, stack: &mut Vec<NodeId>) -> Result<Out, LowerError> {
        if let Some(o) = self.memo.get(id) {
            return Ok(o.clone());
        }
        if stack.contains(id) {
            return Err(LowerError::Invalid(format!("cycle at node {}", id.0)));
        }
        let node = self.plan.nodes.get(id).ok_or_else(|| LowerError::Invalid(format!("missing node {}", id.0)))?.clone();
        stack.push(id.clone());
        let out = self.node(&node, stack)?;
        stack.pop();
        self.memo.insert(id.clone(), out.clone());
        Ok(out)
    }

    fn node(&mut self, n: &PlanNode, stack: &mut Vec<NodeId>) -> Result<Out, LowerError> {
        let mut preds = DuckDbPredicates { safe_int_only: true };
        Ok(match n {
            PlanNode::Source { uri, schema, .. } => {
                let b = self.sources.get(uri).ok_or_else(|| LowerError::Invalid("source is not bound to an authorized URL".into()))?.clone();
                let idx = match self.files.iter().position(|f| f == uri) {
                    Some(i) => i,
                    None => {
                        self.files.push(uri.clone());
                        self.files.len() - 1
                    }
                };
                let geom = schema.geometry_columns.first().map(|g| g.name.clone());
                let read = format!("read_parquet({})", quote_string(&format!("gf_src_{idx}.parquet")));
                let name = self.push(match &geom {
                    Some(g) => format!("SELECT * REPLACE (ST_AsWKB({0}) AS {0}) FROM {read}", quote_ident(g)),
                    None => format!("SELECT * FROM {read}"),
                });
                Out {
                    name,
                    covering: b.covering.as_ref().map(|c| [quote_path(&c[0]), quote_path(&c[1]), quote_path(&c[2]), quote_path(&c[3])]),
                    covering_geom: geom,
                    covering_top: b.covering.as_ref().map(|c| c[0].split('.').next().unwrap_or("").to_string()),
                }
            }
            PlanNode::Filter { input, predicate: p } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT * FROM {} WHERE {}", quote_ident(&i.name), predicate(p, &mut preds)?));
                Out { name, ..i }
            }
            PlanNode::BboxFilter { input, geometry_column, bbox } => {
                let i = self.visit(input, stack)?;
                let Some(cov) = i.covering.clone().filter(|_| i.covering_geom.as_deref() == Some(geometry_column.as_str())) else {
                    return Err(LowerError::Unsupported("bbox filter needs a bbox covering column (the browser has no spatial extension)".into()));
                };
                let w = [format_float(bbox.min_x())?, format_float(bbox.min_y())?, format_float(bbox.max_x())?, format_float(bbox.max_y())?];
                let name = self.push(format!("SELECT * FROM {} WHERE {}", quote_ident(&i.name), bbox_covering_overlap(&cov, &w)));
                Out { name, ..i }
            }
            PlanNode::Aggregate { input, group_by, aggregates } => {
                let i = self.visit(input, stack)?;
                let mut sel: Vec<String> = group_by.iter().map(|g| quote_ident(g)).collect();
                for a in aggregates {
                    sel.push(format!("{} AS {}", aggregate_call(a.func, a.column.as_deref())?, quote_ident(&a.alias)));
                }
                let mut sql = format!("SELECT {} FROM {}", sel.join(", "), quote_ident(&i.name));
                if !group_by.is_empty() {
                    sql.push_str(&format!(" GROUP BY {}", group_by.iter().map(|g| quote_ident(g)).collect::<Vec<_>>().join(", ")));
                }
                Out::without_covering(self.push(sql))
            }
            PlanNode::Project { input, columns } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT {} FROM {}", columns.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", "), quote_ident(&i.name)));
                let keep = i.covering_top.as_ref().is_some_and(|t| columns.contains(t)) && i.covering_geom.as_ref().is_some_and(|g| columns.contains(g));
                if keep {
                    Out { name, ..i }
                } else {
                    Out::without_covering(name)
                }
            }
            PlanNode::Limit { input, n } => {
                let i = self.visit(input, stack)?;
                let name = self.push(format!("SELECT * FROM {} LIMIT {n}", quote_ident(&i.name)));
                Out { name, ..i }
            }
            PlanNode::Sort { input, keys } => {
                let i = self.visit(input, stack)?;
                // DuckDB compares VARCHAR byte-wise: no collation clause is needed for the canonical text order
                let ob = order_by(keys, &|_| None)?;
                let name = self.push(format!("SELECT * FROM {} ORDER BY {ob}", quote_ident(&i.name)));
                Out { name, ..i }
            }
            other => return Err(LowerError::Unsupported(format!("operation '{}' is not supported by the browser engine", other.kind().name()))),
        })
    }
}

/// Lower `plan` for DuckDB-Wasm. Returns the SQL and the source URIs it reads; source `i` is file `gf_src_{i}.parquet`.
pub fn lower_browser(plan: &Plan, sources: &BTreeMap<String, BrowserSource>, max_rows: u64) -> Result<(String, Vec<String>), LowerError> {
    let mut l = Lowering { plan, sources, memo: HashMap::new(), ctes: Vec::new(), files: Vec::new() };
    let root = l.visit(&plan.root.clone(), &mut Vec::new())?;
    let with = l.ctes.iter().map(|(n, s)| format!("{} AS NOT MATERIALIZED ({})", quote_ident(n), s)).collect::<Vec<_>>().join(",\n");
    Ok((format!("WITH {with}\nSELECT * FROM {} LIMIT {}", quote_ident(&root.name), row_cap(max_rows)), l.files))
}
