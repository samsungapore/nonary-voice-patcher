#!/usr/bin/env python3

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from apply_reviewed_alignment_overrides import (  # noqa: E402
    LEDGER_FIELDS,
    apply_overrides,
    raw_settext_sha256,
    read_overrides,
)


HASH_A = "ab" * 32


def override_row(**updates: str) -> dict[str, str]:
    row = {
        "override_group": "single",
        "component_order": "0",
        "target_action": "add",
        "replace_pc_message_id": "",
        "pc_message_id": "A01e_010_010_01_00",
        "ds_script": "a01e.fsb.txt",
        "ds_settext_ordinal": "41",
        "ds_speaker": "踊り子２",
        "pc_speaker": "踊り子",
        "target_text_sha256": "",
        "jp_start_ms": "",
        "jp_end_ms": "",
        "en_start_ms": "",
        "en_end_ms": "",
        "review_basis": "speaker_alias",
    }
    row.update(updates)
    return row


def write_ledger(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=LEDGER_FIELDS, delimiter="\t")
        writer.writeheader()
        writer.writerows(rows)


class ReviewedAlignmentOverrideTests(unittest.TestCase):
    def read(self, rows: list[dict[str, str]]):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "reviewed.tsv"
            write_ledger(path, rows)
            return read_overrides(path)

    def test_exact_alias_conditional_target_and_language_slices_are_valid(self) -> None:
        rows = [
            override_row(),
            override_row(
                override_group="translated",
                pc_message_id="M10a_110_190_01_10",
                ds_script="m10a.fsb.txt",
                ds_settext_ordinal="563",
                ds_speaker="淳平",
                pc_speaker="淳平",
                target_text_sha256=f"{HASH_A} | {'cd' * 32}",
                review_basis="translated_dialogue",
            ),
            override_row(
                override_group="niels_split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="16",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="0",
                jp_end_ms="3200",
                en_start_ms="0",
                en_end_ms="3050",
                review_basis="multi_page_split",
            ),
            override_row(
                override_group="niels_split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="17",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="3200",
                jp_end_ms="",
                en_start_ms="3050",
                en_end_ms="",
                review_basis="multi_page_split",
            ),
        ]
        parsed = self.read(rows)
        self.assertEqual(len(parsed), 4)
        self.assertEqual(parsed[1].target_text_sha256, (HASH_A, "cd" * 32))
        self.assertEqual(parsed[2].jp_interval.start_ms, 0)
        self.assertEqual(parsed[3].en_interval.start_ms, 3050)

    def test_hashes_are_exact_lowercase_sha256_values(self) -> None:
        with self.assertRaisesRegex(ValueError, "lowercase SHA-256"):
            self.read([override_row(target_text_sha256=HASH_A.upper())])

    def test_wildcard_speaker_equivalence_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "wildcard"):
            self.read([override_row(ds_speaker="踊り子*")])

    def test_split_targets_must_be_consecutive(self) -> None:
        rows = [
            override_row(
                override_group="split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="16",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="0",
                jp_end_ms="1000",
                en_start_ms="0",
                en_end_ms="1000",
                review_basis="multi_page_split",
            ),
            override_row(
                override_group="split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="18",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="1000",
                en_start_ms="1000",
                review_basis="multi_page_split",
            ),
        ]
        with self.assertRaisesRegex(ValueError, "consecutive ordinals"):
            self.read(rows)

    def test_split_intervals_cannot_overlap(self) -> None:
        rows = [
            override_row(
                override_group="split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="16",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="0",
                jp_end_ms="1000",
                en_start_ms="0",
                en_end_ms="1000",
                review_basis="multi_page_split",
            ),
            override_row(
                override_group="split",
                pc_message_id="B11d_010_060_02_10",
                ds_script="b11d.fsb.txt",
                ds_settext_ordinal="17",
                ds_speaker="ニルス",
                pc_speaker="ニルス",
                jp_start_ms="999",
                en_start_ms="1000",
                review_basis="multi_page_split",
            ),
        ]
        with self.assertRaisesRegex(ValueError, "overlapping JP"):
            self.read(rows)

    def test_application_checks_exact_speakers_and_exports_runtime_metadata(
        self,
    ) -> None:
        override = self.read([override_row(target_text_sha256=HASH_A)])[0]
        pc = {
            override.pc_message_id: {
                "id": override.pc_message_id,
                "speaker": "踊り子",
                "en_text": "A companion.",
                "jp": "踊り子 :: 仲間。",
                "paths": [f"/sound/voice/{override.pc_message_id}_00.ogg"],
            }
        }
        ds = [
            {
                "line": number + 10,
                "ordinal": number,
                "speaker": "踊り子２" if number == 41 else "NOVEL",
                "text": "Target" if number == 41 else "Other",
            }
            for number in range(42)
        ]
        result = apply_overrides([], [override], pc, {"a01e": ds})
        self.assertEqual(result[0]["speaker_policy"], "reviewed_equivalent")
        self.assertEqual(result[0]["target_text_sha256"], HASH_A)
        self.assertEqual(result[0]["alignment_method"], "reviewed_override")

        pc[override.pc_message_id]["speaker"] = "踊り子２"
        with self.assertRaisesRegex(ValueError, "PC speaker changed"):
            apply_overrides([], [override], pc, {"a01e": ds})

    def test_application_refuses_baseline_message_reuse(self) -> None:
        override = self.read([override_row()])[0]
        baseline = [
            {
                "ds_script": "other.fsb.txt",
                "ds_settext_ordinal": "0",
                "pc_message_id": override.pc_message_id,
            }
        ]
        with self.assertRaisesRegex(ValueError, "reuses baseline PC message"):
            apply_overrides(baseline, [override], {}, {})

    def test_exact_replacement_can_compose_a_split_tail_and_baseline_voice(
        self,
    ) -> None:
        split_message = "A41d_010_540_03_10"
        baseline_message = "A41d_010_540_03_20"
        rows = [
            override_row(
                override_group="a41d_03",
                pc_message_id=split_message,
                ds_script="a41d.fsb.txt",
                ds_settext_ordinal="163",
                ds_speaker="ニルス２",
                pc_speaker="ニルス２",
                jp_start_ms="0",
                jp_end_ms="2223",
                en_start_ms="0",
                en_end_ms="4550",
                review_basis="multi_page_composite",
            ),
            override_row(
                override_group="a41d_03",
                pc_message_id=split_message,
                ds_script="a41d.fsb.txt",
                ds_settext_ordinal="164",
                ds_speaker="ニルス２",
                pc_speaker="ニルス２",
                jp_start_ms="2223",
                en_start_ms="4550",
                target_action="replace",
                replace_pc_message_id=baseline_message,
                review_basis="multi_page_composite",
            ),
            override_row(
                override_group="a41d_03",
                component_order="1",
                pc_message_id=baseline_message,
                ds_script="a41d.fsb.txt",
                ds_settext_ordinal="164",
                ds_speaker="ニルス２",
                pc_speaker="ニルス２",
                target_action="replace",
                replace_pc_message_id=baseline_message,
                review_basis="multi_page_composite",
            ),
        ]
        overrides = self.read(rows)
        pc = {
            message_id: {
                "id": message_id,
                "speaker": "ニルス２",
                "en_text": message_id,
                "jp": message_id,
                "paths": [f"/sound/voice/{message_id}_00.ogg"],
            }
            for message_id in (split_message, baseline_message)
        }
        ds = [
            {
                "line": ordinal + 400,
                "ordinal": ordinal,
                "speaker": "ニルス２" if ordinal >= 163 else "NOVEL",
                "text": f"DS {ordinal}",
            }
            for ordinal in range(165)
        ]
        baseline = [
            {
                "ds_script": "a41d.fsb.txt",
                "ds_source_line": "564",
                "ds_settext_ordinal": "164",
                "pc_message_id": baseline_message,
            }
        ]
        result = apply_overrides(baseline, overrides, pc, {"a41d": ds})
        self.assertEqual(len(result), 2)
        composite = next(row for row in result if row["ds_settext_ordinal"] == "164")
        self.assertEqual(
            composite["pc_message_id"], f"{split_message} | {baseline_message}"
        )
        self.assertEqual(composite["message_count"], "2")
        self.assertEqual(composite["jp_start_ms"], "2223 | ")
        self.assertEqual(composite["jp_end_ms"], " | ")
        self.assertEqual(composite["en_start_ms"], "4550 | ")
        self.assertEqual(composite["en_end_ms"], " | ")

    def test_replacement_requires_the_declared_exact_baseline_message(self) -> None:
        baseline_message = "A41d_010_540_03_20"
        rows = [
            override_row(
                override_group="replace",
                target_action="replace",
                replace_pc_message_id=baseline_message,
                pc_message_id=baseline_message,
                review_basis="multi_page_composite",
            ),
            override_row(
                override_group="replace",
                component_order="1",
                target_action="replace",
                replace_pc_message_id=baseline_message,
                pc_message_id="A41d_010_540_03_10",
                review_basis="multi_page_composite",
            ),
        ]
        overrides = self.read(rows)
        override = overrides[0]
        baseline = [
            {
                "ds_script": override.ds_script,
                "ds_settext_ordinal": str(override.ds_settext_ordinal),
                "pc_message_id": "A41d_010_540_03_30",
            }
        ]
        with self.assertRaisesRegex(ValueError, "expected baseline PC message"):
            apply_overrides(baseline, overrides, {}, {})

    def test_composite_component_orders_must_be_contiguous(self) -> None:
        rows = [
            override_row(),
            override_row(
                component_order="2",
                pc_message_id="A01e_010_010_01_10",
            ),
        ]
        with self.assertRaisesRegex(ValueError, "component_order values"):
            self.read(rows)

    def test_all_components_must_repeat_the_exact_replacement_claim(self) -> None:
        baseline_message = "A41d_010_540_03_20"
        rows = [
            override_row(
                target_action="replace",
                replace_pc_message_id=baseline_message,
                pc_message_id=baseline_message,
            ),
            override_row(
                component_order="1",
                pc_message_id="A41d_010_540_03_30",
            ),
        ]
        with self.assertRaisesRegex(ValueError, "inconsistent component metadata"):
            self.read(rows)

    def test_raw_text_hash_does_not_normalize_payload_bytes(self) -> None:
        self.assertNotEqual(
            raw_settext_sha256("é".encode("utf-8")),
            raw_settext_sha256("e\u0301".encode("utf-8")),
        )


if __name__ == "__main__":
    unittest.main()
