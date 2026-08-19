#!/usr/bin/env python3

from __future__ import annotations

import csv
import hashlib
import sys
import unittest
from collections import Counter
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
REVIEWS = PROJECT_ROOT / "research" / "reviews"
ALIGNMENT = PROJECT_ROOT / "research" / "alignment" / "final_voice_alignment.tsv"
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from merge_extended_alignment import policy_allows  # noqa: E402
from apply_reviewed_alignment_overrides import read_overrides  # noqa: E402


def read_rows(name: str) -> list[dict[str, str]]:
    with (REVIEWS / name).open(encoding="utf-8", newline="") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def sha256(name: str) -> str:
    return hashlib.sha256((REVIEWS / name).read_bytes()).hexdigest()


class ReviewLedgerTests(unittest.TestCase):
    def test_runtime_alignment_is_text_free_and_hash_pinned(self) -> None:
        with ALIGNMENT.open(encoding="utf-8", newline="") as source:
            reader = csv.DictReader(source, delimiter="\t")
            fields = set(reader.fieldnames or ())
            rows = list(reader)
        self.assertEqual(len(rows), 6_475)
        self.assertEqual(
            hashlib.sha256(ALIGNMENT.read_bytes()).hexdigest(),
            "c1c1f6fdd9100c6fb7f1e88e43ccb2473ea4becb3c9402da9c7abf43b0367acc",
        )
        self.assertTrue(
            {
                "ds_script",
                "ds_settext_ordinal",
                "pc_message_id",
                "jp_ogg_paths",
                "speaker_policy",
            }.issubset(fields)
        )
        self.assertTrue({"ds_text", "pc_en_text"}.isdisjoint(fields))
        self.assertTrue(all(row["selection_status"] == "selected" for row in rows))
        self.assertTrue(
            all(
                not line.endswith(b"\t") for line in ALIGNMENT.read_bytes().splitlines()
            )
        )

    def test_reviewed_override_ledger_is_complete_and_hash_pinned(self) -> None:
        path = REVIEWS / "reviewed_alignment_overrides.tsv"
        overrides = read_overrides(path)
        self.assertEqual(len(overrides), 65)
        self.assertEqual(
            len({(row.ds_script, row.ds_settext_ordinal) for row in overrides}),
            63,
        )
        self.assertEqual(
            Counter(row.review_basis for row in overrides),
            Counter(
                {
                    "multi_page_split": 46,
                    "speaker_alias": 11,
                    "multi_page_composite": 7,
                    "translated_dialogue": 1,
                }
            ),
        )
        self.assertEqual(
            hashlib.sha256(path.read_bytes()).hexdigest(),
            "abee0b504ab42e85e165b2e131b9d7c7cc2be687a17b129990f3406e53bf6f6a",
        )
        self.assertTrue(
            all(not line.endswith(b"\t") for line in path.read_bytes().splitlines())
        )

    def test_tracked_ledgers_match_the_reviewed_inputs(self) -> None:
        expected = {
            "borderline_voice_review.tsv": (
                230,
                "7a2d6ccf5123ca09a08444587c4832a659d63b49534f79ef8d25552ad36d53e7",
            ),
            "alignment_extended_candidates.tsv": (
                242,
                "d0971200b2a2ee88fc973fa7909737181d21d8b2fa4ea5cb4039d3aba5fb4433",
            ),
        }
        for name, (row_count, digest) in expected.items():
            with self.subTest(name=name):
                self.assertEqual(len(read_rows(name)), row_count)
                self.assertEqual(sha256(name), digest)

    def test_borderline_decisions_cover_the_reviewed_population(self) -> None:
        rows = read_rows("borderline_voice_review.tsv")
        self.assertEqual(
            Counter(row["decision"] for row in rows),
            Counter({"accept": 189, "reject": 41}),
        )
        self.assertEqual(len({row["pc_message_id"] for row in rows}), len(rows))

    def test_extended_ledger_preserves_the_seventeen_dialogue_additions(self) -> None:
        rows = read_rows("alignment_extended_candidates.tsv")
        self.assertTrue(
            {
                "ds_text",
                "pc_en_text",
                "pc_jp_speaker_and_text",
                "review_reason",
            }.isdisjoint(rows[0])
        )
        accepted = [row for row in rows if policy_allows(row, "same")]
        self.assertEqual(len(accepted), 17)
        self.assertEqual(
            Counter(row["speaker_policy"] for row in rows),
            Counter({"novel_semantic": 225, "same": 17}),
        )


if __name__ == "__main__":
    unittest.main()
