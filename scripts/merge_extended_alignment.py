#!/usr/bin/env python3
"""Merge strict automatic and reviewed extended voice candidates.

The strict window aligner has priority whenever a reviewed candidate targets
the same DS line or reuses one of its PC messages.  The safe baseline embedded
in the strict manifest is never changed.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import unicodedata
from collections import Counter
from pathlib import Path

from align_dialogue import natural_key


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_STRICT = PROJECT_ROOT / "work/manifests/alignment_extended.tsv"
DEFAULT_REVIEWED = (
    PROJECT_ROOT / "research/reviews/alignment_extended_candidates.tsv"
)
DEFAULT_OUTPUT = PROJECT_ROOT / "work/manifests/alignment_extended_coverage.tsv"
DEFAULT_ADDITIONS = (
    PROJECT_ROOT / "work/manifests/alignment_extended_coverage_additions.tsv"
)
DEFAULT_REPORT = PROJECT_ROOT / "work/manifests/alignment_extended_coverage_report.json"
REQUIRED = {
    "alignment_method",
    "confidence",
    "ds_script",
    "ds_settext_ordinal",
    "ds_source_line",
    "ds_speaker",
    "jp_ogg_paths",
    "message_count",
    "pc_message_id",
    "pc_speaker",
    "selection_status",
    "speaker_policy",
}
NARRATION_SPEAKERS = frozenset({"novel", "hero"})


def read_tsv(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        fields = reader.fieldnames
        if not fields:
            raise ValueError(f"Empty TSV: {path}")
        missing = REQUIRED - set(fields)
        if missing:
            raise ValueError(f"{path} lacks columns: {sorted(missing)}")
        return fields, list(reader)


def message_ids(row: dict[str, str]) -> list[str]:
    values = [part.strip() for part in row["pc_message_id"].split("|") if part.strip()]
    try:
        declared = int(row["message_count"])
    except ValueError as exc:
        raise ValueError(f"Invalid message_count for {row['pc_message_id']}") from exc
    if not values or declared != len(values) or len(values) != len(set(values)):
        raise ValueError(f"Inconsistent grouped PC messages: {row['pc_message_id']}")
    return values


def target(row: dict[str, str]) -> tuple[str, int, int]:
    return (
        Path(row["ds_script"]).name.casefold(),
        int(row["ds_source_line"]),
        int(row["ds_settext_ordinal"]),
    )


def line_target(row: dict[str, str]) -> tuple[str, int]:
    return (Path(row["ds_script"]).name.casefold(), int(row["ds_source_line"]))


def ordinal_target(row: dict[str, str]) -> tuple[str, int]:
    return (
        Path(row["ds_script"]).name.casefold(),
        int(row["ds_settext_ordinal"]),
    )


def speaker_key(value: str) -> str:
    return unicodedata.normalize("NFKC", value).strip().casefold()


def policy_allows(row: dict[str, str], mode: str) -> bool:
    """Return whether a row is allowed by the requested speaker policy."""
    if mode == "all":
        return True
    if mode != "same":
        raise ValueError(f"Unknown speaker-policy mode: {mode}")
    ds_speaker = speaker_key(row["ds_speaker"])
    pc_speaker = speaker_key(row["pc_speaker"])
    return (
        row["speaker_policy"].strip().casefold() == "same"
        and ds_speaker == pc_speaker
        and ds_speaker not in NARRATION_SPEAKERS
    )


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


def normalize_reviewed(row: dict[str, str], index: int) -> dict[str, str]:
    result = dict(row)
    result["selection_status"] = "selected"
    result["selection_origin"] = "extended_reviewed"
    result.setdefault("extended_group_id", f"REVIEW_{index:04d}")
    result.setdefault("window_pc_message_count", result["message_count"])
    result.setdefault("window_ds_line_count", "1")
    result.setdefault("combined_normalized_text_exact", result["normalized_text_exact"])
    result.setdefault("combined_char_tfidf_cosine", result["char_tfidf_cosine"])
    result.setdefault("combined_sequence_ratio", result["sequence_ratio"])
    result.setdefault("ambiguity_margin", "reviewed")
    result["pc_message_id"] = " | ".join(message_ids(result))
    if not result["alignment_method"].startswith("extended_"):
        raise ValueError(
            f"Reviewed candidate has a non-extended method: {result['pc_message_id']}"
        )
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--strict", type=Path, default=DEFAULT_STRICT)
    parser.add_argument("--reviewed", type=Path, default=DEFAULT_REVIEWED)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--additions-output", type=Path, default=DEFAULT_ADDITIONS)
    parser.add_argument("--report", type=Path, default=DEFAULT_REPORT)
    parser.add_argument(
        "--speaker-policy",
        choices=("all", "same"),
        default="all",
        help=(
            "Keep every accepted row (all), or only rows where a named DS "
            "speaker exactly matches the PC speaker (same)."
        ),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    strict_fields, strict_rows = read_tsv(args.strict)
    reviewed_fields, reviewed_rows = read_tsv(args.reviewed)
    fields = [
        *strict_fields,
        *(field for field in reviewed_fields if field not in strict_fields),
    ]

    used_lines: set[tuple[str, int]] = set()
    used_ordinals: set[tuple[str, int]] = set()
    used_messages: set[str] = set()
    merged: list[dict[str, str]] = []
    counts: Counter[str] = Counter()
    for row in strict_rows:
        if row["selection_status"] != "selected":
            raise ValueError("Strict manifest contains an unselected row")
        if not policy_allows(row, args.speaker_policy):
            counts["strict_filtered_speaker_policy"] += 1
            continue
        row_line = line_target(row)
        row_ordinal = ordinal_target(row)
        row_messages = message_ids(row)
        if (
            row_line in used_lines
            or row_ordinal in used_ordinals
            or used_messages.intersection(row_messages)
        ):
            raise ValueError("Strict manifest is not one-target/one-use")
        used_lines.add(row_line)
        used_ordinals.add(row_ordinal)
        used_messages.update(row_messages)
        merged.append(dict(row))

    accepted_reviewed: list[dict[str, str]] = []
    for index, source in enumerate(reviewed_rows):
        row = normalize_reviewed(source, index)
        if not policy_allows(row, args.speaker_policy):
            counts["reviewed_filtered_speaker_policy"] += 1
            continue
        row_line = line_target(row)
        row_ordinal = ordinal_target(row)
        row_messages = message_ids(row)
        if row_line in used_lines or row_ordinal in used_ordinals:
            counts["reviewed_rejected_target_conflict"] += 1
            continue
        if used_messages.intersection(row_messages):
            counts["reviewed_rejected_message_conflict"] += 1
            continue
        used_lines.add(row_line)
        used_ordinals.add(row_ordinal)
        used_messages.update(row_messages)
        merged.append(row)
        accepted_reviewed.append(row)
        counts["reviewed_accepted"] += 1

    merged.sort(
        key=lambda row: (
            Path(row["ds_script"]).name.casefold(),
            int(row["ds_settext_ordinal"]),
            natural_key(row["pc_message_id"]),
        )
    )
    additions = [
        row for row in merged if row.get("selection_origin") != "safe_baseline"
    ]
    if len({line_target(row) for row in merged}) != len(merged):
        raise AssertionError("Merged alignment has duplicate DS source lines")
    if len({ordinal_target(row) for row in merged}) != len(merged):
        raise AssertionError("Merged alignment has duplicate DS ordinals")
    flattened = [message for row in merged for message in message_ids(row)]
    if len(flattened) != len(set(flattened)):
        raise AssertionError("Merged alignment reuses a PC message")
    if args.speaker_policy == "same" and any(
        not policy_allows(row, "same") for row in merged
    ):
        raise AssertionError("Same-speaker alignment contains narration or a mismatch")

    additions_digest = write_tsv(args.additions_output, fields, additions)
    output_digest = write_tsv(args.output, fields, merged)
    report = {
        "speaker_policy_mode": args.speaker_policy,
        "safe_baseline_targets": sum(
            row.get("selection_origin") == "safe_baseline" for row in merged
        ),
        "extended_targets": len(additions),
        "extended_source_messages": sum(len(message_ids(row)) for row in additions),
        "grouped_extended_targets": sum(len(message_ids(row)) > 1 for row in additions),
        "total_targets": len(merged),
        "total_source_messages": len(flattened),
        "speaker_policy": dict(Counter(row["speaker_policy"] for row in additions)),
        "confidence": dict(Counter(row["confidence"] for row in additions)),
        "selection_origin": dict(
            Counter(row.get("selection_origin", "") for row in additions)
        ),
        "reviewed_accounting": dict(counts),
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
