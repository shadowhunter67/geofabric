"""Offline behavior tests for the GeoParquet partition builder."""

from __future__ import annotations

import hashlib
import json
import random
import shutil
import sys
import tempfile
import unittest
import uuid
from pathlib import Path

import duckdb
import pyarrow.parquet as pq

sys.path.insert(0, str(Path(__file__).parents[1]))

import build_partitions as builder
import quadtree_partition as quadtree


class BuildPartitionsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = Path(tempfile.gettempdir()) / f"geofabric-wp2b-{uuid.uuid4().hex}"
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
                CREATE TABLE features(
                  id VARCHAR,
                  attr VARCHAR,
                  geometry GEOMETRY,
                  bbox STRUCT(xmin DOUBLE, ymin DOUBLE, xmax DOUBLE, ymax DOUBLE)
                )
                """
            )
            connection.execute(
                """
                INSERT INTO features
                SELECT 'dense-a-' || i, 'a', ST_Point(x, y),
                       {'xmin': x - 0.0001, 'ymin': y - 0.0001,
                        'xmax': x + 0.0001, 'ymax': y + 0.0001}
                FROM (
                  SELECT i, 103.95 + (i % 100) * 0.001 AS x,
                         7.95 + ((i * 17) % 100) * 0.001 AS y
                  FROM range(2800) r(i)
                )
                """
            )
            connection.execute(
                """
                INSERT INTO features
                SELECT 'dense-b-' || i, 'b', ST_Point(x, y),
                       {'xmin': x - 0.0001, 'ymin': y - 0.0001,
                        'xmax': x + 0.0001, 'ymax': y + 0.0001}
                FROM (
                  SELECT i, 119.95 + (i % 100) * 0.001 AS x,
                         23.95 + ((i * 19) % 100) * 0.001 AS y
                  FROM range(2800) r(i)
                )
                """
            )
            connection.execute(
                """
                INSERT INTO features
                SELECT 'sparse-' || i, 'sparse', ST_Point(x, y),
                       {'xmin': x - 0.001, 'ymin': y - 0.001,
                        'xmax': x + 0.001, 'ymax': y + 0.001}
                FROM (
                  SELECT i, 96.5 + ((i * 37) % 300) * 0.1 AS x,
                         0.5 + ((i * 53) % 300) * 0.1 AS y
                  FROM range(400) r(i)
                )
                """
            )
            connection.execute(
                """
                INSERT INTO features VALUES
                  ('cross-root', 'cross', ST_GeomFromText('LINESTRING(111.98 8, 112.02 8)'),
                   {'xmin': 111.98, 'ymin': 7.999, 'xmax': 112.02, 'ymax': 8.001}),
                  ('cross-horizontal', 'cross', ST_GeomFromText('LINESTRING(104 15.98, 104 16.02)'),
                   {'xmin': 103.999, 'ymin': 15.98, 'xmax': 104.001, 'ymax': 16.02}),
                  ('oversize-line', 'oversize', ST_GeomFromText('LINESTRING(98 5, 126 5)'),
                   {'xmin': 98.0, 'ymin': 4.99, 'xmax': 126.0, 'ymax': 5.01}),
                  ('oversize-polygon', 'oversize', ST_GeomFromText('POLYGON((105 2,107 2,107 4,105 4,105 2))'),
                   {'xmin': 105.0, 'ymin': 2.0, 'xmax': 107.0, 'ymax': 4.0})
                """
            )
            connection.execute(
                "COPY (SELECT * FROM features ORDER BY id) TO ? "
                "(FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 1000)",
                [str(cls.source)],
            )
        finally:
            connection.close()

        cls.config = builder.DatasetConfig(
            dataset_id="synthetic",
            theme="test",
            source_release="2026-09-23.1",
            license="ODbL-1.0",
            target_bytes=40_000,
            hard_cap_bytes=2_000_000,
            oversize_extent_deg=0.5,
            min_zoom=7,
            row_group_size=1000,
        )
        cls.manifest = builder.build_partitions(
            cls.source, cls.root / "first", cls.config, "v2"
        )
        cls.dataset_dir = cls.root / "first" / cls.config.dataset_id

    @classmethod
    def tearDownClass(cls):
        if hasattr(cls, "root") and cls.root.exists():
            shutil.rmtree(cls.root)

    def connect(self):
        connection = duckdb.connect()
        connection.execute("LOAD spatial")
        return connection

    def shard_paths(self, manifest=None, dataset_dir=None):
        manifest = manifest or self.manifest
        dataset_dir = dataset_dir or self.dataset_dir
        return [dataset_dir / f"{part['id']}.parquet" for part in manifest["partitions"]]

    def assert_exact_dataset_files(self, manifest, dataset_dir):
        expected = {
            *(f"{partition['id']}.parquet" for partition in manifest["partitions"]),
            "partitions.json",
            "build-report.json",
        }
        self.assertEqual({path.name for path in dataset_dir.iterdir()}, expected)

    def test_sql_ownership_matches_python_on_random_points_and_borders(self):
        rng = random.Random(20261103)
        points = [
            (rng.uniform(96, 128), rng.uniform(0, 32)) for _ in range(200)
        ]
        for level in (1, 2, 5, 9, quadtree.MAX_DEPTH):
            step = 32 / (2**level)
            points.extend(
                [(96 + step, 0), (96, step), (96 + step, step), (128, 32)]
            )
        ix_sql = builder._index_sql("x", quadtree.ROOT[0])
        iy_sql = builder._index_sql("y", quadtree.ROOT[1])
        connection = duckdb.connect()
        try:
            for x, y in points:
                ix, iy = connection.execute(
                    f"SELECT {ix_sql}, {iy_sql} FROM (SELECT ?::DOUBLE x, ?::DOUBLE y)",
                    [x, y],
                ).fetchone()
                self.assertEqual(
                    builder._fine_path(ix, iy),
                    quadtree.cell_id_for_point(x, y, quadtree.MAX_DEPTH),
                )
        finally:
            connection.close()

    def test_conservation_geoparquet_ownership_and_oversize(self):
        paths = self.shard_paths()
        connection = self.connect()
        try:
            source_ids = {
                row[0]
                for row in connection.execute(
                    "SELECT id FROM read_parquet(?)", [str(self.source)]
                ).fetchall()
            }
            shard_ids = [
                row[0]
                for row in connection.execute(
                    "SELECT id FROM read_parquet(?)", [[str(path) for path in paths]]
                ).fetchall()
            ]
            self.assertEqual(len(shard_ids), len(set(shard_ids)))
            self.assertEqual(set(shard_ids), source_ids)
            self.assertEqual(len(shard_ids), 6004)

            for path in paths:
                metadata = pq.ParquetFile(path).metadata.metadata or {}
                self.assertIn(b"geo", metadata)

            locations = {}
            for partition, path in zip(self.manifest["partitions"], paths):
                ids = {
                    row[0]
                    for row in connection.execute(
                        "SELECT id FROM read_parquet(?)", [str(path)]
                    ).fetchall()
                }
                for feature_id in (
                    "cross-root",
                    "cross-horizontal",
                    "oversize-line",
                    "oversize-polygon",
                ):
                    if feature_id in ids:
                        locations[feature_id] = partition
                if partition["kind"] == "regular":
                    self.assertFalse(
                        {"oversize-line", "oversize-polygon"} & ids
                    )
                else:
                    self.assertTrue(partition["id"].startswith("L"))

            self.assertEqual(
                {locations[name]["kind"] for name in ("oversize-line", "oversize-polygon")},
                {"oversize"},
            )
            for feature_id in ("cross-root", "cross-horizontal"):
                partition = locations[feature_id]
                bbox = connection.execute(
                    "SELECT bbox FROM read_parquet(?) WHERE id = ?",
                    [str(self.source), feature_id],
                ).fetchone()[0]
                centre = ((bbox["xmin"] + bbox["xmax"]) / 2, (bbox["ymin"] + bbox["ymax"]) / 2)
                west, south, east, north = quadtree.cell_bbox(partition["id"])
                self.assertTrue(west <= centre[0] <= east and south <= centre[1] <= north)
                self.assertLessEqual(partition["bbox"][0], bbox["xmin"])
                self.assertLessEqual(partition["bbox"][1], bbox["ymin"])
                self.assertGreaterEqual(partition["bbox"][2], bbox["xmax"])
                self.assertGreaterEqual(partition["bbox"][3], bbox["ymax"])

            q_paths = [
                str(path)
                for partition, path in zip(self.manifest["partitions"], paths)
                if partition["kind"] == "regular"
            ]
            q_extent = connection.execute(
                "SELECT min(bbox.xmin), min(bbox.ymin), max(bbox.xmax), max(bbox.ymax) "
                "FROM read_parquet(?)",
                [q_paths],
            ).fetchone()
            published_q_extent = (
                min(p["bbox"][0] for p in self.manifest["partitions"] if p["kind"] == "regular"),
                min(p["bbox"][1] for p in self.manifest["partitions"] if p["kind"] == "regular"),
                max(p["bbox"][2] for p in self.manifest["partitions"] if p["kind"] == "regular"),
                max(p["bbox"][3] for p in self.manifest["partitions"] if p["kind"] == "regular"),
            )
            self.assertEqual(tuple(float(value) for value in q_extent), published_q_extent)
        finally:
            connection.close()

    def test_bbox_selected_view_queries_equal_source(self):
        windows = [
            (103.97, 7.97, 104.01, 8.01),
            (111.9, 7.9, 120.1, 24.1),
            (111.99, 7.9995, 112.01, 8.0005),
        ]
        connection = self.connect()
        try:
            for window in windows:
                x0, y0, x1, y1 = window
                expected = {
                    row[0]
                    for row in connection.execute(
                        "SELECT id FROM read_parquet(?) WHERE "
                        "bbox.xmin <= ? AND bbox.xmax >= ? AND bbox.ymin <= ? AND bbox.ymax >= ?",
                        [str(self.source), x1, x0, y1, y0],
                    ).fetchall()
                }
                selected = [
                    self.dataset_dir / f"{partition['id']}.parquet"
                    for partition in self.manifest["partitions"]
                    if partition["bbox"][0] <= x1
                    and partition["bbox"][2] >= x0
                    and partition["bbox"][1] <= y1
                    and partition["bbox"][3] >= y0
                ]
                actual_rows = connection.execute(
                    "SELECT id FROM read_parquet(?) WHERE "
                    "bbox.xmin <= ? AND bbox.xmax >= ? AND bbox.ymin <= ? AND bbox.ymax >= ?",
                    [[str(path) for path in selected], x1, x0, y1, y0],
                ).fetchall()
                actual = [row[0] for row in actual_rows]
                self.assertEqual(len(actual), len(set(actual)))
                self.assertEqual(set(actual), expected)
        finally:
            connection.close()

    def test_manifest_report_hashes_and_validation(self):
        text = (self.dataset_dir / "partitions.json").read_text(encoding="utf-8")
        parsed = json.loads(text)
        self.assertEqual(list(parsed), sorted(parsed))
        self.assertEqual(parsed["schema"], 3)
        self.assertEqual(parsed["dataset"], "synthetic")
        self.assertEqual(parsed["data_version"], "v2")
        self.assertEqual(parsed["partition_scheme"], quadtree.SCHEME)
        self.assertEqual(parsed["rows"], sum(p["rows"] for p in parsed["partitions"]))
        for partition in parsed["partitions"]:
            path = self.dataset_dir / f"{partition['id']}.parquet"
            self.assertEqual(partition["bytes"], path.stat().st_size)
            self.assertEqual(
                partition["sha256"], hashlib.sha256(path.read_bytes()).hexdigest()
            )
            self.assertIn(partition["kind"], ("regular", "oversize"))
        self.assertEqual(
            builder.validate_build(
                self.source, self.dataset_dir, parsed, self.config
            ),
            [],
        )
        report = json.loads(
            (self.dataset_dir / "build-report.json").read_text(encoding="utf-8")
        )
        self.assertEqual(report["scheme"], quadtree.SCHEME)
        self.assertEqual(len(report["shards"]), len(parsed["partitions"]))
        self.assertTrue(all("estimate_error" in shard for shard in report["shards"]))
        self.assert_exact_dataset_files(parsed, self.dataset_dir)
        with self.assertRaisesRegex(ValueError, "never 'latest'"):
            builder.build_partitions(
                self.source, self.root / "latest", self.config, "latest"
            )

    def test_immutable_publish_and_overwrite_removes_stale_shards(self):
        out = self.root / "publish-lifecycle"
        first = builder.build_partitions(self.source, out, self.config, "v2")
        dataset_dir = out / self.config.dataset_id
        before = {
            path.name: path.read_bytes()
            for path in dataset_dir.iterdir()
        }

        with self.assertRaisesRegex(FileExistsError, "assets are immutable"):
            builder.build_partitions(self.source, out, self.config, "v2")
        self.assertEqual(
            {path.name: path.read_bytes() for path in dataset_dir.iterdir()}, before
        )

        replacement_config = builder.DatasetConfig(
            **{**builder.asdict(self.config), "target_bytes": 1_000_000}
        )
        replacement = builder.build_partitions(
            self.source,
            out,
            replacement_config,
            "v3",
            overwrite=True,
        )
        self.assertNotEqual(
            {partition["id"] for partition in first["partitions"]},
            {partition["id"] for partition in replacement["partitions"]},
        )
        self.assert_exact_dataset_files(replacement, dataset_dir)

    def test_failed_build_removes_staging_and_final_directories(self):
        duplicate_source = self.root / "duplicate-source.parquet"
        connection = duckdb.connect()
        try:
            connection.execute(
                "CREATE TABLE duplicated AS SELECT * FROM read_parquet(?)",
                [str(self.source)],
            )
            connection.execute(
                "INSERT INTO duplicated SELECT * FROM read_parquet(?) LIMIT 1",
                [str(self.source)],
            )
            connection.execute(
                f"COPY duplicated TO {builder._sql_string(duplicate_source)} (FORMAT parquet)"
            )
        finally:
            connection.close()
        out = self.root / "failed-publish"
        with self.assertRaisesRegex(ValueError, "feature id conservation failed"):
            builder.build_partitions(duplicate_source, out, self.config, "v2")
        self.assertFalse((out / self.config.dataset_id).exists())
        self.assertFalse((out / f"{self.config.dataset_id}.building").exists())

    def test_duckdb_resource_options_are_applied(self):
        temp_dir = self.root / "duckdb-options-temp"
        temp_dir.mkdir()
        connection = duckdb.connect()
        try:
            builder._load_spatial(
                connection,
                memory_limit="128MB",
                temp_dir=temp_dir,
                threads=2,
            )
            memory_limit, threads, configured_temp = connection.execute(
                "SELECT current_setting('memory_limit'), current_setting('threads'), "
                "current_setting('temp_directory')"
            ).fetchone()
            self.assertEqual(memory_limit, "122.0 MiB")
            self.assertEqual(threads, 2)
            self.assertEqual(Path(configured_temp), temp_dir)
        finally:
            connection.close()

    def test_adjust_target(self):
        self.assertEqual(builder.adjust_target(1000, 2000, 1000), 450)
        self.assertEqual(builder.adjust_target(1000, 500, 1000), 1000)
        self.assertEqual(builder.adjust_target(1, 1000, 1), 1)
        with self.assertRaises(ValueError):
            builder.adjust_target(0, 1, 1)

    def test_hard_cap_replans_once_then_reports_offenders(self):
        config = builder.DatasetConfig(
            dataset_id="hard-cap",
            theme="test",
            source_release="2026-09-23.1",
            license="ODbL-1.0",
            target_bytes=40_000,
            hard_cap_bytes=100,
            oversize_extent_deg=0.5,
            row_group_size=1000,
        )
        with self.assertRaisesRegex(
            ValueError, r"still exceed hard cap 100 after one replan: .+=\d+"
        ):
            builder.build_partitions(self.source, self.root / "cap", config, "v2")

    def test_determinism(self):
        config = builder.DatasetConfig(
            **{**builder.asdict(self.config), "dataset_id": "synthetic-second"}
        )
        second = builder.build_partitions(
            self.source, self.root / "second", config, "v2"
        )
        first_shape = [
            (p["id"], p["rows"], p["bbox"], p["sha256"])
            for p in self.manifest["partitions"]
        ]
        second_shape = [
            (p["id"], p["rows"], p["bbox"], p["sha256"])
            for p in second["partitions"]
        ]
        self.assertEqual(first_shape, second_shape)


if __name__ == "__main__":
    unittest.main()
