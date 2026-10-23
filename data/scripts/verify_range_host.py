"""Verify, with real requests, that a host can serve the public demo data cloud-natively to a browser.

For every file of the manifest (data/public-demo/pack.json) it checks what DuckDB-Wasm needs, the way a browser would ask:
  * `HEAD` with `Range: bytes=0-` answers 206 with a correct `Content-Range` total (GitHub Pages answers 200, which breaks range reads)
  * `GET` with `Range` answers 206 with exactly the requested bytes and a correct `Content-Range`
  * no `Content-Encoding` (a transfer-compressed representation makes byte ranges refer to the compressed bytes)
  * CORS: `Access-Control-Allow-Origin` allows the app's origin, and a preflight carrying the headers a range read may add succeeds
  * the first and last bytes are the Parquet magic `PAR1`, and (unless --no-checksum) the SHA-256 of the served bytes equals the manifest
It never trusts provider documentation: a failed check is a failed host.

    python data/scripts/verify_range_host.py                      # uses host.base_url of data/public-demo/pack.json
    python data/scripts/verify_range_host.py --base-url https://cdn.jsdelivr.net/gh/<owner>/<repo>@<sha>/data/public-demo/ --wait 300
Exit code 0 = every check passed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
# what a browser sends: it accepts compression, which is exactly what must not be applied to a ranged file
BROWSER_HEADERS = {"Accept-Encoding": "gzip, deflate, br, zstd", "User-Agent": "Mozilla/5.0 GeoFabric-range-verifier"}


def request(url: str, method: str = "GET", headers: dict | None = None, limit: int | None = None):
    """Returns (status, headers (lower-cased), body). Redirects are NOT followed silently: a 3xx is reported as such."""

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):  # noqa: D401
            return None

    opener = urllib.request.build_opener(NoRedirect)
    req = urllib.request.Request(url, method=method, headers={**BROWSER_HEADERS, **(headers or {})})
    try:
        with opener.open(req, timeout=60) as r:
            body = r.read(limit) if limit is not None else r.read()
            return r.status, {k.lower(): v for k, v in r.headers.items()}, body
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, b""


def cors_ok(headers: dict, origin: str) -> bool:
    allow = headers.get("access-control-allow-origin", "")
    return allow == "*" or allow == origin


def check_file(sid: str, url: str, expected_bytes: int, expected_sha: str, origin: str, checksum: bool) -> list[str]:
    problems: list[str] = []
    o = {"Origin": origin}

    st, h, _ = request(url, "HEAD", {**o, "Range": "bytes=0-"})
    if st != 206:
        problems.append(f"HEAD with Range answered {st}, not 206 (DuckDB-Wasm cannot open the file for range reads)")
    else:
        total = h.get("content-range", "").rsplit("/", 1)[-1]
        if total != str(expected_bytes):
            problems.append(f"HEAD Content-Range total {total!r} != manifest size {expected_bytes}")
    if h.get("content-encoding"):
        problems.append(f"HEAD has Content-Encoding {h['content-encoding']!r}: byte ranges would refer to the compressed representation")
    if not cors_ok(h, origin):
        problems.append(f"HEAD: Access-Control-Allow-Origin {h.get('access-control-allow-origin')!r} does not allow {origin}")

    for rng, want in (("bytes=0-3", b"PAR1"), (f"bytes={expected_bytes - 4}-{expected_bytes - 1}", b"PAR1")):
        st, h, body = request(url, "GET", {**o, "Range": rng})
        if st != 206:
            problems.append(f"GET {rng} answered {st}, not 206")
        elif body != want:
            problems.append(f"GET {rng} returned {body!r}, expected the Parquet magic {want!r}")
        if h.get("content-encoding"):
            problems.append(f"GET {rng} has Content-Encoding {h['content-encoding']!r}")
        if st == 206 and not h.get("content-range", "").endswith(f"/{expected_bytes}"):
            problems.append(f"GET {rng} Content-Range {h.get('content-range')!r} does not end with /{expected_bytes}")

    st, h, _ = request(url, "OPTIONS", {**o, "Access-Control-Request-Method": "GET", "Access-Control-Request-Headers": "range,cache-control,if-match"})
    if not (200 <= st < 300) or not cors_ok(h, origin):
        problems.append(f"CORS preflight answered {st} with Access-Control-Allow-Origin {h.get('access-control-allow-origin')!r}")

    if checksum:
        st, h, body = request(url, "GET", {})
        if st != 200 or len(body) != expected_bytes:
            problems.append(f"full GET answered {st} with {len(body)} bytes (manifest: {expected_bytes})")
        elif hashlib.sha256(body).hexdigest() != expected_sha:
            problems.append("SHA-256 of the served bytes differs from the manifest")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", type=Path, default=ROOT / "data" / "public-demo" / "pack.json")
    ap.add_argument("--base-url", help="override host.base_url (must end with /)")
    ap.add_argument("--origin", default="https://shadowhunter67.github.io", help="the origin of the app that will read the files")
    ap.add_argument("--no-checksum", action="store_true", help="skip downloading every file in full to compare SHA-256")
    ap.add_argument("--wait", type=int, default=0, help="keep retrying for up to N seconds (a freshly pushed file may not be on the CDN yet)")
    ap.add_argument("--json", type=Path, help="write the report here")
    args = ap.parse_args()

    manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    base = args.base_url or (manifest.get("host") or {}).get("base_url")
    if not base:
        print("no host.base_url in the manifest and no --base-url given")
        return 2
    if not base.endswith("/"):
        base += "/"

    deadline = time.time() + args.wait
    while True:
        report = {}
        for sid, f in manifest["files"].items():
            try:
                report[sid] = check_file(sid, base + f["path"], f["bytes"], f["sha256"], args.origin, not args.no_checksum)
            except OSError as e:  # timeout / reset: the CDN may still be pulling a fresh file from GitHub; counts as a failed check and is retried under --wait
                report[sid] = [f"network error: {type(e).__name__}: {e}"]
        failed = {k: v for k, v in report.items() if v}
        if not failed or time.time() >= deadline:
            break
        print(f"not ready yet ({sum(len(v) for v in failed.values())} problems); retrying in 15 s")
        time.sleep(15)

    for sid, problems in report.items():
        print(("FAIL " if problems else "ok   ") + sid)
        for p in problems:
            print("       - " + p)
    if args.json:
        args.json.write_text(json.dumps({"base_url": base, "origin": args.origin, "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "problems": report}, indent=2) + "\n", encoding="utf-8")
    print("RESULT:", "every file passed" if not failed else f"{len(failed)} of {len(report)} files failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
