#!/usr/bin/env python3
"""Fetch a regional subset of Overture Maps buildings as a local GeoParquet file.

Reads the public release over S3 (httpfs; only the needed byte ranges are
transferred), filters by a lon/lat window on Overture's own `bbox` column and
writes ONE local file, preserving Overture's row order (the release is already
spatially sorted, which is what makes row-group pruning possible).

  python overture_fetch.py --name vn_south --bbox 105.0 8.5 108.0 12.0 \
      --release 2026-09-23.1 --out data --max-gb 5

Safety: aborts (and deletes the partial file) if the output exceeds --max-gb.
The downloaded file is data, never code. Not run by tests.
"""
import argparse
import json
import pathlib
import threading
import time

import duckdb


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--bbox", type=float, nargs=4, required=True, metavar=("XMIN", "YMIN", "XMAX", "YMAX"))
    ap.add_argument("--release", default="2026-09-23.1")
    ap.add_argument("--theme", default="buildings")
    ap.add_argument("--type", default="building")
    ap.add_argument("--out", default="data")
    ap.add_argument("--max-gb", type=float, default=5.0)
    ap.add_argument("--threads", type=int, default=4)
    a = ap.parse_args()
    x0, y0, x1, y1 = a.bbox
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    final = out / f"overture_{a.theme}_{a.name}.parquet"
    tmp = out / (final.name + ".part")
    src = f"s3://overturemaps-us-west-2/release/{a.release}/theme={a.theme}/type={a.type}/*"

    c = duckdb.connect()
    c.execute(f"INSTALL httpfs; LOAD httpfs; INSTALL spatial; LOAD spatial; SET s3_region='us-west-2'; SET threads={a.threads};")
    stop = threading.Event()

    def watchdog():
        while not stop.is_set():
            time.sleep(5)
            if tmp.exists() and tmp.stat().st_size > a.max_gb * 1e9:
                print(f"!! output exceeds {a.max_gb} GB: interrupting", flush=True)
                c.interrupt()
                return
            if tmp.exists():
                print(f"   .. {tmp.stat().st_size/1e6:.0f} MB written", flush=True)

    threading.Thread(target=watchdog, daemon=True).start()
    t0 = time.time()
    sql = f"""
        COPY (
          SELECT * EXCLUDE (theme, type)
          FROM read_parquet('{src}', hive_partitioning=true)
          WHERE bbox.xmin <= {x1} AND bbox.xmax >= {x0} AND bbox.ymin <= {y1} AND bbox.ymax >= {y0}
        ) TO '{tmp.as_posix()}' (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 100000)
    """
    try:
        c.execute(sql)
    except Exception as e:  # interrupted or failed: do not leave a partial file behind
        stop.set()
        if tmp.exists():
            tmp.unlink()
        raise SystemExit(f"fetch failed: {e}")
    stop.set()
    tmp.replace(final)
    meta = {
        "name": a.name, "source": src, "release": a.release, "bbox": a.bbox, "crs": "OGC:CRS84",
        "file": final.name, "size_bytes": final.stat().st_size, "seconds": round(time.time() - t0, 1),
        "license_note": "Overture Maps Foundation data; see https://docs.overturemaps.org/attribution/ (ODbL for buildings sources vary per record)",
    }
    (out / (final.stem + ".fetch.json")).write_text(json.dumps(meta, indent=2))
    print(json.dumps(meta, indent=2))


if __name__ == "__main__":
    main()
