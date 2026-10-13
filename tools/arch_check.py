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
    "geo-types": ["geo-ir", "geo-planner", "geo-executor", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-ir": ["geo-planner", "geo-executor", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-executor": ["geo-planner", "geo-runtime", "geo-source-*"] + ENGINES + UI + CLOUD,
    "geo-planner": ENGINES + UI + CLOUD,       # planner is separate from executors; engines arrive via capabilities
    "geo-source-*": ["geo-planner", "geo-runtime"] + ENGINES + UI,   # storage must not depend on renderer/engine
    "geo-engine-*": ["geo-planner", "geo-runtime"] + UI,             # adapters: geo-types/geo-ir/geo-executor only
}
WEB_FORBIDDEN_NPM = ["duckdb", "duckdb-async", "pg", "postgres", "better-sqlite3"]


def match(pat, name):
    if pat.endswith("*"):
        return name.startswith(pat[:-1])
    return name == pat


def main() -> int:
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
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
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
