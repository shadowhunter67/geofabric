//! Result-delivery policy (`result-policy-v1`): can the FULL result of this plan be materialised and handed on, whichever engine computes it?
//!
//! This is deliberately separate from execution risk (does the engine finish in time?). It looks only at the plan's OUTPUT (never at rows scanned or
//! matched: `COUNT(*)` over 2 M matches returns one row and is `safe`), it is the same for every server engine, and it never changes the result: a
//! plan that returns "all matching rows" is either deliverable or refused, never silently cut to the first N. A `Limit` shortens the result only when
//! the user wrote it (`bounded_by_limit`); rendering caps live in the render path, not here.
//!
//! The cap is the runtime's materialisation limit (`ExecutionContext::max_rows`: 1,000,000 by default, 100,001 in the gateway), enforced by
//! every engine after execution (`LimitExceeded`); an engine may declare a tighter one. Judging it at plan time spares a run that would be thrown away.

use serde::Serialize;

pub const RESULT_POLICY_VERSION: &str = "result-policy-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVerdict {
    /// Even the pessimistic row estimate is within the cap.
    Safe,
    /// The result is shortened by an explicit, certain user `Limit` that is within the cap.
    Bounded,
    /// The point estimate fits but the pessimistic bound does not (estimate quality is the doubt).
    Uncertain,
    /// The point estimate of the full result exceeds the cap: no engine can deliver it.
    TooLarge,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultDelivery {
    pub version: &'static str,
    pub verdict: DeliveryVerdict,
    pub rows: u64,
    pub rows_high: u64,
    pub bytes: u64,
    pub cap_rows: u64,
    pub reason: String,
}

/// `rows` / `rows_high` are the OUTPUT rows (after aggregate and LIMIT, as `RowEstimate` defines them). `bounded_by_limit`: an explicit LIMIT is what keeps them small.
pub fn assess(rows: u64, rows_high: u64, bytes: u64, cap_rows: u64, bounded_by_limit: bool) -> ResultDelivery {
    let explicit_bound = bounded_by_limit && rows_high <= cap_rows;
    let (verdict, reason) = if rows > cap_rows {
        (
            DeliveryVerdict::TooLarge,
            format!("the full result is estimated at {rows} rows (up to {rows_high}), above the delivery cap of {cap_rows}. Add a filter, reduce the area, add an explicit limit, or aggregate/count instead of returning every feature"),
        )
    } else if explicit_bound {
        (DeliveryVerdict::Bounded, format!("an explicit limit keeps the result within the cap of {cap_rows} (at most {rows_high} rows)"))
    } else if rows_high > cap_rows {
        (DeliveryVerdict::Uncertain, format!("estimated {rows} rows fit the cap of {cap_rows}, but the pessimistic bound ({rows_high}) does not"))
    } else {
        (DeliveryVerdict::Safe, format!("up to {rows_high} rows, within the cap of {cap_rows}"))
    };
    ResultDelivery { version: RESULT_POLICY_VERSION, verdict, rows, rows_high, bytes, cap_rows, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CAP: u64 = 1_000_000;

    #[test]
    fn an_aggregate_over_a_huge_input_is_safe_because_only_its_output_counts() {
        // the planner's estimate for COUNT(*)/GROUP BY is its OUTPUT rows (1 or the group count), however many rows match
        assert_eq!(assess(1, 1, 16, CAP, false).verdict, DeliveryVerdict::Safe);
        assert_eq!(assess(40, 120, 4_000, CAP, false).verdict, DeliveryVerdict::Safe);
    }

    #[test]
    fn a_raw_feature_result_above_the_cap_is_too_large() {
        let d = assess(1_095_005, 2_371_958, 170_000_000, CAP, false);
        assert_eq!(d.verdict, DeliveryVerdict::TooLarge);
        assert!(d.reason.contains("explicit limit") && d.reason.contains("aggregate"), "the refusal must say what would work: {}", d.reason);
    }

    #[test]
    fn an_explicit_certain_limit_under_the_cap_is_bounded() {
        assert_eq!(assess(1_000, 1_000, 0, CAP, true).verdict, DeliveryVerdict::Bounded);
    }

    #[test]
    fn an_uncertain_limit_is_never_a_guarantee() {
        // no limit shortens it (the planner cannot rely on one) and the pessimistic bound is over the cap -> uncertain, not bounded
        assert_eq!(assess(900_000, 2_000_000, 0, CAP, false).verdict, DeliveryVerdict::Uncertain);
    }

    #[test]
    fn a_limit_above_the_cap_does_not_bound_anything() {
        assert_eq!(assess(2_000_000, 2_000_000, 0, CAP, true).verdict, DeliveryVerdict::TooLarge);
    }

    #[test]
    fn the_boundary_is_exact() {
        assert_eq!(assess(CAP, CAP, 0, CAP, false).verdict, DeliveryVerdict::Safe);
        assert_eq!(assess(CAP + 1, CAP + 1, 0, CAP, false).verdict, DeliveryVerdict::TooLarge);
        assert_eq!(assess(CAP, CAP + 1, 0, CAP, false).verdict, DeliveryVerdict::Uncertain);
    }
}
