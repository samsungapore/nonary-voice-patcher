#!/usr/bin/env python3

from __future__ import annotations

import sys
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from decompile_scripts import to_wine_path  # noqa: E402


class CrossOverPathTests(unittest.TestCase):
    def test_home_path_uses_crossovers_home_drive(self) -> None:
        source = Path.home() / "project" / "script.fsb"
        self.assertEqual(to_wine_path(source), "Y:\\project\\script.fsb")

    def test_external_path_uses_wines_root_drive(self) -> None:
        source = Path("/tmp/nonary/script.fsb")
        expected = "Z:" + str(source.resolve()).replace("/", "\\")
        self.assertEqual(to_wine_path(source), expected)


if __name__ == "__main__":
    unittest.main()
