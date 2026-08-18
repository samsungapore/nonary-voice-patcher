#!/usr/bin/env python3

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import export_voice_alignment as exporter  # noqa: E402


def write_source(
    path: Path, *, status: str = "selected", with_override_fields: bool = False
) -> None:
    fields = [
        *exporter.FIELDS,
        *(exporter.OPTIONAL_OVERRIDE_FIELDS if with_override_fields else ()),
        "ds_text",
        "pc_en_text",
    ]
    row = {field: f"value-{field}" for field in fields}
    row["selection_status"] = status
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, delimiter="\t")
        writer.writeheader()
        writer.writerow(row)


class RuntimeAlignmentExportTests(unittest.TestCase):
    def test_export_keeps_only_the_runtime_allowlist(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tsv"
            output = Path(directory) / "output.tsv"
            write_source(source)
            rows, _ = exporter.export_alignment(source, output, expected_rows=1)
            with output.open(encoding="utf-8", newline="") as stream:
                reader = csv.DictReader(stream, delimiter="\t")
                exported = list(reader)
            self.assertEqual(rows, 1)
            self.assertEqual(tuple(reader.fieldnames or ()), exporter.FIELDS)
            self.assertNotIn("ds_text", exported[0])
            self.assertNotIn("pc_en_text", exported[0])

    def test_export_preserves_complete_runtime_override_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tsv"
            output = Path(directory) / "output.tsv"
            write_source(source, with_override_fields=True)
            exporter.export_alignment(source, output, expected_rows=1)
            with output.open(encoding="utf-8", newline="") as stream:
                reader = csv.DictReader(stream, delimiter="\t")
                rows = list(reader)
            self.assertEqual(
                tuple(reader.fieldnames or ()),
                (
                    *(
                        field
                        for field in exporter.FIELDS
                        if field != "selection_status"
                    ),
                    *exporter.OPTIONAL_OVERRIDE_FIELDS,
                    "selection_status",
                ),
            )
            self.assertEqual(rows[0]["target_text_sha256"], "value-target_text_sha256")
            self.assertFalse(output.read_bytes().splitlines()[1].endswith(b"\t"))

    def test_export_rejects_partial_override_schema(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tsv"
            output = Path(directory) / "output.tsv"
            fields = [*exporter.FIELDS, "target_text_sha256"]
            with source.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=fields, delimiter="\t")
                writer.writeheader()
                writer.writerow({field: "selected" for field in fields})
            with self.assertRaisesRegex(ValueError, "incomplete"):
                exporter.export_alignment(source, output, expected_rows=1)

    def test_export_rejects_an_unselected_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tsv"
            output = Path(directory) / "output.tsv"
            write_source(source, status="rejected")
            with self.assertRaisesRegex(ValueError, "unselected"):
                exporter.export_alignment(source, output, expected_rows=1)


if __name__ == "__main__":
    unittest.main()
