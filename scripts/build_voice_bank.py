#!/usr/bin/env python3
"""Extract, resample, encode, and package an aligned JP or EN voice bank."""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import os
import re
import struct
import tempfile
import unicodedata
from pathlib import Path
from typing import BinaryIO, Mapping

import ndspy.rom
import numpy as np
import soundfile as sf
import soxr

from apply_reviewed_alignment_overrides import (
    ReviewedOverride,
    overrides_from_runtime_row,
    runtime_component_values,
    validate_overrides,
)
from pc_archive_manifest import ENTRY, ZE1_KEY, crypt, path_key
from voice_audio import SAMPLE_RATE, build_se, read_dse_internal_id, validate_se


SEGMENT = re.compile(r"_(\d{2})\.ogg$", re.IGNORECASE)
MESSAGE_SEPARATOR = " | "
NARRATION_SPEAKERS = frozenset({"novel", "hero"})
SELECTED_CONFIDENCES = frozenset({"exact", "high", "medium", "review"})
FIRST_ID_HIGH = 0xC8
LAST_ID_HIGH = 0x64
FIRST_ID_LOW = 0x01
LAST_ID_LOW = 0x65
VOICE_LANGUAGE_SPECS = {
    "jp": ("/sound/voice", 0x2006D),
    "en": ("/sound/voice_us", 0x20038),
}
SLICE_EDGE_FADE_MS = 4.0
PROVENANCE_FILENAME = ".voice-bank-build.json"
PROVENANCE_SCHEMA = "nonary-voice-bank-build-provenance"
PROVENANCE_SCHEMA_VERSION = 1
# A version change intentionally invalidates resumable outputs when generation
# semantics change even if every source file remains byte-identical.
PIPELINE_VERSION = 1
GENERATED_VOICE_NAME = re.compile(r"se_v.*\.se$", re.IGNORECASE)


ArchiveEntry = tuple[int, int, int, int, int, int, int]
ArchiveVoiceIndex = tuple[int, dict[int, ArchiveEntry]]


def file_identity(path: Path) -> dict[str, int | str]:
    """Return a stable content identity without loading large archives in memory."""
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return {"size": size, "sha256": digest.hexdigest()}


def build_provenance(
    *,
    language: str,
    alignment: Path,
    pc_archive: Path,
    pc_manifest: Path,
    template_se: Path,
    id_source_rom: Path,
    silence_ms: float,
    max_duration: float,
    allow_narration_voices: bool,
) -> dict[str, object]:
    """Describe every external input that can affect a resumable voice bank."""
    return {
        "schema": PROVENANCE_SCHEMA,
        "schema_version": PROVENANCE_SCHEMA_VERSION,
        "pipeline_version": PIPELINE_VERSION,
        "language": language,
        "inputs": {
            "alignment": file_identity(alignment),
            "pc_archive": file_identity(pc_archive),
            "jp_manifest": file_identity(pc_manifest),
            "se_template": file_identity(template_se),
            "source_rom": file_identity(id_source_rom),
        },
        "options": {
            # Hexadecimal floats preserve the exact CLI values across JSON
            # implementations instead of relying on decimal rendering details.
            "silence_ms": silence_ms.hex(),
            "max_duration": max_duration.hex(),
            "allow_narration_voices": allow_narration_voices,
        },
    }


def atomic_write_json(path: Path, value: Mapping[str, object]) -> None:
    """Publish JSON only after its complete contents have been written."""
    descriptor, raw_temporary = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(raw_temporary)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            json.dump(value, stream, indent=2, sort_keys=True, ensure_ascii=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def provenance_differences(
    expected: object, actual: object, prefix: str = ""
) -> list[str]:
    """Name changed fields so a rejected resume has an actionable cause."""
    if isinstance(expected, dict) and isinstance(actual, dict):
        differences: list[str] = []
        for key in sorted(set(expected) | set(actual)):
            field = f"{prefix}.{key}" if prefix else str(key)
            if key not in expected or key not in actual:
                differences.append(field)
            else:
                differences.extend(
                    provenance_differences(expected[key], actual[key], field)
                )
        return differences
    return [] if expected == actual else [prefix or "document"]


def generated_voice_paths(output_dir: Path) -> list[Path]:
    if not output_dir.is_dir():
        return []
    return sorted(
        (
            path
            for path in output_dir.iterdir()
            if GENERATED_VOICE_NAME.fullmatch(path.name)
        ),
        key=lambda path: path.name.casefold(),
    )


def preflight_output_directory(output_dir: Path, *, resume: bool) -> None:
    """Reject obvious output conflicts before hashing multi-gigabyte inputs."""
    if resume:
        if not (output_dir / PROVENANCE_FILENAME).is_file():
            raise ValueError(
                f"Cannot resume voice-bank generation because {PROVENANCE_FILENAME} "
                "is missing. Start again without --resume in an output directory "
                "that contains no SE_V files."
            )
        return
    voices = generated_voice_paths(output_dir)
    if voices:
        names = ", ".join(path.name for path in voices[:3])
        suffix = "..." if len(voices) > 3 else ""
        raise ValueError(
            "Voice output directory already contains generated SE_V files "
            f"({names}{suffix}). Choose an empty directory, or use --resume "
            "only for an interrupted build with matching inputs."
        )


def prepare_output_directory(
    output_dir: Path,
    provenance: Mapping[str, object],
    *,
    resume: bool,
    expected_voice_count: int,
) -> None:
    """Accept only an empty new build or an authenticated partial build."""
    output_dir.mkdir(parents=True, exist_ok=True)
    preflight_output_directory(output_dir, resume=resume)
    sidecar = output_dir / PROVENANCE_FILENAME
    voices = generated_voice_paths(output_dir)
    if not resume:
        # Publishing this before the first voice makes an interrupted build
        # resumable without treating unauthenticated legacy files as trusted.
        atomic_write_json(sidecar, provenance)
        return

    try:
        stored = json.loads(sidecar.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(
            f"Cannot resume voice-bank generation because {PROVENANCE_FILENAME} "
            "is unreadable or invalid. Start again in an empty output directory."
        ) from exc
    differences = provenance_differences(dict(provenance), stored)
    if differences:
        shown = ", ".join(differences[:5])
        suffix = ", ..." if len(differences) > 5 else ""
        raise ValueError(
            "Cannot resume voice-bank generation because the build inputs or "
            f"options changed ({shown}{suffix}). Start again without --resume "
            "in an empty output directory."
        )

    expected_names = {f"se_v{index:04d}.se" for index in range(expected_voice_count)}
    unsafe = [path.name for path in voices if path.is_symlink() or not path.is_file()]
    if unsafe:
        raise ValueError(
            "Cannot resume voice-bank generation because a generated voice path "
            f"is not a regular local file: {unsafe[0]}"
        )
    unexpected = [path.name for path in voices if path.name not in expected_names]
    if unexpected:
        raise ValueError(
            "Cannot resume voice-bank generation because the output directory "
            f"contains an unexpected generated file: {unexpected[0]}"
        )


def publish_generated_voice(output: Path, generated: bytes, *, resume: bool) -> bool:
    """Keep an authenticated cached resource only when every byte still matches."""
    if resume and output.is_file() and output.read_bytes() == generated:
        return False
    output.write_bytes(generated)
    return True


def collect_retail_internal_ids(rom_path: Path) -> dict[str, int]:
    """Strictly read every top-level ``sound/*.se`` DSE ID from a ROM."""
    rom = ndspy.rom.NintendoDSRom.fromFile(str(rom_path))
    sound_folder = rom.filenames.subfolder("sound")
    if sound_folder is None:
        raise ValueError(f"ID-source ROM has no sound folder: {rom_path}")

    result: dict[str, int] = {}
    for filename in sound_folder.files:
        if not filename.lower().endswith(".se"):
            continue
        nitro_path = f"sound/{filename}"
        file_id = rom.filenames.idOf(nitro_path)
        if file_id is None:
            raise ValueError(f"FNT cannot resolve {nitro_path} in {rom_path}")
        result[nitro_path] = read_dse_internal_id(bytes(rom.files[file_id]), nitro_path)
    if not result:
        raise ValueError(f"ID-source ROM has no sound/*.se resources: {rom_path}")
    return result


def allocate_internal_ids(retail_ids: set[int], count: int) -> list[int]:
    """Allocate deterministic free DSE pairs, high byte descending.

    Within each high-byte namespace, allocation starts strictly above its
    greatest retail low byte.  This deliberately leaves retail holes such as
    C807 unused and makes C80E the first generated ID: it has been exercised
    successfully by the runtime probe, while no retail resource is renumbered.
    """
    if count < 0:
        raise ValueError("Voice count cannot be negative")
    available: list[int] = []
    for high in range(FIRST_ID_HIGH, LAST_ID_HIGH - 1, -1):
        retail_lows = [
            identifier & 0xFF
            for identifier in retail_ids
            if identifier >> 8 == high
            and FIRST_ID_LOW <= identifier & 0xFF <= LAST_ID_LOW
        ]
        first_low = max(retail_lows, default=FIRST_ID_LOW - 1) + 1
        available.extend(
            (high << 8) | low
            for low in range(first_low, LAST_ID_LOW + 1)
            if ((high << 8) | low) not in retail_ids
        )
    if count > len(available):
        raise ValueError(
            f"Need {count} generated DSE IDs but only {len(available)} "
            "collision-free pairs remain in C8..64:01..65"
        )
    return available[:count]


def load_manifest(path: Path) -> dict[str, dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as stream:
        rows = list(csv.DictReader(stream, delimiter="\t"))
    result = {row["jp_ogg_path"]: row for row in rows}
    if len(result) != len(rows):
        raise ValueError("Duplicate Japanese Ogg path in PC manifest")
    return result


def load_archive_voice_index(archive: BinaryIO) -> ArchiveVoiceIndex:
    """Read the encrypted PC archive index needed for language-specific voices."""
    archive.seek(0)
    encrypted_header = archive.read(0x20)
    if len(encrypted_header) != 0x20:
        raise EOFError("Truncated ze1_data.bin header")
    header = crypt(encrypted_header, ZE1_KEY)
    magic, _, index_offset, _, payload_offset = struct.unpack_from("<4sIIQQ", header)
    if magic != b"bin.":
        raise ValueError(f"Unexpected decrypted PC archive magic: {magic!r}")
    if not 0x20 <= index_offset < payload_offset:
        raise ValueError(
            f"Invalid PC archive index/payload offsets: {index_offset}, {payload_offset}"
        )

    archive.seek(index_offset)
    encrypted_index = archive.read(payload_offset - index_offset)
    if len(encrypted_index) != payload_offset - index_offset:
        raise EOFError("Truncated ze1_data.bin index")
    index = crypt(encrypted_index, ZE1_KEY, index_offset)
    if len(index) < 8:
        raise ValueError("PC archive index is too short")
    table_offset, file_count = struct.unpack_from("<II", index)
    table_end = table_offset + file_count * ENTRY.size
    if table_offset < 8 or table_end > len(index):
        raise ValueError("PC archive entry table is outside the decrypted index")

    entries = [
        ENTRY.unpack_from(index, table_offset + number * ENTRY.size)
        for number in range(file_count)
    ]
    by_key = {entry[1]: entry for entry in entries}
    if len(by_key) != len(entries):
        raise ValueError("PC archive contains duplicate path keys")
    return payload_offset, by_key


def resolve_archive_message(
    archive_index: ArchiveVoiceIndex,
    message_id: str,
    language: str,
) -> list[tuple[str, dict[str, str]]]:
    """Resolve contiguous language-specific Ogg segments starting at ``_00``.

    Stopping at the first absent segment is intentional.  English sometimes
    combines a two-segment Japanese line into ``_00`` only, and unrelated
    archive paths can collide with hypothetical non-contiguous suffixes such
    as ``_28`` or ``_88``.
    """
    try:
        folder, expected_type = VOICE_LANGUAGE_SPECS[language]
    except KeyError as exc:
        raise ValueError(f"Unsupported voice language: {language!r}") from exc

    payload_offset, by_key = archive_index
    result: list[tuple[str, dict[str, str]]] = []
    for segment in range(100):
        path = f"{folder}/{message_id}_{segment:02d}.ogg"
        key = path_key(path)
        entry = by_key.get(key)
        if entry is None or entry[5] != expected_type:
            break
        relative_offset, _, size, _, entry_id, entry_type, _ = entry
        result.append(
            (
                path,
                {
                    # Keep the historical field name so decrypt_ogg and the
                    # voice-map TSV schema remain backward compatible.
                    "jp_ogg_path": path,
                    "archive_key_hex": f"{key:08x}",
                    "archive_entry_id": str(entry_id),
                    "encrypted_absolute_offset": str(payload_offset + relative_offset),
                    "size_bytes": str(size),
                    "archive_entry_type": f"{entry_type:x}",
                },
            )
        )
    if not result:
        raise KeyError(
            f"PC archive has no contiguous {language.upper()} voice for {message_id}"
        )
    return result


def resolve_voice_records(
    row: dict[str, str],
    language: str,
    *,
    manifest: dict[str, dict[str, str]] | None = None,
    archive_index: ArchiveVoiceIndex | None = None,
) -> list[tuple[str, dict[str, str]]]:
    """Return source Ogg records for one selected DS target in playback order."""
    return [
        record
        for component in resolve_voice_components(
            row,
            language,
            manifest=manifest,
            archive_index=archive_index,
        )
        for record in component
    ]


def resolve_voice_components(
    row: dict[str, str],
    language: str,
    *,
    manifest: dict[str, dict[str, str]] | None = None,
    archive_index: ArchiveVoiceIndex | None = None,
) -> list[list[tuple[str, dict[str, str]]]]:
    """Resolve source Ogg records separately for each ordered PC message."""
    if language == "jp":
        if manifest is None:
            raise ValueError("Japanese voice resolution requires the PC manifest")
        result = []
        for _message_id, paths in contiguous_paths_by_message(row):
            component = []
            for path in paths:
                record = manifest.get(path)
                if record is None:
                    raise KeyError(f"Alignment path absent from PC manifest: {path}")
                component.append((path, record))
            result.append(component)
        return result
    if language == "en":
        if archive_index is None:
            raise ValueError("English voice resolution requires the PC archive index")
        return [
            resolve_archive_message(archive_index, message_id, language)
            for message_id in alignment_message_ids(row)
        ]
    raise ValueError(f"Unsupported voice language: {language!r}")


def speaker_key(value: str) -> str:
    return unicodedata.normalize("NFKC", value).strip().casefold()


def alignment_message_ids(row: dict[str, str]) -> list[str]:
    """Return the ordered PC message IDs represented by one DS voice row."""
    raw = row.get("pc_message_ids", "").strip() or row.get("pc_message_id", "").strip()
    message_ids = [part.strip() for part in raw.split("|") if part.strip()]
    if not message_ids:
        raise ValueError("Selected alignment row has no PC message ID")
    if len(message_ids) != len(set(message_ids)):
        raise ValueError(f"Selected alignment row repeats a PC message ID: {raw}")
    declared = row.get("message_count", "").strip()
    if declared:
        try:
            declared_count = int(declared)
        except ValueError as exc:
            raise ValueError(f"Invalid alignment message_count {declared!r}") from exc
        if declared_count != len(message_ids):
            raise ValueError(
                f"Alignment declares {declared_count} messages but contains "
                f"{len(message_ids)} IDs: {raw}"
            )
    return message_ids


def load_alignment(
    path: Path, *, allow_narration_voices: bool = False
) -> list[tuple[tuple[str, int], dict[str, str]]]:
    """Load an explicitly selected alignment with one generated voice per DS line.

    Extended rows may concatenate several consecutive PC messages into a single
    DS resource.  A reviewed split group may instead reuse one PC message for
    consecutive targets, but only with validated per-language audio intervals.
    """
    selected: list[tuple[tuple[str, int], dict[str, str]]] = []
    source_rows: dict[str, list[dict[str, str]]] = {}
    reviewed_overrides: list[ReviewedOverride] = []
    target_lines: set[tuple[str, int]] = set()
    target_ordinals: set[tuple[str, int]] = set()
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {
            "alignment_method",
            "confidence",
            "ds_script",
            "ds_settext_ordinal",
            "ds_source_line",
            "ds_speaker",
            "jp_ogg_paths",
            "pc_message_id",
            "pc_speaker",
            "selection_status",
        }
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(
                f"Alignment lacks columns: {sorted(required - set(reader.fieldnames or []))}"
            )
        for row_number, row in enumerate(reader, start=2):
            if row["selection_status"].strip().casefold() != "selected":
                raise ValueError(
                    f"Alignment row {row_number} is not explicitly selection_status=selected"
                )
            if row.get("mapping_status", "").strip().casefold() == "unmapped":
                raise ValueError(f"Selected alignment row {row_number} is unmapped")
            method = row["alignment_method"].strip().casefold()
            if method == "sequence_fallback":
                raise ValueError(
                    f"Selected alignment row {row_number} uses forbidden sequence_fallback"
                )
            confidence = row["confidence"].strip().casefold()
            if confidence not in SELECTED_CONFIDENCES:
                raise ValueError(
                    f"Selected alignment row {row_number} has unsafe confidence {confidence!r}"
                )

            ds_speaker = speaker_key(row["ds_speaker"])
            pc_speaker = speaker_key(row["pc_speaker"])
            speaker_policy = row.get("speaker_policy", "").strip().casefold()
            if not ds_speaker or not pc_speaker:
                raise ValueError(
                    f"Selected alignment row {row_number} has an empty speaker"
                )
            if ds_speaker in NARRATION_SPEAKERS:
                if not allow_narration_voices:
                    raise ValueError(
                        f"Selected alignment row {row_number} targets narration "
                        f"({row['ds_speaker']!r}); pass --allow-narration-voices "
                        "only when that behavior is explicitly intended"
                    )
                if speaker_policy != "novel_semantic" or not method.startswith(
                    "extended_"
                ):
                    raise ValueError(
                        f"Selected alignment row {row_number} targets narration without "
                        "an explicit extended novel_semantic policy"
                    )
            elif ds_speaker != pc_speaker:
                if not (
                    method == "reviewed_override"
                    and speaker_policy == "reviewed_equivalent"
                ):
                    raise ValueError(
                        f"Selected alignment row {row_number} has incompatible speakers: "
                        f"{row['ds_speaker']!r} vs {row['pc_speaker']!r}"
                    )
            elif speaker_policy not in {"", "same"}:
                raise ValueError(
                    f"Selected alignment row {row_number} has unexpected speaker policy "
                    f"{speaker_policy!r}"
                )

            script = Path(row["ds_script"]).name
            if not script:
                raise ValueError(
                    f"Selected alignment row {row_number} has no DS script"
                )
            try:
                source_line = int(row["ds_source_line"])
                ordinal = int(row["ds_settext_ordinal"])
            except ValueError as exc:
                raise ValueError(
                    f"Selected alignment row {row_number} has a non-integer DS target"
                ) from exc
            if source_line <= 0 or ordinal < 0:
                raise ValueError(
                    f"Selected alignment row {row_number} has an invalid DS target"
                )

            pc_message_ids = alignment_message_ids(row)
            if not row["jp_ogg_paths"].strip():
                raise ValueError(
                    f"Selected alignment row {row_number} lacks a PC message or Japanese Ogg"
                )
            line_target = (script.casefold(), source_line)
            ordinal_target = (script.casefold(), ordinal)
            if line_target in target_lines or ordinal_target in target_ordinals:
                raise ValueError(
                    f"DS target is mapped more than once: {script}:{source_line} "
                    f"(SetText ordinal {ordinal})"
                )
            target_lines.add(line_target)
            target_ordinals.add(ordinal_target)
            normalized = dict(row)
            normalized["ds_script"] = script
            normalized["pc_message_id"] = MESSAGE_SEPARATOR.join(pc_message_ids)
            normalized["pc_message_ids"] = normalized["pc_message_id"]
            normalized["message_count"] = str(len(pc_message_ids))
            normalized["confidence"] = confidence
            normalized["alignment_method"] = method
            normalized["speaker_policy"] = speaker_policy or "same"
            if method == "reviewed_override":
                if confidence != "review":
                    raise ValueError(
                        f"Reviewed override row {row_number} must use review confidence"
                    )
                reviewed_overrides.extend(
                    overrides_from_runtime_row(normalized, row_number)
                )
            elif any(
                normalized.get(field, "").strip()
                for field in (
                    "reviewed_override_group",
                    "target_text_sha256",
                    "jp_start_ms",
                    "jp_end_ms",
                    "en_start_ms",
                    "en_end_ms",
                )
            ):
                raise ValueError(
                    f"Alignment row {row_number} carries override metadata without "
                    "alignment_method=reviewed_override"
                )
            for message_id in pc_message_ids:
                source_rows.setdefault(message_id, []).append(normalized)
            selected.append(((script, source_line), normalized))
    if reviewed_overrides:
        validate_overrides(reviewed_overrides)
    for message_id, rows in source_rows.items():
        if len(rows) == 1:
            continue
        if any(row["alignment_method"] != "reviewed_override" for row in rows):
            raise ValueError(f"PC message is mapped more than once: {message_id}")
        groups = {row["reviewed_override_group"] for row in rows}
        if len(groups) != 1:
            raise ValueError(
                f"PC message {message_id!r} is reused outside one reviewed split group"
            )
    if not selected:
        raise ValueError("Alignment contains no explicitly selected voice rows")
    return sorted(
        selected,
        key=lambda item: (
            item[0][0].casefold(),
            int(item[1]["ds_settext_ordinal"]),
            item[0][1],
        ),
    )


def contiguous_paths(row: dict[str, str]) -> list[str]:
    """Return grouped message segments in order, dropping hash-collision tails."""
    return [
        path
        for _message_id, paths in contiguous_paths_by_message(row)
        for path in paths
    ]


def contiguous_paths_by_message(
    row: dict[str, str],
) -> list[tuple[str, list[str]]]:
    """Return one contiguous JP path list per ordered PC message."""
    paths = [part.strip() for part in row["jp_ogg_paths"].split("|") if part.strip()]
    message_ids = alignment_message_ids(row)
    grouped: dict[str, list[str]] = {message_id: [] for message_id in message_ids}
    for path in paths:
        stem = Path(path).stem
        matches = [
            message_id
            for message_id in message_ids
            if stem.startswith(f"{message_id}_")
        ]
        if len(matches) != 1:
            raise ValueError(
                f"Voice path does not resolve to exactly one selected message: {path}"
            )
        grouped[matches[0]].append(path)

    result: list[tuple[str, list[str]]] = []
    for message_id in message_ids:
        if not grouped[message_id]:
            raise ValueError(f"Selected PC message has no Ogg path: {message_id}")
        result.append((message_id, contiguous_message_paths(grouped[message_id])))
    return result


def contiguous_message_paths(paths: list[str]) -> list[str]:
    """Discard accidental non-contiguous path-hash collisions (notably _88)."""
    numbered = []
    for path in paths:
        match = SEGMENT.search(path)
        if not match:
            raise ValueError(f"Voice path has no numeric segment: {path}")
        numbered.append((int(match.group(1)), path))
    numbered.sort()
    numbers = [number for number, _ in numbered]
    if 0 in numbers:
        allowed = []
        expected = 0
        for number, path in numbered:
            if number != expected:
                break
            allowed.append(path)
            expected += 1
        return allowed
    return [path for _, path in numbered]


def decrypt_ogg(archive, row: dict[str, str]) -> bytes:
    offset = int(row["encrypted_absolute_offset"])
    size = int(row["size_bytes"])
    key = int(row["archive_key_hex"], 16)
    archive.seek(offset)
    encrypted = archive.read(size)
    if len(encrypted) != size:
        raise EOFError(f"Truncated PC archive entry at {offset}")
    data = crypt(encrypted, key)
    if not data.startswith(b"OggS"):
        path = row.get("jp_ogg_path", "<unknown archive path>")
        raise ValueError(f"Decryption did not yield Ogg: {path}")
    return data


def decode_pcm(ogg: bytes) -> np.ndarray:
    samples, rate = sf.read(io.BytesIO(ogg), dtype="float32", always_2d=True)
    if samples.size == 0:
        raise ValueError("Decoded an empty Ogg")
    mono = samples.mean(axis=1, dtype=np.float32)
    if rate != SAMPLE_RATE:
        mono = soxr.resample(mono, rate, SAMPLE_RATE, quality="HQ")
    return np.rint(np.clip(mono, -0.999969, 0.999969) * 32767.0).astype(np.int16)


def _slice_value(
    row: Mapping[str, str],
    field: str,
    component_index: int,
    component_count: int,
) -> str:
    return runtime_component_values(
        row,
        field,
        component_count,
        label=f"{field} for component {component_index}",
    )[component_index]


def _fade_slice_edges(pcm: np.ndarray, *, fade_in: bool, fade_out: bool) -> np.ndarray:
    if not fade_in and not fade_out:
        return pcm
    faded = pcm.astype(np.float64, copy=True)
    fade_length = min(
        len(faded), max(2, round(SAMPLE_RATE * SLICE_EDGE_FADE_MS / 1000))
    )
    envelope = np.ones(len(faded), dtype=np.float64)
    ramp = np.linspace(0.0, 1.0, fade_length)
    if fade_in:
        envelope[:fade_length] = np.minimum(envelope[:fade_length], ramp)
    if fade_out:
        envelope[-fade_length:] = np.minimum(envelope[-fade_length:], ramp[::-1])
    return np.rint(faded * envelope).astype(np.int16)


def slice_pcm(
    pcm: np.ndarray,
    row: Mapping[str, str],
    language: str,
    *,
    component_index: int = 0,
    component_count: int | None = None,
) -> np.ndarray:
    """Apply one component interval and soften only newly cut waveform edges."""
    if component_count is None:
        declared = row.get("message_count", "").strip()
        component_count = int(declared) if declared else 1
    if component_count < 1 or not 0 <= component_index < component_count:
        raise ValueError("Invalid reviewed audio component position")
    start_value = _slice_value(
        row, f"{language}_start_ms", component_index, component_count
    )
    end_value = _slice_value(
        row, f"{language}_end_ms", component_index, component_count
    )
    if not start_value and not end_value:
        return pcm
    if not start_value.isascii() or not start_value.isdecimal():
        raise ValueError(f"Invalid {language.upper()} slice start: {start_value!r}")
    if end_value and (not end_value.isascii() or not end_value.isdecimal()):
        raise ValueError(f"Invalid {language.upper()} slice end: {end_value!r}")
    start_ms = int(start_value)
    end_ms = int(end_value) if end_value else None
    start = (start_ms * SAMPLE_RATE + 500) // 1000
    end = len(pcm) if end_ms is None else (end_ms * SAMPLE_RATE + 500) // 1000
    if start >= end or end > len(pcm):
        duration_ms = len(pcm) * 1000 / SAMPLE_RATE
        raise ValueError(
            f"{language.upper()} slice {start_ms}.."
            f"{'' if end_ms is None else end_ms} ms is outside a {duration_ms:.3f} ms voice"
        )
    return _fade_slice_edges(
        pcm[start:end],
        fade_in=start > 0,
        fade_out=end < len(pcm),
    )


def compose_voice_pcm(
    archive: BinaryIO,
    row: Mapping[str, str],
    language: str,
    components: list[list[tuple[str, dict[str, str]]]],
    silence: np.ndarray,
) -> tuple[np.ndarray, list[str]]:
    """Decode, slice, then concatenate ordered message components."""
    message_ids = alignment_message_ids(dict(row))
    if len(components) != len(message_ids):
        raise ValueError(
            f"Resolved {len(components)} audio components for {len(message_ids)} messages"
        )
    output_chunks: list[np.ndarray] = []
    used_paths: list[str] = []
    for component_index, records in enumerate(components):
        if not records:
            raise ValueError(
                f"Resolved no voice records for PC message {message_ids[component_index]}"
            )
        source_chunks: list[np.ndarray] = []
        for path, record in records:
            if source_chunks:
                source_chunks.append(silence)
            source_chunks.append(decode_pcm(decrypt_ogg(archive, record)))
            used_paths.append(path)
        component_pcm = slice_pcm(
            np.concatenate(source_chunks),
            row,
            language,
            component_index=component_index,
            component_count=len(components),
        )
        if output_chunks:
            output_chunks.append(silence)
        output_chunks.append(component_pcm)
    return np.concatenate(output_chunks), used_paths


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ze1_data", type=Path)
    parser.add_argument("pc_manifest", type=Path)
    parser.add_argument("alignment", type=Path)
    parser.add_argument("template_se", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument(
        "--language",
        choices=sorted(VOICE_LANGUAGE_SPECS),
        default="jp",
        help=(
            "Source dub language (default: jp). EN is resolved directly from "
            "the contiguous /sound/voice_us archive entries."
        ),
    )
    parser.add_argument(
        "--id-source-rom",
        type=Path,
        required=True,
        help="Original retail ROM whose sound/*.se IDs must never be reused.",
    )
    parser.add_argument("--map-output", type=Path, required=True)
    parser.add_argument("--symbols-output", type=Path, required=True)
    parser.add_argument("--silence-ms", type=float, default=65.0)
    parser.add_argument("--max-duration", type=float, default=45.0)
    parser.add_argument(
        "--resume",
        action="store_true",
        help=(
            "Continue an interrupted build only when its saved input hashes and "
            "generation settings still match."
        ),
    )
    parser.add_argument(
        "--allow-narration-voices",
        action="store_true",
        help=(
            "Allow character audio on NOVEL/HERO targets. Disabled by default "
            "so narration cannot be voiced accidentally."
        ),
    )
    args = parser.parse_args()

    preflight_output_directory(args.output_dir, resume=args.resume)
    manifest = load_manifest(args.pc_manifest) if args.language == "jp" else None
    alignments = load_alignment(
        args.alignment,
        allow_narration_voices=args.allow_narration_voices,
    )
    if len(alignments) > 10_000:
        raise ValueError(
            "SE_V#### symbol namespace supports at most 10,000 voice files"
        )
    retail_id_by_resource = collect_retail_internal_ids(args.id_source_rom)
    retail_ids = set(retail_id_by_resource.values())
    identifiers = allocate_internal_ids(retail_ids, len(alignments))
    template = args.template_se.read_bytes()
    validate_se(template, expected_internal_id=0x6604)
    provenance = build_provenance(
        language=args.language,
        alignment=args.alignment,
        pc_archive=args.ze1_data,
        pc_manifest=args.pc_manifest,
        template_se=args.template_se,
        id_source_rom=args.id_source_rom,
        silence_ms=args.silence_ms,
        max_duration=args.max_duration,
        allow_narration_voices=args.allow_narration_voices,
    )
    prepare_output_directory(
        args.output_dir,
        provenance,
        resume=args.resume,
        expected_voice_count=len(alignments),
    )
    silence = np.zeros(round(SAMPLE_RATE * args.silence_ms / 1000.0), dtype=np.int16)

    map_rows = []
    source_ids: set[str] = set()
    total_samples = 0
    total_bytes = 0
    with args.ze1_data.open("rb") as archive:
        archive_index = (
            load_archive_voice_index(archive) if args.language == "en" else None
        )
        for index, ((script, source_line), row) in enumerate(alignments):
            symbol = f"SE_V{index:04d}"
            identifier = identifiers[index]
            message_ids = alignment_message_ids(row)
            source_ids.update(message_ids)
            components = resolve_voice_components(
                row,
                args.language,
                manifest=manifest,
                archive_index=archive_index,
            )
            pcm, used_paths = compose_voice_pcm(
                archive, row, args.language, components, silence
            )
            duration = len(pcm) / SAMPLE_RATE
            if duration > args.max_duration:
                raise ValueError(
                    f"{script}:{source_line} contains {duration:.2f}s of voice, above "
                    f"--max-duration {args.max_duration:.2f}s"
                )

            output = args.output_dir / f"{symbol.lower()}.se"
            generated = build_se(template, pcm, SAMPLE_RATE, identifier, symbol)
            info = validate_se(generated, symbol, identifier)
            publish_generated_voice(output, generated, resume=args.resume)
            total_samples += len(pcm)
            total_bytes += info.file_size
            map_rows.append(
                {
                    "symbol": symbol,
                    "internal_id_hex": f"{identifier:04x}",
                    "selection_status": row["selection_status"],
                    "ds_script": script,
                    "ds_source_line": source_line,
                    "ds_settext_ordinal": row["ds_settext_ordinal"],
                    "ds_speaker": row["ds_speaker"],
                    "ds_text": row.get("ds_text", ""),
                    "pc_speaker": row["pc_speaker"],
                    "pc_message_ids": MESSAGE_SEPARATOR.join(message_ids),
                    "jp_ogg_paths": " | ".join(used_paths),
                    "message_count": len(message_ids),
                    "duration_seconds": f"{duration:.6f}",
                    "se_size_bytes": info.file_size,
                    "confidence": row["confidence"],
                    "alignment_method": row["alignment_method"],
                    "speaker_policy": row["speaker_policy"],
                    "reviewed_override_group": row.get("reviewed_override_group", ""),
                    "target_text_sha256": row.get("target_text_sha256", ""),
                    "jp_start_ms": row.get("jp_start_ms", ""),
                    "jp_end_ms": row.get("jp_end_ms", ""),
                    "en_start_ms": row.get("en_start_ms", ""),
                    "en_end_ms": row.get("en_end_ms", ""),
                }
            )
            if (index + 1) % 100 == 0 or index + 1 == len(alignments):
                print(
                    f"progress={index + 1}/{len(alignments)} "
                    f"audio_hours={total_samples / SAMPLE_RATE / 3600:.3f} "
                    f"se_mib={total_bytes / 1048576:.2f}",
                    flush=True,
                )

    expected_files = [f"se_v{index:04d}.se" for index in range(len(map_rows))]
    actual_files = [
        path.name.lower() for path in generated_voice_paths(args.output_dir)
    ]
    if actual_files != expected_files:
        raise ValueError(
            "Voice output directory contains a stale or non-contiguous SE_V file set; "
            f"expected {len(expected_files)}, found {len(actual_files)}"
        )

    args.map_output.parent.mkdir(parents=True, exist_ok=True)
    with args.map_output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(
            stream, fieldnames=list(map_rows[0]), delimiter="\t", lineterminator="\n"
        )
        writer.writeheader()
        writer.writerows(map_rows)
    args.symbols_output.parent.mkdir(parents=True, exist_ok=True)
    args.symbols_output.write_text(
        "".join(f"{row['symbol']}\n" for row in map_rows), encoding="ascii"
    )
    print(f"voice_files={len(map_rows)}")
    print(f"pc_messages={len(source_ids)}")
    print(f"retail_se_ids={len(retail_ids)}")
    print(f"first_internal_id={identifiers[0]:04x}")
    print(f"last_internal_id={identifiers[-1]:04x}")
    print(f"duration_hours={total_samples / SAMPLE_RATE / 3600:.6f}")
    print(f"se_bytes={total_bytes}")


if __name__ == "__main__":
    main()
