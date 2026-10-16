"""Download Copernicus DEM GLO-30 COG tiles covering an explicit lon/lat bbox."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from urllib.request import Request, urlopen

CHUNK_SIZE = 1024 * 1024
BASE_URL = "https://copernicus-dem-30m.s3.amazonaws.com"


def tiles_for_bbox(bbox: list[float]) -> list[tuple[int, int]]:
    """Return SW-corner tiles because Copernicus names each one-degree COG that way."""

    x0, y0, x1, y1 = bbox
    if not -180 <= x0 < x1 <= 180 or not -90 <= y0 < y1 <= 90:
        raise ValueError("bbox must be ordered longitude/latitude coordinates")

    lons = range(math.floor(x0), math.ceil(x1))
    lats = range(math.floor(y0), math.ceil(y1))
    return [(lat, lon) for lat in lats for lon in lons]


def tile_name(lat: int, lon: int) -> str:
    """Format the AWS tile key from its signed southwest integer coordinates."""

    north_south = "N" if lat >= 0 else "S"
    east_west = "E" if lon >= 0 else "W"
    return (
        f"Copernicus_DSM_COG_10_{north_south}{abs(lat):02d}_00_"
        f"{east_west}{abs(lon):03d}_00_DEM"
    )


def tile_url(lat: int, lon: int) -> str:
    """Construct the public immutable COG URL without credentials."""

    name = tile_name(lat, lon)
    return f"{BASE_URL}/{name}/{name}.tif"


def content_length(url: str) -> int:
    """Read object length first because the total download limit must be known up front."""

    request = Request(url, method="HEAD")
    with urlopen(request) as response:
        value = response.headers.get("Content-Length")
    if value is None:
        raise ValueError(f"missing Content-Length for {url}")
    return int(value)


def sha256_file(path: Path) -> str:
    """Hash incrementally because downloaded COGs may be large."""

    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(CHUNK_SIZE), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, destination: Path, expected_size: int) -> str:
    """Download atomically and reuse a finished matching file for resume safety."""

    if destination.is_file() and destination.stat().st_size == expected_size:
        return sha256_file(destination)

    temporary = Path(f"{destination}.part")
    if temporary.exists():
        temporary.unlink()

    digest = hashlib.sha256()
    with urlopen(url) as response, temporary.open("wb") as output:
        while chunk := response.read(CHUNK_SIZE):
            output.write(chunk)
            digest.update(chunk)

    if temporary.stat().st_size != expected_size:
        temporary.unlink()
        raise ValueError(f"incomplete download for {destination.name}")

    os.replace(temporary, destination)
    return digest.hexdigest()


def main() -> int:
    """Acquire all required tiles after validating the total size safety limit."""

    parser = argparse.ArgumentParser()
    parser.add_argument("--bbox", type=float, nargs=4, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--max-gb", type=float, default=1.0)
    args = parser.parse_args()

    if args.max_gb <= 0:
        raise ValueError("--max-gb must be positive")

    tiles = tiles_for_bbox(args.bbox)
    lengths = {(lat, lon): content_length(tile_url(lat, lon)) for lat, lon in tiles}
    total = sum(lengths.values())
    if total > args.max_gb * 1_000_000_000:
        raise ValueError(f"requested tiles require {total} bytes, above --max-gb")

    args.out_dir.mkdir(parents=True, exist_ok=True)
    index = []
    for lat, lon in tiles:
        name = tile_name(lat, lon)
        destination = args.out_dir / f"{name}.tif"
        size = lengths[(lat, lon)]
        digest = download(tile_url(lat, lon), destination, size)
        item = {
            "bbox": [lon, lat, lon + 1, lat + 1],
            "path": destination.as_posix(),
            "sha256": digest,
            "size_bytes": size,
            "tile": [lat, lon],
        }
        index.append(item)
        print(json.dumps(item))

    (args.out_dir / "index.json").write_text(
        json.dumps(index, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
