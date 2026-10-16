"""Offline tests for manifest validation and command-level manifest behavior."""

from __future__ import annotations

import contextlib
import io
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[1]))

import geo_data
import manifest


class ManifestTest(unittest.TestCase):
    """Exercise invalid lineage inputs because validation protects reproducibility."""

    def base_manifest(self) -> dict:
        """Read the committed example so tests use the documented complete shape."""

        path = manifest.ROOT / "data" / "manifests" / "example.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def test_example_manifest_is_valid_and_matches_its_filename(self) -> None:
        """The committed example must demonstrate an accepted manifest."""

        example = self.base_manifest()
        manifest.validate(example, "example")

    def test_validation_rejections_name_the_manifest_and_field(self) -> None:
        """Each required validation failure must identify actionable provenance context."""

        cases = [
            ("unknown", lambda value: value.update({"unexpected": True}), "unexpected"),
            (
                "unclear",
                lambda value: value["license"].update({"redistribution": "unclear"}),
                "license.redistribution",
            ),
            (
                "latest",
                lambda value: value["source"].update({"release": "latest"}),
                "source.release",
            ),
            (
                "bad_hash",
                lambda value: value.update(
                    {
                        "files": [
                            {
                                "path": "data/local/example.parquet",
                                "size_bytes": 1,
                                "sha256": "not-a-sha256",
                                "rows": 1,
                                "id_digest": None,
                            }
                        ]
                    }
                ),
                "files[0].sha256",
            ),
            (
                "bad_bbox",
                lambda value: value["coverage"].update({"bbox": [102, 8, 102, 24]}),
                "coverage.bbox",
            ),
        ]

        for name, mutate, field in cases:
            with self.subTest(name=name):
                value = self.base_manifest()
                mutate(value)
                with self.assertRaisesRegex(ValueError, re.escape(field)) as raised:
                    manifest.validate(value, "example")
                self.assertIn("example:", str(raised.exception))

    def test_validate_command_collects_all_invalid_manifests(self) -> None:
        """Validation must report every bad manifest instead of stopping at the first."""

        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            first = self.base_manifest()
            first["id"] = "first_bad"
            first["source"]["release"] = "latest"
            (directory / "first_bad.json").write_text(json.dumps(first), encoding="utf-8")

            second = self.base_manifest()
            second["id"] = "second_bad"
            second["license"]["redistribution"] = "unclear"
            (directory / "second_bad.json").write_text(json.dumps(second), encoding="utf-8")

            stderr = io.StringIO()
            with patch.object(geo_data, "MANIFESTS", directory), contextlib.redirect_stderr(stderr):
                self.assertEqual(geo_data.command_validate(object()), 1)

            output = stderr.getvalue()
            self.assertIn("first_bad: source.release", output)
            self.assertIn("second_bad: license.redistribution", output)

    def test_bootstrap_is_group_then_id_ordered(self) -> None:
        """Bootstrap output must be stable for reproducible operator review."""

        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for dataset_id, group in (
                ("z_places", "places"),
                ("b_base", "base"),
                ("a_base", "base"),
            ):
                value = self.base_manifest()
                value["id"] = dataset_id
                value["group"] = group
                value["fetch"] = {"script": f"fetch_{dataset_id}.py", "args": ["--offline"]}
                (directory / f"{dataset_id}.json").write_text(
                    json.dumps(value),
                    encoding="utf-8",
                )

            stdout = io.StringIO()
            args = type("Args", (), {"region": "Vietnam Demo", "run": False})()
            with patch.object(geo_data, "MANIFESTS", directory), contextlib.redirect_stdout(stdout):
                self.assertEqual(geo_data.command_bootstrap(args), 0)

            self.assertEqual(
                stdout.getvalue().splitlines(),
                [
                    "a_base: fetch_a_base.py --offline",
                    "b_base: fetch_b_base.py --offline",
                    "z_places: fetch_z_places.py --offline",
                ],
            )

    def test_bootstrap_run_is_explicitly_unimplemented(self) -> None:
        """M1 must refuse network execution even when a user adds --run."""

        args = type("Args", (), {"region": "Vietnam Demo", "run": True})()
        with self.assertRaisesRegex(ValueError, "not implemented in M1"):
            geo_data.command_bootstrap(args)

    def test_gitignore_keeps_manifests_tracked_and_local_ignored(self) -> None:
        """The data framework needs committed manifests but untracked downloaded data."""

        local = subprocess.run(
            ["git", "check-ignore", "data/local/probe"],
            cwd=manifest.ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        tracked = subprocess.run(
            ["git", "check-ignore", "data/manifests/example.json"],
            cwd=manifest.ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(local.returncode, 0)
        self.assertNotEqual(tracked.returncode, 0)


if __name__ == "__main__":
    unittest.main()
