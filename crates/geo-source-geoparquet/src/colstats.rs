//! Per-column statistics for predicate selectivity (estimator v3), built in the same one-off pass as the spatial
//! histogram: value frequencies for low-cardinality text columns, a 65-point quantile sketch for numbers, counts for
//! booleans, null counts for all. Parquet footers carry min/max only, which says nothing about `category = 'school'`.
//!
//! Selectivity assumes attributes are independent of location (true for the synthetic files, a stated assumption for
//! real data); an unknown column or a literal of the wrong type falls back to textbook defaults and says so.

use arrow_array::cast::AsArray;
use arrow_array::types::*;
use arrow_array::{Array, ArrayRef};
use arrow_schema::DataType;
use geo_ir::{CompareOp, Expr, Literal};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const MAX_DISTINCT: usize = 4096;
pub const SKETCH_POINTS: usize = 65;
pub const SAMPLE_CAP: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColumnStats {
    /// Text: exact frequencies while the number of distinct values stays under [`MAX_DISTINCT`].
    Categorical { total: u64, nulls: u64, exact: bool, counts: BTreeMap<String, u64> },
    /// Numbers: `quantiles[i]` is the value at rank i/(n-1) of a sample (min and max exact).
    Numeric { total: u64, nulls: u64, quantiles: Vec<f64> },
    Boolean { total: u64, nulls: u64, true_count: u64 },
}

/// Accumulates one column over record batches.
pub struct Accumulator {
    kind: Kind,
    total: u64,
    nulls: u64,
    stride: u64,
    seen: u64,
}

enum Kind {
    Text { counts: HashMap<String, u64>, exact: bool },
    Num { sample: Vec<f64>, min: f64, max: f64 },
    Bool { t: u64 },
}

pub fn supported(t: &DataType) -> bool {
    use DataType::*;
    matches!(t, Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 | Float32 | Float64 | Utf8 | LargeUtf8 | Utf8View | Boolean)
}

impl Accumulator {
    pub fn new(t: &DataType, expected_rows: u64) -> Option<Accumulator> {
        use DataType::*;
        let kind = match t {
            Utf8 | LargeUtf8 | Utf8View => Kind::Text { counts: HashMap::new(), exact: true },
            Boolean => Kind::Bool { t: 0 },
            Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 | Float32 | Float64 => Kind::Num { sample: Vec::new(), min: f64::INFINITY, max: f64::NEG_INFINITY },
            _ => return None,
        };
        Some(Accumulator { kind, total: 0, nulls: 0, stride: (expected_rows / SAMPLE_CAP as u64).max(1), seen: 0 })
    }

    pub fn add(&mut self, a: &ArrayRef) {
        self.total += a.len() as u64;
        self.nulls += a.null_count() as u64;
        match &mut self.kind {
            Kind::Text { counts, exact } => {
                let mut put = |s: &str| {
                    if let Some(c) = counts.get_mut(s) {
                        *c += 1;
                    } else if counts.len() < MAX_DISTINCT {
                        counts.insert(s.to_string(), 1);
                    } else {
                        *exact = false;
                    }
                };
                match a.data_type() {
                    DataType::Utf8 => a.as_string::<i32>().iter().flatten().for_each(&mut put),
                    DataType::LargeUtf8 => a.as_string::<i64>().iter().flatten().for_each(&mut put),
                    _ => a.as_string_view().iter().flatten().for_each(&mut put),
                }
            }
            Kind::Bool { t } => *t += a.as_boolean().iter().flatten().filter(|b| *b).count() as u64,
            Kind::Num { sample, min, max } => {
                let (stride, seen) = (self.stride, &mut self.seen);
                let mut take = |v: f64| {
                    if v.is_finite() {
                        *min = min.min(v);
                        *max = max.max(v);
                        if *seen % stride == 0 {
                            sample.push(v);
                        }
                    }
                    *seen += 1;
                };
                macro_rules! prim {
                    ($t:ty) => {
                        a.as_primitive::<$t>().iter().flatten().for_each(|v| take(v as f64))
                    };
                }
                match a.data_type() {
                    DataType::Int8 => prim!(Int8Type),
                    DataType::Int16 => prim!(Int16Type),
                    DataType::Int32 => prim!(Int32Type),
                    DataType::Int64 => prim!(Int64Type),
                    DataType::UInt8 => prim!(UInt8Type),
                    DataType::UInt16 => prim!(UInt16Type),
                    DataType::UInt32 => prim!(UInt32Type),
                    DataType::UInt64 => prim!(UInt64Type),
                    DataType::Float32 => prim!(Float32Type),
                    _ => prim!(Float64Type),
                }
            }
        }
    }

    pub fn finish(self) -> ColumnStats {
        match self.kind {
            Kind::Text { counts, exact } => ColumnStats::Categorical { total: self.total, nulls: self.nulls, exact, counts: counts.into_iter().collect() },
            Kind::Bool { t } => ColumnStats::Boolean { total: self.total, nulls: self.nulls, true_count: t },
            Kind::Num { mut sample, min, max } => {
                sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let mut q = Vec::with_capacity(SKETCH_POINTS);
                if sample.is_empty() {
                    return ColumnStats::Numeric { total: self.total, nulls: self.nulls, quantiles: q };
                }
                for i in 0..SKETCH_POINTS {
                    q.push(sample[((sample.len() - 1) as f64 * i as f64 / (SKETCH_POINTS - 1) as f64).round() as usize]);
                }
                q[0] = min;
                q[SKETCH_POINTS - 1] = max;
                ColumnStats::Numeric { total: self.total, nulls: self.nulls, quantiles: q }
            }
        }
    }
}

impl ColumnStats {
    /// Expected number of distinct values among `n` rows drawn independently (exact frequencies only):
    /// sum over values of 1 - (1 - p)^n. `None` for columns without exact frequencies.
    pub fn expected_distinct(&self, n: f64) -> Option<f64> {
        match self {
            ColumnStats::Categorical { total, exact: true, counts, .. } if *total > 0 => {
                Some(counts.values().map(|c| 1.0 - (1.0 - *c as f64 / *total as f64).powf(n.max(0.0))).sum())
            }
            _ => None,
        }
    }

    fn non_null_fraction(&self) -> f64 {
        let (t, n) = match self {
            ColumnStats::Categorical { total, nulls, .. } | ColumnStats::Numeric { total, nulls, .. } | ColumnStats::Boolean { total, nulls, .. } => (*total, *nulls),
        };
        if t == 0 { 1.0 } else { 1.0 - n as f64 / t as f64 }
    }

    /// Fraction of the sketch at or below `v` (linear inside a quantile step; ties at `v` all count as "below").
    fn cdf(q: &[f64], v: f64, inclusive: bool) -> f64 {
        let n = q.len();
        if n < 2 {
            return 0.5;
        }
        if v < q[0] {
            return 0.0;
        }
        if v > q[n - 1] {
            return 1.0;
        }
        // first index with q[i] > v (or >= v when exclusive)
        let idx = if inclusive { q.partition_point(|x| *x <= v) } else { q.partition_point(|x| *x < v) };
        if idx == 0 {
            return 0.0;
        }
        if idx >= n {
            return 1.0;
        }
        let (lo, hi) = (q[idx - 1], q[idx]);
        let within = if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        ((idx - 1) as f64 + within) / (n - 1) as f64
    }

    /// Selectivity of one comparison against this column; `None` when the literal does not fit the column.
    fn compare(&self, op: CompareOp, lit: &Literal) -> Option<f64> {
        let nn = self.non_null_fraction();
        match (self, lit) {
            (ColumnStats::Categorical { total, counts, exact, .. }, Literal::Str(s)) => {
                let t = (*total).max(1) as f64;
                let known: u64 = counts.values().sum();
                let eq = match counts.get(s) {
                    Some(c) => *c as f64 / t,
                    None if *exact => 0.0,
                    None => ((*total).saturating_sub(known)) as f64 / t / 100.0, // unseen among the many: a guess, never zero
                };
                Some(match op {
                    CompareOp::Eq => eq,
                    CompareOp::Ne => (nn - eq).max(0.0),
                    _ => return None, // ordering of text is not modelled
                })
            }
            (ColumnStats::Numeric { quantiles: q, .. }, Literal::Int(_) | Literal::Float(_)) => {
                let v = match lit {
                    Literal::Int(i) => *i as f64,
                    Literal::Float(f) => *f,
                    _ => unreachable!(),
                };
                let below = Self::cdf(q, v, false);
                let upto = Self::cdf(q, v, true);
                // ties: the sketch holds repeated values, so (upto - below) is the mass equal to v, floored at one sample step
                let eq = (upto - below).max(1.0 / (q.len().max(2) - 1) as f64 / 20.0);
                Some(nn * match op {
                    CompareOp::Eq => eq.min(1.0),
                    CompareOp::Ne => (1.0 - eq).max(0.0),
                    CompareOp::Lt => below,
                    CompareOp::Le => upto.max(below + eq).min(1.0),
                    CompareOp::Gt => (1.0 - upto.max(below + eq)).max(0.0),
                    CompareOp::Ge => 1.0 - below,
                })
            }
            (ColumnStats::Boolean { total, true_count, .. }, Literal::Bool(b)) => {
                let t = (*total).max(1) as f64;
                let p = if *b { *true_count as f64 / t } else { (*total - *true_count) as f64 / t - (1.0 - nn) };
                Some(match op {
                    CompareOp::Eq => p.max(0.0),
                    CompareOp::Ne => (nn - p).max(0.0),
                    _ => return None,
                })
            }
            _ => None,
        }
    }
}

/// Result of estimating a predicate: the selectivity and whether every part of it came from real column statistics.
#[derive(Debug, Clone, Copy)]
pub struct Selectivity {
    pub value: f64,
    pub known: bool,
}

/// Textbook defaults for what the statistics cannot answer (unknown column, mismatched literal, text ordering).
const DEFAULT_EQ: f64 = 0.1;
const DEFAULT_RANGE: f64 = 0.33;

/// Selectivity of `e` over a source with `cols` statistics, assuming independent terms.
pub fn selectivity(e: &Expr, cols: &BTreeMap<String, ColumnStats>) -> Selectivity {
    match e {
        Expr::Compare { column, op, value } => match cols.get(column).and_then(|c| c.compare(*op, value)) {
            Some(v) => Selectivity { value: v.clamp(0.0, 1.0), known: true },
            None => Selectivity { value: if matches!(op, CompareOp::Eq) { DEFAULT_EQ } else if matches!(op, CompareOp::Ne) { 1.0 - DEFAULT_EQ } else { DEFAULT_RANGE }, known: false },
        },
        Expr::In { column, values } => {
            let parts: Vec<Option<f64>> = values.iter().map(|v| cols.get(column).and_then(|c| c.compare(CompareOp::Eq, v))).collect();
            if parts.iter().all(|p| p.is_some()) {
                Selectivity { value: parts.iter().flatten().sum::<f64>().min(1.0), known: true }
            } else {
                Selectivity { value: (DEFAULT_EQ * values.len() as f64).min(1.0), known: false }
            }
        }
        Expr::IsNull { column } => match cols.get(column) {
            Some(c) => Selectivity { value: 1.0 - c.non_null_fraction(), known: true },
            None => Selectivity { value: 0.05, known: false },
        },
        Expr::And { exprs } => {
            let parts: Vec<Selectivity> = exprs.iter().map(|x| selectivity(x, cols)).collect();
            Selectivity { value: parts.iter().map(|s| s.value).product(), known: parts.iter().all(|s| s.known) }
        }
        Expr::Or { exprs } => {
            let parts: Vec<Selectivity> = exprs.iter().map(|x| selectivity(x, cols)).collect();
            Selectivity { value: 1.0 - parts.iter().map(|s| 1.0 - s.value).product::<f64>(), known: parts.iter().all(|s| s.known) }
        }
        Expr::Not { expr } => {
            let s = selectivity(expr, cols);
            Selectivity { value: 1.0 - s.value, known: s.known }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> ColumnStats {
        ColumnStats::Categorical { total: 1000, nulls: 0, exact: true, counts: [("school".to_string(), 100), ("farm".to_string(), 400), ("other".to_string(), 500)].into_iter().collect() }
    }

    fn num() -> ColumnStats {
        // uniform 0..64 over 65 quantile points
        ColumnStats::Numeric { total: 1000, nulls: 0, quantiles: (0..65).map(|i| i as f64).collect() }
    }

    fn cols() -> BTreeMap<String, ColumnStats> {
        [("category".to_string(), cat()), ("height".to_string(), num())].into_iter().collect()
    }

    fn cmp(c: &str, op: CompareOp, v: Literal) -> Expr {
        Expr::Compare { column: c.into(), op, value: v }
    }

    #[test]
    fn categorical_equality_uses_exact_frequencies() {
        let s = selectivity(&cmp("category", CompareOp::Eq, Literal::Str("school".into())), &cols());
        assert!(s.known && (s.value - 0.1).abs() < 1e-9);
        let n = selectivity(&cmp("category", CompareOp::Ne, Literal::Str("other".into())), &cols());
        assert!((n.value - 0.5).abs() < 1e-9);
        assert_eq!(selectivity(&cmp("category", CompareOp::Eq, Literal::Str("zoo".into())), &cols()).value, 0.0, "exact counts know a value never occurs");
    }

    #[test]
    fn numeric_ranges_follow_the_quantile_sketch() {
        let gt = selectivity(&cmp("height", CompareOp::Gt, Literal::Float(16.0)), &cols());
        assert!((gt.value - 0.75).abs() < 0.02, "{}", gt.value);
        let lt = selectivity(&cmp("height", CompareOp::Lt, Literal::Int(32)), &cols());
        assert!((lt.value - 0.5).abs() < 0.02, "{}", lt.value);
        assert_eq!(selectivity(&cmp("height", CompareOp::Gt, Literal::Float(1000.0)), &cols()).value, 0.0);
        assert!((selectivity(&cmp("height", CompareOp::Ge, Literal::Float(-5.0)), &cols()).value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn boolean_combinations_assume_independence() {
        let a = cmp("category", CompareOp::Eq, Literal::Str("school".into()));
        let b = cmp("height", CompareOp::Gt, Literal::Float(32.0));
        let and = selectivity(&Expr::And { exprs: vec![a.clone(), b.clone()] }, &cols());
        assert!((and.value - 0.05).abs() < 0.01 && and.known);
        let or = selectivity(&Expr::Or { exprs: vec![a.clone(), b] }, &cols());
        assert!((or.value - (1.0 - 0.9 * 0.5)).abs() < 0.02);
        assert!((selectivity(&Expr::Not { expr: Box::new(a) }, &cols()).value - 0.9).abs() < 1e-9);
    }

    #[test]
    fn unknown_columns_fall_back_to_defaults_and_say_so() {
        let s = selectivity(&cmp("mystery", CompareOp::Eq, Literal::Int(1)), &cols());
        assert!(!s.known && s.value == DEFAULT_EQ);
        let t = selectivity(&cmp("category", CompareOp::Gt, Literal::Str("a".into())), &cols());
        assert!(!t.known, "text ordering is not modelled");
        let m = selectivity(&cmp("height", CompareOp::Eq, Literal::Str("tall".into())), &cols());
        assert!(!m.known, "literal of the wrong type");
    }

    #[test]
    fn expected_distinct_values_grow_with_the_sample() {
        let c = cat();
        let d1 = c.expected_distinct(1.0).unwrap();
        let d50 = c.expected_distinct(50.0).unwrap();
        let d1e6 = c.expected_distinct(1e6).unwrap();
        assert!((d1 - 1.0).abs() < 1e-9 && d50 > 2.9 && d1e6 <= 3.0 + 1e-9 && d1e6 > 2.99);
        assert!(num().expected_distinct(10.0).is_none());
    }

    #[test]
    fn accumulator_builds_frequencies_and_quantiles() {
        use arrow_array::{Float64Array, StringArray};
        use std::sync::Arc;
        let mut t = Accumulator::new(&DataType::Utf8, 100).unwrap();
        t.add(&(Arc::new(StringArray::from(vec![Some("a"), Some("b"), Some("a"), None])) as ArrayRef));
        match t.finish() {
            ColumnStats::Categorical { total, nulls, exact, counts } => assert_eq!((total, nulls, exact, counts["a"], counts["b"]), (4, 1, true, 2, 1)),
            _ => panic!(),
        }
        let mut n = Accumulator::new(&DataType::Float64, 100).unwrap();
        n.add(&(Arc::new(Float64Array::from((0..=100).map(|i| i as f64).collect::<Vec<_>>())) as ArrayRef));
        match n.finish() {
            ColumnStats::Numeric { quantiles, .. } => assert!(quantiles.len() == 65 && quantiles[0] == 0.0 && quantiles[64] == 100.0 && (quantiles[32] - 50.0).abs() <= 2.0),
            _ => panic!(),
        }
    }
}
