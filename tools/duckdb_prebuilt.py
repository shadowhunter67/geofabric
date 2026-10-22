#!/usr/bin/env python3
"""Fetch + verify the official prebuilt libduckdb (ADR-006, development mode).

Why not let the cargo build script download it: we want an explicit,
verifiable, offline-repeatable step. The archive is checked against

  1. the sha256 digest GitHub publishes for the release asset (API), and
  2. tools/duckdb-prebuilt.lock.json (trust-on-first-use, then pinned),

and extracted into tools/.cache/duckdb-<ver>/lib (git-ignored). The DuckDB
version is derived from Cargo.lock (libduckdb-sys 1.10506.0 -> DuckDB v1.5.6),
so the library can never drift from the Rust binding.

Usage:
    python tools/duckdb_prebuilt.py            # fetch/verify/extract, print env
    python tools/duckdb_prebuilt.py --env      # only print the env lines (bash)
Then (bash):
    export DUCKDB_LIB_DIR=<printed dir>; export PATH="$DUCKDB_LIB_DIR:$PATH"
    cargo test --workspace --no-default-features
"""
import hashlib
import json
import pathlib
import re
import sys
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCK = ROOT / "tools" / "duckdb-prebuilt.lock.json"
ASSET = "libduckdb-windows-amd64.zip"


def duckdb_version() -> str:
    text = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    m = re.search(r'name = "libduckdb-sys"\s+version = "1\.(\d)(\d{2})(\d{2})\.\d+"', text)
    if not m:
        sys.exit("libduckdb-sys not found in Cargo.lock (run cargo fetch first)")
    # crate 1.10506.0 -> DuckDB 1.5.6 : digits "10506" = major 1, minor 05, patch 06
    return f"{m.group(1)}.{int(m.group(2))}.{int(m.group(3))}"


def http_json(url):
    """GitHub API read. Authenticated when GITHUB_TOKEN is set (CI: the anonymous limit of a shared runner IP is exhausted by other tenants,
    which once failed a whole CI run with 'HTTP 403 rate limit exceeded'), retried with a pause on transient errors."""
    import os
    import time
    import urllib.error

    headers = {"User-Agent": "geofabric-tools"}
    if os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    for attempt in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            if attempt == 3 or e.code not in (403, 429, 500, 502, 503):
                raise
            time.sleep(10 * (attempt + 1))


def sha256(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ver = duckdb_version()
    cache = ROOT / "tools" / ".cache" / f"duckdb-{ver}"
    lib = cache / "lib"
    if "--env" in sys.argv:
        print(f'export DUCKDB_LIB_DIR="{lib.as_posix()}"; export PATH="$DUCKDB_LIB_DIR:$PATH"')
        return
    if (lib / "duckdb.dll").exists() and (lib / "duckdb.lib").exists() and LOCK.exists():
        print(f"already present: {lib}")
    else:
        lock = json.loads(LOCK.read_text()) if LOCK.exists() else {}
        pinned = lock.get(f"v{ver}", {}).get(ASSET)
        if pinned:
            # the digest is already pinned locally (trust-on-first-use, then pinned): no API call is needed, only the download
            expected = pinned
            url = f"https://github.com/duckdb/duckdb/releases/download/v{ver}/{ASSET}"
        else:
            rel = http_json(f"https://api.github.com/repos/duckdb/duckdb/releases/tags/v{ver}")
            asset = next(a for a in rel["assets"] if a["name"] == ASSET)
            expected = asset["digest"].split(":", 1)[1]
            url = asset["browser_download_url"]
        dl = cache / "download"
        dl.mkdir(parents=True, exist_ok=True)
        zpath = dl / ASSET
        urllib.request.urlretrieve(url, zpath)
        got = sha256(zpath)
        if got != expected:
            sys.exit(f"CHECKSUM MISMATCH vs GitHub digest: {got} != {expected}")
        if pinned and pinned != got:
            sys.exit(f"CHECKSUM MISMATCH vs lock file: {got} != {pinned}")
        lock.setdefault(f"v{ver}", {})[ASSET] = got
        LOCK.write_text(json.dumps(lock, indent=2) + "\n")
        lib.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(zpath) as z:
            for n in z.namelist():
                target = (lib / n).resolve()
                if not str(target).startswith(str(lib.resolve())):
                    sys.exit(f"unsafe path in archive: {n}")
            z.extractall(lib)
        print(f"verified sha256 {got} (GitHub digest + lock file)")
    print(f"DuckDB v{ver} prebuilt in {lib}")
    print(f'bash:  export DUCKDB_LIB_DIR="{lib.as_posix()}"; export PATH="$DUCKDB_LIB_DIR:$PATH"')


if __name__ == "__main__":
    main()
