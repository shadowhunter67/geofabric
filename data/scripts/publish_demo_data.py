"""Public demo data pipeline: generate -> validate -> checksums -> publish an immutable version -> verify range/CORS -> write the manifest.

The data does NOT live in this repository's history. Each data version is a directory (`v1/`, `v2/`...) of the dedicated, ODbL-licensed data
repository (default `shadowhunter67/geofabric-demo-data`), served by jsDelivr at commit-pinned URLs: immutable, CORS, true HTTP range reads, no account,
no credentials, no server of ours. This repository keeps only the manifest (`data/public-demo/pack.json`, the source of truth) and the small metadata.

  1. generate   (separate step) `python data/scripts/make_public_demo.py --out data/public-demo-build` writes <build>/<version>/*.parquet.zip,
                pack.json, sources.json, catalog.json; then `python data/scripts/ir_sources.py data/public-demo-build` adds the IR Source nodes.
                The build directory is git-ignored: generated data never enters this repository.
  2. validate   every file named by the manifest exists in <build>/<version>/, size + SHA-256 match, it is Parquet, the Rust GeoParquet reader reads it
                (schema, CRS, row count equal to the manifest), licence/format/CRS/release metadata are present, each file is under the CDN limit
  3. checksums  SHA256SUMS + MANIFEST.json for the version are written next to the files
  4. publish    the version directory is added to a clone of the data repository as ONE commit (+ tag `<version>`) and pushed. A version that already
                exists must be byte-identical (versions are immutable; changed data is a new version)
  5. verify     data/scripts/verify_range_host.py against the commit-pinned URLs: HEAD/GET Range -> 206, Content-Range, no Content-Encoding,
                CORS for the app origin, Parquet magic, SHA-256 of the served bytes (waits for a fresh commit to reach the CDN)
  6. manifest   only after 5 passed: data/public-demo/{pack.json,sources.json,catalog.json} are replaced by the build's, with `host` and an absolute
                `url` per file. Old versions' URLs are never touched (they stay valid).

    python data/scripts/publish_demo_data.py --check                  # steps 2-3 only, writes nothing outside the build directory
    python data/scripts/publish_demo_data.py --publish                # 2-6: needs push access to the data repository (your gh login)
    python data/scripts/publish_demo_data.py --publish --dry-run      # 2-4 into a local clone, nothing pushed, nothing verified
Nothing here needs a secret beyond your own git credentials.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACK = ROOT / "data" / "public-demo"
BUILD = ROOT / "data" / "public-demo-build"
DATA_REPO = "shadowhunter67/geofabric-demo-data"
CDN_FILE_LIMIT = 20 * 1024 * 1024  # jsDelivr does not serve larger GitHub files
REQUIRED = ("file", "path", "format", "crs", "rows", "bytes", "sha256", "license", "source_release")
sys.path.insert(0, str(Path(__file__).resolve().parent))


def run(*a: str, cwd: Path = ROOT) -> str:
    return subprocess.run(list(a), cwd=cwd, capture_output=True, text=True, encoding="utf-8", check=True).stdout.strip()


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def validate(manifest: dict, build: Path) -> list[str]:
    problems: list[str] = []
    version = manifest["version"]
    for key in ("license", "attribution", "overture_release", "version"):
        if not manifest.get(key):
            problems.append(f"manifest lacks {key}")
    if "ODbL" not in str(manifest.get("license", "")):
        problems.append("the pack licence must be the ODbL (it is a derived database of ODbL data); the code licence does not apply to data")
    if manifest.get("attribution") and not (ROOT / "web" / "public" / manifest["attribution"]).exists():
        problems.append("the attribution page named by the manifest does not exist in web/public/")
    files: dict[str, Path] = {}
    for sid, f in manifest["files"].items():
        miss = [k for k in REQUIRED if k not in f]
        if miss:
            problems.append(f"{sid}: manifest entry lacks {miss}")
            continue
        p = build / f["path"]
        if not f["path"].startswith(version + "/"):
            problems.append(f"{sid}: path {f['path']!r} is not under the version directory {version}/ (versioned, immutable paths only)")
        if not p.exists():
            problems.append(f"{sid}: {p} does not exist")
            continue
        size = p.stat().st_size
        if size != f["bytes"]:
            problems.append(f"{sid}: size {size} != manifest {f['bytes']}")
        if size > CDN_FILE_LIMIT:
            problems.append(f"{sid}: {size} bytes exceeds the CDN per-file limit {CDN_FILE_LIMIT}")
        if sha256_file(p) != f["sha256"]:
            problems.append(f"{sid}: SHA-256 differs from the manifest")
        with p.open("rb") as fh:
            head = fh.read(4)
            fh.seek(-4, 2)
            tail = fh.read(4)
        if head != b"PAR1" or tail != b"PAR1":
            problems.append(f"{sid}: not a complete Parquet file")
        files[sid] = p
    if problems:
        return problems
    # schema / rows / CRS: read by the same Rust reader the engines use, not re-derived here
    import ir_sources

    for sid, node in ir_sources.ir_source_nodes(files).items():
        f = manifest["files"][sid]
        if node["statistics"]["row_count"] != f["rows"]:
            problems.append(f"{sid}: the reader counts {node['statistics']['row_count']} rows, the manifest says {f['rows']}")
        if not node["schema"]["fields"]:
            problems.append(f"{sid}: empty schema")
    return problems


def checksums(manifest: dict, build: Path) -> None:
    vdir = build / manifest["version"]
    lines = [f"{f['sha256']}  {Path(f['path']).name}" for f in manifest["files"].values()]
    (vdir / "SHA256SUMS").write_text("\n".join(sorted(lines, key=lambda l: l.split("  ")[1])) + "\n", encoding="utf-8", newline="\n")
    (vdir / "MANIFEST.json").write_text(json.dumps({k: v for k, v in manifest.items() if k != "host"}, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


def publish(manifest: dict, build: Path, repo: str, dry: bool) -> str:
    """Add build/<version>/ to the data repository as one commit, tag it, push. Returns the commit sha."""
    version = manifest["version"]
    clone = BUILD / "_data-repo"
    if clone.exists():
        run("git", "-C", str(clone), "fetch", "-q", "origin", "main")
        run("git", "-C", str(clone), "reset", "-q", "--hard", "origin/main")
    else:
        run("gh", "repo", "clone", repo, str(clone), "--", "-q")
    target = clone / version
    src = build / version
    if target.exists():
        same = sorted(p.name for p in src.iterdir()) == sorted(p.name for p in target.iterdir()) and all(sha256_file(p) == sha256_file(target / p.name) for p in src.iterdir())
        if not same:
            sys.exit(f"{version}/ already exists in {repo} with different content: versions are immutable, publish the change as a new version")
        print(f"{version}/ is already published and identical; nothing to commit")
        return run("git", "-C", str(clone), "log", "-1", "--format=%H", "--", version)
    shutil.copytree(src, target)
    run("git", "-C", str(clone), "add", version)
    msg = f"Data {version}: Overture {manifest['overture_release']}, {len(manifest['files'])} files, {manifest['total_bytes']} bytes (ODbL-1.0)"
    run("git", "-C", str(clone), "commit", "-q", "-m", msg)
    run("git", "-C", str(clone), "tag", version)
    commit = run("git", "-C", str(clone), "rev-parse", "HEAD")
    if dry:
        print(f"dry run: committed {commit[:10]} in {clone}, not pushed")
        return commit
    run("git", "-C", str(clone), "push", "-q", "origin", "main", version)
    return commit


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", type=Path, default=BUILD, help="build directory holding <version>/, pack.json, sources.json, catalog.json")
    ap.add_argument("--check", action="store_true", help="validate + checksums only")
    ap.add_argument("--publish", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--repo", default=DATA_REPO)
    ap.add_argument("--origin", default="https://shadowhunter67.github.io")
    # NOT cdn.jsdelivr.net: that name load-balances over several CDNs and one of them (Fastly) answers a ranged HEAD with 200, which DuckDB-Wasm
    # cannot use (measured 2026-10-22). gcore.jsdelivr.net is one backend that answers 206 every time (12 of 12 requests).
    ap.add_argument("--cdn-host", default="gcore.jsdelivr.net")
    ap.add_argument("--wait", type=int, default=600, help="seconds to wait for a fresh commit to reach the CDN")
    args = ap.parse_args()
    if not (args.check or args.publish):
        ap.error("choose --check or --publish")

    build = args.build
    manifest = json.loads((build / "pack.json").read_text(encoding="utf-8"))
    problems = validate(manifest, build)
    if problems:
        print("VALIDATION FAILED:\n  - " + "\n  - ".join(problems))
        return 1
    checksums(manifest, build)
    print(f"validated {len(manifest['files'])} files, {manifest['total_bytes']} bytes, version {manifest['version']}; SHA256SUMS + MANIFEST.json written")
    if args.check:
        return 0

    commit = publish(manifest, build, args.repo, args.dry_run)
    if args.dry_run:
        return 0
    base = f"https://{args.cdn_host}/gh/{args.repo}@{commit}/"
    print(f"pinned to {args.repo}@{commit[:10]}  ->  {base}")
    probe = build / ".verify-manifest.json"
    probe.write_text(json.dumps({**manifest, "host": {"base_url": base}}), encoding="utf-8")
    rc = subprocess.run([sys.executable, str(ROOT / "data" / "scripts" / "verify_range_host.py"), "--manifest", str(probe), "--base-url", base, "--origin", args.origin, "--wait", str(args.wait)], cwd=ROOT).returncode
    probe.unlink(missing_ok=True)
    if rc != 0:
        print("the host failed verification; the manifest is NOT written")
        return rc

    manifest["host"] = {
        "kind": "jsdelivr-github",
        "cdn_host": args.cdn_host,
        "repository": args.repo,
        "base_url": base,
        "commit": commit,
        "range": True,
        "verified_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "verified": "HEAD/GET Range -> 206 with correct Content-Range, no Content-Encoding, CORS for the app origin, Parquet magic, SHA-256 of the served bytes",
    }
    for f in manifest["files"].values():
        f["url"] = base + f["path"]
    for name in ("sources.json", "catalog.json"):
        if (build / name).exists():
            shutil.copyfile(build / name, PACK / name)
    (PACK / "pack.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print("data/public-demo/pack.json written (the source of truth); commit it. The Pages workflow re-verifies the host before deploy.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
