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
# Downloads stay in the repo cache (gitignored). The SERVER lives in an ASCII-only directory OUTSIDE the repo: PostgreSQL cannot start
# from a path with Vietnamese diacritics (the workspace root is "Lap trinh" with diacritics; same family of bug as Tesseract, see the
# workspace CLAUDE.md). Default: %LOCALAPPDATA%/geofabric-runtime/postgres (no admin rights, no service); override with GEOFABRIC_RUNTIME_DIR.
DL = ROOT / "tools" / ".cache" / "postgres" / "dl"
BASE = pathlib.Path(os.environ.get("GEOFABRIC_RUNTIME_DIR") or (pathlib.Path(os.environ["LOCALAPPDATA"]) / "geofabric-runtime" / "postgres"))
PGHOME = BASE / "pg16"
DATA = BASE / "data"
PORT = 5433
DB = "geofabric"
PG_URL = "https://get.enterprisedb.com/postgresql/postgresql-16.9-1-windows-x64-binaries.zip"
GIS_URL = "https://download.osgeo.org/postgis/windows/pg16/postgis-bundle-pg16-3.6.2x64.zip"
GIS_MD5 = "6730107131c29b7ff9a36702f736007b"


LOCK = ROOT / "tools" / "postgis.lock.json"
# exact sizes observed on 2026-10-20 (HEAD request); a different size means a different file: stop
PINNED = {
    "postgresql-16.9-1-windows-x64-binaries.zip": {"url": PG_URL, "size": 314500604},
    "postgis-bundle-pg16-3.6.2x64.zip": {"url": GIS_URL, "size": 123957610, "md5": GIS_MD5},
}


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        raise SystemExit("STOP: the download URL redirected; the pinned URLs are expected to answer directly. Investigate before continuing.")


def fetch(url: str, dest: pathlib.Path):
    if dest.exists():
        return
    DL.mkdir(parents=True, exist_ok=True)
    print("downloading", url)
    opener = urllib.request.build_opener(_NoRedirect())
    tmp = dest.with_suffix(".part")
    with opener.open(url) as r:
        print("  status", r.status, "length", r.headers.get("Content-Length"), "server", r.headers.get("Server"))
        with open(tmp, "wb") as f:
            shutil.copyfileobj(r, f, 1 << 20)
    tmp.rename(dest)


def digest(path: pathlib.Path, algo: str) -> str:
    h = hashlib.new(algo)
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download():
    lock = json.loads(LOCK.read_text()) if LOCK.exists() else {}
    for name, pin in PINNED.items():
        path = DL / name
        fetch(pin["url"], path)
        if path.stat().st_size != pin["size"]:
            path.unlink()
            sys.exit(f"STOP: {name} has {path.stat().st_size} bytes, expected {pin['size']}: deleted, nothing extracted")
        if "md5" in pin and digest(path, "md5") != pin["md5"]:
            path.unlink()
            sys.exit(f"STOP: {name} md5 does not match the vendor-published value: deleted")
        sha = digest(path, "sha256")
        if name in lock and lock[name]["sha256"] != sha:
            sys.exit(f"STOP: {name} sha256 differs from the locked value in {LOCK.name}")
        lock.setdefault(name, {"url": pin["url"], "size": pin["size"], "sha256": sha,
                               "integrity": "vendor md5 verified" if "md5" in pin else "trusted on first acquisition over HTTPS; locally pinned SHA-256"})
        print(f"  {name}: {lock[name]['integrity']}; sha256 {sha}")
    LOCK.write_text(json.dumps(lock, indent=1))
    print("ok: archives verified; lock manifest:", LOCK)


def exe(name: str) -> str:
    return str(PGHOME / "pgsql" / "bin" / f"{name}.exe")


def run(cmd, **kw):
    r = subprocess.run(cmd, text=True, capture_output=True, **kw)
    if r.returncode != 0:
        sys.exit("STOP: " + pathlib.Path(cmd[0]).name + " failed (" + str(r.returncode) + ")" + chr(10) + r.stdout[-1500:] + chr(10) + r.stderr[-2500:])
    return r


def psql(sql: str, db: str = "postgres", user: str = "gf_admin", password: str | None = None):
    env = dict(os.environ, PGPASSWORD=password or "")
    return run([exe("psql"), "-h", "127.0.0.1", "-p", str(PORT), "-U", user, "-d", db, "-v", "ON_ERROR_STOP=1", "-At", "-c", sql], env=env)


def setup():
    download()
    if not (PGHOME / "pgsql").exists():
        PGHOME.mkdir(parents=True, exist_ok=True)
        skip = ("pgsql/pgAdmin 4/", "pgsql/doc/", "pgsql/StackBuilder/", "pgsql/include/")  # not needed to run a server (~18,000 files)
        with zipfile.ZipFile(DL / "postgresql-16.9-1-windows-x64-binaries.zip") as z:
            z.extractall(PGHOME, members=[m for m in z.namelist() if not m.startswith(skip)])
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
