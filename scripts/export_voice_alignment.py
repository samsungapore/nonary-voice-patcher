#!/usr/bin/env python3
"""Export the reviewed production alignment without dialogue text."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path
import tempfile


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = ROOT / "work/manifests/alignment_with_reviewed_overrides.tsv"
DEFAULT_OUTPUT = ROOT / "research/alignment/final_voice_alignment.tsv"
FIELDS = (
    "ds_script",
    "ds_source_line",
    "ds_settext_ordinal",
    "ds_speaker",
    "pc_message_id",
    "pc_speaker",
    "jp_ogg_paths",
    "alignment_method",
    "confidence",
    "selection_status",
    "message_count",
    "speaker_policy",
)
OPTIONAL_OVERRIDE_FIELDS = (
    "reviewed_override_group",
    "target_text_sha256",
    "jp_start_ms",
    "jp_end_ms",
    "en_start_ms",
    "en_end_ms",
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def export_alignment(
    source: Path, output: Path, *, expected_rows: int | None = None
) -> tuple[int, str]:
    with source.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        source_fields = set(reader.fieldnames or ())
        missing = set(FIELDS) - source_fields
        if missing:
            raise ValueError(f"alignment lacks columns: {sorted(missing)}")
        present_override_fields = set(OPTIONAL_OVERRIDE_FIELDS).intersection(
            source_fields
        )
        if present_override_fields and present_override_fields != set(
            OPTIONAL_OVERRIDE_FIELDS
        ):
            missing_override = set(OPTIONAL_OVERRIDE_FIELDS) - source_fields
            raise ValueError(
                "alignment has an incomplete reviewed override schema: "
                f"missing {sorted(missing_override)}"
            )
        output_fields = (
            (
                *(field for field in FIELDS if field != "selection_status"),
                *OPTIONAL_OVERRIDE_FIELDS,
                "selection_status",
            )
            if present_override_fields
            else FIELDS
        )
        rows = [{field: row[field] for field in output_fields} for row in reader]
    if expected_rows is not None and len(rows) != expected_rows:
        raise ValueError(f"expected {expected_rows} rows, found {len(rows)}")
    if any(row["selection_status"] != "selected" for row in rows):
        raise ValueError("production alignment contains an unselected row")

    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(
            stream,
            fieldnames=output_fields,
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        writer.writerows(rows)
    return len(rows), digest(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail when a fresh export differs from the tracked alignment",
    )
    parser.add_argument(
        "--expected-rows",
        type=int,
        help="optional audited row count; omitted when reviewed overrides change coverage",
    )
    args = parser.parse_args()

    if args.check:
        if not args.output.is_file():
            raise FileNotFoundError(f"tracked alignment is missing: {args.output}")
        with tempfile.TemporaryDirectory(prefix="nonary-alignment-") as directory:
            generated = Path(directory) / args.output.name
            rows, generated_hash = export_alignment(
                args.source, generated, expected_rows=args.expected_rows
            )
            if generated.read_bytes() != args.output.read_bytes():
                raise SystemExit(
                    f"tracked alignment differs: generated {generated_hash}, "
                    f"tracked {digest(args.output)}"
                )
        print(f"alignment is reproducible: rows={rows} sha256={digest(args.output)}")
        return 0

    rows, output_hash = export_alignment(
        args.source, args.output, expected_rows=args.expected_rows
    )
    print(f"wrote {args.output}: rows={rows} sha256={output_hash}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
