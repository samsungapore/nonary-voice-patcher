#!/usr/bin/env python3
"""Audit combined PC voice duration for every mapped Nintendo DS text line."""

from __future__ import annotations

import argparse
import csv
import re
import struct
from collections import defaultdict
from pathlib import Path

try:
    import numpy as np
except ImportError as error:  # pragma: no cover - depends on local environment
    raise SystemExit(
        "audit_alignment_audio.py requires numpy "
        "(for example: python3 -m pip install numpy)"
    ) from error


SEGMENT = re.compile(r"_(\d{2})\.ogg$", re.IGNORECASE)


def decrypt_tail(data: bytes, key: int, relative_offset: int) -> bytes:
    """Decrypt one archive slice using the position-dependent XOR stream."""
    encrypted = np.frombuffer(data, dtype=np.uint8)
    positions = np.arange(
        relative_offset,
        relative_offset + len(encrypted),
        dtype=np.uint32,
    )
    key_bytes = np.frombuffer(key.to_bytes(4, "little"), dtype=np.uint8)
    return (
        encrypted
        ^ key_bytes[positions & 3]
        ^ (positions & 0xFF).astype(np.uint8)
    ).tobytes()


def read_durations(archive_path: Path, manifest_path: Path) -> dict[str, float]:
    with manifest_path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {
            "jp_ogg_path",
            "archive_key_hex",
            "encrypted_absolute_offset",
            "size_bytes",
            "sample_rate_hz",
        }
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(
                f"Manifest lacks columns: {sorted(required - set(reader.fieldnames or []))}"
            )
        records = list(reader)

    durations: dict[str, float] = {}
    with archive_path.open("rb") as archive:
        for record in records:
            path = record["jp_ogg_path"]
            if path in durations:
                raise ValueError(f"Duplicate Ogg path in manifest: {path}")
            offset = int(record["encrypted_absolute_offset"])
            size = int(record["size_bytes"])
            key = int(record["archive_key_hex"], 16)
            sample_rate = int(record["sample_rate_hz"])
            tail_offset = max(0, size - 65_536)
            archive.seek(offset + tail_offset)
            encrypted = archive.read(size - tail_offset)
            if len(encrypted) != size - tail_offset:
                raise EOFError(f"Truncated archive entry: {path}")
            tail = decrypt_tail(encrypted, key, tail_offset)
            page = tail.rfind(b"OggS")
            if page < 0 or page + 14 > len(tail):
                raise ValueError(f"Final Ogg page not found: {path}")
            granule_position = struct.unpack_from("<Q", tail, page + 6)[0]
            durations[path] = granule_position / sample_rate
    return durations


def contiguous_paths(row: dict[str, str]) -> list[str]:
    """Mirror build_voice_bank.py's path-hash collision filtering."""
    numbered = []
    for path in row["jp_ogg_paths"].split(" | "):
        path = path.strip()
        if not path:
            continue
        match = SEGMENT.search(path)
        if not match:
            raise ValueError(f"Voice path has no numeric segment: {path}")
        numbered.append((int(match.group(1)), path))
    numbered.sort()
    if numbered and numbered[0][0] == 0:
        result = []
        for expected, (number, path) in enumerate(numbered):
            if number != expected:
                break
            result.append(path)
        return result
    return [path for _, path in numbered]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ze1_data", type=Path)
    parser.add_argument("pc_manifest", type=Path)
    parser.add_argument("alignment", type=Path)
    parser.add_argument("output_tsv", type=Path)
    parser.add_argument("--silence-ms", type=float, default=65.0)
    parser.add_argument("--max-duration", type=float, default=45.0)
    args = parser.parse_args()
    if args.silence_ms < 0 or args.max_duration <= 0:
        parser.error("duration options must be positive")

    durations = read_durations(args.ze1_data, args.pc_manifest)
    with args.alignment.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {
            "ds_script",
            "ds_source_line",
            "ds_settext_ordinal",
            "ds_speaker",
            "ds_text",
            "pc_message_id",
            "jp_ogg_paths",
        }
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(
                f"Alignment lacks columns: {sorted(required - set(reader.fieldnames or []))}"
            )
        alignment = list(reader)

    grouped: dict[tuple[str, int], list[dict[str, str]]] = defaultdict(list)
    source_ids = set()
    for row in alignment:
        message_id = row["pc_message_id"]
        if message_id in source_ids:
            raise ValueError(f"Duplicate PC message ID: {message_id}")
        source_ids.add(message_id)
        grouped[(row["ds_script"], int(row["ds_source_line"]))].append(row)

    audit_rows = []
    failures = []
    for (script, source_line), rows in sorted(grouped.items()):
        paths = [path for row in rows for path in contiguous_paths(row)]
        try:
            seconds = sum(durations[path] for path in paths)
        except KeyError as error:
            raise KeyError(f"Alignment path absent from manifest: {error.args[0]}") from error
        seconds += args.silence_ms / 1000.0 * max(0, len(paths) - 1)
        first = rows[0]
        audit_rows.append(
            {
                "ds_script": script,
                "ds_source_line": source_line,
                "ds_settext_ordinal": first["ds_settext_ordinal"],
                "ds_speaker": first["ds_speaker"],
                "ds_text": first["ds_text"],
                "pc_message_ids": " | ".join(row["pc_message_id"] for row in rows),
                "message_count": len(rows),
                "ogg_clip_count": len(paths),
                "duration_seconds": f"{seconds:.6f}",
                "below_limit": str(seconds < args.max_duration).lower(),
            }
        )
        if seconds >= args.max_duration:
            failures.append((seconds, script, source_line))

    args.output_tsv.parent.mkdir(parents=True, exist_ok=True)
    with args.output_tsv.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(
            stream,
            fieldnames=list(audit_rows[0]),
            delimiter="\t",
            lineterminator="\n",
        )
        writer.writeheader()
        writer.writerows(audit_rows)

    maximum = max(float(row["duration_seconds"]) for row in audit_rows)
    print(
        f"pc_messages={len(source_ids)}, ds_lines={len(audit_rows)}, "
        f"max_duration={maximum:.6f}, failures={len(failures)}, "
        f"output={args.output_tsv}"
    )
    if failures:
        worst = max(failures)
        raise SystemExit(
            f"duration limit exceeded: {worst[1]}:{worst[2]} "
            f"is {worst[0]:.6f}s (limit {args.max_duration:.6f}s)"
        )


if __name__ == "__main__":
    main()
