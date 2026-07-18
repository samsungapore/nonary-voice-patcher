#!/usr/bin/env python3
"""Rebuild the bundled voice profile from reviewed compatibility data."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "src-tauri" / "Cargo.toml"
VOICE_MAP = ROOT / "build" / "voice_map_extended_dialogue_only.tsv"
STOCK_ROMFS = ROOT / "work" / "romfs"
COMPATIBILITY_MANIFEST = (
    ROOT / "research" / "compatibility" / "voice-profile-compatibility.json"
)
DEFAULT_OUTPUT = ROOT / "src-tauri" / "resources" / "voice-profile.json"


def rebuild(
    output: Path,
    voice_map: Path,
    stock_romfs: Path,
    compatibility_manifest: Path | None,
    french_fsb: Path | None,
    alternative_fsb_roots: tuple[Path, ...],
) -> None:
    required_inputs = [MANIFEST, voice_map, stock_romfs]
    if compatibility_manifest is not None:
        required_inputs.append(compatibility_manifest)
    if french_fsb is not None:
        required_inputs.extend((french_fsb, *alternative_fsb_roots))
    for required in required_inputs:
        if not required.exists():
            raise FileNotFoundError(f"required input is missing: {required}")
    output.parent.mkdir(parents=True, exist_ok=True)
    command = [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(MANIFEST),
        "--no-default-features",
        "--bin",
        "build_voice_profile",
        "--features",
        "dev-tools",
        "--",
        str(voice_map),
        str(stock_romfs),
        str(output),
    ]
    if compatibility_manifest is not None:
        command.extend(("--compatibility-manifest", str(compatibility_manifest)))
    else:
        command.extend(("--french-fsb-root", str(french_fsb)))
        for root in alternative_fsb_roots:
            command.extend(("--alternative-fsb-root", str(root)))
    subprocess.run(
        command,
        check=True,
        cwd=ROOT,
    )


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--voice-map", type=Path, default=VOICE_MAP)
    parser.add_argument("--stock-romfs", type=Path, default=STOCK_ROMFS)
    parser.add_argument(
        "--compatibility-manifest",
        type=Path,
        help=f"reviewed compatibility input (default: {COMPATIBILITY_MANIFEST})",
    )
    parser.add_argument(
        "--french-fsb-root",
        type=Path,
        help="recompute compatibility from the raw French patcher FSB directory",
    )
    parser.add_argument(
        "--alternative-fsb-root",
        type=Path,
        action="append",
        default=[],
        help="additional raw compatibility tree (requires --french-fsb-root)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="rebuild in a temporary directory and fail if the resource differs",
    )
    args = parser.parse_args()
    output = args.output.resolve()
    voice_map = args.voice_map.resolve()
    stock_romfs = args.stock_romfs.resolve()
    if args.compatibility_manifest is not None and args.french_fsb_root is not None:
        parser.error(
            "--compatibility-manifest and --french-fsb-root are mutually exclusive"
        )
    if args.alternative_fsb_root and args.french_fsb_root is None:
        parser.error("--alternative-fsb-root requires --french-fsb-root")
    french_fsb = (
        args.french_fsb_root.resolve() if args.french_fsb_root is not None else None
    )
    compatibility_manifest = (
        args.compatibility_manifest.resolve()
        if args.compatibility_manifest is not None
        else None if french_fsb is not None else COMPATIBILITY_MANIFEST.resolve()
    )
    alternative_fsb_roots = tuple(path.resolve() for path in args.alternative_fsb_root)
    if args.check:
        if not output.is_file():
            raise FileNotFoundError(f"profile resource is missing: {output}")
        with tempfile.TemporaryDirectory(prefix="nonary-profile-") as temporary:
            generated = Path(temporary) / "voice-profile.json"
            rebuild(
                generated,
                voice_map,
                stock_romfs,
                compatibility_manifest,
                french_fsb,
                alternative_fsb_roots,
            )
            if generated.read_bytes() != output.read_bytes():
                raise SystemExit(
                    f"voice profile is stale: generated {digest(generated)}, "
                    f"bundled {digest(output)}"
                )
        print(f"voice profile is reproducible: {digest(output)}")
        return 0

    rebuild(
        output,
        voice_map,
        stock_romfs,
        compatibility_manifest,
        french_fsb,
        alternative_fsb_roots,
    )
    print(f"wrote {output} ({digest(output)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
