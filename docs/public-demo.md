# Public demo data

The static demo (https://shadowhunter67.github.io/geofabric/) reads one window of central Ho Chi Minh City (Overture release 2026-09-23.1, five layers, 27.7 MB of plain GeoParquet) with HTTP range requests.

## Where things live

| What | Where |
|---|---|
| Source of truth: file URLs, sizes, SHA-256, licence, release, host | `data/public-demo/pack.json` (this repository) |
| Layer metadata the page needs (schema, row-group stats, IR `Source` node) | `data/public-demo/{sources,catalog}.json` (this repository, ~0.8 MB) |
| The data files | [`shadowhunter67/geofabric-demo-data`](https://github.com/shadowhunter67/geofabric-demo-data), directory per immutable version (`v1/`) |
| Build output (generated, never committed) | `data/public-demo-build/` (git-ignored; `tools/repo_guard.py` enforces) |

Why a separate repository: generated data in the code history grows it by ~27 MB per version for ever. The data repository has its own history and its own licence (**ODbL 1.0**, not the code licence), with `ATTRIBUTION.md`, `SHA256SUMS` and `MANIFEST.json` per version. Release assets were not used: they redirect and fail the CORS/range probe (ADR-014); the CDN over a Git repository passes it.

## Publishing a version (reproducible)

```
python data/scripts/make_public_demo.py --out data/public-demo-build     # 1 generate (+ IR source nodes)
python data/scripts/publish_demo_data.py --check                          # 2-3 validate + checksums, writes nothing outside the build dir
python data/scripts/publish_demo_data.py --publish                        # 2-6 + push the version to the data repository, verify, write the manifest
```

Validation checks every file's size, SHA-256, Parquet framing and licence/CRS/release metadata, and reads it back with the Rust GeoParquet reader (rows must equal the manifest). A version that already exists must be byte-identical (versions are immutable; changed data is `v2/`). The manifest is written only after `verify_range_host.py` passed on the commit-pinned URLs (HEAD/GET Range → 206, `Content-Range`, no `Content-Encoding`, CORS, Parquet magic, SHA-256 of the served bytes; a cold CDN edge may need a few minutes: `--wait`). Publish assets first, verify, then switch the manifest: URLs of earlier versions are never touched, so old deployments keep working (the v1 URLs pinned to this repository's commit 851b570 still answer 206).

## Using the data without the CDN

`python data/scripts/fetch_demo_data.py` downloads the manifest's files from GitHub at the pinned commit into `data/public-demo-build/<version>/` and verifies SHA-256; `GF_DEMO=bundle node web/scripts/copy-demo.mjs` then bundles them into a self-contained site (CI static tests do exactly this, so they do not depend on the CDN).

## Licence and attribution

The files are a derived database of ODbL (OpenStreetMap-derived) data: offered under the ODbL with the attributions in `docs/DATA-LICENSES.md`, `attribution.html` (published with the site) and the data repository's `ATTRIBUTION.md`. The places layer keeps per-record source licences. Not legal advice; see the notes in DATA-LICENSES.md.
