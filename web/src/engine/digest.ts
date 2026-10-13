// Canonical result digest — must stay identical to `geo_executor::result_digest` (Rust) and to
// benchmarks/oracle*.py: one string per row (cells joined by "|"; integers decimal, booleans
// true/false, strings raw, binary lowercase hex, null "NULL"), rows sorted, SHA-256 over them
// joined by "\n". `count` = the value of a scalar integer result (1 row x 1 column), else #rows.
// Sorting uses plain string comparison, identical to Rust's byte order for ASCII/BMP text.

export function cellString(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "bigint" || typeof v === "number") return v.toString();
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "string") return v;
  if (v instanceof Uint8Array) return Array.from(v, (b) => b.toString(16).padStart(2, "0")).join("");
  throw new Error(`unsupported cell type for the canonical digest: ${typeof v}`);
}

async function sha256Hex(text: string): Promise<string> {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * `types` (Arrow type names, optional) makes the "scalar count" rule exact: only an `Int64` single
 * cell is a count, exactly like the Rust digest (a Float64 `4.0` is a row, not a count).
 */
export async function resultDigest(rows: unknown[][], types?: string[]): Promise<{ count: number; sha256: string }> {
  const strings = rows.map((r) => r.map(cellString).join("|"));
  const isInt64 = types ? types[0] === "Int64" : typeof rows[0]?.[0] === "bigint" || Number.isInteger(rows[0]?.[0]);
  const scalar = rows.length === 1 && rows[0].length === 1 && isInt64;
  const count = scalar ? Number(strings[0]) : rows.length;
  strings.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  return { count, sha256: await sha256Hex(strings.join("\n")) };
}
