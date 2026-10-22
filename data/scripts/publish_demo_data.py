"""Publish the public demo data: validate, pin to a commit, verify the host with real requests, emit the public manifest.

The data lives in Git (data/public-demo/v1/*.parquet.zip, plain GeoParquet; ~27 MB) and is served by jsDelivr's GitHub CDN, which gives
immutable, commit-pinned URLs, CORS and true HTTP range reads without an account, credentials or a server of ours. "Upload" is therefore
`git push`; there is no drag-and-drop step. This script does everything around it:

  1. validate     every file named by the manifest exists, its size and SHA-256 match, licence/format/CRS/release metadata are present,
                  and each file is under the CDN's per-file limit
  2. pin          the URL base is pinned to the commit that last touched data/public-demo/v1/ (content-addressed: a URL never changes
                  meaning); that commit must already be on origin/main (otherwise a force-push could delete the data)
  3. verify       data/scripts/verify_range_host.py against the pinned URLs: HEAD/GET Range -> 206, Content-Range, no Content-Encoding,
                  CORS for the app origin, Parquet magic, SHA-256 of the served bytes (waits for a fresh commit to reach the CDN)
  4. emit         rewrites data/public-demo/pack.json: `host` (kind, base_url, commit, range=true, verified_at) and an absolute `url` per file

    python data/scripts/publish_demo_data.py            # validate + pin + verify, print the manifest it would write
    python data/scripts/publish_demo_data.py --write    # ... and write pack.json (then commit it; the Pages workflow re-verifies before deploy)
Nothing here needs a secret. If the verification fails the manifest is NOT written.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACK = ROOT / "data" / "public-demo"
CDN_FILE_LIMIT = 20 * 1024 * 1024  # jsDelivr does not serve larger GitHub files
REQUIRED = ("file", "path", "format", "crs", "rows", "bytes", "sha256", "license", "source_release")


def git(*a: str) -> str:
    return subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()


def repo_slug() -> str:
    url = git("remote", "get-url", "origin")
    m = re.search(r"github\.com[:/]+([^/]+/[^/.]+?)(?:\.git)?$", url)
    if not m:
        sys.exit(f"origin is not a GitHub repository: {url}")
    return m.group(1)


def validate(manifest: dict) -> list[str]:
    problems = []
    for sid, f in manifest["files"].items():
        miss = [k for k in REQUIRED if k not in f]
        if miss:
            problems.append(f"{sid}: manifest entry lacks {miss}")
            continue
        p = PACK / f["path"]
        if not p.exists():
            problems.append(f"{sid}: {p} does not exist")
            continue
        if p.stat().st_size != f["bytes"]:
            problems.append(f"{sid}: size {p.stat().st_size} != manifest {f['bytes']}")
        if p.stat().st_size > CDN_FILE_LIMIT:
            problems.append(f"{sid}: {p.stat().st_size} bytes exceeds the CDN per-file limit {CDN_FILE_LIMIT}")
        if hashlib.sha256(p.read_bytes()).hexdigest() != f["sha256"]:
            problems.append(f"{sid}: SHA-256 differs from the manifest")
        if p.read_bytes()[:4] != b"PAR1":
            problems.append(f"{sid}: not a Parquet file")
        if not f["path"].startswith(manifest["version"] + "/"):
            problems.append(f"{sid}: path {f['path']!r} is not under the version directory {manifest['version']}/ (versioned, immutable paths only)")
    for key in ("license", "attribution", "overture_release", "version"):
        if not manifest.get(key):
            problems.append(f"manifest lacks {key}")
    if manifest.get("attribution") and not (ROOT / "web" / "public" / manifest["attribution"]).exists():
        problems.append("the attribution page named by the manifest does not exist in web/public/")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--write", action="store_true", help="write pack.json when everything passed")
    ap.add_argument("--origin", default="https://shadowhunter67.github.io")
    # NOT cdn.jsdelivr.net: that name load-balances over several CDNs and one of them (Fastly, also fastly.jsdelivr.net) answers a ranged HEAD with
    # 200, which DuckDB-Wasm cannot use (measured 2026-10-22: a GitHub runner got that backend while a laptop got Cloudflare). gcore.jsdelivr.net is
    # one backend that answers 206 every time (12 of 12 requests); testingcf.jsdelivr.net (Cloudflare) did too but is documented as a test name.
    ap.add_argument("--cdn-host", default="gcore.jsdelivr.net", help="jsDelivr host to serve from (must be a single-backend alias that answers ranged HEAD with 206)")
    ap.add_argument("--wait", type=int, default=600, help="seconds to wait for a fresh commit to reach the CDN")
    args = ap.parse_args()

    manifest = json.loads((PACK / "pack.json").read_text(encoding="utf-8"))
    problems = validate(manifest)
    if problems:
        print("VALIDATION FAILED:\n  - " + "\n  - ".join(problems))
        return 1
    print(f"validated {len(manifest['files'])} files, {manifest['total_bytes']} bytes, version {manifest['version']}")

    commit = git("log", "-1", "--format=%H", "--", f"data/public-demo/{manifest['version']}")
    if not commit:
        print("the data files are not committed yet")
        return 1
    git("fetch", "-q", "origin", "main")
    if subprocess.run(["git", "merge-base", "--is-ancestor", commit, "origin/main"], cwd=ROOT).returncode != 0:
        print(f"commit {commit[:10]} is not on origin/main: push it first (a commit on a deleted branch may disappear from the CDN)")
        return 1
    slug = repo_slug()
    base = f"https://{args.cdn_host}/gh/{slug}@{commit}/data/public-demo/"
    print(f"pinned to commit {commit[:10]}  ->  {base}")

    report = ROOT / "data" / "public-demo" / ".verify-report.json"
    rc = subprocess.run([sys.executable, str(ROOT / "data" / "scripts" / "verify_range_host.py"), "--base-url", base, "--origin", args.origin, "--wait", str(args.wait), "--json", str(report)], cwd=ROOT).returncode
    report.unlink(missing_ok=True)
    if rc != 0:
        print("the host failed verification; pack.json NOT written")
        return rc

    manifest["host"] = {
        "kind": "jsdelivr-github",
        "cdn_host": args.cdn_host,
        "base_url": base,
        "commit": commit,
        "range": True,
        "verified_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "verified": "HEAD/GET Range -> 206 with correct Content-Range, no Content-Encoding, CORS for the app origin, Parquet magic, SHA-256 of the served bytes",
    }
    for f in manifest["files"].values():
        f["url"] = base + f["path"]
    text = json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"
    if args.write:
        (PACK / "pack.json").write_text(text, encoding="utf-8")
        print("pack.json written; commit it.")
    else:
        print("(dry run; --write to save)\n" + text[:1200])
    return 0


if __name__ == "__main__":
    sys.exit(main())
