"""Offline behavior tests for the adaptive quadtree planner."""

from __future__ import annotations

from collections import Counter
import random
import statistics
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

import quadtree_partition as quadtree


def histogram(points, depth=quadtree.MAX_DEPTH, bytes_per_row=1000):
    counts = Counter(quadtree.cell_id_for_point(x, y, depth) for x, y in points)
    return {
        cell_id: (rows, rows * bytes_per_row)
        for cell_id, rows in counts.items()
    }


def contains(cell_id, point):
    west, south, east, north = quadtree.cell_bbox(cell_id)
    x, y = point
    return (
        west <= x < east or (east == quadtree.ROOT[2] and x == east)
    ) and (
        south <= y < north or (north == quadtree.ROOT[3] and y == north)
    )


class QuadtreePartitionTest(unittest.TestCase):
    def test_coverage_exactly_one_owner_and_assign_matches_brute_force(self):
        rng = random.Random(8675309)
        west, south, east, north = quadtree.ROOT
        points = [
            (rng.uniform(west, east), rng.uniform(south, north))
            for _ in range(20_000)
        ]
        for level in (1, 2, 5, 9):
            step = 32.0 / (2**level)
            points.extend(
                [
                    (west + step, south + step / 3),
                    (west + step / 3, south + step),
                ]
            )
        points.extend([(east, north), (east, south), (west, north)])
        weights = histogram(points)
        leaves = quadtree.plan_leaves(weights, 250_000, min_split_rows=1)
        index = quadtree.owner_prefix_index(leaves)

        assigned = list(quadtree.assign(points, leaves))
        self.assertEqual(len(assigned), len(points))
        for (point_index, owner), point in zip(assigned, points):
            self.assertEqual(point_index, assigned[point_index][0])
            brute_force = [leaf.cell_id for leaf in leaves if contains(leaf.cell_id, point)]
            self.assertEqual(brute_force, [owner])
            self.assertEqual(quadtree.leaf_for_point(index, *point), owner)
        self.assertEqual(quadtree.validate_plan(leaves, weights, 250_000, min_split_rows=1), [])

    def test_border_rule_and_outside_root(self):
        self.assertEqual(quadtree.cell_id_for_point(112.0, 8.0, 1), "q1")
        self.assertEqual(quadtree.cell_id_for_point(104.0, 16.0, 1), "q2")
        self.assertEqual(quadtree.cell_id_for_point(112.0, 16.0, 1), "q3")
        self.assertEqual(quadtree.cell_id_for_point(128.0, 32.0, 2), "q33")
        for point in ((95.999, 1), (128.001, 1), (100, -0.001), (100, 32.001)):
            with self.subTest(point=point), self.assertRaises(ValueError):
                quadtree.cell_id_for_point(*point, 4)

    def test_clustered_balance(self):
        rng = random.Random(20261016)
        points = []
        for centre_x, centre_y, count, spread in (
            (105.0, 10.0, 10_000, 0.8),
            (119.0, 25.0, 8_000, 1.1),
        ):
            for _ in range(count):
                points.append(
                    (
                        min(127.999, max(96.0, rng.gauss(centre_x, spread))),
                        min(31.999, max(0.0, rng.gauss(centre_y, spread))),
                    )
                )
        points.extend((rng.uniform(96, 128), rng.uniform(0, 32)) for _ in range(2_000))
        weights = histogram(points, bytes_per_row=10_000)
        target = 25_000_000
        leaves = quadtree.plan_leaves(weights, target, min_split_rows=1)

        self.assertEqual(quadtree.validate_plan(leaves, weights, target, min_split_rows=1), [])
        self.assertTrue(all(leaf.rows and leaf.bytes <= target for leaf in leaves))
        lower_bound = sum(value[1] for value in weights.values()) / target
        self.assertLessEqual(len(leaves), 4 * lower_bound)
        self.assertGreaterEqual(statistics.median(leaf.bytes for leaf in leaves), target * 0.2)

    def test_sparse_cluster_does_not_create_empty_shards(self):
        rng = random.Random(7)
        points = [(126 + rng.random() * 0.001, 2 + rng.random() * 0.001) for _ in range(300)]
        weights = histogram(points, bytes_per_row=100_000)
        leaves = quadtree.plan_leaves(weights, 1_000_000)
        self.assertLessEqual(len(leaves), 4)
        self.assertTrue(all(leaf.rows > 0 for leaf in leaves))

    def test_pathological_inputs_terminate_without_explosion(self):
        cell_id = quadtree.cell_id_for_point(100.25, 4.75, quadtree.MAX_DEPTH)
        leaves = quadtree.plan_leaves({cell_id: (50_000, 500_000_000)}, 1_000_000)
        self.assertEqual(leaves, [quadtree.Leaf(cell_id, 50_000, 500_000_000)])

        small = {cell_id: (1_500, 500_000_000)}
        self.assertEqual(
            quadtree.plan_leaves(small, 1_000_000),
            [quadtree.Leaf("q", 1_500, 500_000_000)],
        )

    def test_determinism_and_large_feature_prefix(self):
        rng = random.Random(88)
        points = [(rng.uniform(96, 128), rng.uniform(0, 32)) for _ in range(5_000)]
        weights = histogram(points, bytes_per_row=10_000)
        shuffled_items = list(weights.items())
        rng.shuffle(shuffled_items)
        first = quadtree.plan_leaves(weights, 1_000_000, min_split_rows=1)
        second = quadtree.plan_leaves(dict(shuffled_items), 1_000_000, min_split_rows=1)
        large = quadtree.plan_leaves(weights, 1_000_000, min_split_rows=1, prefix="L")
        self.assertEqual(first, second)
        self.assertEqual(
            [(leaf.cell_id[1:], leaf.rows, leaf.bytes) for leaf in first],
            [(leaf.cell_id[1:], leaf.rows, leaf.bytes) for leaf in large],
        )

    def test_ids_round_trip_and_children_partition_parent(self):
        rng = random.Random(42)
        for level in range(quadtree.MAX_DEPTH + 1):
            point = (rng.uniform(96, 128), rng.uniform(0, 32))
            cell_id = quadtree.cell_id_for_point(*point, level)
            self.assertEqual(quadtree.depth(cell_id), level)
            self.assertTrue(contains(cell_id, point))
            if level:
                self.assertIn(cell_id, quadtree.children(quadtree.parent(cell_id)))

        for cell_id in ("q", "q0", "L123", "q3012"):
            child_ids = quadtree.children(cell_id)
            parent_area = self.area(quadtree.cell_bbox(cell_id))
            self.assertAlmostEqual(sum(self.area(quadtree.cell_bbox(child)) for child in child_ids), parent_area)
            self.assertTrue(all(quadtree.parent(child) == cell_id for child in child_ids))

        self.assertIsNone(quadtree.parent("q"))
        for invalid in ("", "x0", "q4", "Q1", "q-1", "q" + "0" * 14):
            with self.subTest(invalid=invalid), self.assertRaises((TypeError, ValueError)):
                quadtree.cell_bbox(invalid)

    def test_validate_plan_reports_constructed_defects(self):
        left = "q" + "0" * quadtree.MAX_DEPTH
        right = "q" + "1" * quadtree.MAX_DEPTH
        weights = {left: (3_000, 30_000), right: (3_000, 30_000)}
        valid = quadtree.plan_leaves(weights, 20_000, min_split_rows=1)
        self.assertEqual(quadtree.validate_plan(valid, weights, 20_000, min_split_rows=1), [])

        defects = {
            "overlap": valid + [quadtree.Leaf("q0", 3_000, 30_000)],
            "uncovered": [leaf for leaf in valid if leaf.cell_id != right],
            "above target": [quadtree.Leaf("q0", 3_000, 30_000), valid[1]],
            "row loss": [quadtree.Leaf(valid[0].cell_id, valid[0].rows - 1, valid[0].bytes), valid[1]],
        }
        expected_words = {
            "overlap": "overlapping",
            "uncovered": "covered by 0",
            "above target": "exceeds target_bytes",
            "row loss": "row totals",
        }
        for name, broken in defects.items():
            with self.subTest(name=name):
                messages = quadtree.validate_plan(broken, weights, 20_000, min_split_rows=1)
                self.assertTrue(any(expected_words[name] in message for message in messages), messages)

    def test_oversize_predicate_boundaries_and_disabled_threshold(self):
        self.assertFalse(quadtree.oversize_split(2.0, 2.0))
        self.assertTrue(quadtree.oversize_split(2.00001, 2.0))
        self.assertFalse(quadtree.oversize_split(1_000.0, 0))
        self.assertFalse(quadtree.oversize_split(1_000.0, None))

    @staticmethod
    def area(bbox):
        west, south, east, north = bbox
        return (east - west) * (north - south)


if __name__ == "__main__":
    unittest.main()
