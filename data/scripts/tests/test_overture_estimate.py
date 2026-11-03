"""Offline tests for conservative Overture footer estimation."""

from __future__ import annotations

import json
import io
import re
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parents[1]))

import fetch_overture
import overture_estimate


def metadata_group(
    file_name: str,
    row_group_id: int,
    bounds: tuple[float, float, float, float] | None,
    *,
    rows: int = 10,
    selected_bytes: int = 100,
) -> dict:
    """Build one compact row-group record returned by the SQL aggregation."""

    xmin_min = ymin_min = xmax_max = ymax_max = None
    if bounds is not None:
        west, south, east, north = bounds
        xmin_min, ymin_min, xmax_max, ymax_max = west, south, east, north
    return {
        "file_name": file_name,
        "row_group_id": row_group_id,
        "row_group_num_rows": rows,
        "xmin_min": xmin_min,
        "ymin_min": ymin_min,
        "xmax_max": xmax_max,
        "ymax_max": ymax_max,
        "compressed_bytes": selected_bytes,
        "uncompressed_bytes": selected_bytes * 2,
    }


class OvertureEstimateTest(unittest.TestCase):
    """Verify all pure estimation rules without touching S3 or data pages."""

    def test_intersection_true_false_and_edge_touching(self) -> None:
        """Inclusive bbox predicates must retain exact boundary contact."""

        bbox = [10, 10, 20, 20]
        self.assertEqual(
            overture_estimate.row_group_may_intersect(
                metadata_group("a", 0, (15, 15, 16, 16)), bbox
            ),
            (True, False),
        )
        self.assertEqual(
            overture_estimate.row_group_may_intersect(
                metadata_group("a", 0, (21, 15, 22, 16)), bbox
            ),
            (False, False),
        )
        self.assertEqual(
            overture_estimate.row_group_may_intersect(
                metadata_group("a", 0, (20, 20, 21, 21)), bbox
            ),
            (True, False),
        )

    def test_sql_aggregation_returns_compact_real_format_row_groups(self) -> None:
        """DuckDB must normalize, filter, parse, and group real footer-shaped rows."""

        connection = overture_estimate.duckdb.connect()
        try:
            connection.execute(
                """
                CREATE TABLE metadata_fixture (
                    file_name VARCHAR,
                    row_group_id BIGINT,
                    row_group_num_rows BIGINT,
                    path_in_schema VARCHAR,
                    stats_min VARCHAR,
                    stats_max VARCHAR,
                    stats_min_value VARCHAR,
                    stats_max_value VARCHAR,
                    total_compressed_size BIGINT,
                    total_uncompressed_size BIGINT
                )
                """
            )
            rows = [
                ("a.parquet", 0, 10, "names, primary", None, None, None, None, 11, 21),
                ("a.parquet", 0, 10, "unused, child", None, None, None, None, 999, 999),
                ("a.parquet", 0, 10, "bbox, xmin", None, "1", "1", None, 2, 3),
                ("a.parquet", 0, 10, "bbox, ymin", "2", "2", None, None, 2, 3),
                ("a.parquet", 0, 10, "bbox, xmax", "3", "3", None, None, 2, 3),
                ("a.parquet", 0, 10, "bbox, ymax", "4", "4", None, None, 2, 3),
                ("b.parquet", 1, 20, "names.primary", None, None, None, None, 13, 23),
                ("b.parquet", 1, 20, "bbox, xmin", None, None, None, None, 0, 0),
                ("b.parquet", 1, 20, "bbox, ymin", None, None, None, None, 0, 0),
                ("b.parquet", 1, 20, "bbox, xmax", None, None, None, None, 0, 0),
                ("b.parquet", 1, 20, "bbox, ymax", None, "bad", None, None, 0, 0),
                ("c.parquet", 2, 30, "bbox.xmin", "5", "5", None, None, 0, 0),
                ("c.parquet", 2, 30, "bbox.ymin", "6", "6", None, None, 0, 0),
                ("c.parquet", 2, 30, "bbox.xmax", "7", "7", None, None, 0, 0),
                ("c.parquet", 2, 30, "bbox.ymax", "8", "8", None, None, 0, 0),
            ]
            connection.executemany(
                "INSERT INTO metadata_fixture VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rows,
            )
            result = connection.execute(
                overture_estimate.metadata_aggregation_sql("metadata_fixture"),
                [["names"]],
            )
            names = [column[0] for column in result.description]
            compact = [dict(zip(names, row, strict=True)) for row in result.fetchall()]
        finally:
            connection.close()

        self.assertEqual(len(compact), 3)
        self.assertEqual(
            compact[0],
            {
                "file_name": "a.parquet",
                "row_group_id": 0,
                "row_group_num_rows": 10,
                "xmin_min": 1.0,
                "ymin_min": 2.0,
                "xmax_max": 3.0,
                "ymax_max": 4.0,
                "compressed_bytes": 11,
                "uncompressed_bytes": 21,
            },
        )
        self.assertIsNone(compact[1]["xmin_min"])
        self.assertIsNone(compact[1]["ymin_min"])
        self.assertIsNone(compact[1]["xmax_max"])
        self.assertIsNone(compact[1]["ymax_max"])
        self.assertEqual(compact[1]["compressed_bytes"], 13)
        self.assertEqual(
            (
                compact[2]["xmin_min"],
                compact[2]["ymin_min"],
                compact[2]["xmax_max"],
                compact[2]["ymax_max"],
            ),
            (5.0, 6.0, 7.0, 8.0),
        )

    def test_real_format_bbox_stats_exclude_far_row_groups(self) -> None:
        """Comma-space footer paths must preserve row-group selectivity."""

        groups = [
            metadata_group("far", 0, (30, 30, 31, 31)),
            metadata_group("overlap", 1, (15, 15, 16, 16)),
        ]
        touched, without_stats = overture_estimate.touched_row_groups(
            groups, [10, 10, 20, 20]
        )
        self.assertEqual(
            [(group["file_name"], group["row_group_id"]) for group in touched],
            [("overlap", 1)],
        )
        self.assertEqual(without_stats, set())

    def test_missing_or_unparsable_stats_are_conservative(self) -> None:
        """Unknown footer bounds cannot safely exclude a row group."""

        missing = metadata_group("a", 0, None)
        invalid = metadata_group("b", 0, (1, 1, 2, 2))
        invalid["ymax_max"] = None
        self.assertEqual(
            overture_estimate.row_group_may_intersect(missing, [10, 10, 20, 20]),
            (True, True),
        )
        self.assertEqual(
            overture_estimate.row_group_may_intersect(invalid, [10, 10, 20, 20]),
            (True, True),
        )

    def test_per_region_sums_and_selected_top_level_filtering(self) -> None:
        """Only touched groups and selected top-level chunks contribute bytes."""

        groups = [
            metadata_group("a", 0, (0, 0, 2, 2), rows=7, selected_bytes=30),
            metadata_group("b", 0, (10, 10, 12, 12), rows=11, selected_bytes=50),
            metadata_group("c", 0, None, rows=13, selected_bytes=70),
        ]
        reports, _ = overture_estimate.summarize_regions(
            groups, {"one": [1, 1, 3, 3]}
        )
        self.assertEqual(
            reports["one"],
            {
                "rows_upper_bound": 20,
                "row_groups_touched": 2,
                "row_groups_total": 3,
                "files_touched": 2,
                "compressed_bytes": 100,
                "uncompressed_bytes": 200,
                "row_groups_without_stats": 1,
            },
        )

    def test_union_counts_shared_row_group_once(self) -> None:
        """Several region hits on one source group must not inflate the download."""

        groups = [metadata_group("shared", 2, (1, 1, 4, 4), rows=9, selected_bytes=25)]
        reports, union = overture_estimate.summarize_regions(
            groups,
            {"west": [0, 0, 2, 2], "east": [3, 3, 5, 5]},
        )
        self.assertEqual(reports["west"]["compressed_bytes"], 25)
        self.assertEqual(reports["east"]["compressed_bytes"], 25)
        self.assertEqual(union["row_groups_touched"], 1)
        self.assertEqual(union["compressed_bytes"], 25)

    def test_gate_boundary_and_overage(self) -> None:
        """The owner-approved decimal gate stops only strictly larger estimates."""

        self.assertEqual(overture_estimate.gate_exit_code(3_000_000_000), 0)
        self.assertEqual(overture_estimate.gate_exit_code(3_000_000_001), 3)
        self.assertEqual(overture_estimate.gate_exit_code(3_000_000_001, partial=True), 0)

    def test_all_touched_groups_without_bbox_stats_is_a_failure(self) -> None:
        """Spatial themes fail loudly when multiple touched groups all lack bounds."""

        report = {"row_groups_touched": 2, "row_groups_without_stats": 2}
        self.assertTrue(overture_estimate.bbox_stats_failure(report))
        self.assertFalse(
            overture_estimate.bbox_stats_failure(
                {"row_groups_touched": 1, "row_groups_without_stats": 1}
            )
        )
        self.assertFalse(
            overture_estimate.bbox_stats_failure(
                {"row_groups_touched": 2, "row_groups_without_stats": 1}
            )
        )

    def test_admin_uses_only_national_bbox_and_not_pilot_windows(self) -> None:
        """Admin JSON must contain one pruned national estimate, not pilot entries."""

        groups = [
            metadata_group("outside", 0, (120, 30, 121, 31), selected_bytes=900),
            metadata_group("inside", 1, (105, 10, 106, 11), selected_bytes=100),
        ]
        self.assertEqual(
            overture_estimate.NATIONAL_ADMIN_BBOX,
            [100.0, 6.0, 118.0, 25.0],
        )
        regions_document = {
            "schema": 1,
            "overture_release": "test-release",
            "regions": {
                "pilot_one": [105, 10, 106, 11],
                "pilot_two": [107, 12, 108, 13],
            },
        }
        connection = mock.MagicMock()
        connection.execute.return_value = connection
        written: list[str] = []
        printed = io.StringIO()

        def capture_write(_path: Path, value: str, **_kwargs: object) -> int:
            written.append(value)
            return len(value)

        with (
            mock.patch.object(
                overture_estimate,
                "load_regions",
                return_value=(
                    regions_document["overture_release"],
                    regions_document["regions"],
                ),
            ),
            mock.patch.object(
                overture_estimate.duckdb, "connect", return_value=connection
            ),
            mock.patch.object(
                overture_estimate,
                "fetch_row_group_metadata",
                return_value=groups,
            ),
            mock.patch.object(Path, "mkdir"),
            mock.patch.object(
                Path, "write_text", autospec=True, side_effect=capture_write
            ),
            redirect_stdout(printed),
        ):
            status = overture_estimate.main(
                ["--regions", "unused.json", "--out", "unused.json", "--themes", "admin"]
            )
        output = json.loads(written[0])

        self.assertEqual(status, 0)
        self.assertEqual(output["regions"]["national"]["admin"]["compressed_bytes"], 100)
        self.assertEqual(output["theme_unions"]["admin"]["compressed_bytes"], 100)
        self.assertNotIn("admin", output["regions"]["pilot_one"])
        self.assertNotIn("admin", output["regions"]["pilot_two"])
        self.assertIn("national admin 10 1/2 1 100 200 0", printed.getvalue())
        self.assertNotIn("pilot_one admin", printed.getvalue())
        self.assertNotIn("pilot_two admin", printed.getvalue())
        self.assertIn(
            "national extent of the Vietnam sanity box",
            output["explanations"]["admin"],
        )

    def test_preset_columns_are_covered_by_estimator_mapping(self) -> None:
        """Every source column referenced by a fetch preset must be byte-accounted."""

        theme_names = {
            "transport": "roads",
            "places": "places",
            "landuse": "land_use",
            "admin": "admin",
        }
        if "buildings" in fetch_overture.PRESETS:
            theme_names["buildings"] = "buildings"
        # If absent, buildings cannot be covered because fetch_overture has no preset.
        for preset_name, theme_name in theme_names.items():
            with self.subTest(preset=preset_name):
                referenced = {
                    re.split(r"[.\[]", expression.split(" AS ", 1)[0], maxsplit=1)[0]
                    for expression in fetch_overture.PRESETS[preset_name].select
                }
                self.assertLessEqual(
                    referenced,
                    overture_estimate.SELECTED_TOP_LEVEL_COLUMNS[theme_name],
                )

    def test_regions_file_is_pinned_and_valid(self) -> None:
        """Pilot windows remain explicit CRS84 extents within the Vietnam envelope."""

        path = Path(__file__).parents[2] / "pilot" / "regions.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(value["schema"], 1)
        self.assertEqual(value["overture_release"], fetch_overture.DEFAULT_RELEASE)
        self.assertNotIn("latest", value["overture_release"].lower())
        self.assertEqual(len(value["regions"]), 5)
        for bbox in value["regions"].values():
            west, south, east, north = bbox
            self.assertLess(west, east)
            self.assertLess(south, north)
            self.assertTrue(100 <= west < east <= 118)
            self.assertTrue(6 <= south < north <= 25)


if __name__ == "__main__":
    unittest.main()
