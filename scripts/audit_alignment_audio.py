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

from apply_reviewed_alignment_overrides import (
    ReviewedOverride,
    overrides_from_runtime_row,
    runtime_component_values,
    validate_overrides,
)

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
        encrypted ^ key_bytes[positions & 3] ^ (positions & 0xFF).astype(np.uint8)
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
    return [
        path
        for _message_id, paths in contiguous_paths_by_message(row)
        for path in paths
    ]


def contiguous_paths_by_message(
    row: dict[str, str],
) -> list[tuple[str, list[str]]]:
    identifiers = message_ids(row)
    grouped: dict[str, list[str]] = {message_id: [] for message_id in identifiers}
    for path in row["jp_ogg_paths"].split("|"):
        path = path.strip()
        if not path:
            continue
        stem = Path(path).stem
        matches = [
            message_id
            for message_id in identifiers
            if stem.startswith(f"{message_id}_")
        ]
        if len(matches) != 1:
            raise ValueError(
                f"Voice path does not resolve to exactly one selected message: {path}"
            )
        grouped[matches[0]].append(path)

    result = []
    for message_id in identifiers:
        numbered = []
        for path in grouped[message_id]:
            match = SEGMENT.search(path)
            if not match:
                raise ValueError(f"Voice path has no numeric segment: {path}")
            numbered.append((int(match.group(1)), path))
        if not numbered:
            raise ValueError(f"Selected PC message has no Ogg path: {message_id}")
        numbered.sort()
        paths = []
        if numbered[0][0] == 0:
            for expected, (number, path) in enumerate(numbered):
                if number != expected:
                    break
                paths.append(path)
        else:
            paths = [path for _number, path in numbered]
        result.append((message_id, paths))
    return result


def message_ids(row: dict[str, str]) -> list[str]:
    raw = row.get("pc_message_ids", "").strip() or row["pc_message_id"].strip()
    values = [part.strip() for part in raw.split("|") if part.strip()]
    if not values or len(values) != len(set(values)):
        raise ValueError(f"Invalid grouped PC message IDs: {raw!r}")
    return values


def sliced_seconds(
    seconds: float,
    row: dict[str, str],
    language: str,
    *,
    component_index: int = 0,
    component_count: int = 1,
) -> float:
    start_value = runtime_component_values(
        row,
        f"{language}_start_ms",
        component_count,
        label=f"{language.upper()} start interval",
    )[component_index]
    end_value = runtime_component_values(
        row,
        f"{language}_end_ms",
        component_count,
        label=f"{language.upper()} end interval",
    )[component_index]
    if not start_value and not end_value:
        return seconds
    if not start_value.isascii() or not start_value.isdecimal():
        raise ValueError(f"Invalid {language.upper()} slice start: {start_value!r}")
    if end_value and (not end_value.isascii() or not end_value.isdecimal()):
        raise ValueError(f"Invalid {language.upper()} slice end: {end_value!r}")
    start = int(start_value) / 1000.0
    end = int(end_value) / 1000.0 if end_value else seconds
    if start >= end or end > seconds + 1e-6:
        raise ValueError(
            f"{language.upper()} slice {start:.3f}..{end:.3f}s is outside "
            f"a {seconds:.3f}s voice"
        )
    return end - start


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
    source_rows: dict[str, list[dict[str, str]]] = defaultdict(list)
    reviewed_overrides: list[ReviewedOverride] = []
    for row_number, row in enumerate(alignment, 2):
        identifiers = message_ids(row)
        if row.get("alignment_method", "").strip().casefold() == "reviewed_override":
            normalized = dict(row)
            normalized["pc_message_id"] = " | ".join(identifiers)
            reviewed_overrides.extend(
                overrides_from_runtime_row(normalized, row_number)
            )
        for message_id in identifiers:
            source_rows[message_id].append(row)
        grouped[(row["ds_script"], int(row["ds_source_line"]))].append(row)
    if reviewed_overrides:
        validate_overrides(reviewed_overrides)
    for message_id, usages in source_rows.items():
        if len(usages) == 1:
            continue
        if any(
            row.get("alignment_method", "").strip().casefold() != "reviewed_override"
            for row in usages
        ):
            raise ValueError(f"Duplicate PC message ID: {message_id}")
        groups = {row.get("reviewed_override_group", "") for row in usages}
        if len(groups) != 1 or not next(iter(groups)):
            raise ValueError(
                f"PC message {message_id!r} is reused outside one reviewed split group"
            )

    audit_rows = []
    failures = []
    for (script, source_line), rows in sorted(grouped.items()):
        paths = [path for row in rows for path in contiguous_paths(row)]
        seconds = 0.0
        try:
            for row in rows:
                components = contiguous_paths_by_message(row)
                for component_index, (_message_id, component_paths) in enumerate(
                    components
                ):
                    component_seconds = sum(durations[path] for path in component_paths)
                    component_seconds += (
                        args.silence_ms / 1000.0 * max(0, len(component_paths) - 1)
                    )
                    if component_index:
                        seconds += args.silence_ms / 1000.0
                    seconds += sliced_seconds(
                        component_seconds,
                        row,
                        "jp",
                        component_index=component_index,
                        component_count=len(components),
                    )
        except KeyError as error:
            raise KeyError(
                f"Alignment path absent from manifest: {error.args[0]}"
            ) from error
        seconds += args.silence_ms / 1000.0 * max(0, len(rows) - 1)
        first = rows[0]
        audit_rows.append(
            {
                "ds_script": script,
                "ds_source_line": source_line,
                "ds_settext_ordinal": first["ds_settext_ordinal"],
                "ds_speaker": first["ds_speaker"],
                "ds_text": first["ds_text"],
                "pc_message_ids": " | ".join(row["pc_message_id"] for row in rows),
                "message_count": sum(len(message_ids(row)) for row in rows),
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
        f"pc_messages={len(source_rows)}, ds_lines={len(audit_rows)}, "
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
