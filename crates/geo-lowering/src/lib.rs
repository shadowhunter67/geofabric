//! Canonical lowering (RFC-004): the single definition of what a PlanGraph's operators *mean* when they are turned into SQL.
//!
//! Before this crate three compilers (DuckDB server, DuckDB-Wasm, PostGIS) each re-implemented the semantics (predicate trees, literal
//! rules, `BboxFilter`, sort order and NULL placement, aggregate calls, the semi-join `EXISTS` form, the row cap). A live PostGIS run
//! showed the cost: `BboxFilter` drifted (geometry envelope vs. the covering-box overlap every other engine uses). Here each decision is
//! written ONCE; an engine contributes only its *dialect* (how a decided thing is spelled, and which resource it reads from).
//!
//! What is decided here, for every engine:
//! * **Predicate trees**: `And`/`Or`/`Not` structure, empty-list refusals, `HashBucket` validity ([`predicate`]).
//! * **Literals**: finite numbers, the portable float spelling, the safe-integer rule ([`format_float`], [`inline_literal`]).
//! * **`BboxFilter`**: the closed-interval overlap of the file's *covering box* with the window ([`bbox_covering_overlap`]).
//! * **Sorting**: ascending/descending, `NULLS LAST` in BOTH directions, text compared byte-wise ([`order_by`]).
//! * **Aggregates**: the function table and the "needs a column" rule ([`aggregate_call`]).
//! * **`SpatialSemiJoin`**: each left row at most once (`EXISTS`), metres never degrees ([`semi_join_exists`], [`within_distance_meters`]).
//! * **`Transform`**: only known CRSs, never guessed ([`proj_name`]).
//! * **Row cap**: one extra row is requested so "exactly the cap" is distinguishable from "over the cap" ([`row_cap`]).
//!
//! What stays in an engine: SQL dialect details (function names, parameter syntax, `ST_AsWKB` vs `ST_AsBinary`), the relational
//! skeleton that depends on the engine's resources (files, catalog, authorization) and capability refusals.
//!
//! The crate is pure (no I/O, no engine, no file format) so the browser lowering built on it also compiles to WebAssembly ([`wasm`]).

use geo_ir::{AggFunc, CompareOp, Expr, Literal, SortKey};
use geo_types::{Crs, Distance};

pub mod browser;
pub mod wasm;

/// Why a plan cannot be lowered. Engines map this onto their own error taxonomy.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LowerError {
    /// The plan is malformed or asks for something the semantics forbid (empty IN list, non-finite number, angular distance...).
    #[error("invalid plan: {0}")]
    Invalid(String),
    /// Well-formed, but this lowering/engine does not implement it (never approximated).
    #[error("unsupported: {0}")]
    Unsupported(String),
}

fn invalid<T>(m: impl Into<String>) -> Result<T, LowerError> {
    Err(LowerError::Invalid(m.into()))
}

// ---------------------------------------------------------------- spelling shared by the DuckDB family and used as a reference

/// `"name"` with embedded quotes doubled. Every identifier is quoted.
pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// `'text'` with embedded quotes doubled.
pub fn quote_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A dotted covering path (`bbox.xmin`) as quoted identifiers (`"bbox"."xmin"`).
pub fn quote_path(path: &str) -> String {
    path.split('.').map(quote_ident).collect::<Vec<_>>().join(".")
}

/// The SQL comparison operator of a [`CompareOp`]. Identical in every dialect.
pub fn cmp_symbol(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "=",
        CompareOp::Ne => "<>",
        CompareOp::Lt => "<",
        CompareOp::Le => "<=",
        CompareOp::Gt => ">",
        CompareOp::Ge => ">=",
    }
}

/// Largest magnitude a literal may have and still be spelled identically by every engine (and by JavaScript).
const MAX_LITERAL: f64 = 1e15;
/// Smallest non-zero magnitude: below it the decimal spelling would need exponents, which dialects spell differently.
const MIN_LITERAL: f64 = 1e-9;

/// The portable spelling of a finite number: integral values keep one decimal (`105.0`), others use Rust's shortest round-trip form.
/// Non-finite numbers and magnitudes outside `[1e-9, 1e15]` (where spellings diverge between engines) are refused, never rounded.
pub fn format_float(v: f64) -> Result<String, LowerError> {
    if !v.is_finite() {
        return invalid(format!("non-finite number {v}"));
    }
    if v != 0.0 && !(MIN_LITERAL..=MAX_LITERAL).contains(&v.abs()) {
        return invalid(format!("number {v:e} is outside the portable literal range"));
    }
    Ok(if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") })
}

/// Integers a JavaScript engine can hold exactly (2^53 - 1).
pub const SAFE_INT: i64 = (1i64 << 53) - 1;

/// A literal spelled INLINE (DuckDB family). `safe_int_only` refuses integers beyond 2^53-1 (the browser: JavaScript numbers).
pub fn inline_literal(l: &Literal, safe_int_only: bool) -> Result<String, LowerError> {
    Ok(match l {
        Literal::Int(i) => {
            if safe_int_only && i.abs() > SAFE_INT {
                return invalid("integer literal out of the safe range");
            }
            i.to_string()
        }
        Literal::Float(f) => format_float(*f)?,
        Literal::Str(s) => quote_string(s),
        Literal::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Literal::Null => "NULL".to_string(),
    })
}

// ---------------------------------------------------------------- predicates

/// How one engine spells the leaves of a predicate tree. The TREE (and its validity rules) is [`predicate`]'s; a dialect decides only
/// the leaves (and may refuse a leaf: PostGIS refuses a comparison that would need a type coercion).
pub trait PredicateDialect {
    fn compare(&mut self, column: &str, op: CompareOp, value: &Literal) -> Result<String, LowerError>;
    fn in_list(&mut self, column: &str, values: &[Literal]) -> Result<String, LowerError>;
    fn is_null(&mut self, column: &str) -> Result<String, LowerError>;
    /// `hash(column) % modulus = remainder`; already validated (`modulus >= 1`, `remainder < modulus`).
    fn hash_bucket(&mut self, column: &str, modulus: u32, remainder: u32) -> Result<String, LowerError>;
}

/// The canonical predicate tree. Refuses an empty `IN`, an empty `AND`/`OR` and an out-of-range `HashBucket` for every engine.
pub fn predicate<D: PredicateDialect>(e: &Expr, d: &mut D) -> Result<String, LowerError> {
    Ok(match e {
        Expr::Compare { column, op, value } => d.compare(column, *op, value)?,
        Expr::In { column, values } => {
            if values.is_empty() {
                return invalid("empty IN list");
            }
            d.in_list(column, values)?
        }
        Expr::IsNull { column } => d.is_null(column)?,
        Expr::HashBucket { column, modulus, remainder } => {
            if *modulus == 0 || remainder >= modulus {
                return invalid("hash_bucket needs modulus >= 1 and remainder < modulus");
            }
            d.hash_bucket(column, *modulus, *remainder)?
        }
        Expr::And { exprs } | Expr::Or { exprs } => {
            if exprs.is_empty() {
                return invalid("empty AND/OR");
            }
            let sep = if matches!(e, Expr::And { .. }) { " AND " } else { " OR " };
            let mut parts = Vec::with_capacity(exprs.len());
            for x in exprs {
                parts.push(format!("({})", predicate(x, d)?));
            }
            parts.join(sep)
        }
        Expr::Not { expr } => format!("NOT ({})", predicate(expr, d)?),
    })
}

/// The DuckDB-family dialect (server DuckDB and DuckDB-Wasm spell predicates identically): literals inline, `md5_number_lower` for the
/// render-thinning bucket (md5 is the same on every DuckDB build, unlike `hash()`, so a thinned sample is identical in the browser and on
/// the server).
#[derive(Debug, Clone, Copy, Default)]
pub struct DuckDbPredicates {
    /// refuse integers beyond 2^53-1 (the browser)
    pub safe_int_only: bool,
}

impl PredicateDialect for DuckDbPredicates {
    fn compare(&mut self, column: &str, op: CompareOp, value: &Literal) -> Result<String, LowerError> {
        Ok(format!("{} {} {}", quote_ident(column), cmp_symbol(op), inline_literal(value, self.safe_int_only)?))
    }
    fn in_list(&mut self, column: &str, values: &[Literal]) -> Result<String, LowerError> {
        let items: Result<Vec<_>, _> = values.iter().map(|v| inline_literal(v, self.safe_int_only)).collect();
        Ok(format!("{} IN ({})", quote_ident(column), items?.join(", ")))
    }
    fn is_null(&mut self, column: &str) -> Result<String, LowerError> {
        Ok(format!("{} IS NULL", quote_ident(column)))
    }
    fn hash_bucket(&mut self, column: &str, modulus: u32, remainder: u32) -> Result<String, LowerError> {
        Ok(format!("(md5_number_lower(CAST({} AS VARCHAR)) % {modulus}) = {remainder}", quote_ident(column)))
    }
}

// ---------------------------------------------------------------- BboxFilter

/// **`BboxFilter` means: the file's covering box overlaps the window** (closed intervals). Not "the geometry's envelope overlaps":
/// Overture's covering boxes are padded, and an engine that tests the geometry instead returns different rows (a live PostGIS run
/// returned 41,150 vs 41,165). `cov` = SQL for the covering `[xmin, ymin, xmax, ymax]`, `window` = SQL for `[x0, y0, x1, y1]`.
pub fn bbox_covering_overlap(cov: &[String; 4], window: &[String; 4]) -> String {
    format!("{} <= {} AND {} >= {} AND {} <= {} AND {} >= {}", cov[0], window[2], cov[2], window[0], cov[1], window[3], cov[3], window[1])
}

// ---------------------------------------------------------------- sorting

/// The canonical `ORDER BY` list: explicit direction, **`NULLS LAST` in both directions** (PostgreSQL's DESC default is NULLS FIRST),
/// text keys compared byte-wise. `text_collation(column)` returns the extra clause an engine needs to get byte-wise order for a text key
/// (PostgreSQL: ` COLLATE "C"`); DuckDB's VARCHAR order is already byte-wise, so it returns `None`.
pub fn order_by(keys: &[SortKey], text_collation: &dyn Fn(&str) -> Option<&'static str>) -> Result<String, LowerError> {
    if keys.is_empty() {
        return invalid("sort without keys");
    }
    Ok(keys
        .iter()
        .map(|k| format!("{}{} {} NULLS LAST", quote_ident(&k.column), text_collation(&k.column).unwrap_or(""), if k.descending { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", "))
}

// ---------------------------------------------------------------- aggregates

/// `COUNT(*)`, `COUNT("c")`, `SUM("c")`... Only `COUNT` may omit the column.
pub fn aggregate_call(func: AggFunc, column: Option<&str>) -> Result<String, LowerError> {
    Ok(match (func, column) {
        (AggFunc::Count, None) => "COUNT(*)".to_string(),
        (_, None) => return invalid("aggregate needs a column"),
        (f, Some(c)) => {
            let name = match f {
                AggFunc::Count => "COUNT",
                AggFunc::Sum => "SUM",
                AggFunc::Min => "MIN",
                AggFunc::Max => "MAX",
                AggFunc::Avg => "AVG",
            };
            format!("{name}({})", quote_ident(c))
        }
    })
}

// ---------------------------------------------------------------- semi join, transform, row cap

/// A `WithinDistance` threshold in METRES, for a predicate evaluated in the plane of a local metric CRS. An angular distance is refused:
/// 200 m is never 200 degrees and never a degree approximation.
pub fn within_distance_meters(d: &Distance) -> Result<f64, LowerError> {
    if d.unit().is_angular() {
        return invalid("within_distance with an angular distance");
    }
    d.to_meters().map_err(|e| LowerError::Invalid(e.to_string()))
}

/// **`SpatialSemiJoin` means: return a left row once if ANY right row satisfies the predicate.** `EXISTS`, so each left row appears at
/// most once and no pair table is materialised (a join + DISTINCT would build one). `left_select` is the select list of the left rows.
pub fn semi_join_exists(left_select: &str, left_rel: &str, right_rel: &str, predicate_sql: &str) -> String {
    format!("SELECT {left_select} FROM {left_rel} AS l WHERE EXISTS (SELECT 1 FROM {right_rel} AS r WHERE {predicate_sql})")
}

/// The PROJ name of a CRS for a transform. Only known CRSs: an undefined or unknown CRS is never guessed.
pub fn proj_name(c: &Crs) -> Result<String, LowerError> {
    match c {
        Crs::OgcCrs84 => Ok("OGC:CRS84".into()),
        Crs::Epsg(code) => Ok(format!("EPSG:{code}")),
        other => invalid(format!("cannot transform {other}: CRS is not known")),
    }
}

/// The number of rows an engine requests so the caller can tell "exactly `max_rows`" from "more than `max_rows`".
pub fn row_cap(max_rows: u64) -> u64 {
    max_rows.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit_str(s: &str) -> Literal {
        Literal::Str(s.into())
    }

    #[test]
    fn floats_have_one_portable_spelling() {
        assert_eq!(format_float(105.0).unwrap(), "105.0");
        assert_eq!(format_float(13.25).unwrap(), "13.25");
        assert_eq!(format_float(-0.5).unwrap(), "-0.5");
        assert_eq!(format_float(0.0).unwrap(), "0.0");
        assert!(format_float(f64::NAN).is_err());
        assert!(format_float(f64::INFINITY).is_err());
        assert!(format_float(1e21).is_err(), "exponent territory is refused, not spelled differently per engine");
        assert!(format_float(1e-12).is_err());
    }

    #[test]
    fn integers_beyond_javascript_precision_are_refused_only_when_asked() {
        assert!(inline_literal(&Literal::Int(1 << 60), false).is_ok());
        assert!(inline_literal(&Literal::Int(1 << 60), true).is_err());
        assert_eq!(inline_literal(&Literal::Int(SAFE_INT), true).unwrap(), SAFE_INT.to_string());
    }

    #[test]
    fn strings_cannot_break_out_of_their_quotes() {
        assert_eq!(inline_literal(&lit_str("O'Brien"), false).unwrap(), "'O''Brien'");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_path("bbox.xmin"), "\"bbox\".\"xmin\"");
    }

    #[test]
    fn predicate_tree_is_validated_once_for_every_dialect() {
        let mut d = DuckDbPredicates::default();
        let e = Expr::And { exprs: vec![Expr::Compare { column: "a".into(), op: CompareOp::Ge, value: Literal::Int(3) }, Expr::Not { expr: Box::new(Expr::IsNull { column: "b".into() }) }] };
        assert_eq!(predicate(&e, &mut d).unwrap(), "(\"a\" >= 3) AND (NOT (\"b\" IS NULL))");
        assert!(predicate(&Expr::In { column: "a".into(), values: vec![] }, &mut d).is_err());
        assert!(predicate(&Expr::And { exprs: vec![] }, &mut d).is_err());
        assert!(predicate(&Expr::HashBucket { column: "id".into(), modulus: 0, remainder: 0 }, &mut d).is_err());
        assert!(predicate(&Expr::HashBucket { column: "id".into(), modulus: 4, remainder: 4 }, &mut d).is_err());
    }

    #[test]
    fn bbox_is_the_covering_box_overlap() {
        let cov = ["\"bbox\".\"xmin\"".to_string(), "\"bbox\".\"ymin\"".to_string(), "\"bbox\".\"xmax\"".to_string(), "\"bbox\".\"ymax\"".to_string()];
        let w = ["104.0".to_string(), "12.0".to_string(), "105.5".to_string(), "13.25".to_string()];
        assert_eq!(
            bbox_covering_overlap(&cov, &w),
            "\"bbox\".\"xmin\" <= 105.5 AND \"bbox\".\"xmax\" >= 104.0 AND \"bbox\".\"ymin\" <= 13.25 AND \"bbox\".\"ymax\" >= 12.0"
        );
    }

    #[test]
    fn sort_puts_nulls_last_in_both_directions_and_names_the_collation() {
        let keys = vec![SortKey { column: "n".into(), descending: true }, SortKey { column: "category".into(), descending: false }];
        assert_eq!(order_by(&keys, &|_| None).unwrap(), "\"n\" DESC NULLS LAST, \"category\" ASC NULLS LAST");
        let c = |col: &str| if col == "category" { Some(" COLLATE \"C\"") } else { None };
        assert_eq!(order_by(&keys, &c).unwrap(), "\"n\" DESC NULLS LAST, \"category\" COLLATE \"C\" ASC NULLS LAST");
        assert!(order_by(&[], &|_| None).is_err());
    }

    #[test]
    fn aggregates_follow_one_table() {
        assert_eq!(aggregate_call(AggFunc::Count, None).unwrap(), "COUNT(*)");
        assert_eq!(aggregate_call(AggFunc::Avg, Some("h")).unwrap(), "AVG(\"h\")");
        assert!(aggregate_call(AggFunc::Sum, None).is_err());
    }

    #[test]
    fn semi_join_is_exists_and_distances_are_metres() {
        assert_eq!(semi_join_exists("l.*", "\"n1\"", "\"n2\"", "P"), "SELECT l.* FROM \"n1\" AS l WHERE EXISTS (SELECT 1 FROM \"n2\" AS r WHERE P)");
        let m = Distance::new(200.0, geo_types::DistanceUnit::Meter).unwrap();
        assert_eq!(within_distance_meters(&m).unwrap(), 200.0);
        let deg = Distance::new(0.002, geo_types::DistanceUnit::Degree).unwrap();
        assert!(within_distance_meters(&deg).is_err());
    }

    #[test]
    fn crs_names_are_known_or_refused() {
        assert_eq!(proj_name(&Crs::OgcCrs84).unwrap(), "OGC:CRS84");
        assert_eq!(proj_name(&Crs::Epsg(32648)).unwrap(), "EPSG:32648");
        assert!(proj_name(&Crs::Undefined).is_err());
    }

    #[test]
    fn row_cap_never_overflows() {
        assert_eq!(row_cap(1000), 1001);
        assert_eq!(row_cap(u64::MAX), u64::MAX);
    }
}
