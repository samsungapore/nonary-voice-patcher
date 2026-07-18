#!/usr/bin/env python3

from __future__ import annotations

import sys
import unittest
from pathlib import Path, PurePosixPath

from ndspy.fnt import Folder


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

from extract_nds_romfs import named_files, required_for_research  # noqa: E402


class NitroFsExtractionTests(unittest.TestCase):
    def test_named_files_preserve_paths_and_file_ids(self) -> None:
        root = Folder(
            files=["root.bin"],
            firstID=2,
            folders=[
                (
                    "scr",
                    Folder(files=["a01b.fsb", "a01c.fsb"], firstID=7),
                )
            ],
        )
        self.assertEqual(
            list(named_files(root)),
            [
                (PurePosixPath("root.bin"), 2),
                (PurePosixPath("scr/a01b.fsb"), 7),
                (PurePosixPath("scr/a01c.fsb"), 8),
            ],
        )

    def test_named_files_reject_path_components(self) -> None:
        for name in (
            "../escape",
            "..\\escape",
            "/absolute",
            "C:\\escape",
            "name:stream",
            "trailing.",
            ".",
            "",
        ):
            with self.subTest(name=name):
                with self.assertRaises(ValueError):
                    list(named_files(Folder(files=[name], firstID=0)))

    def test_named_files_reject_directory_traversal(self) -> None:
        child = Folder(files=["payload.bin"], firstID=0)
        with self.assertRaises(ValueError):
            list(named_files(Folder(folders=[("..\\escape", child)])))

    def test_research_filter_keeps_only_pipeline_inputs(self) -> None:
        kept = {
            PurePosixPath("scr/a01b.fsb"),
            PurePosixPath("etc/sound.dat"),
            PurePosixPath("sound/se_a01b_wake.se"),
            PurePosixPath("sound/se_sys.se"),
        }
        skipped = {
            PurePosixPath("data/home_03@invalid|windows.dat"),
            PurePosixPath("sound/bgm_01.se"),
        }
        self.assertTrue(all(required_for_research(path) for path in kept))
        self.assertFalse(any(required_for_research(path) for path in skipped))


if __name__ == "__main__":
    unittest.main()
