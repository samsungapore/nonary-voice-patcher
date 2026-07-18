#!/usr/bin/env python3

from __future__ import annotations

import sys
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from build_extended_alignment import (  # noqa: E402
    Candidate,
    RawCandidate,
    compositions,
    make_addition_rows,
    select_monotonic,
    speaker_policy_for,
)


def pc_record(identifier: str, speaker: str, text: str, path: str) -> dict[str, object]:
    return {
        "id": identifier,
        "scene": "010",
        "speaker": speaker,
        "en_text": text,
        "norm": text.lower(),
        "jp": f"{speaker} :: jp {identifier}",
        "paths": [path],
    }


def ds_record(ordinal: int, speaker: str, text: str) -> dict[str, object]:
    return {
        "line": ordinal + 100,
        "ordinal": ordinal,
        "speaker": speaker,
        "text": text,
        "norm": text.lower(),
    }


class ExtendedAlignmentTests(unittest.TestCase):
    def test_compositions_are_positive_and_contiguous(self) -> None:
        self.assertEqual(
            list(compositions(4, 2)),
            [
                ((0, 1), (1, 4)),
                ((0, 2), (2, 4)),
                ((0, 3), (3, 4)),
            ],
        )

    def test_speaker_policy_separates_named_and_novel_targets(self) -> None:
        pc = [
            pc_record("A_1", "A", "one", "/one_00.ogg"),
            pc_record("A_2", "A", "two", "/two_00.ogg"),
        ]
        named = [ds_record(0, "A", "one two")]
        novel = [ds_record(0, "NOVEL", "one two")]
        wrong = [ds_record(0, "B", "one two")]
        partition = ((0, 2),)
        self.assertEqual(speaker_policy_for(pc, named, (0, 1), (0,), partition), "same")
        self.assertEqual(
            speaker_policy_for(pc, novel, (0, 1), (0,), partition),
            "novel_semantic",
        )
        self.assertIsNone(speaker_policy_for(pc, wrong, (0, 1), (0,), partition))

    def test_monotonic_selector_prefers_more_recovered_messages(self) -> None:
        one = Candidate(
            raw=RawCandidate(
                0,
                0,
                (0,),
                (0,),
                ((0, 1),),
                "same",
                "one",
                "one",
                (("one", "one"),),
            ),
            exact=True,
            cosine=1.0,
            ratio=1.0,
            local_scores=((1.0, 1.0, True),),
            quality=1.0,
        )
        grouped = Candidate(
            raw=RawCandidate(
                0,
                0,
                (0, 1),
                (0,),
                ((0, 2),),
                "same",
                "one two",
                "one two",
                (("one two", "one two"),),
            ),
            exact=True,
            cosine=1.0,
            ratio=1.0,
            local_scores=((1.0, 1.0, True),),
            quality=1.0,
        )
        self.assertEqual(select_monotonic([one, grouped], 2, 1), [grouped])

    def test_grouped_output_preserves_message_and_audio_order(self) -> None:
        pc = [
            pc_record("A_1", "A", "one", "/one_00.ogg"),
            pc_record("A_2", "A", "two", "/two_00.ogg"),
        ]
        ds = [ds_record(0, "NOVEL", "one two")]
        candidate = Candidate(
            raw=RawCandidate(
                0,
                0,
                (0, 1),
                (0,),
                ((0, 2),),
                "novel_semantic",
                "one two",
                "one two",
                (("one two", "one two"),),
            ),
            exact=True,
            cosine=1.0,
            ratio=1.0,
            local_scores=((1.0, 1.0, True),),
            quality=1.0,
        )
        rows = make_addition_rows(
            "a01b",
            {"source": "test"},
            pc,
            ds,
            candidate,
        )
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["pc_message_id"], "A_1 | A_2")
        self.assertEqual(rows[0]["jp_ogg_paths"], "/one_00.ogg | /two_00.ogg")
        self.assertEqual(rows[0]["message_count"], "2")
        self.assertEqual(rows[0]["speaker_policy"], "novel_semantic")


if __name__ == "__main__":
    unittest.main()
