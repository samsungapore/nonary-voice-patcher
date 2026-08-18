#!/usr/bin/env python3

from __future__ import annotations

import csv
import hashlib
import re
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import inject_voice_calls as injector  # noqa: E402


def set_text_match(line: str) -> re.Match[str]:
    match = injector.SET_TEXT.fullmatch(line)
    if match is None:
        raise AssertionError(f"test fixture is not a setText call: {line!r}")
    return match


def write_map(path: Path, *, target_hash: str | None = "") -> None:
    fields = ["symbol", "ds_script", "ds_source_line"]
    if target_hash is not None:
        fields.append("target_text_sha256")
    row = {
        "symbol": "SE_V0001",
        "ds_script": "m10a.fsb.txt",
        "ds_source_line": "1",
    }
    if target_hash is not None:
        row["target_text_sha256"] = target_hash
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, delimiter="\t")
        writer.writeheader()
        writer.writerow(row)


class InjectVoiceCallsTests(unittest.TestCase):
    def test_raw_set_text_bytes_match_compiler_and_accent_substitution(self) -> None:
        match = set_text_match('\tsetText("D~0j~4 ~Ea.⑲Un \\"test\\".▼");')
        self.assertEqual(
            injector.raw_set_text_bytes(match),
            b'D\x84\xbfj\x84\xc3 \x84\xcaa.\x87\x52Un "test".\x81\xa5',
        )

    def test_matching_text_hash_injects_voice_calls(self) -> None:
        line = '\tsetText("D~0j~4 ~Ea.▼");'
        digest = hashlib.sha256(
            injector.raw_set_text_bytes(set_text_match(line))
        ).hexdigest()
        target = injector.VoiceTarget("SE_V0001", (digest,))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.txt"
            output = root / "output.txt"
            source.write_text(f"{line}\n", encoding="utf-8")
            result = injector.inject_file(source, output, {1: target})

            self.assertEqual(result, injector.InjectionResult(1, 0))
            self.assertEqual(
                output.read_text(encoding="utf-8"),
                '\tSound::PlaySE(":SE_V0001", 127f, 0f);\n'
                f"{line}\n"
                '\tSound::WaitSE(":SE_V0001");\n',
            )

    def test_nonmatching_text_hash_leaves_source_line_unmodified(self) -> None:
        line = '\tsetText("The source translation changed.▼");'
        target = injector.VoiceTarget("SE_V0001", ("00" * 32,))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.txt"
            output = root / "output.txt"
            source.write_text(f"{line}\n", encoding="utf-8")
            result = injector.inject_file(source, output, {1: target})

            self.assertEqual(result, injector.InjectionResult(0, 1))
            self.assertEqual(output.read_text(encoding="utf-8"), f"{line}\n")

    def test_second_allowed_text_hash_can_match(self) -> None:
        line = '\tsetText("Reviewed translation.▼");'
        digest = hashlib.sha256(
            injector.raw_set_text_bytes(set_text_match(line))
        ).hexdigest()
        target = injector.VoiceTarget("SE_V0001", ("00" * 32, digest))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.txt"
            output = root / "output.txt"
            source.write_text(f"{line}\n", encoding="utf-8")
            result = injector.inject_file(source, output, {1: target})

            self.assertEqual(result, injector.InjectionResult(1, 0))

    def test_map_without_hash_column_remains_unconditional(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            voice_map = Path(directory) / "voice-map.tsv"
            write_map(voice_map, target_hash=None)
            target = injector.load_map(voice_map)["m10a.fsb.txt"][1]
            self.assertEqual(target.target_text_sha256, ())

    def test_load_map_rejects_malformed_text_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            voice_map = Path(directory) / "voice-map.tsv"
            write_map(voice_map, target_hash="not-a-sha256")
            with self.assertRaisesRegex(ValueError, "Invalid target-text SHA-256"):
                injector.load_map(voice_map)

    def test_load_map_rejects_duplicate_text_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            voice_map = Path(directory) / "voice-map.tsv"
            digest = "ab" * 32
            write_map(voice_map, target_hash=f"{digest} | {digest.upper()}")
            with self.assertRaisesRegex(ValueError, "Duplicate target-text SHA-256"):
                injector.load_map(voice_map)


if __name__ == "__main__":
    unittest.main()
