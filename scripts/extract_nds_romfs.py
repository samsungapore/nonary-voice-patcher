#!/usr/bin/env python3
"""Extract the 999 NitroFS files required by local research builds."""

from __future__ import annotations

import argparse
from collections.abc import Iterator
from pathlib import Path, PurePosixPath, PureWindowsPath

import ndspy.fnt
import ndspy.rom


GAME_CODE = b"BSKE"
# Unrelated NitroFS names include raw Shift-JIS bytes that are illegal on
# Windows, while the research pipeline only consumes scripts and three sound
# resources. Limiting extraction keeps the documented build portable.
RESEARCH_FILES = frozenset(
    {
        PurePosixPath("etc/sound.dat"),
        PurePosixPath("sound/se_a01b_wake.se"),
        PurePosixPath("sound/se_sys.se"),
    }
)


def validate_component(name: str, kind: str) -> None:
    if (
        not name
        or name in {".", ".."}
        or PurePosixPath(name).name != name
        or PureWindowsPath(name).name != name
        or ":" in name
        or "\0" in name
        or name.endswith((" ", "."))
    ):
        raise ValueError(f"unsafe NitroFS {kind}: {name!r}")


def named_files(
    folder: ndspy.fnt.Folder,
    prefix: PurePosixPath = PurePosixPath(),
) -> Iterator[tuple[PurePosixPath, int]]:
    for index, name in enumerate(folder.files):
        validate_component(name, "filename")
        yield prefix / name, folder.firstID + index
    for name, child in folder.folders:
        validate_component(name, "directory")
        yield from named_files(child, prefix / name)


def required_for_research(path: PurePosixPath) -> bool:
    return path in RESEARCH_FILES or path.parts[:1] == ("scr",)


def extract(rom_path: Path, output: Path) -> int:
    data = rom_path.read_bytes()
    if len(data) < 0x200 or data[0x0C:0x10] != GAME_CODE:
        raise ValueError("expected the supported USA 999 ROM (game code BSKE)")
    if output.exists():
        if not output.is_dir():
            raise NotADirectoryError(f"output path is not a directory: {output}")
        if any(output.iterdir()):
            raise FileExistsError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)

    rom = ndspy.rom.NintendoDSRom(data)
    count = 0
    for relative, file_id in named_files(rom.filenames):
        if not required_for_research(relative):
            continue
        if not 0 <= file_id < len(rom.files):
            raise ValueError(f"NitroFS file ID is out of range: {file_id}")
        destination = output.joinpath(*relative.parts)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(bytes(rom.files[file_id]))
        count += 1
    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rom", type=Path)
    parser.add_argument("output", type=Path, nargs="?", default=Path("work/romfs"))
    args = parser.parse_args()

    count = extract(args.rom, args.output)
    print(f"extracted {count} required NitroFS files to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
