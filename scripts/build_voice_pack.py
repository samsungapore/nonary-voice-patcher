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
import tempfile
from dataclasses import dataclass
from pathlib import Path


MAGIC = b"NVPACK01"
VERSION_V1 = 1
VERSION_V2 = 2
HEADER_SIZE_V1 = 0x70
HEADER_SIZE_V2 = 0x90
# Retain these names for callers that explicitly inspect the legacy layout.
VERSION = VERSION_V1
HEADER_SIZE = HEADER_SIZE_V1
ENTRY_SIZE = 40
V2_INDEX_DOMAIN = b"NVPACK02-INDEX\0"
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


def verify_pack_file(
    path: Path,
    expected_header: bytes,
    expected_index: bytes,
    entries: list[Entry],
    payload_offset: int,
) -> None:
    payload_digest = hashlib.sha256()
    with path.open("rb") as stream:
        if stream.read(len(expected_header)) != expected_header:
            raise ValueError("Finished voice pack header failed verification")
        if stream.read(len(expected_index)) != expected_index:
            raise ValueError("Finished voice pack index failed verification")
        stream.seek(payload_offset)
        for entry in entries:
            remaining = entry.size
            entry_digest = hashlib.sha256()
            while remaining:
                block = stream.read(min(1024 * 1024, remaining))
                if not block:
                    raise ValueError("Finished voice pack payload is truncated")
                remaining -= len(block)
                entry_digest.update(block)
                payload_digest.update(block)
            if entry_digest.digest() != entry.digest:
                raise ValueError(
                    f"Finished voice pack entry failed verification: {entry.path.name}"
                )
        if stream.read(1):
            raise ValueError("Finished voice pack contains trailing data")
    if payload_digest.digest() != expected_header[0x50:0x70]:
        raise ValueError("Finished voice pack payload digest failed verification")


def build_pack(
    directory: Path,
    output: Path,
    language: str,
    catalog_sha256: bytes | None = None,
) -> tuple[int, int, str]:
    language_bytes = language.lower().encode("ascii")
    if language_bytes not in {b"jp", b"en", b"fr"}:
        raise ValueError("language must be jp, en, or fr")
    if catalog_sha256 is not None and len(catalog_sha256) != 32:
        raise ValueError("catalog SHA-256 must contain exactly 32 bytes")
    version = VERSION_V2 if catalog_sha256 is not None else VERSION_V1
    header_size = HEADER_SIZE_V2 if catalog_sha256 is not None else HEADER_SIZE_V1
    entries = collect_entries(directory)
    resolved_output = output.resolve()
    colliding_entry = next(
        (entry.path for entry in entries if entry.path.resolve() == resolved_output),
        None,
    )
    if colliding_entry is not None:
        raise ValueError(
            f"Output pack would overwrite an input voice: {colliding_entry}"
        )
    index = b"".join(
        struct.pack("<IHH32s", entry.size, 0, 0, entry.digest) for entry in entries
    )
    if len(index) != len(entries) * ENTRY_SIZE:
        raise AssertionError("voice-pack index size mismatch")
    payload_offset = (header_size + len(index) + 0x0F) & ~0x0F
    payload_size = sum(entry.size for entry in entries)
    payload_digest = hashlib.sha256()
    index_digest = hashlib.sha256(
        V2_INDEX_DOMAIN + catalog_sha256 + index
        if catalog_sha256 is not None
        else index
    ).digest()

    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, raw_temporary = tempfile.mkstemp(
        prefix=f".{output.name}.", suffix=".tmp", dir=output.parent
    )
    temporary = Path(raw_temporary)
    try:
        with os.fdopen(descriptor, "w+b") as stream:
            stream.write(b"\0" * header_size)
            stream.write(index)
            stream.write(b"\0" * (payload_offset - stream.tell()))
            for entry in entries:
                copied = 0
                copied_digest = hashlib.sha256()
                with entry.path.open("rb") as voice:
                    for block in iter(lambda: voice.read(1024 * 1024), b""):
                        copied += len(block)
                        copied_digest.update(block)
                        payload_digest.update(block)
                        stream.write(block)
                if copied != entry.size or copied_digest.digest() != entry.digest:
                    raise ValueError(
                        f"Input voice changed while the pack was being built: {entry.path}"
                    )
            if stream.tell() != payload_offset + payload_size:
                raise AssertionError("voice-pack payload size mismatch")
            header = struct.pack(
                "<8sII4sIQQQ32s32s",
                MAGIC,
                version,
                len(entries),
                language_bytes.ljust(4, b"\0"),
                0,
                header_size,
                payload_offset,
                payload_size,
                index_digest,
                payload_digest.digest(),
            )
            if catalog_sha256 is not None:
                header += catalog_sha256
            if len(header) != header_size:
                raise AssertionError("voice-pack header size mismatch")
            stream.seek(0)
            stream.write(header)
            stream.flush()
            os.fsync(stream.fileno())
        verify_pack_file(temporary, header, index, entries, payload_offset)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)

    pack_digest = sha256_file(output).hex()
    return len(entries), payload_size, pack_digest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input_dir", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--language", choices=("jp", "en", "fr"), required=True)
    parser.add_argument(
        "--catalog-sha256",
        help="64-hex canonical target-catalog digest; writes a version 2 pack",
    )
    args = parser.parse_args()
    try:
        catalog = (
            bytes.fromhex(args.catalog_sha256)
            if args.catalog_sha256 is not None
            else None
        )
    except ValueError as error:
        parser.error(f"invalid --catalog-sha256: {error}")
    count, payload_size, digest = build_pack(
        args.input_dir, args.output, args.language, catalog
    )
    print(f"voices={count}")
    print(f"payload_bytes={payload_size}")
    print(f"sha256={digest}")


if __name__ == "__main__":
    main()
