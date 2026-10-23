"""Download the public demo data named by data/public-demo/pack.json into data/public-demo-build/<version>/ (git-ignored), verifying SHA-256.

Needed only to BUNDLE the data with a site build (CI static tests, offline/dev): `GF_DEMO=bundle node web/scripts/copy-demo.mjs` reads this directory.
The deployed demo does not bundle; it reads the data host directly. Files already present with the right hash are kept.

    python data/scripts/fetch_demo_data.py
"""

from __future__ import annotations

import hashlib
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def main() -> int:
    manifest = json.loads((ROOT / "data" / "public-demo" / "pack.json").read_text(encoding="utf-8"))
    out = ROOT / "data" / "public-demo-build"
    bad = 0
    for sid, f in manifest["files"].items():
        dest = out / f["path"]
        if dest.exists() and hashlib.sha256(dest.read_bytes()).hexdigest() == f["sha256"]:
            print(f"ok (cached) {sid}")
            continue
        host = manifest.get("host") or {}
        # from GitHub itself at the pinned commit (same bytes as the CDN serves, and no CDN dependence in CI); fall back to the manifest URL
        url = f"https://raw.githubusercontent.com/{host['repository']}/{host['commit']}/{f['path']}" if host.get("repository") and host.get("commit") else f.get("url")
        if not url:
            print(f"{sid}: the manifest names no host")
            bad += 1
            continue
        req = urllib.request.Request(url, headers={"User-Agent": "geofabric-fetch/1"})
        with urllib.request.urlopen(req, timeout=120) as r:
            body = r.read()
        if hashlib.sha256(body).hexdigest() != f["sha256"] or len(body) != f["bytes"]:
            print(f"FAIL {sid}: downloaded bytes differ from the manifest (size {len(body)}, expected {f['bytes']})")
            bad += 1
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(body)
        print(f"ok (downloaded {len(body)} bytes) {sid}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
