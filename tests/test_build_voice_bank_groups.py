#!/usr/bin/env python3

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import build_voice_bank as voice_bank  # noqa: E402
from build_voice_bank import (  # noqa: E402
    alignment_message_ids,
    compose_voice_pcm,
    contiguous_paths,
    load_alignment,
    resolve_archive_message,
    resolve_voice_records,
    slice_pcm,
)
from pc_archive_manifest import path_key  # noqa: E402
from voice_audio import SAMPLE_RATE  # noqa: E402


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

    def test_reviewed_split_can_reuse_one_message_for_consecutive_targets(self) -> None:
        fields = (
            "alignment_method\tconfidence\tds_script\tds_settext_ordinal\t"
            "ds_source_line\tds_speaker\tjp_ogg_paths\tpc_message_id\t"
            "pc_speaker\tselection_status\tspeaker_policy\tmessage_count\t"
            "reviewed_override_group\ttarget_text_sha256\tjp_start_ms\t"
            "jp_end_ms\ten_start_ms\ten_end_ms\n"
        )
        first = (
            "reviewed_override\treview\tb11d.fsb.txt\t16\t60\tニルス\t"
            "/voice/B_1_00.ogg\tB_1\tニルス\tselected\tsame\t1\t"
            "niels\t\t0\t3200\t0\t3000\n"
        )
        second = (
            "reviewed_override\treview\tb11d.fsb.txt\t17\t62\tニルス\t"
            "/voice/B_1_00.ogg\tB_1\tニルス\tselected\tsame\t1\t"
            "niels\t\t3200\t\t3000\t\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "alignment.tsv"
            path.write_text(fields + first + second, encoding="utf-8")
            loaded = load_alignment(path)
        self.assertEqual(len(loaded), 2)
        self.assertEqual(loaded[1][1]["reviewed_override_group"], "niels")

    def test_reviewed_target_can_contain_ordered_sliced_components(self) -> None:
        fields = (
            "alignment_method\tconfidence\tds_script\tds_settext_ordinal\t"
            "ds_source_line\tds_speaker\tjp_ogg_paths\tpc_message_id\t"
            "pc_speaker\tselection_status\tspeaker_policy\tmessage_count\t"
            "reviewed_override_group\ttarget_text_sha256\tjp_start_ms\t"
            "jp_end_ms\ten_start_ms\ten_end_ms\n"
        )
        first = (
            "reviewed_override\treview\ta41d.fsb.txt\t163\t562\tニルス２\t"
            "/voice/A41d_010_540_03_10_00.ogg\tA41d_010_540_03_10\t"
            "ニルス２\tselected\tsame\t1\ta41d\t\t0\t2223\t0\t4550\n"
        )
        second = (
            "reviewed_override\treview\ta41d.fsb.txt\t164\t564\tニルス２\t"
            "/voice/A41d_010_540_03_10_00.ogg | "
            "/voice/A41d_010_540_03_20_00.ogg\t"
            "A41d_010_540_03_10 | A41d_010_540_03_20\tニルス２\t"
            "selected\tsame\t2\ta41d\t\t2223 | \t | \t4550 | \t | \n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "alignment.tsv"
            path.write_text(fields + first + second, encoding="utf-8")
            loaded = load_alignment(path)
        self.assertEqual(len(loaded), 2)
        self.assertEqual(
            alignment_message_ids(loaded[1][1]),
            ["A41d_010_540_03_10", "A41d_010_540_03_20"],
        )

    def test_unreviewed_duplicate_message_remains_rejected(self) -> None:
        content = (
            "alignment_method\tconfidence\tds_script\tds_settext_ordinal\t"
            "ds_source_line\tds_speaker\tjp_ogg_paths\tpc_message_id\t"
            "pc_speaker\tselection_status\tspeaker_policy\tmessage_count\n"
            "monotonic_tfidf\thigh\ta.fsb.txt\t1\t10\tA\t/voice/A_00.ogg\t"
            "A\tA\tselected\tsame\t1\n"
            "monotonic_tfidf\thigh\ta.fsb.txt\t2\t12\tA\t/voice/A_00.ogg\t"
            "A\tA\tselected\tsame\t1\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "alignment.tsv"
            path.write_text(content, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "mapped more than once"):
                load_alignment(path)

    def test_audio_slice_uses_deterministic_language_specific_sample_bounds(
        self,
    ) -> None:
        pcm = np.arange(SAMPLE_RATE, dtype=np.int16)
        row = {
            "jp_start_ms": "100",
            "jp_end_ms": "250",
            "en_start_ms": "200",
            "en_end_ms": "",
        }
        jp = slice_pcm(pcm, row, "jp")
        en = slice_pcm(pcm, row, "en")
        self.assertEqual(jp[0], 0)
        self.assertEqual(len(jp), round(SAMPLE_RATE * 0.150))
        self.assertEqual(jp[-1], 0)
        self.assertEqual(en[0], 0)
        self.assertEqual(en[-1], pcm[-1])
        self.assertEqual(jp[100], pcm[round(SAMPLE_RATE * 0.100) + 100])

    def test_unsliced_audio_is_returned_byte_identically(self) -> None:
        pcm = np.arange(SAMPLE_RATE, dtype=np.int16)
        result = slice_pcm(pcm, {}, "jp")
        self.assertIs(result, pcm)
        self.assertEqual(result.tobytes(), pcm.tobytes())

    def test_components_are_sliced_before_they_are_concatenated(self) -> None:
        first = np.full(SAMPLE_RATE, 1000, dtype=np.int16)
        second = np.full(SAMPLE_RATE // 4, 2000, dtype=np.int16)
        row = {
            "pc_message_id": "A_1 | A_2",
            "message_count": "2",
            "alignment_method": "reviewed_override",
            "jp_start_ms": "500 | ",
            "jp_end_ms": " | ",
        }
        components = [
            [("first.ogg", {"token": "first"})],
            [("second.ogg", {"token": "second"})],
        ]
        silence = np.zeros(10, dtype=np.int16)
        samples = {b"first": first, b"second": second}
        with (
            patch.object(
                voice_bank,
                "decrypt_ogg",
                side_effect=lambda _archive, record: record["token"].encode("ascii"),
            ),
            patch.object(
                voice_bank,
                "decode_pcm",
                side_effect=lambda payload: samples[payload],
            ),
        ):
            pcm, paths = compose_voice_pcm(object(), row, "jp", components, silence)
        first_length = SAMPLE_RATE // 2
        self.assertEqual(len(pcm), first_length + len(silence) + len(second))
        self.assertEqual(paths, ["first.ogg", "second.ogg"])
        self.assertEqual(pcm[0], 0)
        self.assertEqual(pcm[100], 1000)
        self.assertTrue(np.all(pcm[first_length : first_length + 10] == 0))
        self.assertEqual(pcm[first_length + 10], 2000)

    def test_audio_slice_cannot_extend_past_decoded_voice(self) -> None:
        pcm = np.zeros(SAMPLE_RATE, dtype=np.int16)
        with self.assertRaisesRegex(ValueError, "outside"):
            slice_pcm(
                pcm,
                {"jp_start_ms": "0", "jp_end_ms": "1001"},
                "jp",
            )


if __name__ == "__main__":
    unittest.main()
