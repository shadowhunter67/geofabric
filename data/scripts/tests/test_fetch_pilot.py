"""Offline tests for the authorized, resumable pilot fetcher."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
import uuid

import duckdb

SCRIPTS = Path(__file__).parents[1]
sys.path.insert(0, str(SCRIPTS))

import fetch_overture  # noqa: E402
import fetch_pilot  # noqa: E402
from overture_estimate import DOWNLOAD_GATE_BYTES, NATIONAL_ADMIN_BBOX  # noqa: E402


REGIONS = {
    "hanoi": [105.65, 20.9, 106.05, 21.2],
    "da_nang": [108.05, 15.95, 108.35, 16.2],
    "buon_ma_thuot": [107.95, 12.55, 108.15, 12.8],
    "mekong_can_tho": [105.65, 9.9, 105.9, 10.15],
    "hcmc": [106.55, 10.65, 106.9, 10.95],
}


class GateTests(unittest.TestCase):
    def test_missing_partial_over_gate_and_approved(self) -> None:
        directory = Path(tempfile.gettempdir()) / f"geofabric-fetch-gate-{uuid.uuid4().hex}"
        directory.mkdir()
        try:
            with self.assertRaisesRegex(ValueError, "missing"):
                fetch_pilot.check_gate(directory / "missing.json")
        finally:
            shutil.rmtree(directory)
        with self.assertRaisesRegex(ValueError, "partial"):
            fetch_pilot.check_gate(
                {"partial": True, "estimated_pilot_download_bytes": 1}
            )
        with self.assertRaisesRegex(ValueError, "exceeds"):
            fetch_pilot.check_gate(
                {
                    "partial": False,
                    "estimated_pilot_download_bytes": DOWNLOAD_GATE_BYTES + 1,
                }
            )
        approved = fetch_pilot.check_gate(
            {
                "partial": False,
                "estimated_pilot_download_bytes": DOWNLOAD_GATE_BYTES,
            }
        )
        self.assertEqual(approved["estimated_pilot_download_bytes"], DOWNLOAD_GATE_BYTES)


class PlanTests(unittest.TestCase):
    def test_full_plan_has_twenty_regional_and_one_national_job(self) -> None:
        jobs = fetch_pilot.plan_jobs(REGIONS, fetch_pilot.DEFAULT_THEMES, Path("raw"))
        self.assertEqual(len(jobs), 21)
        self.assertEqual(jobs[0].output, Path("raw/buildings/hanoi.parquet"))
        self.assertEqual(jobs[-1].output, Path("raw/admin/vn.parquet"))
        self.assertEqual(list(jobs[-1].bbox), NATIONAL_ADMIN_BBOX)
        self.assertEqual(jobs[-1].preset, "admin")

    def test_theme_and_region_filtering_and_unknown_values(self) -> None:
        selected = fetch_pilot._select_regions(REGIONS, ["hanoi", "hcmc"])
        jobs = fetch_pilot.plan_jobs(selected, ["roads", "admin"], Path("raw"))
        self.assertEqual(
            [(job.theme, job.region) for job in jobs],
            [("roads", "hanoi"), ("roads", "hcmc"), ("admin", "vn")],
        )
        with self.assertRaisesRegex(ValueError, "unknown theme"):
            fetch_pilot.plan_jobs(REGIONS, ["water"], Path("raw"))
        with self.assertRaisesRegex(ValueError, "unknown region"):
            fetch_pilot._select_regions(REGIONS, ["not_a_region"])


class PresetTests(unittest.TestCase):
    def test_buildings_preset_and_sql_are_exact_and_pinned(self) -> None:
        preset = fetch_overture.PRESETS["buildings"]
        self.assertEqual(
            preset.select,
            (
                "id",
                "names.primary AS name",
                "subtype",
                "class",
                "height",
                "num_floors",
                "is_underground",
                "has_parts",
                "sources[1].dataset AS source_dataset",
                "version",
                "geometry",
                "bbox",
            ),
        )
        sql = fetch_overture.build_sql(
            "buildings", [105.0, 10.0, 106.0, 11.0], "release-1", Path("x.part")
        )
        for predicate in (
            "bbox.xmin <= 106.0",
            "bbox.xmax >= 105.0",
            "bbox.ymin <= 11.0",
            "bbox.ymax >= 10.0",
        ):
            self.assertIn(predicate, sql)
        self.assertIn("theme=buildings/type=building", sql)
        self.assertNotIn("latest", sql.casefold())


class ResumeTests(unittest.TestCase):
    def test_checksum_pair_controls_skip_and_run_removes_leftover_part(self) -> None:
        root = Path(tempfile.gettempdir()) / f"geofabric-fetch-resume-{uuid.uuid4().hex}"
        root.mkdir()
        try:
            output = root / "buildings" / "hanoi.parquet"
            output.parent.mkdir()
            output.write_bytes(b"verified")
            sidecar = Path(f"{output}.json")
            sidecar.write_text(
                json.dumps({"sha256": hashlib.sha256(b"verified").hexdigest(), "rows": 1, "bytes": 8}),
                encoding="utf-8",
            )
            self.assertTrue(fetch_pilot.should_skip(output))

            part = Path(f"{output}.part")
            part.write_bytes(b"interrupted")
            job = fetch_pilot.Job(
                "buildings", "hanoi", "buildings", (105.0, 10.0, 106.0, 11.0), output
            )
            result = fetch_pilot.run_job(job, "release-1")
            self.assertTrue(result["skipped"])
            self.assertFalse(part.exists())

            sidecar.write_text(json.dumps({"sha256": "0" * 64}), encoding="utf-8")
            self.assertFalse(fetch_pilot.should_skip(output))
            sidecar.unlink()
            self.assertFalse(fetch_pilot.should_skip(output))
        finally:
            shutil.rmtree(root)


class LocalWriteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.root = Path(tempfile.gettempdir()) / f"geofabric-fetch-pilot-{uuid.uuid4().hex}"
        cls.root.mkdir()
        cls.source = cls.root / "source.parquet"
        connection = duckdb.connect()
        try:
            try:
                connection.execute("LOAD spatial")
            except Exception as error:
                raise unittest.SkipTest(
                    f"DuckDB spatial extension is unavailable offline: {error}"
                )
            connection.execute(
                """
                CREATE TABLE source AS SELECT
                  'building-1'::VARCHAR AS id,
                  {'primary': 'Pilot building'} AS names,
                  'residential'::VARCHAR AS subtype,
                  'house'::VARCHAR AS class,
                  12.5::DOUBLE AS height,
                  3::INTEGER AS num_floors,
                  false AS is_underground,
                  false AS has_parts,
                  [{'dataset': 'local-test'}] AS sources,
                  1::BIGINT AS version,
                  ST_GeomFromText('POLYGON((105 10,106 10,106 11,105 11,105 10))') AS geometry,
                  {'xmin': 105.0, 'xmax': 106.0, 'ymin': 10.0, 'ymax': 11.0} AS bbox
                """
            )
            connection.execute(
                "COPY source TO ? (FORMAT parquet, COMPRESSION zstd)", [str(cls.source)]
            )
        finally:
            connection.close()

    @classmethod
    def tearDownClass(cls) -> None:
        if hasattr(cls, "root") and cls.root.exists():
            shutil.rmtree(cls.root)

    def test_local_write_verifies_seals_and_then_skips(self) -> None:
        output = self.root / "raw" / "buildings" / "local.parquet"
        job = fetch_pilot.Job(
            "buildings", "local", "buildings", (105.0, 10.0, 106.0, 11.0), output
        )

        def local_builder(
            preset_name: str, bbox: list[float], release: str, out_tmp: Path
        ) -> str:
            del bbox, release
            preset = fetch_overture.PRESETS[preset_name]
            return (
                f"COPY (SELECT {', '.join(preset.select)} "
                f"FROM read_parquet({fetch_overture.sql_string(self.source.as_posix())})) "
                f"TO {fetch_overture.sql_string(out_tmp.as_posix())} "
                f"(FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE {preset.row_group_size})"
            )

        first = fetch_pilot.run_job(job, "release-1", sql_builder=local_builder)
        self.assertFalse(first["skipped"])
        self.assertEqual(first["rows"], 1)
        self.assertTrue(output.exists())
        self.assertTrue(job.sidecar.exists())
        fetch_overture.verify_geoparquet(output)
        sealed = json.loads(job.sidecar.read_text(encoding="utf-8"))
        self.assertEqual(sealed["sha256"], fetch_pilot.sha256_file(output))

        second = fetch_pilot.run_job(job, "release-1", sql_builder=local_builder)
        self.assertTrue(second["skipped"])


if __name__ == "__main__":
    unittest.main()
