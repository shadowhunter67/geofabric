"""Pure planning helpers for the immutable ``adaptive-quadtree-v1`` scheme."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass
import math
from numbers import Real
from typing import Any


SCHEME = "adaptive-quadtree-v1"
ROOT = (96.0, 0.0, 128.0, 32.0)
MAX_DEPTH = 13
ROOT_DEPTH = 0

_PREFIXES = frozenset(("q", "L"))


@dataclass(frozen=True, order=True)
class Leaf:
    """A non-empty planned shard and its aggregate histogram weight."""

    cell_id: str
    rows: int
    bytes: int


@dataclass(frozen=True)
class _OwnerPrefixIndex:
    cell_ids: frozenset[str]
    prefix: str
    max_depth: int


def _validate_depth(value: Any, *, name: str = "depth") -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be an integer")
    if not ROOT_DEPTH <= value <= MAX_DEPTH:
        raise ValueError(f"{name} must be between {ROOT_DEPTH} and {MAX_DEPTH}")
    return value


def _validate_cell_id(cell_id: Any) -> str:
    if not isinstance(cell_id, str):
        raise TypeError("cell_id must be a string")
    if not cell_id or cell_id[0] not in _PREFIXES:
        raise ValueError("cell_id must begin with 'q' or 'L'")
    if len(cell_id) - 1 > MAX_DEPTH:
        raise ValueError(f"cell_id exceeds maximum depth {MAX_DEPTH}")
    if any(digit not in "0123" for digit in cell_id[1:]):
        raise ValueError("cell_id digits must be in the range 0..3")
    return cell_id


def _validate_prefix(prefix: Any) -> str:
    if prefix not in _PREFIXES:
        raise ValueError("prefix must be 'q' or 'L'")
    return prefix


def _validate_count(value: Any, name: str, *, positive: bool = False) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be an integer")
    lower = 1 if positive else 0
    if value < lower:
        qualifier = "positive" if positive else "non-negative"
        raise ValueError(f"{name} must be {qualifier}")
    return value


def depth(cell_id: str) -> int:
    """Return the quadtree depth encoded by *cell_id*."""

    return len(_validate_cell_id(cell_id)) - 1


def parent(cell_id: str) -> str | None:
    """Return a cell's parent, or ``None`` for either group root."""

    cell_id = _validate_cell_id(cell_id)
    return cell_id[:-1] if len(cell_id) > 1 else None


def children(cell_id: str) -> tuple[str, str, str, str]:
    """Return the SW, SE, NW, and NE child ids, in that order."""

    cell_id = _validate_cell_id(cell_id)
    if depth(cell_id) == MAX_DEPTH:
        raise ValueError(f"a depth-{MAX_DEPTH} cell has no children in this scheme")
    return tuple(cell_id + digit for digit in "0123")  # type: ignore[return-value]


def cell_bbox(cell_id: str) -> tuple[float, float, float, float]:
    """Return the CRS84 rectangle ``(west, south, east, north)`` for a cell."""

    cell_id = _validate_cell_id(cell_id)
    west, south, east, north = ROOT
    for digit in cell_id[1:]:
        middle_x = (west + east) / 2.0
        middle_y = (south + north) / 2.0
        if digit in "02":
            east = middle_x
        else:
            west = middle_x
        if digit in "01":
            north = middle_y
        else:
            south = middle_y
    return west, south, east, north


def cell_id_for_point(x: Real, y: Real, depth: int) -> str:
    """Return the ``q`` cell containing a CRS84 point at the requested depth."""

    depth = _validate_depth(depth)
    if not isinstance(x, Real) or not isinstance(y, Real):
        raise TypeError("point coordinates must be real numbers")
    x_float = float(x)
    y_float = float(y)
    if not math.isfinite(x_float) or not math.isfinite(y_float):
        raise ValueError("point coordinates must be finite")
    west, south, east, north = ROOT
    if not (west <= x_float <= east and south <= y_float <= north):
        raise ValueError(f"point ({x}, {y}) lies outside ROOT")

    digits: list[str] = []
    for _ in range(depth):
        middle_x = (west + east) / 2.0
        middle_y = (south + north) / 2.0
        east_half = x_float >= middle_x
        north_half = y_float >= middle_y
        digits.append(str(int(east_half) + 2 * int(north_half)))
        if east_half:
            west = middle_x
        else:
            east = middle_x
        if north_half:
            south = middle_y
        else:
            north = middle_y
    return "q" + "".join(digits)


def _normalise_weights(
    weights: Mapping[str, tuple[int, int]], max_depth: int
) -> dict[str, tuple[int, int]]:
    normalised: dict[str, tuple[int, int]] = {}
    for cell_id, weight in weights.items():
        validated_id = _validate_cell_id(cell_id)
        if depth(validated_id) != max_depth:
            raise ValueError(f"weight cell {cell_id!r} is not at depth {max_depth}")
        if not isinstance(weight, (tuple, list)) or len(weight) != 2:
            raise TypeError(f"weight for {cell_id!r} must be a (rows, bytes) pair")
        rows = _validate_count(weight[0], f"rows for {cell_id!r}", positive=True)
        byte_count = _validate_count(weight[1], f"bytes for {cell_id!r}")
        path = validated_id[1:]
        if path in normalised:
            raise ValueError(f"duplicate fine cell path {path!r}")
        normalised[path] = (rows, byte_count)
    return normalised


def plan_leaves(
    weights: Mapping[str, tuple[int, int]],
    target_bytes: int,
    *,
    max_depth: int = MAX_DEPTH,
    min_split_rows: int = 2000,
    prefix: str = "q",
) -> list[Leaf]:
    """Plan non-empty leaves from a histogram of max-depth owning cells."""

    max_depth = _validate_depth(max_depth, name="max_depth")
    target_bytes = _validate_count(target_bytes, "target_bytes", positive=True)
    min_split_rows = _validate_count(min_split_rows, "min_split_rows")
    prefix = _validate_prefix(prefix)
    fine_weights = _normalise_weights(weights, max_depth)
    if not fine_weights:
        return []

    totals: dict[str, list[int]] = {}
    for path, (rows, byte_count) in fine_weights.items():
        for path_depth in range(max_depth + 1):
            ancestor = path[:path_depth]
            aggregate = totals.setdefault(ancestor, [0, 0])
            aggregate[0] += rows
            aggregate[1] += byte_count

    leaves: list[Leaf] = []

    def visit(path: str) -> None:
        rows, byte_count = totals[path]
        if (
            byte_count <= target_bytes
            or len(path) == max_depth
            or rows < min_split_rows
        ):
            leaves.append(Leaf(prefix + path, rows, byte_count))
            return
        for digit in "0123":
            child_path = path + digit
            if child_path in totals:
                visit(child_path)

    visit("")
    return sorted(leaves, key=lambda leaf: leaf.cell_id)


def _cell_ids_from_leaves(leaves: Any) -> list[str]:
    if isinstance(leaves, Mapping):
        candidates = leaves.keys()
    else:
        candidates = leaves
    cell_ids: list[str] = []
    for candidate in candidates:
        cell_id = candidate.cell_id if isinstance(candidate, Leaf) else candidate
        cell_ids.append(_validate_cell_id(cell_id))
    return cell_ids


def owner_prefix_index(leaves: Any) -> _OwnerPrefixIndex:
    """Build the compact prefix lookup used for O(depth) owner searches."""

    cell_ids = _cell_ids_from_leaves(leaves)
    if not cell_ids:
        raise ValueError("cannot index an empty leaf set")
    prefixes = {cell_id[0] for cell_id in cell_ids}
    if len(prefixes) != 1:
        raise ValueError("all indexed leaves must use the same prefix")
    return _OwnerPrefixIndex(
        frozenset(cell_ids),
        prefixes.pop(),
        max(depth(cell_id) for cell_id in cell_ids),
    )


def leaf_for_point(leaves_by_id_or_index: Any, x: Real, y: Real) -> str:
    """Find the unique owning leaf for a point, or raise ``LookupError``."""

    index = (
        leaves_by_id_or_index
        if isinstance(leaves_by_id_or_index, _OwnerPrefixIndex)
        else owner_prefix_index(leaves_by_id_or_index)
    )
    fine_id = cell_id_for_point(x, y, index.max_depth)
    path = fine_id[1:]
    for path_depth in range(index.max_depth + 1):
        candidate = index.prefix + path[:path_depth]
        if candidate in index.cell_ids:
            return candidate
    raise LookupError(f"point ({x}, {y}) is not covered by any leaf")


def assign(points: Iterable[tuple[Real, Real]], leaves: Any):
    """Yield ``(point_index, owning_cell_id)`` for an iterable of centres."""

    index = owner_prefix_index(leaves)
    for point_index, (x, y) in enumerate(points):
        yield point_index, leaf_for_point(index, x, y)


def validate_plan(
    leaves: Iterable[Leaf],
    weights: Mapping[str, tuple[int, int]],
    target_bytes: int | None = None,
    *,
    max_depth: int = MAX_DEPTH,
    min_split_rows: int = 2000,
) -> list[str]:
    """Return human-readable invariant violations for a proposed plan.

    ``target_bytes`` is optional because the work-package's two-argument API does
    not carry it.  Supplying it enables the target-size invariant check.
    """

    problems: list[str] = []
    try:
        max_depth = _validate_depth(max_depth, name="max_depth")
        min_split_rows = _validate_count(min_split_rows, "min_split_rows")
        if target_bytes is not None:
            target_bytes = _validate_count(target_bytes, "target_bytes", positive=True)
        fine_weights = _normalise_weights(weights, max_depth)
    except (TypeError, ValueError) as error:
        return [f"invalid weights or validation options: {error}"]

    proposed = list(leaves)
    valid_leaves: list[Leaf] = []
    prefixes: set[str] = set()
    seen_ids: set[str] = set()
    for position, leaf in enumerate(proposed):
        if not isinstance(leaf, Leaf):
            problems.append(f"leaf {position} is not a Leaf")
            continue
        try:
            cell_id = _validate_cell_id(leaf.cell_id)
            _validate_count(leaf.rows, f"rows for leaf {cell_id}")
            _validate_count(leaf.bytes, f"bytes for leaf {cell_id}")
        except (TypeError, ValueError) as error:
            problems.append(f"invalid leaf at position {position}: {error}")
            continue
        if depth(cell_id) > max_depth:
            problems.append(f"leaf {cell_id} is below configured max_depth {max_depth}")
        if cell_id in seen_ids:
            problems.append(f"duplicate leaf {cell_id}")
        seen_ids.add(cell_id)
        prefixes.add(cell_id[0])
        if leaf.rows == 0:
            problems.append(f"empty leaf {cell_id}")
        if (
            target_bytes is not None
            and leaf.bytes > target_bytes
            and depth(cell_id) < max_depth
            and leaf.rows >= min_split_rows
        ):
            problems.append(f"leaf {cell_id} exceeds target_bytes and should split")
        valid_leaves.append(leaf)

    if len(prefixes) > 1:
        problems.append("leaves mix q and L prefixes")

    positions_by_id: dict[str, list[int]] = {}
    for position, leaf in enumerate(valid_leaves):
        positions_by_id.setdefault(leaf.cell_id, []).append(position)

    # Find the same pairs as the nested scan by looking up only ancestors of each
    # leaf. Sorting restores the original input-pair message order.
    overlapping_pairs: set[tuple[int, int]] = set()
    for descendant_position, descendant in enumerate(valid_leaves):
        descendant_path = descendant.cell_id[1:]
        for ancestor_depth in range(len(descendant_path) + 1):
            ancestor_id = descendant.cell_id[0] + descendant_path[:ancestor_depth]
            for ancestor_position in positions_by_id.get(ancestor_id, ()):
                if ancestor_position != descendant_position:
                    overlapping_pairs.add(
                        tuple(sorted((ancestor_position, descendant_position)))
                    )
    for first_position, second_position in sorted(overlapping_pairs):
        problems.append(
            f"overlapping leaves {valid_leaves[first_position].cell_id} and "
            f"{valid_leaves[second_position].cell_id}"
        )

    expected: dict[str, list[int]] = {
        leaf.cell_id: [0, 0] for leaf in valid_leaves
    }
    counted_rows = 0
    counted_bytes = 0
    leaves_by_id: dict[str, list[Leaf]] = {}
    for leaf in valid_leaves:
        leaves_by_id.setdefault(leaf.cell_id, []).append(leaf)

    for path, (rows, byte_count) in fine_weights.items():
        counted_rows += rows
        counted_bytes += byte_count
        covering: list[Leaf] = []
        for prefix in _PREFIXES:
            for ancestor_depth in range(max_depth + 1):
                covering.extend(leaves_by_id.get(prefix + path[:ancestor_depth], ()))
        if len(covering) != 1:
            fine_id = "q" + path
            problems.append(
                f"fine cell {fine_id} is covered by {len(covering)} leaves (expected 1)"
            )
        for leaf in covering:
            aggregate = expected[leaf.cell_id]
            aggregate[0] += rows
            aggregate[1] += byte_count

    for leaf in valid_leaves:
        rows, byte_count = expected[leaf.cell_id]
        if leaf.rows != rows or leaf.bytes != byte_count:
            problems.append(
                f"leaf {leaf.cell_id} declares ({leaf.rows}, {leaf.bytes}) but "
                f"covers ({rows}, {byte_count})"
            )

    declared_rows = sum(leaf.rows for leaf in valid_leaves)
    declared_bytes = sum(leaf.bytes for leaf in valid_leaves)
    if declared_rows != counted_rows:
        problems.append(
            f"row totals are not conserved: {declared_rows} != {counted_rows}"
        )
    if declared_bytes != counted_bytes:
        problems.append(
            f"byte totals are not conserved: {declared_bytes} != {counted_bytes}"
        )
    return problems


def oversize_split(features_extent_deg: Real, threshold: Real | None) -> bool:
    """Return whether a feature's maximum bbox extent belongs in group ``L``."""

    if threshold is None or threshold == 0:
        return False
    if not isinstance(features_extent_deg, Real) or not isinstance(threshold, Real):
        raise TypeError("extent and threshold must be real numbers")
    extent = float(features_extent_deg)
    limit = float(threshold)
    if not math.isfinite(extent) or not math.isfinite(limit):
        raise ValueError("extent and threshold must be finite")
    if extent < 0 or limit < 0:
        raise ValueError("extent and threshold must be non-negative")
    return extent > limit
