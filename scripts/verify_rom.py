#!/usr/bin/env python3
"""Verify the rebuilt 999 ROM and every injected Japanese voice resource."""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
from collections import defaultdict
from pathlib import Path

import ndspy.rom

from apply_reviewed_alignment_overrides import (
    ReviewedOverride,
    overrides_from_runtime_row,
    validate_overrides,
)
from silence_text_bleeps import BleepPatchError, parse_bank
from sound_registry import DEFAULT_SE_CATEGORY, validate_registry
from voice_audio import read_dse_internal_id, validate_se


VOICE_FILENAME_RE = re.compile(r"se_v\d+\.se", re.IGNORECASE)
INTERNAL_ID_RE = re.compile(r"[0-9a-fA-F]{4}")


def sha256(data: bytes | bytearray) -> str:
    return hashlib.sha256(data).hexdigest()


def read_voice_map(path: Path) -> tuple[list[dict[str, str]], list[str], list[int]]:
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {
            "ds_script",
            "ds_settext_ordinal",
            "ds_source_line",
            "internal_id_hex",
            "message_count",
            "pc_message_ids",
            "selection_status",
            "symbol",
        }
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(
                f"voice map lacks columns: {sorted(required - set(reader.fieldnames or []))}"
            )
        rows = list(reader)
    if not rows:
        raise ValueError("voice map is empty")

    expected_symbols = [f"SE_V{index:04d}" for index in range(len(rows))]
    actual_symbols = [row["symbol"] for row in rows]
    if actual_symbols != expected_symbols:
        raise ValueError("voice-map symbols are not contiguous SE_V0000, SE_V0001, ...")

    identifiers: list[int] = []
    seen_ids: set[int] = set()
    message_rows: dict[str, list[dict[str, str]]] = defaultdict(list)
    reviewed_overrides: list[ReviewedOverride] = []
    target_lines: set[tuple[str, int]] = set()
    target_ordinals: set[tuple[str, int]] = set()
    for row_number, row in enumerate(rows, start=2):
        raw_identifier = row["internal_id_hex"].strip()
        if not INTERNAL_ID_RE.fullmatch(raw_identifier):
            raise ValueError(
                f"voice-map row {row_number} has invalid internal_id_hex "
                f"{raw_identifier!r}"
            )
        identifier = int(raw_identifier, 16)
        high, low = identifier >> 8, identifier & 0xFF
        if not 0x64 <= high <= 0xC8 or not 0x01 <= low <= 0x65:
            raise ValueError(
                f"voice-map row {row_number} ID 0x{identifier:04x} is outside "
                "the safe C8..64:01..65 allocation space"
            )
        if identifier in seen_ids:
            raise ValueError(f"duplicate generated DSE ID 0x{identifier:04x}")
        seen_ids.add(identifier)
        identifiers.append(identifier)

        if row["selection_status"].strip().casefold() != "selected":
            raise ValueError(f"voice-map row {row_number} is not semantically selected")
        try:
            message_count = int(row["message_count"].strip())
        except ValueError as exc:
            raise ValueError(
                f"voice-map row {row_number} has an invalid message_count"
            ) from exc
        message_ids = [
            part.strip() for part in row["pc_message_ids"].split("|") if part.strip()
        ]
        if message_count < 1 or message_count != len(message_ids):
            raise ValueError(
                f"voice-map row {row_number} declares {message_count} PC messages "
                f"but contains {len(message_ids)} IDs"
            )
        if len(message_ids) != len(set(message_ids)):
            raise ValueError(f"voice-map row {row_number} repeats a PC message")
        normalized = dict(row)
        normalized["pc_message_id"] = " | ".join(message_ids)
        if row.get("alignment_method", "").strip().casefold() == "reviewed_override":
            reviewed_overrides.extend(
                overrides_from_runtime_row(normalized, row_number)
            )
        for message_id in message_ids:
            message_rows[message_id].append(normalized)

        script = Path(row["ds_script"]).name.casefold()
        if not script:
            raise ValueError(f"voice-map row {row_number} has no DS script")
        try:
            source_line = int(row["ds_source_line"])
            ordinal = int(row["ds_settext_ordinal"])
        except ValueError as exc:
            raise ValueError(
                f"voice-map row {row_number} has a non-integer DS target"
            ) from exc
        if source_line <= 0 or ordinal < 0:
            raise ValueError(f"voice-map row {row_number} has an invalid DS target")
        line_target = (script, source_line)
        ordinal_target = (script, ordinal)
        if line_target in target_lines or ordinal_target in target_ordinals:
            raise ValueError(f"voice-map row {row_number} duplicates a DS target")
        target_lines.add(line_target)
        target_ordinals.add(ordinal_target)
    if reviewed_overrides:
        validate_overrides(reviewed_overrides)
    for message_id, usages in message_rows.items():
        if len(usages) == 1:
            continue
        if any(
            row.get("alignment_method", "").strip().casefold() != "reviewed_override"
            for row in usages
        ):
            raise ValueError(f"voice map reuses PC message {message_id}")
        groups = {row.get("reviewed_override_group", "") for row in usages}
        if len(groups) != 1 or not next(iter(groups)):
            raise ValueError(
                f"voice map reuses PC message {message_id} outside one reviewed split group"
            )
    return rows, expected_symbols, identifiers


def verify_registry_symbols(
    categories, expected_symbols: list[str], category_name: str
) -> None:
    matches = [category for category in categories if category.name == category_name]
    if len(matches) != 1:
        raise ValueError(
            f"sound registry has {len(matches)} categories named {category_name!r}, expected one"
        )
    category = matches[0]
    expected_tuple = tuple(expected_symbols)
    if len(category.entries) < len(expected_tuple):
        raise ValueError(
            f"sound registry category {category_name!r} is too short for generated symbols"
        )
    if category.entries[-len(expected_tuple) :] != expected_tuple:
        raise ValueError(
            f"sound registry category {category_name!r} does not end with the generated symbols"
        )

    expected_folded = {symbol.casefold(): symbol for symbol in expected_symbols}
    occurrences: dict[str, list[tuple[str, int, str]]] = defaultdict(list)
    for item in categories:
        for index, entry in enumerate(item.entries):
            folded = entry.casefold()
            if folded in expected_folded:
                occurrences[folded].append((item.name, index, entry))
    generated_start = len(category.entries) - len(expected_symbols)
    for index, symbol in enumerate(expected_symbols):
        found = occurrences[symbol.casefold()]
        expected = [(category_name, generated_start + index, symbol)]
        if found != expected:
            raise ValueError(
                f"registry symbol {symbol} is missing, duplicated, misplaced, or case-mangled: "
                f"{found!r}"
            )


def collect_retail_ids_from_built_rom(
    rom: ndspy.rom.NintendoDSRom, expected_filenames: list[str]
) -> dict[str, int]:
    """Parse all non-generated ``sound/*.se`` IDs in the rebuilt ROM."""
    sound_folder = rom.filenames.subfolder("sound")
    if sound_folder is None:
        raise ValueError("ROM has no sound folder")
    generated_names = [
        filename.lower()
        for filename in sound_folder.files
        if VOICE_FILENAME_RE.fullmatch(filename)
    ]
    if generated_names != expected_filenames:
        raise ValueError(
            "ROM generated voice filenames are stale, non-contiguous, or differ from the map"
        )

    generated_set = set(expected_filenames)
    retail: dict[str, int] = {}
    for filename in sound_folder.files:
        if not filename.lower().endswith(".se") or filename.lower() in generated_set:
            continue
        nitro_path = f"sound/{filename}"
        file_id = rom.filenames.idOf(nitro_path)
        if file_id is None:
            raise ValueError(f"FNT cannot resolve retail resource {nitro_path}")
        retail[nitro_path] = read_dse_internal_id(bytes(rom.files[file_id]), nitro_path)
    if not retail:
        raise ValueError("ROM has no non-generated sound/*.se resources")
    return retail


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rom", type=Path)
    parser.add_argument("voice_map", type=Path)
    parser.add_argument("--compiled-dir", type=Path)
    parser.add_argument("--voice-dir", type=Path)
    parser.add_argument(
        "--registry-category",
        default=DEFAULT_SE_CATEGORY,
        help=f"category expected to contain generated symbols (default: {DEFAULT_SE_CATEGORY})",
    )
    parser.add_argument(
        "--se-sys",
        type=Path,
        help=(
            "Require sound/se_sys.se to be byte-identical to this fully "
            "silenced anti-bleep bank."
        ),
    )
    args = parser.parse_args()

    raw = args.rom.read_bytes()
    rom = ndspy.rom.NintendoDSRom(raw)
    rows, expected_symbols, identifiers = read_voice_map(args.voice_map)
    expected_filenames = [f"se_v{index:04d}.se" for index in range(len(rows))]

    sound_dat_id = rom.filenames.idOf("etc/sound.dat")
    if sound_dat_id is None:
        raise ValueError("ROM has no etc/sound.dat")
    categories, relocations = validate_registry(bytes(rom.files[sound_dat_id]))
    verify_registry_symbols(categories, expected_symbols, args.registry_category)

    retail_id_by_resource = collect_retail_ids_from_built_rom(rom, expected_filenames)
    retail_resources_by_id: dict[int, list[str]] = defaultdict(list)
    for resource, identifier in retail_id_by_resource.items():
        retail_resources_by_id[identifier].append(resource)
    collisions = {
        identifier: retail_resources_by_id[identifier]
        for identifier in identifiers
        if identifier in retail_resources_by_id
    }
    if collisions:
        rendered = ", ".join(
            f"0x{identifier:04x} ({' | '.join(resources)})"
            for identifier, resources in sorted(collisions.items())
        )
        raise ValueError(f"generated DSE IDs collide with retail resources: {rendered}")

    text_bleeps = "unchecked"
    if args.se_sys is not None:
        expected_se_sys = args.se_sys.read_bytes()
        try:
            expected_parsed = parse_bank(expected_se_sys)
        except BleepPatchError as error:
            raise ValueError(
                f"Invalid expected anti-bleep se_sys.se: {error}"
            ) from error
        if expected_parsed.state != "silenced":
            raise ValueError(
                f"Expected anti-bleep se_sys.se is {expected_parsed.state}, not silenced"
            )

        se_sys_id = rom.filenames.idOf("sound/se_sys.se")
        if se_sys_id is None:
            raise ValueError("ROM has no sound/se_sys.se")
        embedded_se_sys = bytes(rom.files[se_sys_id])
        if embedded_se_sys != expected_se_sys:
            raise ValueError(
                "ROM sound/se_sys.se differs from the expected anti-bleep bank"
            )
        try:
            embedded_parsed = parse_bank(embedded_se_sys)
        except BleepPatchError as error:
            raise ValueError(f"Embedded sound/se_sys.se is invalid: {error}") from error
        if embedded_parsed.state != "silenced":
            raise ValueError("Embedded sound/se_sys.se is not fully silenced")
        text_bleeps = "silenced"

    first_voice_id = rom.filenames.idOf("sound/se_v0000.se")
    if first_voice_id is None:
        raise ValueError("ROM has no sound/se_v0000.se")
    total_voice_bytes = 0
    for index, (row, symbol, identifier) in enumerate(
        zip(rows, expected_symbols, identifiers, strict=True)
    ):
        path = f"sound/{symbol.lower()}.se"
        file_id = rom.filenames.idOf(path)
        if file_id != first_voice_id + index:
            raise ValueError(f"non-contiguous NitroFS ID for {path}: {file_id}")
        payload = bytes(rom.files[file_id])
        info = validate_se(payload, symbol, identifier)
        if read_dse_internal_id(payload, path) != identifier:
            raise ValueError(f"strict DSE ID reparse disagrees for {path}")
        total_voice_bytes += info.file_size
        if args.voice_dir is not None:
            source = (args.voice_dir / f"{symbol.lower()}.se").read_bytes()
            if payload != source:
                raise ValueError(f"ROM payload differs from source file: {path}")

    scripts = {Path(row["ds_script"]).name.removesuffix(".txt") for row in rows}
    if args.compiled_dir is not None:
        for script in scripts:
            file_id = rom.filenames.idOf(f"scr/{script}")
            if file_id is None:
                raise ValueError(f"ROM has no scr/{script}")
            if bytes(rom.files[file_id]) != (args.compiled_dir / script).read_bytes():
                raise ValueError(f"ROM script differs from compiled source: {script}")

    print(f"sha256={sha256(raw)}")
    print(f"bytes={len(raw)}")
    print(f"game_code={rom.idCode.decode('ascii')}")
    print(f"files={len(rom.files)}")
    print(f"voices={len(rows)}")
    print(f"pc_messages={sum(int(row['message_count']) for row in rows)}")
    print(
        f"grouped_voice_resources={sum(int(row['message_count']) > 1 for row in rows)}"
    )
    print(f"voice_bytes={total_voice_bytes}")
    print(f"scripts={len(scripts)}")
    print(f"registry_categories={len(categories)}")
    print(f"registry_relocations={len(relocations)}")
    print(f"registry_voice_category={args.registry_category}")
    print(f"retail_se_ids={len(set(retail_id_by_resource.values()))}")
    print("dse_id_collisions=0")
    print(f"text_bleeps={text_bleeps}")
    print("status=OK")


if __name__ == "__main__":
    main()
