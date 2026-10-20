#!/usr/bin/env python3
"""Portable PostgreSQL + PostGIS for local development: no installer, no admin rights, no service, no registry, no PATH change.

  python tools/postgis_portable.py download     # fetch + verify the two pinned archives into tools/.cache/postgres/dl (~440 MB)
  python tools/postgis_portable.py setup        # extract, initdb, create database/roles/extension (idempotent)
  python tools/postgis_portable.py start        # start the server on 127.0.0.1:5433 and wait until it answers
  python tools/postgis_portable.py status
  python tools/postgis_portable.py stop         # stops ONLY the server this script started (by the PID in postmaster.pid)

*** NOT RUN IN THE PHASE THAT WROTE IT. *** The user declined the ~440 MB download, so this script (and everything that needs a
database) is unverified on this machine. Read it before running it; report anything that fails.

Pinned (never "latest"):
  PostgreSQL 16.9   EnterpriseDB binaries zip   (sha256 is recorded on the first download in dl/SHA256SUMS: trust-on-first-use, EDB
                                                 publishes no checksum next to the file)
  PostGIS    3.6.2  osgeo.org Windows bundle for PG16, md5 6730107131c29b7ff9a36702f736007b (published next to the file)

Security: the server listens on 127.0.0.1 only; two roles with LOCAL, NON-PRODUCTION development passwords that this script prints
(no secret is committed): `gf_admin` (owner; setup/import) and `gf_reader` (SELECT only; what the gateway uses). Never expose the port.
"""
import hashlib
import json
import os
import pathlib
import secrets
import shutil
import subprocess
import sys
import time
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
BASE = ROOT / "tools" / ".cache" / "postgres"
DL = BASE / "dl"
PGHOME = BASE / "pg16"
DATA = BASE / "data"
PORT = 5433
DB = "geofabric"
PG_URL = "https://get.enterprisedb.com/postgresql/postgresql-16.9-1-windows-x64-binaries.zip"
GIS_URL = "https://download.osgeo.org/postgis/windows/pg16/postgis-bundle-pg16-3.6.2x64.zip"
GIS_MD5 = "6730107131c29b7ff9a36702f736007b"


def fetch(url: str, dest: pathlib.Path):
    if dest.exists():
        return
    DL.mkdir(parents=True, exist_ok=True)
    print("downloading", url)
    tmp = dest.with_suffix(".part")
    with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f, 1 << 20)
    tmp.rename(dest)


def digest(path: pathlib.Path, algo: str) -> str:
    h = hashlib.new(algo)
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download():
    pg, gis = DL / "postgresql-16.9-1-windows-x64-binaries.zip", DL / "postgis-bundle-pg16-3.6.2x64.zip"
    fetch(PG_URL, pg)
    fetch(GIS_URL, gis)
    if digest(gis, "md5") != GIS_MD5:
        gis.unlink()
        sys.exit("PostGIS bundle md5 mismatch: deleted, nothing was extracted")
    sums = DL / "SHA256SUMS"
    known = dict(l.split("  ", 1)[::-1] for l in sums.read_text().splitlines()) if sums.exists() else {}
    for p in (pg, gis):
        d = digest(p, "sha256")
        if p.name in known and known[p.name] != d:
            sys.exit(f"{p.name}: sha256 changed since the first download - refusing to continue")
        known[p.name] = d
    sums.write_text("".join(f"{d}  {n}\n" for n, d in known.items()))
    print("ok: archives verified (PostGIS md5 matches the published one; sha256s recorded)")


def exe(name: str) -> str:
    return str(PGHOME / "pgsql" / "bin" / f"{name}.exe")


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, text=True, capture_output=True, **kw)


def psql(sql: str, db: str = "postgres", user: str = "gf_admin", password: str | None = None):
    env = dict(os.environ, PGPASSWORD=password or "")
    return run([exe("psql"), "-h", "127.0.0.1", "-p", str(PORT), "-U", user, "-d", db, "-v", "ON_ERROR_STOP=1", "-At", "-c", sql], env=env)


def setup():
    download()
    if not (PGHOME / "pgsql").exists():
        PGHOME.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(DL / "postgresql-16.9-1-windows-x64-binaries.zip") as z:
            z.extractall(PGHOME)
        # the PostGIS bundle is laid out as the PostgreSQL tree (bin, lib, share): copy it over the server tree
        tmp = BASE / "gis-extract"
        with zipfile.ZipFile(DL / "postgis-bundle-pg16-3.6.2x64.zip") as z:
            z.extractall(tmp)
        top = next(tmp.iterdir())
        shutil.copytree(top, PGHOME / "pgsql", dirs_exist_ok=True)
        shutil.rmtree(tmp)
    cred = BASE / "credentials.json"
    if not cred.exists():
        cred.write_text(json.dumps({"gf_admin": secrets.token_urlsafe(18), "gf_reader": secrets.token_urlsafe(18)}))
    creds = json.loads(cred.read_text())
    if not (DATA / "PG_VERSION").exists():
        pw = BASE / "pw.tmp"
        pw.write_text(creds["gf_admin"])
        # UTF8 + the C locale: byte-order text comparison, the same order as DuckDB (the SQL also says COLLATE "C" on sort keys)
        run([exe("initdb"), "-D", str(DATA), "-U", "gf_admin", "--pwfile", str(pw), "-E", "UTF8", "--locale=C", "--auth=scram-sha-256"])
        pw.unlink()
        with open(DATA / "postgresql.conf", "a") as f:
            f.write(f"\nlisten_addresses = '127.0.0.1'\nport = {PORT}\nshared_buffers = 512MB\nwork_mem = 64MB\nmax_connections = 40\n")
    start()
    pwa = creds["gf_admin"]
    if psql(f"SELECT 1 FROM pg_database WHERE datname='{DB}'", password=pwa).stdout.strip() != "1":
        psql(f"CREATE DATABASE {DB}", password=pwa)
    psql("CREATE EXTENSION IF NOT EXISTS postgis; CREATE SCHEMA IF NOT EXISTS geofabric", db=DB, password=pwa)
    if psql("SELECT 1 FROM pg_roles WHERE rolname='gf_reader'", password=pwa).stdout.strip() != "1":
        psql(f"CREATE ROLE gf_reader LOGIN PASSWORD '{creds['gf_reader']}'", password=pwa)
    psql(f"GRANT CONNECT ON DATABASE {DB} TO gf_reader; GRANT USAGE ON SCHEMA geofabric TO gf_reader; "
         "ALTER DEFAULT PRIVILEGES IN SCHEMA geofabric GRANT SELECT ON TABLES TO gf_reader", db=DB, password=pwa)
    print("PostgreSQL:", psql("SELECT version()", db=DB, password=pwa).stdout.strip())
    print("PostGIS   :", psql("SELECT postgis_full_version()", db=DB, password=pwa).stdout.strip())
    print(f"\nGateway (runtime, SELECT only):\n  GEOFABRIC_POSTGIS_ENABLED=1\n  GEOFABRIC_POSTGIS_URL=postgres://gf_reader:<see {cred}>@127.0.0.1:{PORT}/{DB}\n  GEOFABRIC_POSTGIS_SOURCES=data/manifests/postgis-sources.json")


def pid():
    f = DATA / "postmaster.pid"
    return int(f.read_text().splitlines()[0]) if f.exists() else None


def start():
    if pid():
        return
    subprocess.Popen([exe("pg_ctl"), "start", "-D", str(DATA), "-l", str(BASE / "server.log"), "-w", "-t", "60"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).wait()
    for _ in range(60):
        r = subprocess.run([exe("pg_isready"), "-h", "127.0.0.1", "-p", str(PORT)], capture_output=True)
        if r.returncode == 0:
            print(f"PostgreSQL is ready on 127.0.0.1:{PORT} (pid {pid()})")
            return
        time.sleep(1)
    sys.exit("PostgreSQL did not become ready in 60 s: see tools/.cache/postgres/server.log")


def stop():
    if not pid():
        print("not running")
        return
    run([exe("pg_ctl"), "stop", "-D", str(DATA), "-m", "fast", "-w"])  # pg_ctl signals the PID in postmaster.pid, nothing else
    print("stopped")


def status():
    p = pid()
    print(f"running (pid {p})" if p else "not running")


if __name__ == "__main__":
    cmds = {"download": download, "setup": setup, "start": start, "stop": stop, "status": status}
    if len(sys.argv) != 2 or sys.argv[1] not in cmds:
        sys.exit(__doc__)
    cmds[sys.argv[1]]()
