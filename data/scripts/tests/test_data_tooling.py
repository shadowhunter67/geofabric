"""Offline coverage for fetch SQL, DEM tiling, and bootstrap execution safety."""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[1]))

import fetch_dem
import fetch_overture
import geo_data
import manifest


class FetcherTest(unittest.TestCase):
    """Keep source-query and tile naming contracts independently testable offline."""

    def test_overture_sql_presets_are_pinned_and_flat(self) -> None:
        """Each preset must retain its bbox filter and explicit selected aliases."""

        expected_aliases = {
            "transport": ("name", "surface", "max_speed", "source_dataset"),
            "places": ("name", "category", "taxonomy", "source_dataset"),
            "admin": ("name", "source_dataset"),
            "landuse": ("name", "source_dataset"),
        }
        for name, aliases in expected_aliases.items():
            with self.subTest(name=name):
                sql = fetch_overture.build_sql(
                    name,
                    [106.35, 10.35, 107.05, 11.25],
                    "2026-09-23.1",
                    Path("output.part"),
                )
                self.assertIn("bbox.xmin <=", sql)
                self.assertIn("bbox.xmax >=", sql)
                self.assertIn("bbox.ymin <=", sql)
                self.assertIn("bbox.ymax >=", sql)
                self.assertIn("/release/2026-09-23.1/", sql)
                self.assertNotIn("ORDER BY", sql)
                for alias in aliases:
                    self.assertIn(f" AS {alias}", sql)
                self.assertNotIn("source_tags", sql)
                self.assertNotIn("taxonomy.hierarchy", sql)

        self.assertIn("country = 'VN'", fetch_overture.build_sql(
            "admin", [106.35, 10.35, 107.05, 11.25], "2026-09-23.1", Path("x.part")
        ))
        landuse = fetch_overture.build_sql(
            "landuse", [106.35, 10.35, 107.05, 11.25], "2026-09-23.1", Path("x.part")
        )
        self.assertIn("bbox.xmax - bbox.xmin <= 5", landuse)
        self.assertIn("bbox.ymax - bbox.ymin <= 5", landuse)
        with self.assertRaisesRegex(ValueError, "unknown preset"):
            fetch_overture.build_sql("unknown", [0, 0, 1, 1], "2026-09-23.1", Path("x.part"))

    def test_dem_tiles_cover_boundaries_and_signed_names(self) -> None:
        """Tile enumeration must use southwest degree cells for all hemispheres."""

        self.assertEqual(
            fetch_dem.tiles_for_bbox([106.35, 10.35, 107.05, 11.25]),
            [(10, 106), (10, 107), (11, 106), (11, 107)],
        )
        self.assertEqual(fetch_dem.tiles_for_bbox([106.9, 10.9, 107.1, 11.1]), [
            (10, 106), (10, 107), (11, 106), (11, 107),
        ])
        self.assertIn("_S01_00_W002_00_", fetch_dem.tile_name(-1, -2))


class ManifestAndBootstrapTest(unittest.TestCase):
    """Validate committed lineage and bootstrap behavior without local downloads."""

    def test_committed_manifests_are_sealed_and_pinned(self) -> None:
        """Committed manifests remain complete provenance records."""

        manifests = sorted((manifest.ROOT / "data" / "manifests").glob("*.json"))
        self.assertTrue(manifests)
        for path in manifests:
            with self.subTest(path=path.name):
                value = manifest.load(path)
                self.assertTrue(value["license"]["attribution"])
                self.assertNotIn(value["source"]["release"].lower(), {"latest", "current"})
                self.assertTrue(value["coverage"]["name"])
                script = manifest.ROOT / value["fetch"]["script"]
                self.assertTrue(script.is_file())
                for file_info in value["files"]:
                    self.assertRegex(file_info["sha256"], r"^[0-9a-fA-F]{64}$")
                    if file_info["id_digest"] is not None:
                        self.assertRegex(file_info["id_digest"], r"^[0-9a-fA-F]{64}$")

    def test_bootstrap_dry_run_orders_filters_and_does_not_execute(self) -> None:
        """Dry runs inspect commands only, even when subprocess execution would fail."""

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = root / "data" / "manifests"
            scripts = root / "data" / "scripts"
            manifests.mkdir(parents=True)
            scripts.mkdir(parents=True)
            (scripts / "fetch.py").write_text("", encoding="utf-8")

            for dataset_id, group in (("z_places", "places"), ("a_base", "base")):
                value = {
                    "id": dataset_id,
                    "title": {"vi": "Mau", "en": "Sample"},
                    "group": group,
                    "provider": "Test",
                    "source": {"type": "generated", "reference": "x", "release": "1"},
                    "license": {"name": "CC0", "url": "x", "attribution": "Test", "redistribution": "allowed"},
                    "retrieved": "2026-10-16",
                    "coverage": {"name": "Demo", "bbox": [0, 0, 1, 1]},
                    "crs": "OGC:CRS84",
                    "format_original": "COG",
                    "format_normalized": "COG",
                    "geometry_type": "Raster",
                    "files": [],
                    "fetch": {"script": "data/scripts/fetch.py", "args": ["--out", f"data/local/{dataset_id}.tif"]},
                }
                (manifests / f"{dataset_id}.json").write_text(json.dumps(value), encoding="utf-8")

            args = type("Args", (), {"region": "Demo", "run": True, "dry_run": True, "ids": ["z_places"]})()
            output = io.StringIO()
            with (
                patch.object(geo_data, "ROOT", root),
                patch.object(geo_data, "MANIFESTS", manifests),
                patch.object(geo_data.subprocess, "run", side_effect=AssertionError("must not execute")),
                contextlib.redirect_stdout(output),
            ):
                self.assertEqual(geo_data.command_bootstrap(args), 0)

            self.assertEqual(len(output.getvalue().splitlines()), 1)
            self.assertIn("z_places", output.getvalue())

    def test_bootstrap_skips_verified_files(self) -> None:
        """Verified sealed data must not invoke its fetcher again."""

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = root / "data" / "manifests"
            scripts = root / "data" / "scripts"
            local = root / "data" / "local"
            manifests.mkdir(parents=True)
            scripts.mkdir(parents=True)
            local.mkdir(parents=True)
            (scripts / "fetch.py").write_text("", encoding="utf-8")
            output = local / "one.tif"
            output.write_bytes(b"verified")
            value = {
                "id": "one",
                "title": {"vi": "Mau", "en": "Sample"},
                "group": "raster",
                "provider": "Test",
                "source": {"type": "generated", "reference": "x", "release": "1"},
                "license": {"name": "CC0", "url": "x", "attribution": "Test", "redistribution": "allowed"},
                "retrieved": "2026-10-16",
                "coverage": {"name": "Demo", "bbox": [0, 0, 1, 1]},
                "crs": "EPSG:4326",
                "format_original": "COG",
                "format_normalized": "COG",
                "geometry_type": "Raster",
                "files": [{
                    "path": "data/local/one.tif",
                    "size_bytes": output.stat().st_size,
                    "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
                    "rows": None,
                    "id_digest": None,
                }],
                "fetch": {"script": "data/scripts/fetch.py", "args": ["--out", "data/local/one.tif"]},
            }
            (manifests / "one.json").write_text(json.dumps(value), encoding="utf-8")
            args = type("Args", (), {"region": "Demo", "run": True, "dry_run": False, "ids": None})()

            stdout = io.StringIO()
            with (
                patch.object(geo_data, "ROOT", root),
                patch.object(geo_data, "MANIFESTS", manifests),
                patch.object(geo_data.subprocess, "run", side_effect=AssertionError("must not execute")),
                contextlib.redirect_stdout(stdout),
            ):
                self.assertEqual(geo_data.command_bootstrap(args), 0)
            self.assertIn("SKIP one (verified)", stdout.getvalue())

    def test_bootstrap_rejects_unknown_only_id(self) -> None:
        """Unknown --only ids must fail before a fetch command is considered."""

        args = type("Args", (), {"region": "Vietnam Demo", "run": False, "dry_run": False, "ids": ["missing"]})()
        with self.assertRaisesRegex(ValueError, "unknown manifest"):
            geo_data.command_bootstrap(args)


if __name__ == "__main__":
    unittest.main()
