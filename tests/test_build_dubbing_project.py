#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
from pathlib import Path

import numpy as np
import soundfile as sf


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT / "scripts"))

import build_dubbing_project as builder  # noqa: E402


class DubbingProjectBuilderTests(unittest.TestCase):
    def test_profile_order_is_the_runtime_symbol_order(self) -> None:
        profile = {
            "version": 1,
            "voice_count": 2,
            "scripts": [
                {
                    "path": "scr/a01b.fsb",
                    "voices": [
                        {"symbol": "SE_V0000", "ordinal": 88},
                        {"symbol": "SE_V0001", "ordinal": 92},
                    ],
                }
            ],
        }
        self.assertEqual(
            builder.profile_targets(profile),
            [
                ("SE_V0000", "scr/a01b.fsb", 88),
                ("SE_V0001", "scr/a01b.fsb", 92),
            ],
        )
        profile["scripts"][0]["voices"][1]["symbol"] = "SE_V0002"
        with self.assertRaisesRegex(ValueError, "expected SE_V0001"):
            builder.profile_targets(profile)

    def test_catalog_digest_matches_the_cross_language_contract(self) -> None:
        digest = builder.catalog_sha256(
            [
                ("SE_V0000", "scr/a01b.fsb", 88),
                ("SE_V0001", "scr/a01b.fsb", 92),
            ]
        )
        self.assertEqual(
            digest.hex(),
            "5ccf96e3ae549556643b71b0039671a28da9f9eacd0797783d2657eb4d398c55",
        )

    def test_take_path_cannot_escape_the_project(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            self.assertEqual(
                builder.safe_project_file(project, "recordings/SE_V0000/take.wav"),
                project.resolve() / "recordings/SE_V0000/take.wav",
            )
            with self.assertRaisesRegex(ValueError, "escapes"):
                builder.safe_project_file(project, "../take.wav")

    @unittest.skipIf(os.name == "nt", "symlink creation is not reliably available in Windows CI")
    def test_take_path_rejects_a_symlink_even_when_it_resolves_inside_project(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            recordings = project / "recordings"
            real_target = recordings / "real"
            real_target.mkdir(parents=True)
            (recordings / "SE_V0000").symlink_to(real_target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symbolic link"):
                builder.safe_project_file(
                    project, "recordings/SE_V0000/take.wav"
                )

    def test_output_pack_cannot_replace_inputs_or_recordings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            manifest = project / builder.PROJECT_FILE
            profile = project / "voice-profile.json"
            rom = project / "999.nds"
            recordings = project / "recordings"
            recordings.mkdir()
            for path in (manifest, profile, rom):
                path.write_bytes(b"fixture")

            builder.validate_output_pack_path(
                project / "builds" / "voices-fr.nvpack",
                project,
                manifest,
                profile,
                rom,
            )
            for unsafe in (
                manifest,
                project / builder.PROJECT_LOCK_FILE,
                profile,
                rom,
                recordings / "take.wav",
            ):
                with self.assertRaisesRegex(ValueError, "overwrite|required|recordings"):
                    builder.validate_output_pack_path(
                        unsafe, project, manifest, profile, rom
                    )
            with self.assertRaisesRegex(ValueError, "overwrite"):
                builder.validate_output_pack_path(
                    (project / "project.nvdub").with_suffix(".nvdub.json"),
                    project,
                    manifest,
                    profile,
                    rom,
                )

    def test_build_holds_the_project_lock_until_publication_returns(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            project = Path(directory)
            marker = project / "contender-acquired"
            child: subprocess.Popen[str] | None = None

            def fake_publication(_arguments: object, locked_project: Path) -> dict[str, int]:
                nonlocal child
                script = "\n".join(
                    (
                        "import sys",
                        "from pathlib import Path",
                        "from build_dubbing_project import project_mutation_lock",
                        "print('ready', flush=True)",
                        "with project_mutation_lock(Path(sys.argv[1])):",
                        "    Path(sys.argv[2]).write_text('acquired', encoding='utf-8')",
                    )
                )
                environment = os.environ.copy()
                environment["PYTHONPATH"] = str(PROJECT_ROOT / "scripts")
                child = subprocess.Popen(
                    [sys.executable, "-c", script, str(locked_project), str(marker)],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    env=environment,
                )
                assert child.stdout is not None
                self.assertEqual(child.stdout.readline().strip(), "ready")
                with self.assertRaises(subprocess.TimeoutExpired):
                    child.wait(timeout=0.2)
                self.assertFalse(marker.exists())
                return {"voices": 0}

            arguments = SimpleNamespace(project_dir=project)
            with mock.patch.object(builder, "_build_locked", side_effect=fake_publication):
                self.assertEqual(builder.build(arguments), {"voices": 0})

            assert child is not None
            try:
                _stdout, stderr = child.communicate(timeout=5)
            except BaseException:
                child.kill()
                child.communicate()
                raise
            self.assertEqual(child.returncode, 0, stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "acquired")

    def test_wav_master_is_downmixed_resampled_trimmed_and_gained(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "take.wav"
            rate = 48_000
            seconds = 0.2
            timeline = np.arange(round(rate * seconds), dtype=np.float32) / rate
            left = np.sin(2 * np.pi * 440 * timeline) * 0.2
            right = np.sin(2 * np.pi * 440 * timeline) * 0.1
            sf.write(path, np.column_stack((left, right)), rate, subtype="PCM_16")
            take = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

            pcm = builder.decode_master(
                path,
                {"trimStartMs": 25, "trimEndMs": 25, "gainDb": 6.0},
                take,
                "production",
            )

        self.assertEqual(pcm.dtype, np.int16)
        self.assertAlmostEqual(len(pcm) / builder.SAMPLE_RATE, 0.15, places=2)
        self.assertGreater(int(np.max(np.abs(pcm))), 8_000)

    def test_production_decodes_only_the_authenticated_non_clipping_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "take.wav"
            sf.write(path, np.ones(800, dtype=np.float32), 8_000, subtype="PCM_16")
            take = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            target = {"trimStartMs": 0, "trimEndMs": 0, "gainDb": 0.0}

            with self.assertRaisesRegex(ValueError, "would clip"):
                builder.decode_master(path, target, take, "production")
            preview = builder.decode_master(path, target, take, "preview")
            self.assertEqual(preview.dtype, np.int16)

            path.write_bytes(path.read_bytes() + b"changed")
            with self.assertRaisesRegex(ValueError, "changed after project validation"):
                builder.decode_master(path, target, take, "preview")

    def test_project_validation_binds_rom_profile_and_target_identity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile_path = root / "voice-profile.json"
            profile = {
                "version": 1,
                "game_code": "BSKE",
                "voice_count": 1,
                "scripts": [
                    {
                        "path": "scr/a01b.fsb",
                        "voices": [{"symbol": "SE_V0000", "ordinal": 88}],
                    }
                ],
            }
            profile_path.write_text(json.dumps(profile), encoding="utf-8")
            rom_path = root / "999.nds"
            rom = bytearray(0x200)
            rom[0x0C:0x10] = b"BSKE"
            rom_path.write_bytes(rom)
            project = {
                "version": 1,
                "profileSha256": hashlib.sha256(profile_path.read_bytes()).hexdigest(),
                "profileCatalogSha256": builder.catalog_sha256(
                    builder.profile_targets(profile)
                ).hex(),
                "sourceRomSha256": hashlib.sha256(rom).hexdigest(),
                "gameCode": "BSKE",
                "targets": {
                    "SE_V0000": {
                        "targetId": "scr/a01b.fsb#88",
                        "status": "missing",
                        "takes": [],
                        "activeTake": None,
                    }
                },
            }
            targets = builder.profile_targets(profile)
            builder.validate_project(
                project, root, profile_path, profile, targets, rom_path, bytes(rom)
            )
            profile_path.write_text(
                json.dumps(profile, indent=2, sort_keys=True), encoding="utf-8"
            )
            builder.validate_project(
                project, root, profile_path, profile, targets, rom_path, bytes(rom)
            )
            project["targets"]["SE_V0000"]["targetId"] = "scr/a01b.fsb#89"
            with self.assertRaisesRegex(ValueError, "expected"):
                builder.validate_project(
                    project, root, profile_path, profile, targets, rom_path, bytes(rom)
                )

    def test_project_validation_binds_take_and_approval_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile_path = root / "voice-profile.json"
            profile = {
                "version": 1,
                "game_code": "BSKE",
                "voice_count": 1,
                "scripts": [
                    {
                        "path": "scr/a01b.fsb",
                        "voices": [{"symbol": "SE_V0000", "ordinal": 88}],
                    }
                ],
            }
            profile_path.write_text(json.dumps(profile), encoding="utf-8")
            rom_path = root / "999.nds"
            rom = bytearray(0x200)
            rom[0x0C:0x10] = b"BSKE"
            rom_path.write_bytes(rom)
            take_path = root / "recordings" / "SE_V0000" / "take-1.wav"
            take_path.parent.mkdir(parents=True)
            sf.write(take_path, np.zeros(800, dtype=np.int16), 8_000, subtype="PCM_16")
            take = {
                "id": "take-1",
                "file": "recordings/SE_V0000/take-1.wav",
                "sha256": hashlib.sha256(take_path.read_bytes()).hexdigest(),
            }
            target = {
                "targetId": "scr/a01b.fsb#88",
                "status": "approved",
                "takes": [take],
                "activeTake": "take-1",
                "gainDb": 0.0,
                "trimStartMs": 0,
                "trimEndMs": 0,
            }
            target["approvalSha256"] = builder.approval_sha256(target, take)
            targets = builder.profile_targets(profile)
            project = {
                "version": 1,
                "profileSha256": hashlib.sha256(profile_path.read_bytes()).hexdigest(),
                "profileCatalogSha256": builder.catalog_sha256(targets).hex(),
                "sourceRomSha256": hashlib.sha256(rom).hexdigest(),
                "gameCode": "BSKE",
                "targets": {"SE_V0000": target},
            }
            builder.validate_project(
                project, root, profile_path, profile, targets, rom_path, bytes(rom)
            )

            target["gainDb"] = 1.0
            with self.assertRaisesRegex(ValueError, "approval does not match"):
                builder.validate_project(
                    project, root, profile_path, profile, targets, rom_path, bytes(rom)
                )
            target["gainDb"] = 0.0
            take_path.write_bytes(take_path.read_bytes() + b"tamper")
            with self.assertRaisesRegex(ValueError, "integrity check"):
                builder.validate_project(
                    project, root, profile_path, profile, targets, rom_path, bytes(rom)
                )


if __name__ == "__main__":
    unittest.main()
