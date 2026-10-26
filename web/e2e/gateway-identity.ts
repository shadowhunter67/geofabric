// Is the gateway the full-stack suite is about to test the one built from THIS source tree?
//
// 2026-10-26: Windows kept the old gateway.exe locked, `cargo build` failed, and a whole Chrome + Edge run went against the stale binary. The suite now
// records the gateway's build identity (commit + "dirty" flag, from /api/health) and refuses a binary that is provably stale:
//   * clean build whose commit is not HEAD                      -> stale
//   * the binary file is older than the newest Rust source file -> stale (covers dirty builds and uncommitted edits)
// A dirty build at HEAD with no newer source is accepted and reported (it is what a developer runs between commits).

export interface GatewayBuild { commit: string; dirty: boolean }

export interface IdentityInput {
  build: GatewayBuild | undefined;
  head: string;
  /** mtime (ms) of the gateway executable, when the suite knows its path (GATEWAY_BIN) */
  binaryMtimeMs?: number;
  /** newest mtime (ms) among crates/**.rs, Cargo.toml and Cargo.lock */
  newestSourceMtimeMs?: number;
}

export interface IdentityVerdict { ok: boolean; problems: string[]; notes: string[] }

export function checkGatewayIdentity(i: IdentityInput): IdentityVerdict {
  const problems: string[] = [];
  const notes: string[] = [];
  if (!i.build) {
    problems.push("the gateway does not report a build identity (/api/health has no `build`): it is an old binary");
  } else {
    if (!i.build.dirty && i.build.commit !== i.head) problems.push(`the gateway was built from ${i.build.commit.slice(0, 10)} but HEAD is ${i.head.slice(0, 10)}`);
    if (i.build.dirty) notes.push(`built from ${i.build.commit.slice(0, 10)} with uncommitted Rust changes`);
  }
  if (i.binaryMtimeMs !== undefined && i.newestSourceMtimeMs !== undefined && i.binaryMtimeMs + 1000 < i.newestSourceMtimeMs) {
    problems.push(`the gateway executable (${new Date(i.binaryMtimeMs).toISOString()}) is older than the newest Rust source (${new Date(i.newestSourceMtimeMs).toISOString()}): rebuild it`);
  }
  return { ok: problems.length === 0, problems, notes };
}
