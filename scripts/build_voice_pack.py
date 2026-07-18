#!/usr/bin/env python3
"""Bundle a contiguous generated 999 voice bank into one validated binary pack.

The desktop patcher only needs one resource per language.  ``NVPACK01`` keeps
the already-compressed DSE ``.se`` payloads byte-for-byte, records every size
and SHA-256, and gives the Rust backend enough metadata to plan NitroFS before
streaming the large payload section.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import struct
from dataclasses import dataclass
from pathlib import Path


MAGIC = b"NVPACK01"
VERSION = 1
HEADER_SIZE = 0x70
ENTRY_SIZE = 40
VOICE_NAME = re.compile(r"se_v(?P<index>[0-9]{4})\.se", re.IGNORECASE)


@dataclass(frozen=True)
class Entry:
    path: Path
    size: int
    digest: bytes


def sha256_file(path: Path) -> bytes:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.digest()


def collect_entries(directory: Path) -> list[Entry]:
    paths = sorted(directory.glob("se_v*.se"))
    if not paths:
        raise ValueError(f"No se_v####.se files in {directory}")
    entries: list[Entry] = []
    for index, path in enumerate(paths):
        match = VOICE_NAME.fullmatch(path.name)
        if match is None or int(match.group("index")) != index:
            raise ValueError(
                f"Voice sequence is not contiguous at {path.name!r}; "
                f"expected se_v{index:04d}.se"
            )
        size = path.stat().st_size
        if not 0 < size <= 0xFFFFFFFF:
            raise ValueError(f"Invalid voice size for {path}: {size}")
        with path.open("rb") as stream:
            if stream.read(4) != b"SIR0":
                raise ValueError(f"Voice is not a SIR0 container: {path}")
        entries.append(Entry(path=path, size=size, digest=sha256_file(path)))
    return entries


def build_pack(directory: Path, output: Path, language: str) -> tuple[int, int, str]:
    language_bytes = language.lower().encode("ascii")
    if language_bytes not in {b"jp", b"en"}:
        raise ValueError("language must be jp or en")
    entries = collect_entries(directory)
    index = b"".join(
        struct.pack("<IHH32s", entry.size, 0, 0, entry.digest) for entry in entries
    )
    if len(index) != len(entries) * ENTRY_SIZE:
        raise AssertionError("voice-pack index size mismatch")
    payload_offset = (HEADER_SIZE + len(index) + 0x0F) & ~0x0F
    payload_size = sum(entry.size for entry in entries)
    payload_digest = hashlib.sha256()
    index_digest = hashlib.sha256(index).digest()

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp-{os.getpid()}")
    try:
        with temporary.open("wb") as stream:
            stream.write(b"\0" * HEADER_SIZE)
            stream.write(index)
            stream.write(b"\0" * (payload_offset - stream.tell()))
            for entry in entries:
                with entry.path.open("rb") as voice:
                    for block in iter(lambda: voice.read(1024 * 1024), b""):
                        payload_digest.update(block)
                        stream.write(block)
            if stream.tell() != payload_offset + payload_size:
                raise AssertionError("voice-pack payload size mismatch")
            header = struct.pack(
                "<8sII4sIQQQ32s32s",
                MAGIC,
                VERSION,
                len(entries),
                language_bytes.ljust(4, b"\0"),
                0,
                HEADER_SIZE,
                payload_offset,
                payload_size,
                index_digest,
                payload_digest.digest(),
            )
            if len(header) != HEADER_SIZE:
                raise AssertionError("voice-pack header size mismatch")
            stream.seek(0)
            stream.write(header)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)

    pack_digest = sha256_file(output).hex()
    return len(entries), payload_size, pack_digest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input_dir", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--language", choices=("jp", "en"), required=True)
    args = parser.parse_args()
    count, payload_size, digest = build_pack(args.input_dir, args.output, args.language)
    print(f"voices={count}")
    print(f"payload_bytes={payload_size}")
    print(f"sha256={digest}")


if __name__ == "__main__":
    main()
