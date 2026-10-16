"""Offline tests for GeoParquet CRS, sealing, verification, and quality reports."""

from __future__ import annotations

import contextlib
import io
import json
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pyarrow as pa
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).parents[1]))

import geo_data
import manifest
from geoparquet_meta import resolved_crs


def point_wkb(x: float, y: float) -> bytes:
    """Create compact WKB fixtures without adding geometry-library dependencies."""

    return b"\x01" + struct.pack("<Idd", 1, x, y)


def polygon_wkb() -> bytes:
    """Create a square polygon so report bbox tests cover non-point geometry."""

    coordinates = [(0.0, 0.0), (3.0, 0.0), (3.0, 2.0), (0.0, 2.0), (0.0, 0.0)]
    header = b"\x01" + struct.pack("<I", 3) + struct.pack("<I", 1) + struct.pack("<I", len(coordinates))
    return header + b"".join(struct.pack("<dd", x, y) for x, y in coordinates)


class GeoParquetMetaTest(unittest.TestCase):
    """Build tiny local files because tests must remain offline and portable."""

    def make_file(
        self,
        path: Path,
        *,
        crs: object = "absent",
        include_geo: bool = True,
        duplicate_ids: bool = False,
        null_geometry: bool = False,
        polygon: bool = False,
    ) -> None:
        """Write direct GeoParquet metadata to test exact CRS semantics."""

        geometries = [
            polygon_wkb() if polygon else point_wkb(1.0, 2.0),
            None if null_geometry else point_wkb(3.0, 4.0),
        ]
        identifiers = ["a", "a" if duplicate_ids else "b"]
        table = pa.table(
            {
                "id": pa.array(identifiers, type=pa.string()),
                "geometry": pa.array(geometries, type=pa.binary()),
                "value": pa.array([None, 2], type=pa.int64()),
            }
        )

        if include_geo:
            column: dict[str, object] = {
                "encoding": "WKB",
                "geometry_types": ["Polygon" if polygon else "Point"],
            }
            if crs != "absent":
                column["crs"] = crs
            geo = {
                "version": "1.0.0",
                "primary_column": "geometry",
                "columns": {"geometry": column},
            }
            table = table.replace_schema_metadata({b"geo": json.dumps(geo).encode("utf-8")})

        pq.write_table(table, path, row_group_size=1)

    def make_manifest(self, dataset_id: str, relative_path: str) -> dict:
        """Provide an otherwise valid manifest for CLI command integration tests."""

        return {
            "id": dataset_id,
            "title": {"vi": "Mẫu", "en": "Sample"},
            "group": "places",
            "provider": "GeoFabric",
            "source": {
                "type": "generated",
                "reference": "https://example.invalid/test",
                "release": "2026-10-16",
            },
            "license": {
                "name": "CC0-1.0",
                "url": "https://creativecommons.org/publicdomain/zero/1.0/",
                "attribution": "GeoFabric test fixture",
                "redistribution": "allowed",
            },
            "retrieved": "2026-10-16",
            "coverage": {"name": "Test Region", "bbox": [0, 0, 4, 5]},
            "crs": "OGC:CRS84",
            "format_original": "GeoParquet",
            "format_normalized": "GeoParquet",
            "geometry_type": "Point",
            "files": [],
            "fetch": {"script": "generate.py", "args": []},
        }

    def test_four_crs_cases(self) -> None:
        """GeoParquet CRS rules distinguish absent, null, PROJJSON, and no geo."""

        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            cases = [
                ("default.parquet", "absent", True, "OGC:CRS84"),
                ("undefined.parquet", None, True, "UNDEFINED"),
                (
                    "epsg.parquet",
                    {"name": "WGS 84", "id": {"authority": "EPSG", "code": 4326}},
                    True,
                    "EPSG:4326",
                ),
                ("nogeo.parquet", "absent", False, "NOGEO"),
            ]

            for filename, crs, include_geo, expected in cases:
                with self.subTest(filename=filename):
                    path = directory / filename
                    self.make_file(path, crs=crs, include_geo=include_geo)
                    self.assertEqual(resolved_crs(path), expected)

    def test_seal_verify_round_trip_and_flipped_byte(self) -> None:
        """Sealing must enable verification, which must catch byte-level corruption."""

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = root / "data" / "manifests"
            local = root / "data" / "local"
            manifests.mkdir(parents=True)
            local.mkdir(parents=True)

            data_path = local / "sample.parquet"
            self.make_file(data_path)

            value = self.make_manifest("sample", "data/local/sample.parquet")
            manifest_path = manifests / "sample.json"
            manifest_path.write_text(json.dumps(value), encoding="utf-8")

            with (
                patch.object(geo_data, "ROOT", root),
                patch.object(geo_data, "MANIFESTS", manifests),
            ):
                seal_args = type("Args", (), {"id": "sample", "paths": ["data/local/sample.parquet"]})()
                self.assertEqual(geo_data.command_seal(seal_args), 0)

                verify_args = type("Args", (), {"ids": ["sample"]})()
                self.assertEqual(geo_data.command_verify(verify_args), 0)

                with data_path.open("r+b") as output:
                    output.seek(16)
                    byte = output.read(1)
                    output.seek(16)
                    output.write(bytes([byte[0] ^ 0x01]))

                self.assertEqual(geo_data.command_verify(verify_args), 1)

    def test_report_counts_duplicate_null_and_polygon_bbox(self) -> None:
        """Reports must stream useful data-quality facts, including non-point extents."""

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = root / "data" / "manifests"
            local = root / "data" / "local"
            manifests.mkdir(parents=True)
            local.mkdir(parents=True)

            data_path = local / "quality.parquet"
            self.make_file(data_path, duplicate_ids=True, null_geometry=True, polygon=True)

            value = self.make_manifest("quality", "data/local/quality.parquet")
            value["geometry_type"] = "Polygon"
            value["files"] = [
                {
                    "path": "data/local/quality.parquet",
                    "size_bytes": data_path.stat().st_size,
                    "sha256": manifest.sha256_file(data_path),
                    "rows": 2,
                    "id_digest": geo_data.id_digest(data_path),
                }
            ]
            (manifests / "quality.json").write_text(json.dumps(value), encoding="utf-8")

            args = type("Args", (), {"ids": ["quality"]})()
            with (
                patch.object(geo_data, "ROOT", root),
                patch.object(geo_data, "MANIFESTS", manifests),
            ):
                self.assertEqual(geo_data.command_report(args), 0)

            report = json.loads((root / "data" / "quality" / "quality.json").read_text(encoding="utf-8"))
            metrics = report["files"][0]
            self.assertEqual(metrics["rows"], 2)
            self.assertEqual(metrics["duplicate_id_count"], 1)
            self.assertEqual(metrics["null_geometry_count"], 1)
            self.assertEqual(metrics["geometry_types"], {"Polygon": 1})
            self.assertEqual(metrics["bbox"], [0.0, 0.0, 3.0, 2.0])
            self.assertEqual(metrics["per_column_null_ratio"]["value"], 0.5)
            self.assertEqual(metrics["crs"], "OGC:CRS84")

    def test_verify_rejects_undefined_crs_when_manifest_disagrees(self) -> None:
        """Undefined CRS must never pass verification against an explicit different CRS."""

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifests = root / "data" / "manifests"
            local = root / "data" / "local"
            manifests.mkdir(parents=True)
            local.mkdir(parents=True)

            data_path = local / "undefined.parquet"
            self.make_file(data_path, crs=None)

            value = self.make_manifest("undefined", "data/local/undefined.parquet")
            value["files"] = [
                {
                    "path": "data/local/undefined.parquet",
                    "size_bytes": data_path.stat().st_size,
                    "sha256": manifest.sha256_file(data_path),
                    "rows": 2,
                    "id_digest": geo_data.id_digest(data_path),
                }
            ]
            (manifests / "undefined.json").write_text(json.dumps(value), encoding="utf-8")

            args = type("Args", (), {"ids": ["undefined"]})()
            stdout = io.StringIO()
            with (
                patch.object(geo_data, "ROOT", root),
                patch.object(geo_data, "MANIFESTS", manifests),
                contextlib.redirect_stdout(stdout),
            ):
                self.assertEqual(geo_data.command_verify(args), 1)

            self.assertIn("MISMATCH (crs UNDEFINED)", stdout.getvalue())


if __name__ == "__main__":
    unittest.main()
