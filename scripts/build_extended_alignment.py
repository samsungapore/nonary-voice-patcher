#!/usr/bin/env python3
"""Build a cautious, non-destructive extension of the safe voice alignment.

The safe manifest remains the authority.  Between two safe anchors in one PC
scene, this script compares short monotonic windows of one to four PC messages
with one to three still-unvoiced DS ``setText`` calls.  A selected DS target is
always unique, but its voice may concatenate several consecutive PC messages.

Named DS speakers must exactly match the PC speaker.  ``NOVEL`` targets use a
separate, deliberately stricter semantic policy because the DS script does not
carry a character name for those windows.  Ambiguous repeated matches are
discarded even when their textual score is high.
"""

from __future__ import annotations

import argparse
import csv
import difflib
import hashlib
import itertools
import json
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

try:
    from sklearn.feature_extraction.text import TfidfVectorizer
except ImportError as error:  # pragma: no cover - local dependency
    raise SystemExit("build_extended_alignment.py requires scikit-learn") from error

from align_dialogue import (
    initial_alignment,
    infer_scene_ranges,
    natural_key,
    read_ds_scripts,
    read_pc_manifest,
)


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PC_MANIFEST = PROJECT_ROOT / "work/manifests/ze1_jp_dialogue_voice_manifest.tsv"
# Always derive source-line numbers from the untouched retail decompilation.
# Injected source trees contain extra PlaySE/WaitSE lines and therefore cannot
# be used as the authority for later injection targets.
DEFAULT_DS_SCRIPTS = PROJECT_ROOT / "work/romfs/scr"
DEFAULT_FULL = PROJECT_ROOT / "work/manifests/alignment_full.tsv"
DEFAULT_SAFE = PROJECT_ROOT / "work/manifests/alignment_safe.tsv"
DEFAULT_OUTPUT = PROJECT_ROOT / "work/manifests/alignment_extended.tsv"
DEFAULT_ADDITIONS = PROJECT_ROOT / "work/manifests/alignment_extended_additions.tsv"
DEFAULT_REPORT = PROJECT_ROOT / "work/manifests/alignment_extended_report.json"

TARGET_COLUMNS = ("ds_script", "ds_source_line", "ds_settext_ordinal")
REQUIRED_ALIGNMENT_COLUMNS = {
    *TARGET_COLUMNS,
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
}
EXTRA_COLUMNS = (
    "message_count",
    "speaker_policy",
    "extended_group_id",
    "window_pc_message_count",
    "window_ds_line_count",
    "combined_normalized_text_exact",
    "combined_char_tfidf_cosine",
    "combined_sequence_ratio",
    "ambiguity_margin",
    "selection_origin",
)


@dataclass(frozen=True)
class RawCandidate:
    pc_offset: int
    ds_offset: int
    pc_indices: tuple[int, ...]
    ds_indices: tuple[int, ...]
    partitions: tuple[tuple[int, int], ...]
    speaker_policy: str
    combined_pc_text: str
    combined_ds_text: str
    local_texts: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class Candidate:
    raw: RawCandidate
    exact: bool
    cosine: float
    ratio: float
    local_scores: tuple[tuple[float, float, bool], ...]
    quality: float
    ambiguity_margin: float = 1.0


def read_tsv(
    path: Path, required: Iterable[str]
) -> tuple[list[str], list[dict[str, str]]]:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        fields = reader.fieldnames
        if not fields:
            raise ValueError(f"Empty TSV manifest: {path}")
        missing = set(required) - set(fields)
        if missing:
            raise ValueError(f"{path} lacks columns: {sorted(missing)}")
        return fields, list(reader)


def joined_text(records: Iterable[dict[str, Any]]) -> str:
    return " ".join(record["norm"] for record in records if record["norm"])


def compositions(total: int, groups: int) -> Iterable[tuple[tuple[int, int], ...]]:
    """Yield positive contiguous partitions of ``range(total)``."""
    if groups < 1 or groups > total:
        return
    for cuts in itertools.combinations(range(1, total), groups - 1):
        boundaries = (0, *cuts, total)
        yield tuple(
            (boundaries[index], boundaries[index + 1]) for index in range(groups)
        )


def speaker_policy_for(
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    pc_indices: tuple[int, ...],
    ds_indices: tuple[int, ...],
    partitions: tuple[tuple[int, int], ...],
) -> str | None:
    ds_speakers = [ds[index]["speaker"] for index in ds_indices]
    if all(speaker == "NOVEL" for speaker in ds_speakers):
        policy = "novel_semantic"
    elif any(speaker == "NOVEL" for speaker in ds_speakers):
        return None
    else:
        policy = "same"

    for ds_index, (start, stop) in zip(ds_indices, partitions):
        speakers = {pc[pc_indices[offset]]["speaker"] for offset in range(start, stop)}
        if len(speakers) != 1:
            return None
        if policy == "same" and next(iter(speakers)) != ds[ds_index]["speaker"]:
            return None
    return policy


def enumerate_raw_candidates(
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    pc_gap: list[int],
    ds_gap: list[int],
    low_ids: set[str],
    occupied_ds: set[int],
    max_pc_window: int,
    max_ds_window: int,
    minimum_combined_ratio: float,
    local_ratio: float,
    novel_ratio: float,
) -> list[RawCandidate]:
    result: list[RawCandidate] = []
    for pc_offset in range(len(pc_gap)):
        for pc_count in range(1, max_pc_window + 1):
            pc_indices = tuple(pc_gap[pc_offset : pc_offset + pc_count])
            if len(pc_indices) != pc_count:
                continue
            if any(
                pc[index]["id"] not in low_ids or not pc[index]["norm"]
                for index in pc_indices
            ):
                continue
            if any(
                right != left + 1 for left, right in zip(pc_indices, pc_indices[1:])
            ):
                continue
            combined_pc_text = joined_text(pc[index] for index in pc_indices)

            for ds_offset in range(len(ds_gap)):
                for ds_count in range(1, min(max_ds_window, pc_count) + 1):
                    ds_indices = tuple(ds_gap[ds_offset : ds_offset + ds_count])
                    if len(ds_indices) != ds_count:
                        continue
                    if any(index in occupied_ds for index in ds_indices):
                        continue
                    if any(
                        right != left + 1
                        for left, right in zip(ds_indices, ds_indices[1:])
                    ):
                        continue
                    if any(not ds[index]["norm"] for index in ds_indices):
                        continue
                    combined_ds_text = joined_text(ds[index] for index in ds_indices)
                    if (
                        combined_pc_text != combined_ds_text
                        and difflib.SequenceMatcher(
                            None, combined_pc_text, combined_ds_text
                        ).quick_ratio()
                        < minimum_combined_ratio
                    ):
                        # quick_ratio is an upper bound on the full ratio used
                        # by the acceptance gate, so this cannot hide a match.
                        continue

                    for partitions in compositions(pc_count, ds_count):
                        policy = speaker_policy_for(
                            pc, ds, pc_indices, ds_indices, partitions
                        )
                        if policy is None:
                            continue
                        local_texts = tuple(
                            (
                                joined_text(
                                    pc[pc_indices[offset]]
                                    for offset in range(start, stop)
                                ),
                                ds[ds_index]["norm"],
                            )
                            for ds_index, (start, stop) in zip(ds_indices, partitions)
                        )
                        required_local_ratio = (
                            novel_ratio - 0.10
                            if policy == "novel_semantic"
                            else local_ratio
                        )
                        if any(
                            pc_text != ds_text
                            and difflib.SequenceMatcher(
                                None, pc_text, ds_text
                            ).quick_ratio()
                            < required_local_ratio
                            for pc_text, ds_text in local_texts
                        ):
                            continue
                        result.append(
                            RawCandidate(
                                pc_offset=pc_offset,
                                ds_offset=ds_offset,
                                pc_indices=pc_indices,
                                ds_indices=ds_indices,
                                partitions=partitions,
                                speaker_policy=policy,
                                combined_pc_text=combined_pc_text,
                                combined_ds_text=combined_ds_text,
                                local_texts=local_texts,
                            )
                        )
    return result


def score_candidates(
    raw_candidates: list[RawCandidate],
    vectorizer: TfidfVectorizer,
    same_cosine: float,
    same_ratio: float,
    novel_cosine: float,
    novel_ratio: float,
    local_cosine: float,
    local_ratio: float,
) -> tuple[list[Candidate], Counter[str]]:
    counts: Counter[str] = Counter()
    if not raw_candidates:
        return [], counts

    texts = sorted(
        {
            text
            for candidate in raw_candidates
            for text in (
                candidate.combined_pc_text,
                candidate.combined_ds_text,
                *(part for pair in candidate.local_texts for part in pair),
            )
        }
    )
    matrix = vectorizer.transform(texts)
    vectors = {text: matrix[index] for index, text in enumerate(texts)}

    def cosine(left: str, right: str) -> float:
        return float(vectors[left].multiply(vectors[right]).sum())

    # Several partitions can describe the same pair of windows.  Retain the
    # partition whose weakest local text pair is strongest.
    best_by_span: dict[tuple[tuple[int, ...], tuple[int, ...], str], Candidate] = {}
    for raw in raw_candidates:
        combined_exact = raw.combined_pc_text == raw.combined_ds_text
        combined_cosine = cosine(raw.combined_pc_text, raw.combined_ds_text)
        combined_ratio = difflib.SequenceMatcher(
            None, raw.combined_pc_text, raw.combined_ds_text
        ).ratio()
        local_scores = tuple(
            (
                cosine(pc_text, ds_text),
                difflib.SequenceMatcher(None, pc_text, ds_text).ratio(),
                pc_text == ds_text,
            )
            for pc_text, ds_text in raw.local_texts
        )
        weakest_local_cosine = min(score[0] for score in local_scores)
        weakest_local_ratio = min(score[1] for score in local_scores)

        if raw.speaker_policy == "novel_semantic":
            accepted = (
                combined_exact
                or (combined_cosine >= novel_cosine and combined_ratio >= novel_ratio)
            ) and (
                all(score[2] for score in local_scores)
                or (
                    weakest_local_cosine >= novel_cosine - 0.08
                    and weakest_local_ratio >= novel_ratio - 0.10
                )
            )
        else:
            accepted = (
                combined_exact
                or (combined_cosine >= same_cosine and combined_ratio >= same_ratio)
            ) and (
                all(score[2] for score in local_scores)
                or (
                    weakest_local_cosine >= local_cosine
                    and weakest_local_ratio >= local_ratio
                )
            )
        if not accepted:
            counts[f"rejected_score_{raw.speaker_policy}"] += 1
            continue

        quality = (
            combined_cosine
            + combined_ratio
            + weakest_local_cosine
            + weakest_local_ratio
        ) / 4.0
        candidate = Candidate(
            raw=raw,
            exact=combined_exact,
            cosine=combined_cosine,
            ratio=combined_ratio,
            local_scores=local_scores,
            quality=quality,
        )
        key = (raw.pc_indices, raw.ds_indices, raw.speaker_policy)
        previous = best_by_span.get(key)
        rank = (
            candidate.exact,
            min(score[2] for score in candidate.local_scores),
            candidate.quality,
        )
        if previous is None or rank > (
            previous.exact,
            min(score[2] for score in previous.local_scores),
            previous.quality,
        ):
            best_by_span[key] = candidate

    candidates = list(best_by_span.values())
    by_source: dict[tuple[int, ...], list[Candidate]] = defaultdict(list)
    by_target: dict[tuple[int, ...], list[Candidate]] = defaultdict(list)
    for candidate in candidates:
        by_source[candidate.raw.pc_indices].append(candidate)
        by_target[candidate.raw.ds_indices].append(candidate)

    unambiguous: list[Candidate] = []
    for candidate in candidates:
        source_alternatives = [
            other.quality
            for other in by_source[candidate.raw.pc_indices]
            if other.raw.ds_indices != candidate.raw.ds_indices
        ]
        target_alternatives = [
            other.quality
            for other in by_target[candidate.raw.ds_indices]
            if other.raw.pc_indices != candidate.raw.pc_indices
        ]
        margin = min(
            candidate.quality - max(source_alternatives, default=-1.0),
            candidate.quality - max(target_alternatives, default=-1.0),
        )
        required_margin = 0.02 if candidate.exact else 0.05
        if margin + 1e-12 < required_margin:
            counts["rejected_ambiguity"] += 1
            continue
        unambiguous.append(
            Candidate(
                raw=candidate.raw,
                exact=candidate.exact,
                cosine=candidate.cosine,
                ratio=candidate.ratio,
                local_scores=candidate.local_scores,
                quality=candidate.quality,
                ambiguity_margin=min(1.0, margin),
            )
        )
    counts["eligible_unambiguous"] += len(unambiguous)
    return unambiguous, counts


def select_monotonic(
    candidates: list[Candidate], pc_count: int, ds_count: int
) -> list[Candidate]:
    """Select disjoint candidates, maximizing recovered messages then quality."""
    by_start: dict[tuple[int, int], list[Candidate]] = defaultdict(list)
    for candidate in candidates:
        by_start[(candidate.raw.pc_offset, candidate.raw.ds_offset)].append(candidate)

    # score = recovered PC messages, exact PC messages, number of voiced DS
    # targets, quality in millionths.  All candidates already passed strict
    # semantic gates, so coverage is a safe primary objective here.
    scores = [[(0, 0, 0, 0) for _ in range(ds_count + 1)] for _ in range(pc_count + 1)]
    choices: list[list[tuple[str, Candidate | None] | None]] = [
        [None for _ in range(ds_count + 1)] for _ in range(pc_count + 1)
    ]
    for pc_offset in range(pc_count, -1, -1):
        for ds_offset in range(ds_count, -1, -1):
            if pc_offset == pc_count and ds_offset == ds_count:
                continue
            options: list[tuple[tuple[int, int, int, int], str, Candidate | None]] = []
            if pc_offset < pc_count:
                options.append((scores[pc_offset + 1][ds_offset], "skip_pc", None))
            if ds_offset < ds_count:
                options.append((scores[pc_offset][ds_offset + 1], "skip_ds", None))
            for candidate in by_start.get((pc_offset, ds_offset), []):
                pc_next = pc_offset + len(candidate.raw.pc_indices)
                ds_next = ds_offset + len(candidate.raw.ds_indices)
                tail = scores[pc_next][ds_next]
                message_count = len(candidate.raw.pc_indices)
                addition = (
                    message_count,
                    message_count if candidate.exact else 0,
                    len(candidate.raw.ds_indices),
                    round(candidate.quality * 1_000_000),
                )
                options.append(
                    (
                        tuple(left + right for left, right in zip(addition, tail)),
                        "take",
                        candidate,
                    )
                )
            best = max(options, key=lambda item: item[0])
            scores[pc_offset][ds_offset] = best[0]
            choices[pc_offset][ds_offset] = (best[1], best[2])

    selected: list[Candidate] = []
    pc_offset = ds_offset = 0
    while pc_offset < pc_count or ds_offset < ds_count:
        choice = choices[pc_offset][ds_offset]
        if choice is None:
            break
        operation, candidate = choice
        if operation == "skip_pc":
            pc_offset += 1
        elif operation == "skip_ds":
            ds_offset += 1
        else:
            assert candidate is not None
            selected.append(candidate)
            pc_offset += len(candidate.raw.pc_indices)
            ds_offset += len(candidate.raw.ds_indices)
    return selected


def stable_group_id(group: str, candidate: Candidate) -> str:
    payload = "|".join(
        [
            group,
            *(str(index) for index in candidate.raw.pc_indices),
            "ds",
            *(str(index) for index in candidate.raw.ds_indices),
        ]
    )
    return "EXT_" + hashlib.sha256(payload.encode("ascii")).hexdigest()[:12].upper()


def make_addition_rows(
    group: str,
    scene_range: dict[str, Any],
    pc: list[dict[str, Any]],
    ds: list[dict[str, Any]],
    candidate: Candidate,
) -> list[dict[str, str]]:
    rows = []
    group_id = stable_group_id(group, candidate)
    for target_offset, (ds_index, partition, local_score) in enumerate(
        zip(candidate.raw.ds_indices, candidate.raw.partitions, candidate.local_scores)
    ):
        start, stop = partition
        items = [pc[candidate.raw.pc_indices[offset]] for offset in range(start, stop)]
        ds_item = ds[ds_index]
        pc_ids = " | ".join(item["id"] for item in items)
        paths = [path for item in items for path in item["paths"]]
        local_cosine, local_ratio, local_exact = local_score
        rows.append(
            {
                "ds_script": f"{group}.fsb.txt",
                "ds_source_line": str(ds_item["line"]),
                "ds_settext_ordinal": str(ds_item["ordinal"]),
                "ds_speaker": ds_item["speaker"],
                "ds_text": ds_item["text"],
                "pc_message_id": pc_ids,
                "pc_speaker": items[0]["speaker"],
                "pc_en_text": " || ".join(item["en_text"] for item in items),
                "pc_jp_speaker_and_text": " || ".join(item["jp"] for item in items),
                "jp_ogg_paths": " | ".join(paths),
                "ogg_segment_count": str(len(paths)),
                "alignment_method": (
                    "extended_window_exact" if local_exact else "extended_window_tfidf"
                ),
                "normalized_text_exact": str(local_exact).lower(),
                "char_tfidf_cosine": f"{local_cosine:.6f}",
                "sequence_ratio": f"{local_ratio:.6f}",
                "confidence": "exact" if local_exact else "high",
                "pc_scene": pc[candidate.raw.pc_indices[0]]["scene"],
                "scene_range_source": scene_range["source"],
                "scene_max_per_line": "1",
                "selection_status": "selected",
                "message_count": str(len(items)),
                "speaker_policy": candidate.raw.speaker_policy,
                "extended_group_id": group_id,
                "window_pc_message_count": str(len(candidate.raw.pc_indices)),
                "window_ds_line_count": str(len(candidate.raw.ds_indices)),
                "combined_normalized_text_exact": str(candidate.exact).lower(),
                "combined_char_tfidf_cosine": f"{candidate.cosine:.6f}",
                "combined_sequence_ratio": f"{candidate.ratio:.6f}",
                "ambiguity_margin": f"{candidate.ambiguity_margin:.6f}",
                "selection_origin": "extended_automatic",
            }
        )
    return rows


def write_tsv(path: Path, fields: list[str], rows: list[dict[str, str]]) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as destination:
        writer = csv.DictWriter(
            destination,
            fieldnames=fields,
            delimiter="\t",
            lineterminator="\n",
            extrasaction="ignore",
        )
        writer.writeheader()
        writer.writerows(rows)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pc-manifest", type=Path, default=DEFAULT_PC_MANIFEST)
    parser.add_argument("--ds-scripts", type=Path, default=DEFAULT_DS_SCRIPTS)
    parser.add_argument("--full", type=Path, default=DEFAULT_FULL)
    parser.add_argument("--safe", type=Path, default=DEFAULT_SAFE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--additions-output", type=Path, default=DEFAULT_ADDITIONS)
    parser.add_argument("--report", type=Path, default=DEFAULT_REPORT)
    parser.add_argument("--max-pc-window", type=int, default=4)
    parser.add_argument("--max-ds-window", type=int, default=3)
    parser.add_argument("--same-cosine", type=float, default=0.85)
    parser.add_argument("--same-ratio", type=float, default=0.80)
    parser.add_argument("--novel-cosine", type=float, default=0.93)
    parser.add_argument("--novel-ratio", type=float, default=0.90)
    parser.add_argument("--local-cosine", type=float, default=0.72)
    parser.add_argument("--local-ratio", type=float, default=0.65)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not 1 <= args.max_pc_window <= 8:
        raise ValueError("--max-pc-window must be in 1..8")
    if not 1 <= args.max_ds_window <= args.max_pc_window:
        raise ValueError("--max-ds-window must be in 1..max-pc-window")

    safe_fields, safe_rows = read_tsv(args.safe, REQUIRED_ALIGNMENT_COLUMNS)
    _, full_rows = read_tsv(args.full, REQUIRED_ALIGNMENT_COLUMNS)
    pc_groups = read_pc_manifest(args.pc_manifest)
    ds_groups, ds_scene_starts = read_ds_scripts(args.ds_scripts)
    full_by_id = {row["pc_message_id"]: row for row in full_rows}
    if len(full_by_id) != len(full_rows):
        raise ValueError("Full alignment has duplicate PC message IDs")
    low_ids = {row["pc_message_id"] for row in full_rows if row["confidence"] == "low"}

    pc_by_id = {
        item["id"]: (group, index, item)
        for group, records in pc_groups.items()
        for index, item in enumerate(records)
    }
    safe_ids: set[str] = set()
    occupied: dict[str, set[int]] = defaultdict(set)
    safe_anchors: dict[tuple[str, str], list[tuple[int, int]]] = defaultdict(list)
    for row in safe_rows:
        message_id = row["pc_message_id"]
        if message_id in safe_ids:
            raise ValueError(f"Safe alignment duplicates PC message: {message_id}")
        safe_ids.add(message_id)
        try:
            group, pc_index, item = pc_by_id[message_id]
        except KeyError as error:
            raise ValueError(
                f"Safe PC message absent from source manifest: {message_id}"
            ) from error
        script_group = Path(row["ds_script"]).name.removesuffix(".fsb.txt").lower()
        if script_group != group:
            raise ValueError(f"Safe group mismatch for {message_id}")
        ds_index = int(row["ds_settext_ordinal"])
        if ds_index in occupied[group]:
            raise ValueError(f"Safe alignment duplicates DS target: {group}:{ds_index}")
        occupied[group].add(ds_index)
        safe_anchors[(group, item["scene"])].append((pc_index, ds_index))

    additions: list[dict[str, str]] = []
    selected_candidates: list[Candidate] = []
    counters: Counter[str] = Counter()
    for group in sorted(pc_groups):
        pc = pc_groups[group]
        ds = ds_groups[group]
        vectorizer = TfidfVectorizer(
            analyzer="char_wb", ngram_range=(2, 5), sublinear_tf=True, norm="l2"
        )
        vectorizer.fit([item["norm"] or " " for item in pc + ds])
        global_fixed, _, _ = initial_alignment(pc, ds, 0.34)
        scene_ranges = infer_scene_ranges(
            pc, len(ds), ds_scene_starts.get(group, {}), global_fixed
        )

        for scene_range in scene_ranges:
            scene_indices = scene_range["pc_indices"]
            position_by_pc = {
                pc_index: position for position, pc_index in enumerate(scene_indices)
            }
            anchors = sorted(safe_anchors[(group, scene_range["scene"])])
            if any(
                left_pc >= right_pc or left_ds >= right_ds
                for (left_pc, left_ds), (right_pc, right_ds) in zip(
                    anchors, anchors[1:]
                )
            ):
                raise ValueError(
                    f"Non-monotonic safe anchors in {group}:{scene_range['scene']}"
                )
            bounds = [
                (-1, scene_range["ds_start"] - 1),
                *(
                    (position_by_pc[pc_index], ds_index)
                    for pc_index, ds_index in anchors
                ),
                (len(scene_indices), scene_range["ds_stop"]),
            ]
            for (pc_left, ds_left), (pc_right, ds_right) in zip(bounds, bounds[1:]):
                pc_gap = scene_indices[pc_left + 1 : pc_right]
                ds_gap = list(range(ds_left + 1, ds_right))
                if not pc_gap or not ds_gap:
                    continue
                raw = enumerate_raw_candidates(
                    pc,
                    ds,
                    pc_gap,
                    ds_gap,
                    low_ids,
                    occupied[group],
                    args.max_pc_window,
                    args.max_ds_window,
                    min(args.same_ratio, args.novel_ratio),
                    args.local_ratio,
                    args.novel_ratio,
                )
                counters["raw_candidates"] += len(raw)
                eligible, score_counts = score_candidates(
                    raw,
                    vectorizer,
                    args.same_cosine,
                    args.same_ratio,
                    args.novel_cosine,
                    args.novel_ratio,
                    args.local_cosine,
                    args.local_ratio,
                )
                counters.update(score_counts)
                selected = select_monotonic(eligible, len(pc_gap), len(ds_gap))
                counters["selected_windows"] += len(selected)
                for candidate in selected:
                    selected_candidates.append(candidate)
                    rows = make_addition_rows(group, scene_range, pc, ds, candidate)
                    for row in rows:
                        target = int(row["ds_settext_ordinal"])
                        if target in occupied[group]:
                            raise AssertionError(
                                f"Extended target collision: {group}:{target}"
                            )
                        occupied[group].add(target)
                        additions.append(row)

    # A shared one-row-per-target schema lets the final uniqueness checks treat
    # reviewed additions and the immutable baseline identically.
    baseline: list[dict[str, str]] = []
    for index, source in enumerate(safe_rows):
        row = dict(source)
        row.update(
            {
                "message_count": "1",
                "speaker_policy": "same",
                "extended_group_id": f"SAFE_{index:04d}",
                "window_pc_message_count": "1",
                "window_ds_line_count": "1",
                "combined_normalized_text_exact": row["normalized_text_exact"],
                "combined_char_tfidf_cosine": row["char_tfidf_cosine"],
                "combined_sequence_ratio": row["sequence_ratio"],
                "ambiguity_margin": "1.000000",
                "selection_origin": "safe_baseline",
            }
        )
        baseline.append(row)

    output_fields = [
        *safe_fields,
        *(field for field in EXTRA_COLUMNS if field not in safe_fields),
    ]
    additions.sort(
        key=lambda row: (
            row["ds_script"].casefold(),
            int(row["ds_settext_ordinal"]),
            natural_key(row["pc_message_id"]),
        )
    )
    merged = sorted(
        [*baseline, *additions],
        key=lambda row: (
            row["ds_script"].casefold(),
            int(row["ds_settext_ordinal"]),
            natural_key(row["pc_message_id"]),
        ),
    )

    targets = [
        (row["ds_script"].casefold(), int(row["ds_settext_ordinal"])) for row in merged
    ]
    if len(targets) != len(set(targets)):
        raise AssertionError("Extended output has duplicate DS targets")
    used_pc_ids = [
        message_id.strip()
        for row in merged
        for message_id in row["pc_message_id"].split(" | ")
        if message_id.strip()
    ]
    if len(used_pc_ids) != len(set(used_pc_ids)):
        raise AssertionError("Extended output reuses a PC message")
    recovered_ids = set(used_pc_ids) - safe_ids
    if not recovered_ids <= low_ids:
        raise AssertionError("Extended output selected a non-low, non-safe PC message")

    additions_digest = write_tsv(args.additions_output, output_fields, additions)
    output_digest = write_tsv(args.output, output_fields, merged)
    report = {
        "baseline_target_rows": len(baseline),
        "low_source_messages": len(low_ids),
        "extended_target_rows": len(additions),
        "recovered_low_source_messages": len(recovered_ids),
        "remaining_low_source_messages": len(low_ids - recovered_ids),
        "total_target_rows": len(merged),
        "total_source_messages": len(used_pc_ids),
        "selected_windows": counters["selected_windows"],
        "grouped_extended_targets": sum(
            int(row["message_count"]) > 1 for row in additions
        ),
        "same_speaker_extended_targets": sum(
            row["speaker_policy"] == "same" for row in additions
        ),
        "novel_semantic_extended_targets": sum(
            row["speaker_policy"] == "novel_semantic" for row in additions
        ),
        "extended_confidence": dict(Counter(row["confidence"] for row in additions)),
        "extended_methods": dict(Counter(row["alignment_method"] for row in additions)),
        "window_shapes": dict(
            Counter(
                f"{len(candidate.raw.pc_indices)}pc_to_{len(candidate.raw.ds_indices)}ds"
                for candidate in selected_candidates
            )
        ),
        "candidate_accounting": dict(counters),
        "thresholds": {
            "same_cosine": args.same_cosine,
            "same_ratio": args.same_ratio,
            "novel_cosine": args.novel_cosine,
            "novel_ratio": args.novel_ratio,
            "local_cosine": args.local_cosine,
            "local_ratio": args.local_ratio,
            "max_pc_window": args.max_pc_window,
            "max_ds_window": args.max_ds_window,
        },
        "output_sha256": output_digest,
        "additions_sha256": additions_digest,
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(
        json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(report, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, ValueError) as error:
        raise SystemExit(f"error: {error}") from error
