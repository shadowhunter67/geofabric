#!/usr/bin/env python3
"""Architecture fitness check (CONSTITUTION 4, 11, 12, 10).

Uses the resolved Cargo dependency graph (transitive), not regex over source.
Also scans web/package.json for forbidden engine/server deps.
Exit code 1 on any violation. Run: python tools/arch_check.py
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

CLOUD = ["aws-sdk-*", "aws-config", "rusoto*", "google-cloud-*", "gcp_auth", "azure_*", "object_store-gcp"]
ENGINES = ["geo-engine-*", "duckdb", "libduckdb-sys", "postgres", "tokio-postgres", "sqlx*", "postgis*"]
UI = ["maplibre*", "web-sys", "wasm-bindgen", "geo-web*"]

# crate pattern -> forbidden transitive dependency patterns
RULES = {
    # engine FAMILIES are isolated from each other (first match wins, so these come before the generic geo-engine-* rule):
    # the PostGIS adapter knows no DuckDB and no GeoParquet reader; the DuckDB/browser adapters know no PostgreSQL driver
    "geo-engine-postgis": ["geo-planner", "geo-runtime", "geo-engine-duckdb", "geo-engine-browser", "geo-source-*", "duckdb", "libduckdb-sys"] + UI,
    "geo-engine-duckdb": ["geo-planner", "geo-runtime", "geo-engine-postgis", "postgres", "tokio-postgres", "sqlx*"] + UI,
    "geo-engine-browser": ["geo-planner", "geo-runtime", "geo-engine-postgis", "postgres", "tokio-postgres", "sqlx*"] + UI,
    # the canonical lowering is PURE (RFC-004): semantics only, no engine, no file format, no planner, no UI; it must also build for wasm32
    "geo-lowering": ["geo-planner", "geo-executor", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-types": ["geo-ir", "geo-planner", "geo-executor", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-ir": ["geo-planner", "geo-executor", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-executor": ["geo-planner", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-planner": ENGINES + UI + CLOUD,       # planner is separate from executors; engines arrive via capabilities
    "geo-source-*": ["geo-planner", "geo-runtime"] + ENGINES + UI,   # storage must not depend on renderer/engine
    "geo-engine-*": ["geo-planner", "geo-runtime"] + UI,             # adapters: geo-types/geo-ir/geo-executor only
}
# direct (non-transitive) dependency rules: the gateway must go through geo-runtime
DIRECT_RULES = {"geo-gateway": ["geo-engine-*", "duckdb", "libduckdb-sys", "postgres", "tokio-postgres", "sqlx*"],
                # only the composition root wires engine adapters; the planner and executor contract know none
                "geo-planner": ["geo-engine-*"]}
# every engine adapter must build its SQL on the canonical lowering (RFC-004), not carry its own copy of the semantics
REQUIRED_DIRECT = {"geo-engine-duckdb": "geo-lowering", "geo-engine-postgis": "geo-lowering", "geo-engine-browser": "geo-lowering"}
# semantic building blocks that only geo-lowering may spell: an engine crate that writes one of these itself is re-implementing semantics
ENGINE_FORBIDDEN_TEXT = {
    "NULLS LAST": "sort null placement is canonical (geo_lowering::order_by)",
    "COUNT(*)": "aggregate calls are canonical (geo_lowering::aggregate_call)",
    "EXISTS (SELECT 1": "the semi-join form is canonical (geo_lowering::semi_join_exists)",
}
# the web client must not compile plans: the browser runs the Rust lowering as WebAssembly (web/src/engine/lowering.ts)
WEB_FORBIDDEN_TEXT = ["NULLS LAST", "read_parquet(", "ORDER BY", "NOT MATERIALIZED"]
WEB_FORBIDDEN_NPM = ["duckdb", "duckdb-async", "pg", "postgres", "better-sqlite3"]


def match(pat, name):
    if pat.endswith("*"):
        return name.startswith(pat[:-1])
    return name == pat


def main() -> int:
    # self-test hook: --extra-forbid <crate>=<dep> proves the checker catches violations
    for a in sys.argv[1:]:
        if a.startswith('--extra-forbid='):
            c, d = a.split('=', 2)[1:]
            RULES.setdefault(c, []).append(d)
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if out.returncode != 0:
        out = subprocess.run(["cargo", "metadata", "--format-version", "1"], cwd=ROOT, capture_output=True, text=True)
    if out.returncode != 0:
        print("cargo metadata failed:\n" + out.stderr)
        return 2
    md = json.loads(out.stdout)
    pkgs = {p["id"]: p for p in md["packages"]}
    graph = {n["id"]: [d["pkg"] for d in n["deps"]] for n in md["resolve"]["nodes"]}  # normal+dev+build
    # dev-deps count too only if kind is None; keep it strict: filter dev deps
    graph = {}
    for n in md["resolve"]["nodes"]:
        deps = []
        for d in n["deps"]:
            if any(k["kind"] in (None, "build") and "wasm" not in str(k.get("target")) for k in d["dep_kinds"]):
                deps.append(d["pkg"])
        graph[n["id"]] = deps
    members = set(md["workspace_members"])
    violations = []
    checked = 0
    for mid in members:
        name = pkgs[mid]["name"]
        forb = None
        for pat, f in RULES.items():
            if match(pat, name):
                forb = f
                break
        if forb is None:
            continue
        checked += 1
        seen, stack = set(), list(graph[mid])
        while stack:
            cur = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            stack.extend(graph.get(cur, []))
        for dep in seen:
            dn = pkgs[dep]["name"]
            if any(match(p, dn) for p in forb):
                violations.append(f"{name} -> {dn} (forbidden by rules)")
    for mid in members:
        name = pkgs[mid]["name"]
        for pat, forb in DIRECT_RULES.items():
            if match(pat, name):
                for dep in graph[mid]:
                    dn = pkgs[dep]["name"]
                    if any(match(p, dn) for p in forb):
                        violations.append(f"{name} -> {dn} (direct dependency forbidden; use geo-runtime)")
    # source-text rules (cheap, complement the graph rules)
    import re
    for f in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        crate = f.relative_to(ROOT / "crates").parts[0]
        text = f.read_text(encoding="utf-8", errors="ignore")
        # unchecked opens bypass the authorization boundary (only the source crate itself may define it)
        if crate != "geo-source-geoparquet" and re.search(r"GeoParquetSource::open\(", text):
            violations.append(f"{crate}: {f.name} uses GeoParquetSource::open (unchecked); use open_authorized with an AuthorizedSource")
        if crate in ("geo-planner", "geo-ir", "geo-executor") and re.search(r"(duckdb|DuckDb|ST_[A-Z][A-Za-z]+)", re.sub(r"//.*", "", text)):
            violations.append(f"{crate}: {f.name} names DuckDB/SQL spatial functions outside comments")
    # canonical-lowering rules (RFC-004)
    for mid in members:
        name = pkgs[mid]["name"]
        need = REQUIRED_DIRECT.get(name)
        if need and not any(pkgs[d]["name"] == need for d in graph[mid]):
            violations.append(f"{name} must depend on {need}: engine adapters lower through the canonical lowering")
    for f in sorted((ROOT / "crates").glob("geo-engine-*/src/**/*.rs")):
        crate = f.relative_to(ROOT / "crates").parts[0]
        code = re.sub(r"//.*", "", f.read_text(encoding="utf-8", errors="ignore"))
        for token, why in ENGINE_FORBIDDEN_TEXT.items():
            if token in code:
                violations.append(f"{crate}: {f.name} spells {token!r}: {why}")
    web_src = ROOT / "web" / "src"
    if (web_src / "engine" / "sqlgen.ts").exists():
        violations.append("web: src/engine/sqlgen.ts exists: the browser must use the Rust lowering (WebAssembly), not a TypeScript compiler")
    for f in sorted(web_src.glob("**/*.ts")) if web_src.exists() else []:
        code = re.sub(r"//.*", "", f.read_text(encoding="utf-8", errors="ignore"))
        for token in WEB_FORBIDDEN_TEXT:
            if token in code and f.name != "i18n.ts":
                violations.append(f"web: {f.relative_to(ROOT).as_posix()} contains {token!r}: SQL is produced only by the Rust lowering")
    # web package.json
    pj = ROOT / "web" / "package.json"
    if pj.exists():
        j = json.loads(pj.read_text(encoding="utf-8"))
        deps = {**j.get("dependencies", {}), **j.get("devDependencies", {})}
        for d in deps:
            if d in WEB_FORBIDDEN_NPM:
                violations.append(f"web -> {d} (UI must not embed an engine; go through the API)")
    print(f"arch_check: {checked} workspace crates checked against rules")
    if violations:
        print("VIOLATIONS:")
        for v in sorted(set(violations)):
            print("  - " + v)
        return 1
    print("OK: no dependency-boundary violations")
    return 0


if __name__ == "__main__":
    sys.exit(main())
