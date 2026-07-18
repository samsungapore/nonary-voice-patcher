#!/usr/bin/env python3
"""Extract the small set of authentic 999 assets used by the Tauri frontend.

The generated files are build assets only. They come from the user's own USA
ROM and are intentionally kept separate from the binary patch resources.

Requirements:
    pip install ndspy Pillow
    vgmstream-cli and ffmpeg in PATH (only for the music track)
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import ndspy.rom
from PIL import Image


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROM = PROJECT_ROOT / "original/Nine Hours, Nine Persons, Nine Doors (USA).nds"
DEFAULT_OUTPUT = PROJECT_ROOT / "src/assets/999"
DEFAULT_ICON_SOURCE = PROJECT_ROOT / "src-tauri/icons/icon-source.png"
BG_TOOL = PROJECT_ROOT / "tools/999-tools/bg_files.py"

IMAGE_ASSETS = {
    "logo.png": "bg/minigame/start/resource/bg_main.dat",
    "start-sub.png": "bg/minigame/start/resource/bg_sub.dat",
    "flooded-corridor.png": "bg/novel/passaged_000_00.dat",
    "numbered-door.png": "bg/novel/zero_010_00.dat",
}
# A restrained in-game mystery cue. Its retail loop begins at 32.025 s; the
# frontend seeks back there after the first complete play-through.
MUSIC_SOURCE = "sound/m19_bgm_e_13.sad"
MUSIC_OUTPUT = "bgm-mystery.mp3"
MUSIC_LOOP_START_SECONDS = 32.025


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_bg_tool() -> ModuleType:
    spec = importlib.util.spec_from_file_location("nonary_bg_files", BG_TOOL)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {BG_TOOL}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rom_file(rom: ndspy.rom.NintendoDSRom, nitro_path: str) -> bytes:
    file_id = rom.filenames.idOf(nitro_path)
    if file_id is None:
        raise RuntimeError(f"ROM does not contain {nitro_path}")
    return bytes(rom.files[file_id])


def decode_music(source: bytes, output: Path, vgmstream: str, ffmpeg: str) -> None:
    decoder = shutil.which(vgmstream) or (vgmstream if Path(vgmstream).is_file() else None)
    encoder = shutil.which(ffmpeg) or (ffmpeg if Path(ffmpeg).is_file() else None)
    if decoder is None:
        raise RuntimeError(f"vgmstream-cli not found: {vgmstream}")
    if encoder is None:
        raise RuntimeError(f"ffmpeg not found: {ffmpeg}")

    # vgmstream dispatches by extension, so the temporary file must still end
    # in .sad (".sad.tmp" is deliberately rejected by its format probe).
    sad_path = output.with_name(f".{output.stem}.tmp.sad")
    wav_path = output.with_name(f".{output.stem}.tmp.wav")
    sad_path.write_bytes(source)
    try:
        decode = subprocess.run(
            [str(decoder), "-o", str(wav_path), "-i", str(sad_path)],
            capture_output=True,
        )
        if decode.returncode != 0:
            message = decode.stderr.decode("utf-8", errors="replace").strip()
            raise RuntimeError(f"vgmstream-cli failed: {message}")
        encode = subprocess.run(
            [
                str(encoder),
                "-v",
                "error",
                "-nostdin",
                "-fflags",
                "+bitexact",
                "-flags:a",
                "+bitexact",
                "-i",
                str(wav_path),
                "-map_metadata",
                "-1",
                "-vn",
                "-c:a",
                "libmp3lame",
                "-q:a",
                "4",
                "-y",
                str(output),
            ],
            capture_output=True,
        )
        if encode.returncode != 0:
            message = encode.stderr.decode("utf-8", errors="replace").strip()
            raise RuntimeError(f"ffmpeg failed: {message}")
    finally:
        sad_path.unlink(missing_ok=True)
        wav_path.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rom", type=Path, default=DEFAULT_ROM)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--icon-source", type=Path, default=DEFAULT_ICON_SOURCE)
    parser.add_argument("--vgmstream", default="vgmstream-cli")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--skip-music", action="store_true")
    args = parser.parse_args()

    rom_bytes = args.rom.read_bytes()
    if len(rom_bytes) < 0x200 or rom_bytes[0x0C:0x10] != b"BSKE":
        raise RuntimeError("expected the supported USA 999 ROM (game code BSKE)")
    rom = ndspy.rom.NintendoDSRom(rom_bytes)
    bg_tool = load_bg_tool()
    args.output.mkdir(parents=True, exist_ok=True)

    manifest: dict[str, object] = {
        # Filenames make generated manifests reproducible across workstations
        # without exposing host-specific paths.
        "sourceRom": args.rom.name,
        "sourceRomSha256": sha256(rom_bytes),
        "images": {},
    }
    images = manifest["images"]
    assert isinstance(images, dict)
    rendered_images: dict[str, Image.Image] = {}
    for output_name, nitro_path in IMAGE_ASSETS.items():
        source = rom_file(rom, nitro_path)
        indexed = bg_tool.dump_image(source)
        image = indexed.convert("RGBA")
        destination = args.output / output_name
        image.save(destination, format="PNG", optimize=True)
        rendered_images[output_name] = image.copy()
        generated = destination.read_bytes()
        images[output_name] = {
            "nitroPath": nitro_path,
            "sourceSha256": sha256(source),
            "sha256": sha256(generated),
            "width": image.width,
            "height": image.height,
        }
        print(f"image {output_name}: {image.width}x{image.height}")

    # Nearest-neighbour scaling preserves the DS pixel edges, while letterboxing
    # avoids distorting the 4:3 title art to satisfy square icon formats.
    icon = Image.new("RGBA", (1024, 1024), (1, 11, 19, 255))
    logo = rendered_images["logo.png"].resize(
        (1024, 768),
        resample=Image.Resampling.NEAREST,
    )
    icon.alpha_composite(logo, (0, 128))
    args.icon_source.parent.mkdir(parents=True, exist_ok=True)
    icon.save(args.icon_source, format="PNG", optimize=True)
    manifest["appIcon"] = {
        "derivedFrom": IMAGE_ASSETS["logo.png"],
        "sha256": sha256(args.icon_source.read_bytes()),
        "width": icon.width,
        "height": icon.height,
    }
    print(f"icon {args.icon_source}: {icon.width}x{icon.height}")

    robot = args.output / "zero-robot.png"
    if robot.is_file():
        with Image.open(robot) as image:
            images[robot.name] = {
                "source": "External reference image",
                "sha256": sha256(robot.read_bytes()),
                "width": image.width,
                "height": image.height,
            }

    if not args.skip_music:
        source = rom_file(rom, MUSIC_SOURCE)
        destination = args.output / MUSIC_OUTPUT
        decode_music(source, destination, args.vgmstream, args.ffmpeg)
        generated = destination.read_bytes()
        manifest["music"] = {
            "nitroPath": MUSIC_SOURCE,
            "sourceSha256": sha256(source),
            "sha256": sha256(generated),
            "bytes": len(generated),
            "loopStartSeconds": MUSIC_LOOP_START_SECONDS,
        }
        print(f"music {MUSIC_OUTPUT}: {len(generated)} bytes")

    manifest_path = args.output / "manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"manifest: {manifest_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
