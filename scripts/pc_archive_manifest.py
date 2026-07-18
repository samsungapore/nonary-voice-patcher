#!/usr/bin/env python3
"""Reconstruct 999 PC Japanese voice paths from ``ze1_data.bin``.

The archive keeps only a case-insensitive path hash. Dialogue records in SIR1
contain ``MESSAGE_ID\0Talk\0SPEAKER\0TEXT\0``; ``ze1.exe`` resolves each voiced
clip as ``/sound/voice/<MESSAGE_ID>_<segment:02d>.ogg``.
"""

from __future__ import annotations

import argparse
import csv
import re
import struct
from pathlib import Path


ZE1_KEY = 0xFABACEDA
ENTRY = struct.Struct("<QIIIIII")
TALK = re.compile(
    rb"(?<![A-Za-z0-9_])([A-Za-z_][A-Za-z0-9_]{2,79})\x00Talk\x00"
    rb"([^\x00]*)\x00([^\x00]*)\x00"
)


def crypt(data: bytes, key: int, relative_offset: int = 0) -> bytes:
    """Apply the symmetric Nonary Games XOR stream."""
    key_bytes = key.to_bytes(4, "little")
    return bytes(
        value ^ key_bytes[index & 3] ^ ((relative_offset + index) & 0xFF)
        for index, value in enumerate(data)
    )


def path_key(path: str) -> int:
    """Return the case-insensitive 31-bit key used by the archive index."""
    state = checksum = 0
    for value in path.encode("ascii"):
        checksum = (checksum + value) & 0xFFFFFFFF
        state = (state * 0x83 + (value & 0xDF)) & 0xFFFFFFFF
    return (checksum & 0xF) | ((state & 0x07FFFFFF) << 4)


def clean(value: str) -> str:
    return value.replace("\t", " ").replace("\r", "\\r").replace("\n", "\\n")


def japanese_character_count(value: str) -> int:
    return sum(
        "\u3040" <= character <= "\u30ff" or "\u3400" <= character <= "\u9fff"
        for character in value
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("ze1_data", type=Path)
    parser.add_argument("output_tsv", type=Path)
    args = parser.parse_args()

    with args.ze1_data.open("rb") as archive:
        header = crypt(archive.read(0x20), ZE1_KEY)
        magic, _, index_offset, _, payload_offset = struct.unpack_from("<4sIIQQ", header)
        if magic != b"bin.":
            raise ValueError(f"Unexpected decrypted magic: {magic!r}")
        archive.seek(index_offset)
        index = crypt(archive.read(payload_offset - index_offset), ZE1_KEY, index_offset)

    table_offset, file_count = struct.unpack_from("<II", index)
    entries = [
        ENTRY.unpack_from(index, table_offset + number * ENTRY.size)
        for number in range(file_count)
    ]
    by_key = {entry[1]: entry for entry in entries}
    records: dict[str, dict[str, set]] = {}

    with args.ze1_data.open("rb") as archive:
        for entry in sorted(entries, key=lambda item: item[0]):
            offset, key, size, _, entry_id, _, _ = entry
            archive.seek(payload_offset + offset)
            if crypt(archive.read(min(size, 4)), key) != b"SIR1":
                continue
            archive.seek(payload_offset + offset)
            data = crypt(archive.read(size), key)
            for match in TALK.finditer(data):
                message_id = match.group(1).decode("ascii")
                speaker = match.group(2).decode("cp932", "replace")
                raw_text = match.group(3)
                jp_text = raw_text.decode("cp932", "replace")
                record = records.setdefault(
                    message_id, {"jp": set(), "en": set(), "sources": set()}
                )
                record["sources"].add(entry_id)
                if japanese_character_count(jp_text):
                    record["jp"].add((clean(speaker), clean(jp_text)))
                else:
                    # English prose still uses the game's Shift-JIS/custom glyph
                    # page for apostrophes and control markers (for example Ｓ,
                    # ⑲ and ⑳), just like the decompiled DS scripts.
                    record["en"].add((clean(speaker), clean(jp_text)))

    rows = []
    with args.ze1_data.open("rb") as archive:
        for message_id, record in records.items():
            for segment in range(100):
                voice_path = f"/sound/voice/{message_id}_{segment:02d}.ogg"
                key = path_key(voice_path)
                entry = by_key.get(key)
                if not entry or entry[5] != 0x2006D:
                    continue
                offset, _, size, _, entry_id, _, _ = entry
                archive.seek(payload_offset + offset)
                header = crypt(archive.read(min(size, 64)), key)
                if not header.startswith(b"OggS"):
                    continue
                packet = 27 + header[26]
                channels = (
                    header[packet + 11]
                    if header[packet : packet + 7] == b"\x01vorbis"
                    else ""
                )
                sample_rate = (
                    struct.unpack_from("<I", header, packet + 12)[0]
                    if header[packet : packet + 7] == b"\x01vorbis"
                    else ""
                )
                rows.append(
                    [
                        message_id,
                        segment,
                        voice_path,
                        f"{key:08x}",
                        entry_id,
                        payload_offset + offset,
                        size,
                        channels,
                        sample_rate,
                        ",".join(map(str, sorted(record["sources"]))),
                        " || ".join(
                            f"{speaker} :: {text}"
                            for speaker, text in sorted(record["jp"])
                        ),
                        " || ".join(
                            f"{speaker} :: {text}"
                            for speaker, text in sorted(record["en"])
                        ),
                    ]
                )

    rows.sort(key=lambda row: (row[0].lower(), row[1]))
    args.output_tsv.parent.mkdir(parents=True, exist_ok=True)
    with args.output_tsv.open("w", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "message_id",
                "segment",
                "jp_ogg_path",
                "archive_key_hex",
                "archive_entry_id",
                "encrypted_absolute_offset",
                "size_bytes",
                "channels",
                "sample_rate_hz",
                "source_sir_entry_ids",
                "jp_speaker_and_text_cp932",
                "en_speaker_and_text_cp932",
            ]
        )
        writer.writerows(rows)
    print(f"wrote {len(rows)} voice rows to {args.output_tsv}")


if __name__ == "__main__":
    main()
