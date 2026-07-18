#!/usr/bin/env python3

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from build_voice_bank import (  # noqa: E402
    alignment_message_ids,
    contiguous_paths,
    load_alignment,
    resolve_archive_message,
    resolve_voice_records,
)
from pc_archive_manifest import path_key  # noqa: E402


class GroupedVoiceBankTests(unittest.TestCase):
    @staticmethod
    def archive_entry(path: str, entry_type: int, offset: int = 100):
        return (offset, path_key(path), 20, 0, 123, entry_type, 0)

    def test_message_count_matches_ordered_ids(self) -> None:
        row = {"pc_message_id": "A_1 | A_2", "message_count": "2"}
        self.assertEqual(alignment_message_ids(row), ["A_1", "A_2"])

    def test_paths_are_grouped_before_segment_continuity(self) -> None:
        row = {
            "pc_message_id": "A_1 | A_2",
            "message_count": "2",
            "jp_ogg_paths": (
                "/voice/A_2_01.ogg | /voice/A_1_00.ogg | "
                "/voice/A_2_88.ogg | /voice/A_2_00.ogg"
            ),
        }
        self.assertEqual(
            contiguous_paths(row),
            [
                "/voice/A_1_00.ogg",
                "/voice/A_2_00.ogg",
                "/voice/A_2_01.ogg",
            ],
        )

    def test_duplicate_message_id_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "repeats a PC message"):
            alignment_message_ids({"pc_message_id": "A_1 | A_1", "message_count": "2"})

    def test_english_resolution_stops_at_first_missing_segment(self) -> None:
        zero = "/sound/voice_us/A_1_00.ogg"
        collision = "/sound/voice_us/A_1_88.ogg"
        index = (
            1000,
            {
                path_key(zero): self.archive_entry(zero, 0x20038),
                path_key(collision): self.archive_entry(collision, 0x20038),
            },
        )
        records = resolve_archive_message(index, "A_1", "en")
        self.assertEqual([path for path, _ in records], [zero])
        self.assertEqual(records[0][1]["encrypted_absolute_offset"], "1100")

    def test_english_resolution_accepts_contiguous_extra_segment(self) -> None:
        paths = [f"/sound/voice_us/A_1_{segment:02d}.ogg" for segment in range(2)]
        index = (
            1000,
            {
                path_key(path): self.archive_entry(path, 0x20038, 100 + number)
                for number, path in enumerate(paths)
            },
        )
        records = resolve_archive_message(index, "A_1", "en")
        self.assertEqual([path for path, _ in records], paths)

    def test_english_resolution_requires_english_entry_type(self) -> None:
        path = "/sound/voice_us/A_1_00.ogg"
        index = (1000, {path_key(path): self.archive_entry(path, 0x2006D)})
        with self.assertRaisesRegex(KeyError, "no contiguous EN voice"):
            resolve_archive_message(index, "A_1", "en")

    def test_english_grouped_messages_are_resolved_in_message_order(self) -> None:
        paths = [
            "/sound/voice_us/A_1_00.ogg",
            "/sound/voice_us/A_2_00.ogg",
            "/sound/voice_us/A_2_01.ogg",
        ]
        index = (
            1000,
            {
                path_key(path): self.archive_entry(path, 0x20038, 100 + number)
                for number, path in enumerate(paths)
            },
        )
        row = {"pc_message_id": "A_1 | A_2", "message_count": "2"}
        records = resolve_voice_records(row, "en", archive_index=index)
        self.assertEqual([path for path, _ in records], paths)

    def test_narration_requires_explicit_opt_in(self) -> None:
        content = (
            "alignment_method\tconfidence\tds_script\tds_settext_ordinal\t"
            "ds_source_line\tds_speaker\tjp_ogg_paths\tpc_message_id\t"
            "pc_speaker\tselection_status\tspeaker_policy\tmessage_count\n"
            "extended_novel_semantic\thigh\ta01b.fsb.txt\t127\t702\tNOVEL\t"
            "/voice/A_00.ogg\tA\t淳平\tselected\tnovel_semantic\t1\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "alignment.tsv"
            path.write_text(content, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "targets narration"):
                load_alignment(path)
            loaded = load_alignment(path, allow_narration_voices=True)
        self.assertEqual(len(loaded), 1)


if __name__ == "__main__":
    unittest.main()
