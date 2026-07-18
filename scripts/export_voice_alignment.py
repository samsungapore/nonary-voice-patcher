#!/usr/bin/env python3
"""Export the reviewed production alignment without dialogue text."""

from __future__ import annotations

import argparse
import csv
import hashlib
from pathlib import Path
import tempfile


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = ROOT / "work/manifests/alignment_extended_dialogue_only.tsv"
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
EXPECTED_ROWS = 6_414


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def export_alignment(source: Path, output: Path) -> tuple[int, str]:
    with source.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        missing = set(FIELDS) - set(reader.fieldnames or ())
        if missing:
            raise ValueError(f"alignment lacks columns: {sorted(missing)}")
        rows = [{field: row[field] for field in FIELDS} for row in reader]
    if len(rows) != EXPECTED_ROWS:
        raise ValueError(f"expected {EXPECTED_ROWS} rows, found {len(rows)}")
    if any(row["selection_status"] != "selected" for row in rows):
        raise ValueError("production alignment contains an unselected row")

    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(
            stream,
            fieldnames=FIELDS,
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
    args = parser.parse_args()

    if args.check:
        if not args.output.is_file():
            raise FileNotFoundError(f"tracked alignment is missing: {args.output}")
        with tempfile.TemporaryDirectory(prefix="nonary-alignment-") as directory:
            generated = Path(directory) / args.output.name
            rows, generated_hash = export_alignment(args.source, generated)
            if generated.read_bytes() != args.output.read_bytes():
                raise SystemExit(
                    f"tracked alignment differs: generated {generated_hash}, "
                    f"tracked {digest(args.output)}"
                )
        print(f"alignment is reproducible: rows={rows} sha256={digest(args.output)}")
        return 0

    rows, output_hash = export_alignment(args.source, args.output)
    print(f"wrote {args.output}: rows={rows} sha256={output_hash}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
