#!/usr/bin/env python3
"""Select the semantically safe subset of the full voice alignment."""

from __future__ import annotations

import argparse
import csv
import hashlib
from collections import Counter
from pathlib import Path
from typing import Iterable


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ALIGNMENT = PROJECT_ROOT / "work/manifests/alignment_full.tsv"
DEFAULT_REVIEW = PROJECT_ROOT / "research/reviews/borderline_voice_review.tsv"
DEFAULT_OUTPUT = PROJECT_ROOT / "work/manifests/alignment_safe.tsv"

AUTO_CONFIDENCE = {"exact", "high"}
REVIEW_CONFIDENCE = {"medium", "review"}
KNOWN_CONFIDENCE = AUTO_CONFIDENCE | REVIEW_CONFIDENCE | {"low"}
REVIEW_DECISIONS = {"accept", "reject"}
TARGET_COLUMNS = ("ds_script", "ds_source_line", "ds_settext_ordinal")
SELECTION_STATUS_COLUMN = "selection_status"


def read_tsv(path: Path, required: Iterable[str]) -> tuple[list[str], list[dict[str, str]]]:
    """Read a TSV manifest and require the columns used by the selector."""
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        fieldnames = reader.fieldnames
        if not fieldnames:
            raise ValueError(f"Empty TSV manifest: {path}")
        missing = set(required) - set(fieldnames)
        if missing:
            raise ValueError(f"{path} lacks columns: {sorted(missing)}")
        rows = list(reader)
    return fieldnames, rows


def index_reviews(
    rows: list[dict[str, str]],
) -> dict[str, dict[str, str]]:
    """Index reviewed borderline rows while rejecting ambiguous decisions."""
    reviews: dict[str, dict[str, str]] = {}
    for row_number, row in enumerate(rows, 2):
        message_id = row["pc_message_id"]
        if not message_id:
            raise ValueError(f"Review row {row_number} has an empty pc_message_id")
        if message_id in reviews:
            raise ValueError(f"Duplicate reviewed pc_message_id: {message_id}")
        if row["confidence"] not in REVIEW_CONFIDENCE:
            raise ValueError(
                f"Review row {message_id} has unsupported confidence "
                f"{row['confidence']!r}"
            )
        if row["decision"] not in REVIEW_DECISIONS:
            raise ValueError(
                f"Review row {message_id} has unsupported decision "
                f"{row['decision']!r}"
            )
        reviews[message_id] = row
    return reviews


def select_rows(
    alignment_rows: list[dict[str, str]],
    reviews: dict[str, dict[str, str]],
) -> tuple[list[dict[str, str]], Counter[str]]:
    """Apply the conservative automatic and manually reviewed policies."""
    alignment_by_id: dict[str, dict[str, str]] = {}
    selected: list[dict[str, str]] = []
    counts: Counter[str] = Counter()

    for row_number, row in enumerate(alignment_rows, 2):
        message_id = row["pc_message_id"]
        confidence = row["confidence"]
        if not message_id:
            raise ValueError(f"Alignment row {row_number} has an empty pc_message_id")
        if message_id in alignment_by_id:
            raise ValueError(f"Duplicate alignment pc_message_id: {message_id}")
        alignment_by_id[message_id] = row
        if confidence not in KNOWN_CONFIDENCE:
            raise ValueError(
                f"Alignment row {message_id} has unsupported confidence "
                f"{confidence!r}"
            )

        if row["alignment_method"] == "sequence_fallback" or confidence == "low":
            counts["rejected_low_or_fallback"] += 1
            continue
        if row["ds_speaker"] == "NOVEL" or row["ds_speaker"] != row["pc_speaker"]:
            counts["rejected_speaker"] += 1
            continue
        if confidence in AUTO_CONFIDENCE:
            selected.append(row)
            counts[f"selected_{confidence}"] += 1
            continue

        review = reviews.get(message_id)
        if review is None:
            raise ValueError(f"Missing manual review for borderline row: {message_id}")
        if review["confidence"] != confidence:
            raise ValueError(
                f"Confidence mismatch for {message_id}: alignment={confidence!r}, "
                f"review={review['confidence']!r}"
            )
        if review["decision"] == "accept":
            selected.append(row)
            counts[f"selected_{confidence}"] += 1
        else:
            counts["rejected_manual"] += 1

    borderline_ids = {
        row["pc_message_id"]
        for row in alignment_rows
        if row["confidence"] in REVIEW_CONFIDENCE
    }
    review_ids = set(reviews)
    if review_ids != borderline_ids:
        missing = sorted(borderline_ids - review_ids)
        unexpected = sorted(review_ids - borderline_ids)
        raise ValueError(
            "Manual review coverage mismatch: "
            f"missing={missing[:5]}, unexpected={unexpected[:5]}"
        )

    assert_unique(selected, "pc_message_id", lambda row: row["pc_message_id"])
    assert_unique(
        selected,
        "DS target (ds_script, ds_source_line, ds_settext_ordinal)",
        lambda row: tuple(row[column] for column in TARGET_COLUMNS),
    )
    return selected, counts


def assert_unique(
    rows: list[dict[str, str]],
    label: str,
    key,
) -> None:
    """Fail rather than silently choose between duplicate output mappings."""
    seen: set[object] = set()
    for row in rows:
        value = key(row)
        if value in seen:
            raise ValueError(f"Duplicate {label}: {value!r}")
        seen.add(value)


def write_tsv(path: Path, fieldnames: list[str], rows: list[dict[str, str]]) -> str:
    """Write stable source-order rows and return the SHA-256 of the bytes."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as destination:
        writer = csv.DictWriter(
            destination,
            fieldnames=fieldnames,
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        writer.writerows(rows)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--alignment", type=Path, default=DEFAULT_ALIGNMENT)
    parser.add_argument("--review", type=Path, default=DEFAULT_REVIEW)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    alignment_fields, alignment_rows = read_tsv(
        args.alignment,
        {
            "pc_message_id",
            "confidence",
            "alignment_method",
            "ds_speaker",
            "pc_speaker",
            *TARGET_COLUMNS,
        },
    )
    _, review_rows = read_tsv(
        args.review,
        {"pc_message_id", "confidence", "decision"},
    )
    reviews = index_reviews(review_rows)
    selected, counts = select_rows(alignment_rows, reviews)
    if SELECTION_STATUS_COLUMN in alignment_fields:
        raise ValueError(
            f"Input alignment already contains reserved column "
            f"{SELECTION_STATUS_COLUMN!r}"
        )
    output_fields = [*alignment_fields, SELECTION_STATUS_COLUMN]
    output_rows = [
        {**row, SELECTION_STATUS_COLUMN: "selected"}
        for row in selected
    ]
    digest = write_tsv(args.output, output_fields, output_rows)

    print(f"input_rows={len(alignment_rows)}")
    print(f"review_rows={len(review_rows)}")
    print(f"selected_rows={len(selected)}")
    for name in (
        "selected_exact",
        "selected_high",
        "selected_medium",
        "selected_review",
        "rejected_manual",
        "rejected_low_or_fallback",
        "rejected_speaker",
    ):
        print(f"{name}={counts[name]}")
    print(f"output_sha256={digest}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        raise SystemExit(f"error: {error}") from error
