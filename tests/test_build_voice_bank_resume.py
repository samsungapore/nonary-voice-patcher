#!/usr/bin/env python3

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import build_voice_bank as voice_bank  # noqa: E402


class VoiceBankResumeTests(unittest.TestCase):
    @staticmethod
    def write_inputs(directory: Path) -> dict[str, Path]:
        inputs = {
            "alignment": directory / "alignment.tsv",
            "pc_archive": directory / "ze1_data.bin",
            "pc_manifest": directory / "manifest.tsv",
            "template_se": directory / "template.se",
            "id_source_rom": directory / "source.nds",
        }
        for index, (name, path) in enumerate(inputs.items()):
            path.write_bytes(f"{name}-{index}".encode("ascii"))
        return inputs

    @staticmethod
    def provenance(inputs: dict[str, Path], *, language: str = "jp", **changed_options):
        options = {
            "silence_ms": 65.0,
            "max_duration": 45.0,
            "allow_narration_voices": False,
        }
        options.update(changed_options)
        return voice_bank.build_provenance(
            language=language,
            **options,
            **inputs,
        )

    def test_matching_resume_accepts_an_authenticated_partial_build(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            expected = self.provenance(inputs)

            voice_bank.prepare_output_directory(
                output, expected, resume=False, expected_voice_count=2
            )
            (output / "se_v0000.se").write_bytes(b"partial resource")
            voice_bank.prepare_output_directory(
                output, expected, resume=True, expected_voice_count=2
            )

            stored = json.loads(
                (output / voice_bank.PROVENANCE_FILENAME).read_text(encoding="utf-8")
            )
            self.assertEqual(stored, expected)

    def test_matching_generated_voice_is_kept_and_changed_bytes_are_replaced(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            output = Path(raw_directory) / "se_v0000.se"
            output.write_bytes(b"expected")
            self.assertFalse(
                voice_bank.publish_generated_voice(output, b"expected", resume=True)
            )
            self.assertEqual(output.read_bytes(), b"expected")

            self.assertTrue(
                voice_bank.publish_generated_voice(output, b"regenerated", resume=True)
            )
            self.assertEqual(output.read_bytes(), b"regenerated")

            output.unlink()
            self.assertTrue(
                voice_bank.publish_generated_voice(output, b"new", resume=True)
            )
            self.assertEqual(output.read_bytes(), b"new")

    def test_resume_rejects_each_changed_source_input(self) -> None:
        expected_fields = {
            "alignment": "inputs.alignment",
            "pc_archive": "inputs.pc_archive",
            "pc_manifest": "inputs.jp_manifest",
            "template_se": "inputs.se_template",
            "id_source_rom": "inputs.source_rom",
        }
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            original = self.provenance(inputs)
            voice_bank.prepare_output_directory(
                output, original, resume=False, expected_voice_count=1
            )

            for name, expected_field in expected_fields.items():
                with self.subTest(input=name):
                    path = inputs[name]
                    previous = path.read_bytes()
                    path.write_bytes(previous + b"-changed")
                    changed = self.provenance(inputs)
                    with self.assertRaisesRegex(ValueError, expected_field):
                        voice_bank.prepare_output_directory(
                            output, changed, resume=True, expected_voice_count=1
                        )
                    path.write_bytes(previous)

    def test_resume_rejects_a_changed_language(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            voice_bank.prepare_output_directory(
                output,
                self.provenance(inputs, language="jp"),
                resume=False,
                expected_voice_count=1,
            )
            with self.assertRaisesRegex(ValueError, "language"):
                voice_bank.prepare_output_directory(
                    output,
                    self.provenance(inputs, language="en"),
                    resume=True,
                    expected_voice_count=1,
                )

    def test_resume_rejects_changed_generation_settings(self) -> None:
        changes = {
            "silence_ms": 70.0,
            "max_duration": 44.0,
            "allow_narration_voices": True,
        }
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            original = self.provenance(inputs)
            voice_bank.prepare_output_directory(
                output, original, resume=False, expected_voice_count=1
            )
            for option, value in changes.items():
                with self.subTest(option=option):
                    changed = self.provenance(inputs, **{option: value})
                    with self.assertRaisesRegex(ValueError, f"options.{option}"):
                        voice_bank.prepare_output_directory(
                            output, changed, resume=True, expected_voice_count=1
                        )

    def test_resume_rejects_a_changed_pipeline_version(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            voice_bank.prepare_output_directory(
                output,
                self.provenance(inputs),
                resume=False,
                expected_voice_count=1,
            )
            with patch.object(
                voice_bank, "PIPELINE_VERSION", voice_bank.PIPELINE_VERSION + 1
            ):
                changed = self.provenance(inputs)
            with self.assertRaisesRegex(ValueError, "pipeline_version"):
                voice_bank.prepare_output_directory(
                    output, changed, resume=True, expected_voice_count=1
                )

    def test_resume_rejects_legacy_output_without_a_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            output.mkdir()
            (output / "se_v0000.se").write_bytes(b"legacy resource")
            with self.assertRaisesRegex(ValueError, "is missing"):
                voice_bank.prepare_output_directory(
                    output,
                    self.provenance(inputs),
                    resume=True,
                    expected_voice_count=1,
                )

    def test_non_resume_rejects_preexisting_generated_voice(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            output.mkdir()
            (output / "SE_V0000.SE").write_bytes(b"stale resource")
            with self.assertRaisesRegex(ValueError, "already contains"):
                voice_bank.prepare_output_directory(
                    output,
                    self.provenance(inputs),
                    resume=False,
                    expected_voice_count=1,
                )
            self.assertFalse((output / voice_bank.PROVENANCE_FILENAME).exists())

    def test_resume_rejects_a_generated_file_outside_the_current_symbol_set(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            provenance = self.provenance(inputs)
            voice_bank.prepare_output_directory(
                output, provenance, resume=False, expected_voice_count=1
            )
            (output / "se_v0001.se").write_bytes(b"unexpected resource")
            with self.assertRaisesRegex(ValueError, "unexpected generated file"):
                voice_bank.prepare_output_directory(
                    output, provenance, resume=True, expected_voice_count=1
                )

    def test_resume_rejects_wrong_filename_case(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            inputs = self.write_inputs(directory)
            output = directory / "voices"
            provenance = self.provenance(inputs)
            voice_bank.prepare_output_directory(
                output, provenance, resume=False, expected_voice_count=1
            )
            (output / "SE_V0000.SE").write_bytes(b"unexpected resource")
            with self.assertRaisesRegex(ValueError, "unexpected generated file"):
                voice_bank.prepare_output_directory(
                    output, provenance, resume=True, expected_voice_count=1
                )

    def test_atomic_sidecar_failure_preserves_the_previous_file(self) -> None:
        with tempfile.TemporaryDirectory() as raw_directory:
            directory = Path(raw_directory)
            sidecar = directory / voice_bank.PROVENANCE_FILENAME
            sidecar.write_bytes(b"previous sidecar")
            with (
                patch.object(Path, "replace", side_effect=OSError("replace failed")),
                self.assertRaisesRegex(OSError, "replace failed"),
            ):
                voice_bank.atomic_write_json(sidecar, {"version": 2})
            self.assertEqual(sidecar.read_bytes(), b"previous sidecar")
            leftovers = [
                path
                for path in directory.iterdir()
                if path.name != voice_bank.PROVENANCE_FILENAME
            ]
            self.assertEqual(leftovers, [])


if __name__ == "__main__":
    unittest.main()
