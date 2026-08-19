#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import struct
import sys
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import build_voice_pack as packer  # noqa: E402


class VoicePackBuilderTests(unittest.TestCase):
    def test_french_pack_uses_the_runtime_language_code(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bank = root / "bank"
            bank.mkdir()
            payload = b"SIR0synthetic-test-payload"
            (bank / "se_v0000.se").write_bytes(payload)
            output = root / "voices-fr.nvpack"

            count, payload_size, digest = packer.build_pack(bank, output, "fr")
            header = output.read_bytes()[: packer.HEADER_SIZE]

        self.assertEqual(count, 1)
        self.assertEqual(payload_size, len(payload))
        self.assertEqual(len(digest), 64)
        self.assertEqual(header[:8], packer.MAGIC)
        self.assertEqual(header[16:20], b"fr\0\0")

    def test_unknown_language_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "jp, en, or fr"):
                packer.build_pack(root, root / "voices.nvpack", "de")

    def test_catalog_pack_uses_v2_and_binds_catalog_to_index_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bank = root / "bank"
            bank.mkdir()
            payload = b"SIR0catalog-bound-payload"
            (bank / "se_v0000.se").write_bytes(payload)
            output = root / "voices-fr.nvpack"
            catalog = hashlib.sha256(b"catalog fixture").digest()

            packer.build_pack(bank, output, "fr", catalog)
            data = output.read_bytes()

        self.assertEqual(struct.unpack_from("<I", data, 8)[0], packer.VERSION_V2)
        self.assertEqual(struct.unpack_from("<Q", data, 24)[0], packer.HEADER_SIZE_V2)
        self.assertEqual(data[0x70:0x90], catalog)
        raw_index = data[packer.HEADER_SIZE_V2 : packer.HEADER_SIZE_V2 + packer.ENTRY_SIZE]
        expected = hashlib.sha256(
            packer.V2_INDEX_DOMAIN + catalog + raw_index
        ).digest()
        self.assertEqual(data[0x30:0x50], expected)

    def test_output_cannot_replace_an_input_voice(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bank = Path(directory) / "bank"
            bank.mkdir()
            voice = bank / "se_v0000.se"
            original = b"SIR0irreplaceable-source"
            voice.write_bytes(original)

            with self.assertRaisesRegex(ValueError, "overwrite an input voice"):
                packer.build_pack(bank, voice, "fr")

            self.assertEqual(voice.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
