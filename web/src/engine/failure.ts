// Failure taxonomy (ADR-010), client side. Mirrors `geo_planner::FailureClass`; both implementations are
// checked against crates/geo-planner/tests/fixtures/failure-classes.json so they cannot drift.

export type FailureClass =
  | "CAPABILITY" | "MEMORY" | "TIMEOUT" | "CANCELLED" | "ENGINE_CRASH"
  | "UNSUPPORTED_FORMAT" | "UNSUPPORTED_OPERATION" | "TRANSIENT";

const has = (m: string, ks: string[]) => ks.some((k) => m.includes(k));

/** Order matters: the most specific signals first. Unknown text is TRANSIENT. */
export function classifyFailure(message: string): FailureClass {
  const m = message.toLowerCase();
  if (has(m, ["cancel", "aborted by the user", "client disconnect"])) return "CANCELLED";
  if (has(m, ["crashed", "target closed", "worker terminated", "worker has been terminated", "engine_crash", "sigsegv", "panicked"])) return "ENGINE_CRASH";
  if (has(m, ["out of memory", "outofmemory", "oom", "memory limit", "memory guard", "allocation failed", "cannot allocate", "std::bad_alloc", "limit_exceeded"])) return "MEMORY";
  if (has(m, ["timed out", "timeout", "interrupted after", "execution_timeout"])) return "TIMEOUT";
  if (has(m, ["activetransaction", "unsupported geometry", "cannot read geometry", "not a parquet", "unsupported encoding", "unsupported_format", "invalid parquet", "geoparquet", "unsupported geometry encoding", "parquet:"])) return "UNSUPPORTED_FORMAT";
  if (has(m, ["unsupported operation", "unsupported:", "not supported", "unsupported_operation", "unsupported op"])) return "UNSUPPORTED_OPERATION";
  if (has(m, ["capability", "not available", "no such function", "unknown function", "extension"])) return "CAPABILITY";
  return "TRANSIENT";
}

export const isDeterministic = (c: FailureClass) => c === "CAPABILITY" || c === "UNSUPPORTED_FORMAT" || c === "UNSUPPORTED_OPERATION";
/** Cancellation is the user's decision: no fallback. Everything else gets at most one fallback. */
export const fallbackAllowed = (c: FailureClass) => c !== "CANCELLED";
/** No same-engine retries in v1. */
export const retryAllowed = (_c: FailureClass) => false;
