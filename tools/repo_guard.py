#!/usr/bin/env python3
"""Repository guard: generated artifacts, oversized files and stray binaries must not enter Git.

Why: a 109 MB build directory (`web/dist-range`) was once committed by accident and, once public, stays in history for good. The history is not
rewritten for that; this guard makes the next one impossible. It is fast (one `git ls-files`) and runs first in CI, before any expensive job.

Checks over the TRACKED files (or, with --staged, over what is about to be committed):
  1. forbidden directories anywhere in a path: dist/, dist-*/, target/, node_modules/, .runtime/, coverage/, playwright-report/, test-results/,
     __pycache__/, .venv/, .cache/, pgdata/ (build output, dependencies, caches, local runtimes)
  2. file size: any tracked file above MAX_BYTES (20 MiB; the largest intentional asset is the 14 MB demo layer) unless allowlisted
  3. binary/generated file types (executables, libraries, archives, databases, dumps, parquet, wasm...) outside the approved paths

The allowlist (tools/repo_guard.allow.json) names each approved exception and WHY. An exception without a reason is itself an error.

    python tools/repo_guard.py            # tracked files
    python tools/repo_guard.py --staged   # what `git commit` would add (use as a pre-commit hook: see .githooks/pre-commit)
Exit code 1 on any violation.
"""

from __future__ import annotations

import fnmatch
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MAX_BYTES = 20 * 1024 * 1024
FORBIDDEN_DIRS = re.compile(r"(^|/)(dist|dist-[^/]+|target|node_modules|\.runtime|coverage|playwright-report|test-results|__pycache__|\.venv|\.cache|pgdata)/")
BINARY_EXT = {
    ".exe", ".dll", ".pdb", ".so", ".dylib", ".pyd", ".obj", ".lib", ".rlib", ".o", ".a", ".zip", ".7z", ".tar", ".gz", ".tgz", ".rar",
    ".parquet", ".db", ".sqlite", ".sqlite3", ".dump", ".bak", ".wasm", ".bin", ".jar", ".pkl", ".npy", ".pyc",
}
LOG_LIMIT = 1024 * 1024  # a log above 1 MiB is a captured run, not evidence worth keeping in Git


def tracked(staged: bool) -> list[tuple[str, int]]:
    if staged:
        names = subprocess.run(["git", "diff", "--cached", "--name-only", "--diff-filter=AM"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split("\n")
    else:
        names = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split("\n")
    out = []
    for n in names:
        if not n:
            continue
        p = ROOT / n
        out.append((n, p.stat().st_size if p.exists() else 0))
    return out


def load_allow() -> tuple[list[dict], list[str]]:
    p = ROOT / "tools" / "repo_guard.allow.json"
    data = json.loads(p.read_text(encoding="utf-8")) if p.exists() else {"allow": []}
    problems = [f"allowlist entry {e.get('pattern')!r} has no reason" for e in data["allow"] if not e.get("reason")]
    return data["allow"], problems


def main() -> int:
    staged = "--staged" in sys.argv
    allow, problems = load_allow()
    used: set[str] = set()

    def allowed(path: str, what: str) -> bool:
        for e in allow:
            if what in e.get("covers", ["dir", "size", "binary"]) and fnmatch.fnmatch(path, e["pattern"]):
                used.add(e["pattern"])
                return True
        return False

    files = tracked(staged)
    for path, size in files:
        low = path.lower()
        if FORBIDDEN_DIRS.search("/" + path) and not allowed(path, "dir"):
            problems.append(f"{path}: inside a build/dependency/cache/runtime directory (generated output must stay untracked; .gitignore it)")
        if size > MAX_BYTES and not allowed(path, "size"):
            problems.append(f"{path}: {size / 1048576:.1f} MiB exceeds the {MAX_BYTES // 1048576} MiB limit (publish large data outside the source history)")
        ext = Path(low).suffix
        if ext in BINARY_EXT and not allowed(path, "binary"):
            problems.append(f"{path}: binary/generated file type {ext!r} outside the approved paths (add an allowlist entry with a reason if it is intentional)")
        if ext in (".log", ".out", ".err") and size > LOG_LIMIT and not allowed(path, "size"):
            problems.append(f"{path}: a {size / 1048576:.1f} MiB log; captured run output does not belong in Git")
    if not staged:
        for e in allow:
            if e["pattern"] not in used:
                problems.append(f"allowlist entry {e['pattern']!r} matches no tracked file: remove it (stale exceptions hide future mistakes)")
    total = sum(s for _, s in files)
    print(f"repo_guard: {len(files)} {'staged' if staged else 'tracked'} files, {total / 1048576:.1f} MiB, largest {max((s for _, s in files), default=0) / 1048576:.1f} MiB")
    if problems:
        print("VIOLATIONS:")
        for p in problems:
            print("  - " + p)
        return 1
    print("OK: no generated artifacts, oversized files or stray binaries are tracked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
