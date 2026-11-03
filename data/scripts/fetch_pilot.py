"""Fetch the authorized Overture pilot into restart-safe GeoParquet files."""

from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import threading
import time
from typing import Any

import duckdb
import pyarrow.parquet as pq

from fetch_overture import (
    PRESETS,
    build_sql,
    sql_string,
    validate_bbox,
    verify_geoparquet,
)
from overture_estimate import DOWNLOAD_GATE_BYTES, NATIONAL_ADMIN_BBOX


DEFAULT_REGIONS = Path("data/pilot/regions.json")
DEFAULT_ESTIMATE = Path("data/quality/pilot_estimate.json")
DEFAULT_OUTPUT = Path("data/pilot/raw")
DEFAULT_THEMES = ("buildings", "roads", "places", "landuse", "admin")
THEME_PRESETS = {
    "buildings": "buildings",
    "roads": "transport",
    "places": "places",
    "landuse": "landuse",
    "admin": "admin",
}

SqlBuilder = Callable[[str, list[float], str, Path], str]


@dataclass(frozen=True)
class Job:
    """One independently resumable pilot output."""

    theme: str
    region: str
    preset: str
    bbox: tuple[float, float, float, float]
    output: Path

    @property
    def temporary(self) -> Path:
        return Path(f"{self.output}.part")

    @property
    def sidecar(self) -> Path:
        return Path(f"{self.output}.json")


def check_gate(estimate: Path | Mapping[str, Any]) -> dict[str, Any]:
    """Load and enforce the owner-approved source-download authorization gate."""

    if isinstance(estimate, Path):
        if not estimate.is_file():
            raise ValueError(f"pilot estimate is missing: {estimate}")
        try:
            document = json.loads(estimate.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(f"pilot estimate is unreadable: {estimate}") from error
    else:
        document = dict(estimate)

    if document.get("partial") is True:
        raise ValueError("pilot estimate is partial; download is not authorized")
    estimated_bytes = document.get("estimated_pilot_download_bytes")
    if isinstance(estimated_bytes, bool) or not isinstance(estimated_bytes, int):
        raise ValueError("pilot estimate has no integer estimated_pilot_download_bytes")
    if estimated_bytes > DOWNLOAD_GATE_BYTES:
        raise ValueError(
            f"pilot estimate {estimated_bytes} exceeds download gate {DOWNLOAD_GATE_BYTES}"
        )
    return document


def plan_jobs(
    regions: Mapping[str, Sequence[float]], themes: Sequence[str], out: Path
) -> list[Job]:
    """Return the deterministic regional plan, with admin represented nationally once."""

    unknown = [theme for theme in themes if theme not in THEME_PRESETS]
    if unknown:
        raise ValueError(f"unknown theme: {unknown[0]}")
    if len(set(themes)) != len(themes):
        raise ValueError("themes must not contain duplicates")

    jobs: list[Job] = []
    for theme in themes:
        preset = THEME_PRESETS[theme]
        if theme == "admin":
            jobs.append(
                Job(
                    theme=theme,
                    region="vn",
                    preset=preset,
                    bbox=tuple(NATIONAL_ADMIN_BBOX),
                    output=out / theme / "vn.parquet",
                )
            )
            continue
        for region, raw_bbox in regions.items():
            bbox = validate_bbox([float(value) for value in raw_bbox])
            jobs.append(
                Job(
                    theme=theme,
                    region=region,
                    preset=preset,
                    bbox=tuple(bbox),
                    output=out / theme / f"{region}.parquet",
                )
            )
    return jobs


def sha256_file(path: Path) -> str:
    """Hash a file without loading pilot-sized outputs into memory."""

    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def should_skip(output: Path, sidecar: Path | None = None) -> bool:
    """Return whether an output and its seal form a checksum-matching pair."""

    sidecar = sidecar or Path(f"{output}.json")
    if not output.is_file() or not sidecar.is_file():
        return False
    try:
        metadata = json.loads(sidecar.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    expected = metadata.get("sha256")
    return isinstance(expected, str) and expected == sha256_file(output)


def sidecar_content(
    job: Job,
    release: str,
    *,
    rows: int,
    byte_count: int,
    sha256: str,
    started: str,
    finished: str,
    seconds: float,
) -> dict[str, Any]:
    """Build the stable completion seal without filesystem side effects."""

    return {
        "bbox": list(job.bbox),
        "bytes": byte_count,
        "finished": finished,
        "preset": job.preset,
        "release": release,
        "rows": rows,
        "seconds": round(seconds, 3),
        "sha256": sha256,
        "started": started,
    }


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _write_json_atomic(path: Path, value: Mapping[str, Any]) -> None:
    temporary = Path(f"{path}.part")
    if temporary.exists():
        temporary.unlink()
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def run_job(
    job: Job,
    release: str,
    *,
    max_gb: float = 1.5,
    threads: int = 4,
    memory_limit: str = "4GB",
    sql_builder: SqlBuilder = build_sql,
) -> dict[str, Any]:
    """Write, verify, publish, and seal one job; return its result and skip state."""

    if max_gb <= 0 or threads <= 0:
        raise ValueError("max_gb and threads must be positive")
    if not isinstance(memory_limit, str) or not memory_limit.strip():
        raise ValueError("memory_limit must be a non-empty DuckDB memory size")

    job.output.parent.mkdir(parents=True, exist_ok=True)
    if job.temporary.exists():
        job.temporary.unlink()
    sidecar_temporary = Path(f"{job.sidecar}.part")
    if sidecar_temporary.exists():
        sidecar_temporary.unlink()
    if should_skip(job.output, job.sidecar):
        result = json.loads(job.sidecar.read_text(encoding="utf-8"))
        result["skipped"] = True
        return result

    temp_directory = job.output.parents[1] / ".tmp"
    temp_directory.mkdir(parents=True, exist_ok=True)
    connection = duckdb.connect()
    stopped = threading.Event()
    exceeded = threading.Event()
    started_monotonic = time.monotonic()
    started = _utc_now()

    try:
        connection.execute(f"SET memory_limit = {sql_string(memory_limit)}")
        connection.execute(f"SET temp_directory = {sql_string(temp_directory.as_posix())}")
        connection.execute(f"SET threads = {threads}")
        connection.execute("LOAD spatial")
        if sql_builder is build_sql:
            connection.execute("LOAD httpfs")
            connection.execute(f"SET s3_region = {sql_string('us-west-2')}")

        def watchdog() -> None:
            while not stopped.wait(1):
                if (
                    job.temporary.exists()
                    and job.temporary.stat().st_size > max_gb * 1_000_000_000
                ):
                    exceeded.set()
                    connection.interrupt()
                    return

        watcher = threading.Thread(target=watchdog, daemon=True)
        watcher.start()
        try:
            sql = sql_builder(job.preset, list(job.bbox), release, job.temporary)
            connection.execute(sql)
            if exceeded.is_set() or (
                job.temporary.exists()
                and job.temporary.stat().st_size > max_gb * 1_000_000_000
            ):
                raise ValueError(f"output exceeds {max_gb} GB")
            verify_geoparquet(job.temporary)
            os.replace(job.temporary, job.output)
        except Exception as error:
            if job.temporary.exists():
                job.temporary.unlink()
            if exceeded.is_set() and not isinstance(error, ValueError):
                raise ValueError(f"output exceeds {max_gb} GB") from error
            raise
        finally:
            stopped.set()
            watcher.join()
    finally:
        stopped.set()
        connection.close()

    finished = _utc_now()
    elapsed = time.monotonic() - started_monotonic
    parquet = pq.ParquetFile(job.output)
    content = sidecar_content(
        job,
        release,
        rows=parquet.metadata.num_rows,
        byte_count=job.output.stat().st_size,
        sha256=sha256_file(job.output),
        started=started,
        finished=finished,
        seconds=elapsed,
    )
    _write_json_atomic(job.sidecar, content)
    result = dict(content)
    result["skipped"] = False
    return result


def _csv(value: str) -> list[str]:
    values = [item.strip() for item in value.split(",") if item.strip()]
    if not values:
        raise ValueError("comma-separated selection must not be empty")
    if len(set(values)) != len(values):
        raise ValueError("comma-separated selection must not contain duplicates")
    return values


def _load_regions(path: Path) -> tuple[str, dict[str, Sequence[float]]]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"regions file is unreadable: {path}") from error
    release = document.get("overture_release")
    regions = document.get("regions")
    if not isinstance(release, str) or not release or "latest" in release.casefold():
        raise ValueError("regions file must pin an Overture release")
    if not isinstance(regions, dict) or not regions:
        raise ValueError("regions file has no regions")
    return release, regions


def _select_regions(
    regions: Mapping[str, Sequence[float]], selected: Sequence[str] | None
) -> dict[str, Sequence[float]]:
    if selected is None:
        return dict(regions)
    unknown = [name for name in selected if name not in regions]
    if unknown:
        raise ValueError(f"unknown region: {unknown[0]}")
    return {name: regions[name] for name in selected}


def main(argv: Sequence[str] | None = None) -> int:
    """Apply the gate, print or execute the plan, then write a sorted audit report."""

    parser = argparse.ArgumentParser()
    parser.add_argument("--regions", type=Path, default=DEFAULT_REGIONS)
    parser.add_argument("--estimate", type=Path, default=DEFAULT_ESTIMATE)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--themes", default=",".join(DEFAULT_THEMES))
    parser.add_argument("--only-regions")
    parser.add_argument("--max-gb", type=float, default=1.5)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--memory-limit", default="4GB")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    estimate = check_gate(args.estimate)
    release, all_regions = _load_regions(args.regions)
    estimate_release = estimate.get("release")
    if estimate_release is not None and estimate_release != release:
        raise ValueError("estimate and regions pin different Overture releases")
    themes = _csv(args.themes)
    selected_names = _csv(args.only_regions) if args.only_regions is not None else None
    regions = _select_regions(all_regions, selected_names)
    jobs = plan_jobs(regions, themes, args.out)

    if args.dry_run:
        for job in jobs:
            print(
                json.dumps(
                    {
                        "bbox": list(job.bbox),
                        "output": job.output.as_posix(),
                        "region": job.region,
                        "skipped": should_skip(job.output, job.sidecar),
                        "theme": job.theme,
                    },
                    ensure_ascii=False,
                    sort_keys=True,
                )
            )
        return 0

    if args.max_gb <= 0 or args.threads <= 0:
        raise ValueError("--max-gb and --threads must be positive")
    wall_started = time.monotonic()
    theme_report: dict[str, dict[str, dict[str, int]]] = {}
    generated_paths: list[str] = []
    total_rows = 0
    total_bytes = 0
    for job in jobs:
        result = run_job(
            job,
            release,
            max_gb=args.max_gb,
            threads=args.threads,
            memory_limit=args.memory_limit,
        )
        rows = int(result["rows"])
        byte_count = int(result["bytes"])
        theme_report.setdefault(job.theme, {})[job.region] = {
            "bytes": byte_count,
            "rows": rows,
        }
        generated_paths.append(job.output.relative_to(args.out).as_posix())
        total_rows += rows
        total_bytes += byte_count
        print(
            json.dumps(
                {
                    "bytes": byte_count,
                    "region": job.region,
                    "rows": rows,
                    "skipped": result["skipped"],
                    "theme": job.theme,
                },
                ensure_ascii=False,
                sort_keys=True,
            )
        )

    report = {
        "generated_paths": generated_paths,
        "release": release,
        "source_download_bytes_estimate": estimate["estimated_pilot_download_bytes"],
        "themes": theme_report,
        "totals": {"bytes": total_bytes, "rows": total_rows},
        "wall_seconds": round(time.monotonic() - wall_started, 3),
    }
    _write_json_atomic(args.out / "fetch-report.json", report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
