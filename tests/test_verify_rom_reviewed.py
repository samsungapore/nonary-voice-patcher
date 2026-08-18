#!/usr/bin/env python3

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from verify_rom import read_voice_map  # noqa: E402


class ReviewedVoiceMapTests(unittest.TestCase):
    def test_composite_reviewed_target_preserves_split_reuse_validation(self) -> None:
        fields = (
            "symbol",
            "internal_id_hex",
            "selection_status",
            "ds_script",
            "ds_source_line",
            "ds_settext_ordinal",
            "ds_speaker",
            "pc_speaker",
            "pc_message_ids",
            "message_count",
            "alignment_method",
            "reviewed_override_group",
            "target_text_sha256",
            "jp_start_ms",
            "jp_end_ms",
            "en_start_ms",
            "en_end_ms",
        )
        shared = {
            "selection_status": "selected",
            "ds_script": "a41d.fsb.txt",
            "ds_speaker": "ニルス２",
            "pc_speaker": "ニルス２",
            "alignment_method": "reviewed_override",
            "reviewed_override_group": "a41d_03",
            "target_text_sha256": "",
        }
        rows = [
            {
                **shared,
                "symbol": "SE_V0000",
                "internal_id_hex": "c80e",
                "ds_source_line": "562",
                "ds_settext_ordinal": "163",
                "pc_message_ids": "A41d_010_540_03_10",
                "message_count": "1",
                "jp_start_ms": "0",
                "jp_end_ms": "2223",
                "en_start_ms": "0",
                "en_end_ms": "4550",
            },
            {
                **shared,
                "symbol": "SE_V0001",
                "internal_id_hex": "c80f",
                "ds_source_line": "564",
                "ds_settext_ordinal": "164",
                "pc_message_ids": ("A41d_010_540_03_10 | A41d_010_540_03_20"),
                "message_count": "2",
                "jp_start_ms": "2223 | ",
                "jp_end_ms": " | ",
                "en_start_ms": "4550 | ",
                "en_end_ms": " | ",
            },
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "voice-map.tsv"
            with path.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.DictWriter(
                    stream, fieldnames=fields, delimiter="\t", lineterminator="\n"
                )
                writer.writeheader()
                writer.writerows(rows)
            parsed, symbols, identifiers = read_voice_map(path)
        self.assertEqual(len(parsed), 2)
        self.assertEqual(symbols, ["SE_V0000", "SE_V0001"])
        self.assertEqual(identifiers, [0xC80E, 0xC80F])


if __name__ == "__main__":
    unittest.main()
