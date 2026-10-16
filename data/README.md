# Vietnam Demo Data Pack

This directory holds reproducible dataset metadata and generated quality measurements.

- `manifests/` contains one committed JSON manifest per dataset.
- `scripts/` contains the offline manifest CLI and tests.
- `samples/` may contain committed fixtures smaller than 50 KB.
- `quality/` contains committed, read-only quality reports.
- `local/` contains downloaded or normalized local data and is ignored.
- `.cache/` contains transient local state and is ignored.

From the repository root, validate manifests with:

