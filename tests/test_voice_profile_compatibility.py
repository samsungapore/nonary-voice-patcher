import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
PROFILE = ROOT / "src-tauri" / "resources" / "voice-profile.json"
COMPATIBILITY = (
    ROOT / "research" / "compatibility" / "voice-profile-compatibility.json"
)


class VoiceProfileCompatibilityTests(unittest.TestCase):
    def test_manifest_matches_the_bundled_profile(self) -> None:
        profile = json.loads(PROFILE.read_text(encoding="utf-8"))
        compatibility = json.loads(COMPATIBILITY.read_text(encoding="utf-8"))

        alternatives = {
            script["path"]: script["alternative_structural_sha256"]
            for script in profile["scripts"]
            if script.get("alternative_structural_sha256")
        }
        manifest_alternatives = {
            script["path"]: script["alternative_structural_sha256"]
            for script in compatibility["alternative_scripts"]
        }

        self.assertEqual(compatibility["version"], 1)
        self.assertEqual(compatibility["game_code"], profile["game_code"])
        self.assertEqual(manifest_alternatives, alternatives)
        self.assertEqual(compatibility["exact_repairs"], profile["exact_repairs"])

    def test_manifest_contains_no_dialogue_or_voice_map(self) -> None:
        compatibility = json.loads(COMPATIBILITY.read_text(encoding="utf-8"))

        self.assertEqual(
            set(compatibility),
            {"version", "game_code", "alternative_scripts", "exact_repairs"},
        )
        for script in compatibility["alternative_scripts"]:
            self.assertEqual(
                set(script), {"path", "alternative_structural_sha256"}
            )


if __name__ == "__main__":
    unittest.main()
