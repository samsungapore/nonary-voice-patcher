#!/usr/bin/env python3

from __future__ import annotations

import sys
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from merge_extended_alignment import policy_allows  # noqa: E402


class SpeakerPolicyTests(unittest.TestCase):
    def test_same_policy_accepts_only_matching_named_speakers(self) -> None:
        row = {
            "ds_speaker": "淳平",
            "pc_speaker": "淳平",
            "speaker_policy": "same",
        }
        self.assertTrue(policy_allows(row, "same"))

        row["pc_speaker"] = "セブン"
        self.assertFalse(policy_allows(row, "same"))

    def test_same_policy_rejects_narration_even_if_labels_match(self) -> None:
        for speaker in ("NOVEL", "HERO"):
            row = {
                "ds_speaker": speaker,
                "pc_speaker": speaker,
                "speaker_policy": "same",
            }
            self.assertFalse(policy_allows(row, "same"))

    def test_all_policy_preserves_existing_extended_behavior(self) -> None:
        row = {
            "ds_speaker": "NOVEL",
            "pc_speaker": "淳平",
            "speaker_policy": "novel_semantic",
        }
        self.assertTrue(policy_allows(row, "all"))


if __name__ == "__main__":
    unittest.main()
