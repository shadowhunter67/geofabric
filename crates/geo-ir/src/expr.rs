use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Literal {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Non-spatial predicate over attribute columns (spatial predicates are nodes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expr {
    Compare { column: String, op: CompareOp, value: Literal },
    In { column: String, values: Vec<Literal> },
    IsNull { column: String },
    And { exprs: Vec<Expr> },
    Or { exprs: Vec<Expr> },
    Not { expr: Box<Expr> },
}

impl Expr {
    pub fn columns(&self, out: &mut Vec<String>) {
        match self {
            Expr::Compare { column, .. } | Expr::In { column, .. } | Expr::IsNull { column } => {
                out.push(column.clone())
            }
            Expr::And { exprs } | Expr::Or { exprs } => exprs.iter().for_each(|e| e.columns(out)),
            Expr::Not { expr } => expr.columns(out),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFunc {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggExpr {
    pub func: AggFunc,
    /// `None` only valid for `Count` (count of rows).
    pub column: Option<String>,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortKey {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}
