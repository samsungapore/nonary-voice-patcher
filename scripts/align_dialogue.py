#!/usr/bin/env python3
"""Align PC 999 voice message IDs with Nintendo DS ``setText`` calls.

Pass one preserves high-signal mappings: unique normalized-text anchors are
made monotonic with a longest-increasing subsequence, then gaps are aligned by
a weighted character TF-IDF LCS while requiring identical speakers.

Pass two assigns remaining PC messages monotonically between the fixed
pass-one anchors. It spreads them proportionally over the available DS lines,
subject to a configurable hard per-line cap. If an interval has more messages
than capacity, its evenly distributed overflow is written to a separate reject
file instead of being stacked onto one DS ``setText``. Inferred rows are
deliberately marked ``sequence_fallback`` with ``low`` confidence.
"""

from __future__ import annotations

import argparse
import bisect
import csv
import difflib
import re
import sys
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

try:
    import numpy as np
    from sklearn.feature_extraction.text import TfidfVectorizer
except ImportError as error:  # pragma: no cover - depends on local environment
    raise SystemExit(
        "align_dialogue.py requires numpy and scikit-learn "
        "(for example: python3 -m pip install numpy scikit-learn)"
    ) from error


SET_SPEAKER = re.compile(r'^\s*setSpeaker\("(.*)"\);\s*$')
SET_TEXT = re.compile(r'^\s*setText\("(.*)"\);\s*$')
DEBUG_EXPORT = re.compile(r'^\s*export\s+"DEBUG_([^"]+)"\s*:\s*$')
NATURAL_PART = re.compile(r"(\d+)")
CONTROL = re.compile(r"⑳(?:[nN]|[CcSsFf][0-9.]+)")
NON_TEXT = re.compile(r"[^\w\s\'\"!?.,:;()\[\]{}<>/\\+*=#%&@\-$]")


def normalize_text(value: str) -> str:
    """Normalize custom glyph/control encodings shared by PC and DS scripts."""
    value = (
        value.replace(r'\"', '"')
        .replace("Ｓ", "'")
        .replace("Ｄ", '"')
        .replace("⑲", " ")
        .replace("▼", " ")
    )
    value = CONTROL.sub(" ", value).replace("⑳", " ").replace("│", " ")
    value = unicodedata.normalize("NFKC", value).lower()
    value = (
        value.replace("’", "'")
        .replace("‘", "'")
        .replace("“", '"')
        .replace("”", '"')
        .replace("…", "...")
        .replace("――", "--")
        .replace("—", "--")
        .replace("–", "-")
    )
    value = NON_TEXT.sub(" ", value)
    return re.sub(r"\s+", " ", value).strip()


def natural_key(value: str) -> list[Any]:
    return [
        int(part) if part.isdigit() else part.lower()
        for part in NATURAL_PART.split(value)
    ]


def split_speaker_text(value: str) -> tuple[str, str]:
    first_variant = value.split(" || ", 1)[0]
    speaker, separator, text = first_variant.partition(" :: ")
    return (speaker, text) if separator else ("", "")


def message_scene(message_id: str) -> str:
    """Return the scene field (the first numeric field after the stem)."""
    parts = message_id.split("_")
    if len(parts) < 2 or not parts[1].isdigit():
        raise ValueError(f"Cannot determine scene from message ID: {message_id}")
    return parts[1]


def read_pc_manifest(path: Path) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, dict[str, dict[str, Any]]] = defaultdict(dict)
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if not reader.fieldnames:
            raise ValueError(f"Empty manifest: {path}")
        en_column = next(
            (
                name
                for name in (
                    "en_speaker_and_text_cp932",
                    "en_speaker_cp932_and_text_cp1252",
                )
                if name in reader.fieldnames
            ),
            None,
        )
        required = {"message_id", "jp_ogg_path", "jp_speaker_and_text_cp932"}
        missing = required - set(reader.fieldnames)
        if missing or en_column is None:
            raise ValueError(
                f"Manifest lacks columns: {sorted(missing)}"
                + (" and an English text column" if en_column is None else "")
            )
        for row in reader:
            message_id = row["message_id"]
            group = message_id.split("_", 1)[0].lower()
            item = grouped[group].setdefault(
                message_id,
                {
                    "id": message_id,
                    "scene": message_scene(message_id),
                    "paths": [],
                    "jp": row["jp_speaker_and_text_cp932"],
                    "en": row[en_column],
                },
            )
            item["paths"].append(row["jp_ogg_path"])

    result: dict[str, list[dict[str, Any]]] = {}
    for group, messages in grouped.items():
        ordered = []
        for _, item in sorted(messages.items(), key=lambda pair: natural_key(pair[0])):
            en_speaker, en_text = split_speaker_text(item["en"])
            jp_speaker, _ = split_speaker_text(item["jp"])
            item.update(
                {
                    "speaker": en_speaker or jp_speaker,
                    "en_text": en_text,
                    "norm": normalize_text(en_text),
                    "paths": sorted(set(item["paths"])),
                }
            )
            ordered.append(item)
        result[group] = ordered
    return result


def read_ds_scripts(
    directory: Path,
) -> tuple[dict[str, list[dict[str, Any]]], dict[str, dict[str, int]]]:
    scripts: dict[str, list[dict[str, Any]]] = {}
    scene_starts: dict[str, dict[str, int]] = {}
    for path in sorted(directory.glob("*.fsb.txt")):
        group = path.name[: -len(".fsb.txt")].lower()
        speaker = ""
        records = []
        starts: dict[str, int] = {}
        for line_number, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), 1
        ):
            match = DEBUG_EXPORT.match(line)
            if match:
                debug_parts = match.group(1).split("_")
                # A scene boundary must have DEBUG_<stem>_<scene>_<...>.
                # Short labels such as DEBUG_Aed3_010 are section markers, not
                # the scene exports used by PC message IDs.
                if (
                    len(debug_parts) >= 3
                    and debug_parts[0].lower() == group
                    and debug_parts[1].isdigit()
                ):
                    starts.setdefault(debug_parts[1], len(records))
            match = SET_SPEAKER.match(line)
            if match:
                speaker = match.group(1).lstrip("&")
            match = SET_TEXT.match(line)
            if match:
                text = match.group(1)
                records.append(
                    {
                        "line": line_number,
                        "ordinal": len(records),
                        "speaker": speaker,
                        "text": text,
                        "norm": normalize_text(text),
                    }
                )
        scripts[group] = records
        scene_starts[group] = starts
    return scripts, scene_starts


def longest_increasing_pairs(pairs: list[tuple[int, int]]) -> list[tuple[int, int]]:
    tails: list[int] = []
    tail_indices: list[int] = []
    previous = [-1] * len(pairs)
    for index, (_, ds_index) in enumerate(pairs):
        level = bisect.bisect_left(tails, ds_index)
        if level == len(tails):
            tails.append(ds_index)
            tail_indices.append(index)
        else:
            tails[level] = ds_index
            tail_indices[level] = index
        if level:
            previous[index] = tail_indices[level - 1]
    if not tail_indices:
        return []
    index = tail_indices[-1]
    result = []
    while index >= 0:
        result.append(pairs[index])
        index = previous[index]
    return result[::-1]


def weighted_gap_alignment(
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    similarity: np.ndarray,
    pc_start: int,
    pc_stop: int,
    ds_start: int,
    ds_stop: int,
    threshold: float,
) -> list[tuple[int, int, float]]:
    """Maximum-weight monotonic one-to-one matching for one anchored gap."""
    pc_count = pc_stop - pc_start
    ds_count = ds_stop - ds_start
    score = np.zeros((pc_count + 1, ds_count + 1), dtype=np.float32)
    back = np.zeros((pc_count + 1, ds_count + 1), dtype=np.uint8)
    for pc_offset in range(1, pc_count + 1):
        for ds_offset in range(1, ds_count + 1):
            if score[pc_offset - 1, ds_offset] >= score[pc_offset, ds_offset - 1]:
                best = score[pc_offset - 1, ds_offset]
                operation = 1
            else:
                best = score[pc_offset, ds_offset - 1]
                operation = 2
            pc_index = pc_start + pc_offset - 1
            ds_index = ds_start + ds_offset - 1
            text_score = similarity[pc_index, ds_index]
            candidate = score[pc_offset - 1, ds_offset - 1] + text_score * text_score
            if (
                pc[pc_index]["speaker"] == ds[ds_index]["speaker"]
                and text_score >= threshold
                and candidate > best
            ):
                best = candidate
                operation = 3
            score[pc_offset, ds_offset] = best
            back[pc_offset, ds_offset] = operation

    result = []
    pc_offset, ds_offset = pc_count, ds_count
    while pc_offset and ds_offset:
        operation = back[pc_offset, ds_offset]
        if operation == 3:
            pc_index = pc_start + pc_offset - 1
            ds_index = ds_start + ds_offset - 1
            result.append(
                (pc_index, ds_index, float(similarity[pc_index, ds_index]))
            )
            pc_offset -= 1
            ds_offset -= 1
        elif operation == 1:
            pc_offset -= 1
        else:
            ds_offset -= 1
    return result[::-1]


def initial_alignment(
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    threshold: float,
) -> tuple[dict[int, dict[str, Any]], np.ndarray, dict[int, int]]:
    """Return pass-one mappings keyed by full PC index."""
    textual_full_indices = [index for index, item in enumerate(pc) if item["en_text"]]
    textual_pc = [pc[index] for index in textual_full_indices]
    full_to_textual = {
        full_index: textual_index
        for textual_index, full_index in enumerate(textual_full_indices)
    }
    if not textual_pc or not ds:
        return {}, np.zeros((len(textual_pc), len(ds))), full_to_textual

    vectorizer = TfidfVectorizer(
        analyzer="char_wb", ngram_range=(2, 5), sublinear_tf=True, norm="l2"
    )
    matrix = vectorizer.fit_transform(
        [item["norm"] or " " for item in textual_pc + ds]
    )
    similarity = (matrix[: len(textual_pc)] @ matrix[len(textual_pc) :].T).toarray()

    pc_counts = Counter((item["speaker"], item["norm"]) for item in textual_pc)
    ds_counts = Counter((item["speaker"], item["norm"]) for item in ds)
    unique_ds = {
        (item["speaker"], item["norm"]): index
        for index, item in enumerate(ds)
        if ds_counts[(item["speaker"], item["norm"])] == 1
    }
    candidates = [
        (index, unique_ds[(item["speaker"], item["norm"])])
        for index, item in enumerate(textual_pc)
        if pc_counts[(item["speaker"], item["norm"])] == 1
        and ds_counts[(item["speaker"], item["norm"])] == 1
    ]
    anchors = longest_increasing_pairs(candidates)
    mapped: dict[int, dict[str, Any]] = {
        pc_index: {
            "ds_index": ds_index,
            "similarity": 1.0,
            "method": "unique_exact_anchor",
        }
        for pc_index, ds_index in anchors
    }
    bounds = [(-1, -1), *anchors, (len(textual_pc), len(ds))]
    for (pc_left, ds_left), (pc_right, ds_right) in zip(bounds, bounds[1:]):
        for pc_index, ds_index, text_score in weighted_gap_alignment(
            textual_pc,
            ds,
            similarity,
            pc_left + 1,
            pc_right,
            ds_left + 1,
            ds_right,
            threshold,
        ):
            mapped[pc_index] = {
                "ds_index": ds_index,
                "similarity": text_score,
                "method": "monotonic_tfidf",
            }

    full_mapped = {
        textual_full_indices[textual_index]: data
        for textual_index, data in mapped.items()
    }
    return full_mapped, similarity, full_to_textual


def merge_fixed_mappings(
    preferred: dict[int, dict[str, Any]],
    candidates: dict[int, dict[str, Any]],
) -> dict[int, dict[str, Any]]:
    """Keep preferred anchors and add compatible monotonic candidates."""
    preferred_pairs = sorted(
        (pc_index, data["ds_index"]) for pc_index, data in preferred.items()
    )
    if any(
        left_pc >= right_pc or left_ds >= right_ds
        for (left_pc, left_ds), (right_pc, right_ds) in zip(
            preferred_pairs, preferred_pairs[1:]
        )
    ):
        raise ValueError("Preferred scene anchors are not strictly monotonic")

    result = dict(preferred)
    preferred_pc = [pc_index for pc_index, _ in preferred_pairs]
    for pc_index, data in sorted(candidates.items()):
        if pc_index in result:
            continue
        position = bisect.bisect_left(preferred_pc, pc_index)
        previous_pc, previous_ds = (
            preferred_pairs[position - 1] if position else (-1, -1)
        )
        next_pc, next_ds = (
            preferred_pairs[position]
            if position < len(preferred_pairs)
            else (sys.maxsize, sys.maxsize)
        )
        ds_index = data["ds_index"]
        if previous_pc < pc_index < next_pc and previous_ds < ds_index < next_ds:
            result[pc_index] = data

    pairs = sorted((pc_index, data["ds_index"]) for pc_index, data in result.items())
    if any(
        left_ds >= right_ds
        for (_, left_ds), (_, right_ds) in zip(pairs, pairs[1:])
    ):
        raise AssertionError("Merged scene anchors are not strictly monotonic")
    return result


def infer_scene_ranges(
    pc: list[dict[str, Any]],
    ds_count: int,
    explicit_starts: dict[str, int],
    global_fixed: dict[int, dict[str, Any]],
) -> list[dict[str, Any]]:
    """Build disjoint DS ranges for PC scenes.

    DEBUG exports are authoritative. Missing scene starts use the first exact
    global anchor that fits between neighbouring exports; any remaining holes
    are interpolated between the nearest known boundaries.
    """
    scene_order: list[str] = []
    scene_pc_indices: dict[str, list[int]] = defaultdict(list)
    for pc_index, item in enumerate(pc):
        scene = item["scene"]
        if scene not in scene_pc_indices:
            scene_order.append(scene)
        scene_pc_indices[scene].append(pc_index)

    starts: dict[int, int] = {}
    sources: dict[int, str] = {}
    for scene_index, scene in enumerate(scene_order):
        if scene in explicit_starts:
            starts[scene_index] = explicit_starts[scene]
            sources[scene_index] = "debug_export"

    fixed_by_scene: dict[str, list[int]] = defaultdict(list)
    exact_by_scene: dict[str, list[int]] = defaultdict(list)
    for pc_index, data in global_fixed.items():
        fixed_by_scene[pc[pc_index]["scene"]].append(data["ds_index"])
        if data["method"] == "unique_exact_anchor":
            exact_by_scene[pc[pc_index]["scene"]].append(data["ds_index"])

    # Some DS DEBUG exports mark a later subsection rather than the semantic
    # beginning represented by the corresponding PC scene. A unique exact text
    # anchor before the export is stronger evidence for that earlier boundary
    # (for example Bed3_030 begins semantically at ordinal 134, while its DEBUG
    # export appears at 185). Never move a boundary later than its export.
    for scene_index, scene in enumerate(scene_order):
        exact_indices = exact_by_scene.get(scene)
        if scene_index not in starts or not exact_indices:
            continue
        exact_start = min(exact_indices)
        if exact_start < starts[scene_index]:
            starts[scene_index] = exact_start
            sources[scene_index] = "exact_anchor_before_debug"

    # A missing DEBUG scene uses its first exact anchor. Unlike numeric message
    # IDs, physical scene order is not always increasing (m30a is one example),
    # so ranges are ordered by their resulting DS ordinal below.
    for scene_index, scene in enumerate(scene_order):
        if scene_index in starts or not exact_by_scene.get(scene):
            continue
        candidates = [
            ds_index
            for ds_index in exact_by_scene[scene]
            if 0 <= ds_index < ds_count
        ]
        if candidates:
            starts[scene_index] = min(candidates)
            sources[scene_index] = "exact_anchor"

    known = sorted(starts.items())
    if not known:
        for scene_index in range(len(scene_order)):
            starts[scene_index] = (
                scene_index * ds_count // max(1, len(scene_order))
            )
            sources[scene_index] = "interpolated"
    else:
        first_index, first_start = known[0]
        for scene_index in range(first_index):
            starts[scene_index] = (
                scene_index * first_start // max(1, first_index)
            )
            sources[scene_index] = "interpolated"
        for (left_index, left_start), (right_index, right_start) in zip(
            known, known[1:]
        ):
            distance = right_index - left_index
            for scene_index in range(left_index + 1, right_index):
                if right_start >= left_start:
                    starts[scene_index] = left_start + (
                        (scene_index - left_index) * (right_start - left_start)
                        // distance
                    )
                else:
                    # Numeric PC order is incompatible with physical DS order.
                    # With no export or exact anchor there is no defensible
                    # positive-width range, so give the scene an empty boundary;
                    # its IDs will be explicit rejects rather than misassigned.
                    starts[scene_index] = left_start
                sources[scene_index] = "interpolated"
        last_index, last_start = known[-1]
        distance = len(scene_order) - last_index
        for scene_index in range(last_index + 1, len(scene_order)):
            starts[scene_index] = last_start + (
                (scene_index - last_index) * (ds_count - last_start)
                // distance
            )
            sources[scene_index] = "interpolated"

    # DEBUG ranges follow physical script order, not numeric PC scene order.
    physical_order = sorted(
        range(len(scene_order)),
        key=lambda index: (starts[index], natural_key(scene_order[index])),
    )
    # Refine each boundary within the feasible interval between the last
    # pass-one anchor on its left and the first pass-one anchor on its right.
    # This retains semantically strong boundary anchors without creating
    # overlapping scene ranges. In physically reordered scripts, incompatible
    # anchor spans simply leave the DEBUG-derived boundary unchanged.
    for left_index, right_index in zip(physical_order, physical_order[1:]):
        left_scene = scene_order[left_index]
        right_scene = scene_order[right_index]
        left_anchors = fixed_by_scene.get(left_scene, [])
        right_anchors = fixed_by_scene.get(right_scene, [])
        if not left_anchors or not right_anchors:
            continue
        lower = max(left_anchors) + 1
        upper = min(right_anchors)
        if lower <= upper:
            boundary = max(lower, min(starts[right_index], upper))
            if boundary != starts[right_index]:
                starts[right_index] = boundary
                sources[right_index] = "semantic_anchor_boundary"

    stops: dict[int, int] = {}
    for physical_index, scene_index in enumerate(physical_order):
        stops[scene_index] = (
            starts[physical_order[physical_index + 1]]
            if physical_index + 1 < len(physical_order)
            else ds_count
        )

    ranges = []
    for scene_index, scene in enumerate(scene_order):
        ranges.append(
            {
                "scene": scene,
                "pc_indices": scene_pc_indices[scene],
                "ds_start": starts[scene_index],
                "ds_stop": stops[scene_index],
                "source": sources[scene_index],
            }
        )
    return ranges


def fallback_alignment(
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    fixed: dict[int, dict[str, Any]],
    similarity: np.ndarray,
    full_to_textual: dict[int, int],
    max_per_line: int,
) -> tuple[dict[int, dict[str, Any]], dict[int, dict[str, Any]]]:
    """Fill PC gaps proportionally without exceeding ``max_per_line``.

    Fixed mappings split both sequences into intervals. A fixed DS line's
    residual capacity may be used by either adjacent interval, so a small
    dynamic program allocates that shared capacity to maximize total coverage.
    Overflow is returned separately as explicit rejects.
    """
    if not ds:
        raise ValueError("Cannot sequence-align against a DS script with no setText calls")
    if max_per_line < 1:
        raise ValueError("max_per_line must be at least 1")

    mapped = dict(fixed)
    fixed_pairs = sorted(
        (pc_index, data["ds_index"]) for pc_index, data in fixed.items()
    )
    fixed_occupancy = Counter(ds_index for _, ds_index in fixed_pairs)
    overfull_fixed = {
        ds_index: count
        for ds_index, count in fixed_occupancy.items()
        if count > max_per_line
    }
    if overfull_fixed:
        raise ValueError(
            "Pass-one mappings already exceed max_per_line: "
            + ", ".join(
                f"DS ordinal {ds_index}={count}"
                for ds_index, count in sorted(overfull_fixed.items())
            )
        )

    # There is one PC gap before the first fixed pair, one between every pair,
    # and one after the last. Interior DS lines belong exclusively to a gap;
    # residual capacity on each fixed line is shared by its two neighbours.
    pc_bounds = [-1, *(pc_index for pc_index, _ in fixed_pairs), len(pc)]
    ds_bounds = [-1, *(ds_index for _, ds_index in fixed_pairs), len(ds)]
    interval_pc = [
        list(range(pc_bounds[index] + 1, pc_bounds[index + 1]))
        for index in range(len(pc_bounds) - 1)
    ]
    interval_ds = [
        list(range(ds_bounds[index] + 1, ds_bounds[index + 1]))
        for index in range(len(ds_bounds) - 1)
    ]
    demands = [len(indices) for indices in interval_pc]
    exclusive_capacity = [len(indices) * max_per_line for indices in interval_ds]
    anchor_capacity = [
        max_per_line - fixed_occupancy[ds_index]
        for _, ds_index in fixed_pairs
    ]

    # State key: capacity from the previous anchor available to this interval.
    # State value: (covered, negative squared rejects, choices). The secondary
    # objective avoids concentrating rejects when cardinality ties.
    states: dict[int, tuple[int, int, tuple[int, ...]]] = {0: (0, 0, ())}
    for interval_index, residual in enumerate(anchor_capacity):
        next_states: dict[int, tuple[int, int, tuple[int, ...]]] = {}
        for left_available, (covered, reject_score, choices) in states.items():
            for assigned_to_left in range(residual + 1):
                capacity = (
                    exclusive_capacity[interval_index]
                    + left_available
                    + assigned_to_left
                )
                interval_covered = min(demands[interval_index], capacity)
                interval_rejected = demands[interval_index] - interval_covered
                candidate = (
                    covered + interval_covered,
                    reject_score - interval_rejected * interval_rejected,
                    choices + (assigned_to_left,),
                )
                available_to_right = residual - assigned_to_left
                previous = next_states.get(available_to_right)
                if previous is None or candidate[:2] > previous[:2]:
                    next_states[available_to_right] = candidate
        states = next_states

    best_state: tuple[int, int, tuple[int, ...]] | None = None
    for left_available, (covered, reject_score, choices) in states.items():
        capacity = exclusive_capacity[-1] + left_available
        interval_covered = min(demands[-1], capacity)
        interval_rejected = demands[-1] - interval_covered
        candidate = (
            covered + interval_covered,
            reject_score - interval_rejected * interval_rejected,
            choices,
        )
        if best_state is None or candidate[:2] > best_state[:2]:
            best_state = candidate
    if best_state is None:  # pragma: no cover - states always contains one item
        raise AssertionError("Could not allocate fallback capacity")
    assigned_to_left = best_state[2]

    def proportional_indices(total: int, count: int) -> list[int]:
        """Return ``count`` unique midpoint quantiles in ``range(total)``."""
        if count < 0 or count > total:
            raise ValueError(f"Cannot select {count} proportional items from {total}")
        if not count:
            return []
        return [((2 * index + 1) * total) // (2 * count) for index in range(count)]

    rejected: dict[int, dict[str, Any]] = {}
    for interval_index, remaining in enumerate(interval_pc):
        slots: list[int] = []
        if interval_index:
            previous_anchor_capacity = anchor_capacity[interval_index - 1]
            previous_anchor_left_share = assigned_to_left[interval_index - 1]
            previous_anchor_right_share = (
                previous_anchor_capacity - previous_anchor_left_share
            )
            slots.extend(
                [fixed_pairs[interval_index - 1][1]]
                * previous_anchor_right_share
            )
        for ds_index in interval_ds[interval_index]:
            slots.extend([ds_index] * max_per_line)
        if interval_index < len(fixed_pairs):
            slots.extend(
                [fixed_pairs[interval_index][1]]
                * assigned_to_left[interval_index]
            )

        map_count = min(len(remaining), len(slots))
        selected_pc_offsets = set(proportional_indices(len(remaining), map_count))
        selected_slots = proportional_indices(len(slots), map_count)
        slot_iterator = iter(selected_slots)
        for pc_offset, pc_index in enumerate(remaining):
            if pc_offset not in selected_pc_offsets:
                allowed_start = (
                    fixed_pairs[interval_index - 1][1]
                    if interval_index
                    else 0
                )
                allowed_stop = (
                    fixed_pairs[interval_index][1]
                    if interval_index < len(fixed_pairs)
                    else len(ds) - 1
                )
                rejected[pc_index] = {
                    "method": "unmapped",
                    "reason": "capacity_exhausted_max_per_line",
                    "allowed_start": allowed_start,
                    "allowed_stop": allowed_stop,
                }
                continue

            ds_index = slots[next(slot_iterator)]
            textual_index = full_to_textual.get(pc_index)
            mapped[pc_index] = {
                "ds_index": ds_index,
                "similarity": (
                    float(similarity[textual_index, ds_index])
                    if textual_index is not None and similarity.size
                    else 0.0
                ),
                "method": "sequence_fallback",
            }

    occupancy = Counter(data["ds_index"] for data in mapped.values())
    if occupancy and max(occupancy.values()) > max_per_line:
        raise AssertionError("Fallback allocation exceeded max_per_line")
    if set(mapped) | set(rejected) != set(range(len(pc))):
        raise AssertionError("Fallback allocation lost PC indices")
    if set(mapped) & set(rejected):
        raise AssertionError("A PC index is both mapped and rejected")
    return mapped, rejected


def confidence(method: str, exact: bool, cosine: float, ratio: float) -> str:
    if method == "sequence_fallback":
        return "low"
    if exact:
        return "exact"
    if cosine >= 0.85 and ratio >= 0.80:
        return "high"
    if cosine >= 0.65 and ratio >= 0.60:
        return "medium"
    return "review"


def parse_scene_caps(values: list[str]) -> dict[tuple[str, str], int]:
    """Parse repeated ``GROUP:SCENE=CAP`` command-line overrides."""
    result: dict[tuple[str, str], int] = {}
    for value in values:
        target, separator, cap_text = value.rpartition("=")
        group, scene_separator, scene = target.partition(":")
        if not separator or not scene_separator or not cap_text.isdigit():
            raise ValueError(
                f"Invalid scene cap {value!r}; expected GROUP:SCENE=CAP"
            )
        cap = int(cap_text)
        if cap < 1:
            raise ValueError(f"Scene cap must be at least 1: {value!r}")
        group = group.lower().removesuffix(".fsb.txt")
        key = (group, scene)
        if key in result:
            raise ValueError(f"Duplicate scene cap: {group}:{scene}")
        result[key] = cap
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest_tsv", type=Path)
    parser.add_argument("ds_scripts_dir", type=Path)
    parser.add_argument("output_tsv", type=Path)
    parser.add_argument(
        "--tfidf-threshold",
        type=float,
        default=0.34,
        help="Pass-one cosine threshold (default: 0.34)",
    )
    parser.add_argument(
        "--max-per-line",
        type=int,
        default=4,
        help="Hard cap for PC message IDs assigned to one DS setText (default: 4)",
    )
    parser.add_argument(
        "--rejects-tsv",
        type=Path,
        help="Rejected IDs (default: <output stem>.rejects.tsv)",
    )
    parser.add_argument(
        "--scene-max-per-line",
        action="append",
        default=[],
        metavar="GROUP:SCENE=CAP",
        help="Override the hard cap for one scene; may be repeated",
    )
    args = parser.parse_args()
    if args.max_per_line < 1:
        parser.error("--max-per-line must be at least 1")
    rejects_path = args.rejects_tsv or args.output_tsv.with_name(
        f"{args.output_tsv.stem}.rejects.tsv"
    )
    if rejects_path.resolve() == args.output_tsv.resolve():
        parser.error("--rejects-tsv must differ from output_tsv")
    scene_caps = parse_scene_caps(args.scene_max_per_line)

    pc_groups = read_pc_manifest(args.manifest_tsv)
    ds_groups, ds_scene_starts = read_ds_scripts(args.ds_scripts_dir)
    missing_groups = sorted(set(pc_groups) - set(ds_groups))
    if missing_groups:
        raise ValueError(f"No DS script for PC groups: {', '.join(missing_groups)}")
    unknown_override_groups = sorted(
        {group for group, _ in scene_caps if group not in pc_groups}
    )
    if unknown_override_groups:
        raise ValueError(
            "Scene caps target unknown groups: " + ", ".join(unknown_override_groups)
        )

    output_rows: list[list[Any]] = []
    reject_rows: list[list[Any]] = []
    global_initial_count = 0
    retained_global_count = 0
    scene_initial_count = 0
    fallback_count = 0
    all_pc_ids: set[str] = set()
    output_pc_ids: set[str] = set()
    rejected_pc_ids: set[str] = set()
    scene_source_counts: Counter[str] = Counter()
    maximum_occupancy = 0

    for group in sorted(pc_groups):
        pc = pc_groups[group]
        ds = ds_groups[group]
        all_pc_ids.update(item["id"] for item in pc)
        global_fixed, _, _ = initial_alignment(
            pc, ds, args.tfidf_threshold
        )
        global_initial_count += len(global_fixed)
        ranges = infer_scene_ranges(
            pc,
            len(ds),
            ds_scene_starts.get(group, {}),
            global_fixed,
        )
        known_scenes = {scene_range["scene"] for scene_range in ranges}
        unknown_overrides = sorted(
            scene
            for override_group, scene in scene_caps
            if override_group == group and scene not in known_scenes
        )
        if unknown_overrides:
            raise ValueError(
                f"{group}: scene cap targets unknown scenes: "
                + ", ".join(unknown_overrides)
            )

        mapped: dict[int, dict[str, Any]] = {}
        rejected: dict[int, dict[str, Any]] = {}
        retained_group_global: set[int] = set()
        for scene_range in ranges:
            scene = scene_range["scene"]
            scene_source_counts[scene_range["source"]] += 1
            pc_indices = scene_range["pc_indices"]
            ds_start = scene_range["ds_start"]
            ds_stop = scene_range["ds_stop"]
            scene_pc = [pc[index] for index in pc_indices]
            scene_ds = ds[ds_start:ds_stop]
            scene_cap = scene_caps.get((group, scene), args.max_per_line)
            if not scene_ds:
                for local_pc_index, group_pc_index in enumerate(pc_indices):
                    rejected[group_pc_index] = {
                        "method": "unmapped",
                        "reason": "no_ds_lines_in_scene_range",
                        "allowed_start": ds_start,
                        "allowed_stop": ds_stop - 1,
                        "scene": scene,
                        "scene_source": scene_range["source"],
                        "scene_start": ds_start,
                        "scene_stop": ds_stop,
                        "max_per_line": scene_cap,
                    }
                continue

            local_fixed, similarity, full_to_textual = initial_alignment(
                scene_pc, scene_ds, args.tfidf_threshold
            )
            preferred: dict[int, dict[str, Any]] = {}
            for local_pc_index, group_pc_index in enumerate(pc_indices):
                global_data = global_fixed.get(group_pc_index)
                if global_data is None:
                    continue
                global_ds_index = global_data["ds_index"]
                if not ds_start <= global_ds_index < ds_stop:
                    continue
                local_ds_index = global_ds_index - ds_start
                textual_index = full_to_textual.get(local_pc_index)
                preferred[local_pc_index] = {
                    "ds_index": local_ds_index,
                    "similarity": (
                        float(similarity[textual_index, local_ds_index])
                        if textual_index is not None and similarity.size
                        else float(global_data["similarity"])
                    ),
                    "method": global_data["method"],
                }
                retained_group_global.add(group_pc_index)

            fixed = merge_fixed_mappings(preferred, local_fixed)
            scene_initial_count += len(fixed)
            scene_mapped, scene_rejected = fallback_alignment(
                scene_pc,
                scene_ds,
                fixed,
                similarity,
                full_to_textual,
                scene_cap,
            )
            for local_pc_index, mapping in scene_mapped.items():
                group_pc_index = pc_indices[local_pc_index]
                if group_pc_index in mapped or group_pc_index in rejected:
                    raise AssertionError(f"{group}: duplicate scene PC index")
                mapped[group_pc_index] = {
                    **mapping,
                    "ds_index": mapping["ds_index"] + ds_start,
                    "scene": scene,
                    "scene_source": scene_range["source"],
                    "scene_start": ds_start,
                    "scene_stop": ds_stop,
                    "max_per_line": scene_cap,
                }
            for local_pc_index, rejection in scene_rejected.items():
                group_pc_index = pc_indices[local_pc_index]
                if group_pc_index in mapped or group_pc_index in rejected:
                    raise AssertionError(f"{group}: duplicate rejected PC index")
                rejected[group_pc_index] = {
                    **rejection,
                    "allowed_start": rejection["allowed_start"] + ds_start,
                    "allowed_stop": rejection["allowed_stop"] + ds_start,
                    "scene": scene,
                    "scene_source": scene_range["source"],
                    "scene_start": ds_start,
                    "scene_stop": ds_stop,
                    "max_per_line": scene_cap,
                }

        retained_global_count += len(retained_group_global)
        if set(mapped) | set(rejected) != set(range(len(pc))):
            raise AssertionError(f"{group}: scene allocation lost PC indices")
        if set(mapped) & set(rejected):
            raise AssertionError(f"{group}: PC indices both mapped and rejected")

        occupancy = Counter(data["ds_index"] for data in mapped.values())
        if occupancy:
            group_maximum = max(occupancy.values())
            maximum_occupancy = max(maximum_occupancy, group_maximum)
            for ds_index, count in occupancy.items():
                caps = {
                    data["max_per_line"]
                    for data in mapped.values()
                    if data["ds_index"] == ds_index
                }
                if len(caps) != 1 or count > next(iter(caps)):
                    raise AssertionError(f"{group}: exceeded scene max-per-line")

        previous_ds_by_scene: dict[str, int] = {}
        for pc_index, item in enumerate(pc):
            if pc_index in rejected:
                rejection = rejected[pc_index]
                if item["id"] in rejected_pc_ids or item["id"] in output_pc_ids:
                    raise AssertionError(f"Duplicate rejected PC message ID: {item['id']}")
                rejected_pc_ids.add(item["id"])
                reject_rows.append(
                    [
                        f"{group}.fsb.txt",
                        rejection["scene"],
                        rejection["scene_source"],
                        rejection["scene_start"],
                        rejection["scene_stop"],
                        item["id"],
                        item["speaker"],
                        item["en_text"],
                        item["jp"],
                        " | ".join(item["paths"]),
                        len(item["paths"]),
                        rejection["method"],
                        rejection["reason"],
                        rejection["allowed_start"],
                        rejection["allowed_stop"],
                        rejection["max_per_line"],
                    ]
                )
                continue

            mapping = mapped[pc_index]
            if item["id"] in output_pc_ids or item["id"] in rejected_pc_ids:
                raise AssertionError(f"Duplicate PC message ID: {item['id']}")
            output_pc_ids.add(item["id"])
            ds_index = mapping["ds_index"]
            previous_ds_index = previous_ds_by_scene.get(mapping["scene"], -1)
            if ds_index < previous_ds_index:
                raise AssertionError(
                    f"{group} scene {mapping['scene']}: non-monotonic DS mapping"
                )
            previous_ds_by_scene[mapping["scene"]] = ds_index
            ds_item = ds[ds_index]
            cosine = float(mapping["similarity"])
            ratio = (
                difflib.SequenceMatcher(None, item["norm"], ds_item["norm"]).ratio()
                if item["norm"]
                else 0.0
            )
            exact = bool(item["norm"]) and item["norm"] == ds_item["norm"]
            method = mapping["method"]
            if method == "sequence_fallback":
                fallback_count += 1
            output_rows.append(
                [
                    f"{group}.fsb.txt",
                    ds_item["line"],
                    ds_item["ordinal"],
                    ds_item["speaker"],
                    ds_item["text"],
                    item["id"],
                    item["speaker"],
                    item["en_text"],
                    item["jp"],
                    " | ".join(item["paths"]),
                    len(item["paths"]),
                    method,
                    str(exact).lower(),
                    f"{cosine:.6f}",
                    f"{ratio:.6f}",
                    confidence(method, exact, cosine, ratio),
                    mapping["scene"],
                    mapping["scene_source"],
                    mapping["max_per_line"],
                ]
            )

    if output_pc_ids & rejected_pc_ids:
        raise AssertionError("Mapped and rejected PC ID sets overlap")
    if all_pc_ids != output_pc_ids | rejected_pc_ids:
        missing = sorted(all_pc_ids - output_pc_ids - rejected_pc_ids)
        extra = sorted((output_pc_ids | rejected_pc_ids) - all_pc_ids)
        raise AssertionError(f"PC ID accounting mismatch; missing={missing}, extra={extra}")

    output_rows.sort(key=lambda row: (row[0], int(row[2]), natural_key(row[5])))
    reject_rows.sort(key=lambda row: (row[0], natural_key(row[5])))
    args.output_tsv.parent.mkdir(parents=True, exist_ok=True)
    with args.output_tsv.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "ds_script",
                "ds_source_line",
                "ds_settext_ordinal",
                "ds_speaker",
                "ds_text",
                "pc_message_id",
                "pc_speaker",
                "pc_en_text",
                "pc_jp_speaker_and_text",
                "jp_ogg_paths",
                "ogg_segment_count",
                "alignment_method",
                "normalized_text_exact",
                "char_tfidf_cosine",
                "sequence_ratio",
                "confidence",
                "pc_scene",
                "scene_range_source",
                "scene_max_per_line",
            ]
        )
        writer.writerows(output_rows)

    rejects_path.parent.mkdir(parents=True, exist_ok=True)
    with rejects_path.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "ds_script",
                "pc_scene",
                "scene_range_source",
                "scene_ds_start_ordinal",
                "scene_ds_stop_ordinal_exclusive",
                "pc_message_id",
                "pc_speaker",
                "pc_en_text",
                "pc_jp_speaker_and_text",
                "jp_ogg_paths",
                "ogg_segment_count",
                "alignment_method",
                "reason",
                "allowed_ds_start_ordinal",
                "allowed_ds_stop_ordinal",
                "max_per_line",
            ]
        )
        writer.writerows(reject_rows)

    coverage = 100.0 * len(output_rows) / max(1, len(all_pc_ids))
    print(
        f"wrote mapped={len(output_rows)}, rejected={len(reject_rows)}, "
        f"coverage={coverage:.2f}% to {args.output_tsv}; "
        f"global_pass_one={global_initial_count}, "
        f"retained_global={retained_global_count}, "
        f"scene_pass_one={scene_initial_count}, "
        f"sequence_fallback={fallback_count}, max_occupancy={maximum_occupancy}, "
        f"scene_sources={dict(scene_source_counts)}, rejects={rejects_path}"
    )
    if len(output_rows) + len(reject_rows) != len(all_pc_ids):
        raise AssertionError("Mapped plus rejected rows do not equal unique PC IDs")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
