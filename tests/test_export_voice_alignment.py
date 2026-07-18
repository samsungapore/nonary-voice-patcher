#!/usr/bin/env python3

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import export_voice_alignment as exporter  # noqa: E402


def write_source(path: Path, *, status: str = "selected") -> None:
    fields = [*exporter.FIELDS, "ds_text", "pc_en_text"]
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
            with patch.object(exporter, "EXPECTED_ROWS", 1):
                rows, _ = exporter.export_alignment(source, output)
            with output.open(encoding="utf-8", newline="") as stream:
                reader = csv.DictReader(stream, delimiter="\t")
                exported = list(reader)
            self.assertEqual(rows, 1)
            self.assertEqual(tuple(reader.fieldnames or ()), exporter.FIELDS)
            self.assertNotIn("ds_text", exported[0])
            self.assertNotIn("pc_en_text", exported[0])

    def test_export_rejects_an_unselected_target(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.tsv"
            output = Path(directory) / "output.tsv"
            write_source(source, status="rejected")
            with patch.object(exporter, "EXPECTED_ROWS", 1):
                with self.assertRaisesRegex(ValueError, "unselected"):
                    exporter.export_alignment(source, output)


if __name__ == "__main__":
    unittest.main()
