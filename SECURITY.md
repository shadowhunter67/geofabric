# Security

GeoFabric is **experimental** software. It has not been security-audited and is not hardened for hostile networks.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting for this repository: **Security → Report a vulnerability** (a private advisory visible only to the maintainers). Do not open a public issue for a vulnerability. There is no separate security email.

## Deployment assumptions

* **Static browser profile (the public demo).** There is no server code and no secret. The page is static files plus public GeoParquet. All queries run in the visitor's browser; nothing about them is sent anywhere. Network contacts are: the static host, the basemap host (OpenFreeMap: it can see which tiles are viewed) and `extensions.duckdb.org` (DuckDB-Wasm loads its `parquet` extension code). No analytics, cookies or tracking.
* **Local full stack.** The Gateway binds to `127.0.0.1`, has **no authentication**, and accepts only structured query templates or logical IR, never SQL. Source files must live under `--data`; remote URLs are refused; result rows, request size and concurrency are capped. Do **not** bind it to a LAN or public interface and do not put it behind a tunnel without adding your own authentication.
* **PostgreSQL/PostGIS (optional).** Never expose it directly. The development setup listens on `127.0.0.1` only and uses a bootstrap account (setup/import) and a `SELECT`-only runtime account. Connection strings contain credentials: they are never logged or committed (`Debug` redacts them). The driver has no TLS: local development only.
* Credentials, `.env` files, database dumps and runtime directories must never be committed.

## Scope

In scope: the Gateway (input validation, access guard, resource caps), the SQL lowering (injection), the static gateway shim, the build/deploy workflows. Out of scope: third-party services (basemap, extension CDN) and denial of service against a local development instance.
