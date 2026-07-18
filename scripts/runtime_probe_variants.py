#!/usr/bin/env python3
"""Build small, disposable ROM variants for the Japanese-voice runtime audit.

The variants isolate three possible failure points without touching the final
ROM: the script opcode, registry cardinality, and the generated symbol prefix.
"""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path

import ndspy.rom

from sound_registry import append_entries, parse_categories, read_u32, validate_registry
from voice_audio import CHUNK_HEADER_SIZE, build_se, decode_nds_ima, inspect_se


def replace_first_pair(text: str, old: str, new: str) -> str:
    play = f'Sound::PlaySE(":{old}", 127f, 0f);'
    stop = f'Sound::StopSE(":{old}", 0f);'
    if text.count(play) != 1 or text.count(stop) != 1:
        raise ValueError(f"expected exactly one {old} PlaySE/StopSE pair")
    return text.replace(play, play.replace(old, new)).replace(stop, stop.replace(old, new))


def insert_one_file(
    source_rom: Path,
    output_rom: Path,
    compiled_script: Path,
    sound_dat: bytes,
    filename: str,
    payload: bytes,
) -> None:
    rom = ndspy.rom.NintendoDSRom.fromFile(str(source_rom))
    script_id = rom.filenames.idOf("scr/a01b.fsb")
    sound_dat_id = rom.filenames.idOf("etc/sound.dat")
    sound_folder = rom.filenames.subfolder("sound")
    if script_id is None or sound_dat_id is None or sound_folder is None:
        raise ValueError("input ROM is missing a required NitroFS resource")
    insertion_id = sound_folder.firstID + len(sound_folder.files)

    rom.files[script_id] = bytearray(compiled_script.read_bytes())
    rom.files[sound_dat_id] = bytearray(sound_dat)
    rom.files[insertion_id:insertion_id] = [bytearray(payload)]
    sound_folder.files.append(filename)

    def shift(folder) -> None:
        for _, child in folder.folders:
            if child.firstID >= insertion_id:
                child.firstID += 1
            shift(child)

    shift(rom.filenames)
    rom.sortedFileIds = list(range(len(rom.files)))
    output_rom.parent.mkdir(parents=True, exist_ok=True)
    output_rom.write_bytes(rom.save(updateDeviceCapacity=True))


def replace_registry_symbol(data: bytes, old: str, new: str) -> bytes:
    """Replace one registry string in place without changing the SIR0 layout."""
    categories = parse_categories(data)
    matches = [
        (category, index)
        for category in categories
        for index, entry in enumerate(category.entries)
        if entry == old
    ]
    if len(matches) != 1:
        raise ValueError(f"expected one registry entry {old!r}, found {len(matches)}")
    category, index = matches[0]
    pointer = read_u32(data, category.table_offset + index * 4)
    old_bytes = old.encode("ascii")
    new_bytes = new.encode("ascii")
    if len(new_bytes) > len(old_bytes):
        raise ValueError("replacement registry symbol does not fit in the retail string slot")
    output = bytearray(data)
    output[pointer : pointer + len(old_bytes) + 1] = (
        new_bytes + b"\0" + bytes(len(old_bytes) - len(new_bytes))
    )
    validate_registry(output)
    reparsed = parse_categories(output)
    if sum(entry == new for item in reparsed for entry in item.entries) != 1:
        raise ValueError("replacement registry symbol did not reparse uniquely")
    return bytes(output)


def replace_one_file(
    source_rom: Path,
    output_rom: Path,
    compiled_script: Path,
    sound_dat: bytes,
    old_path: str,
    new_filename: str,
    payload: bytes,
) -> None:
    """Replace and optionally rename one existing NitroFS file without insertion."""
    rom = ndspy.rom.NintendoDSRom.fromFile(str(source_rom))
    script_id = rom.filenames.idOf("scr/a01b.fsb")
    sound_dat_id = rom.filenames.idOf("etc/sound.dat")
    old_id = rom.filenames.idOf(old_path)
    sound_folder = rom.filenames.subfolder("sound")
    if script_id is None or sound_dat_id is None or old_id is None or sound_folder is None:
        raise ValueError("input ROM is missing a required replacement resource")
    direct_index = old_id - sound_folder.firstID
    if not 0 <= direct_index < len(sound_folder.files):
        raise ValueError(f"{old_path} is not a direct file in the sound folder")

    rom.files[script_id] = bytearray(compiled_script.read_bytes())
    rom.files[sound_dat_id] = bytearray(sound_dat)
    rom.files[old_id] = bytearray(payload)
    sound_folder.files[direct_index] = new_filename
    output_rom.parent.mkdir(parents=True, exist_ok=True)
    output_rom.write_bytes(rom.save(updateDeviceCapacity=True))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--original-rom", type=Path, required=True)
    parser.add_argument("--final-rom", type=Path, required=True)
    parser.add_argument("--full-script", type=Path, required=True)
    parser.add_argument("--compiled-existing", type=Path)
    parser.add_argument("--compiled-small", type=Path)
    parser.add_argument("--compiled-prefix", type=Path)
    parser.add_argument("--original-sound-dat", type=Path, required=True)
    parser.add_argument("--template-se", type=Path, required=True)
    parser.add_argument("--voice-se", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--assemble", action="store_true")
    args = parser.parse_args()

    work = args.work_dir
    full_text = args.full_script.read_text(encoding="utf-8")
    if args.prepare:
        variants = {
            "existing": replace_first_pair(full_text, "V0000", "SE_A01B_WAKE"),
            "small": full_text,
            "prefix": replace_first_pair(full_text, "V0000", "SE_V0000"),
            "safe_id": full_text,
            "overwrite_stock": replace_first_pair(full_text, "V0000", "SE_A01B_WAKE"),
        }
        for name, text in variants.items():
            folder = work / "text" / name
            folder.mkdir(parents=True, exist_ok=True)
            (folder / "a01b.fsb.txt").write_text(text, encoding="utf-8", newline="\n")
        print(f"prepared={len(variants)}")

    if args.assemble:
        for path in (args.compiled_existing, args.compiled_small, args.compiled_prefix):
            if path is None or not path.is_file():
                raise FileNotFoundError(path)

        original_registry = args.original_sound_dat.read_bytes()
        v0000 = args.voice_se.read_bytes()
        info = inspect_se(v0000, "V0000", 0xC901)
        pcmd_start = info.pcmd_offset + CHUNK_HEADER_SIZE
        pcm = decode_nds_ima(v0000[pcmd_start : pcmd_start + info.pcmd_size])
        prefixed = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, info.bank_id, "SE_V0000"
        )
        prefixed_safe_id = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, 0xC80E, "SE_V0000"
        )
        safe_id = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, 0xC80E, "V0000"
        )
        overwrite_stock = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, 0x6604, "SE_A01B_WAKE"
        )
        stock_name_safe_id = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, 0xC80E, "SE_A01B_WAKE"
        )
        new_name_stock_id = build_se(
            args.template_se.read_bytes(), pcm, info.sample_rate, 0x6604, "SE_V0000"
        )

        # A stock SE call isolates registry/FAT changes from defects in the
        # generated voice container during runtime experiments.
        existing_rom = ndspy.rom.NintendoDSRom.fromFile(str(args.final_rom))
        script_id = existing_rom.filenames.idOf("scr/a01b.fsb")
        if script_id is None:
            raise ValueError("final ROM has no scr/a01b.fsb")
        existing_rom.files[script_id] = bytearray(args.compiled_existing.read_bytes())
        (work / "rom").mkdir(parents=True, exist_ok=True)
        (work / "rom" / "existing_control.nds").write_bytes(
            existing_rom.save(updateDeviceCapacity=True)
        )

        insert_one_file(
            args.original_rom,
            work / "rom" / "small_v0000.nds",
            args.compiled_small,
            append_entries(original_registry, "SE_CHUN", ["V0000"]),
            "v0000.se",
            v0000,
        )
        insert_one_file(
            args.original_rom,
            work / "rom" / "prefix_se_v0000.nds",
            args.compiled_prefix,
            append_entries(original_registry, "SE_CHUN", ["SE_V0000"]),
            "se_v0000.se",
            prefixed,
        )
        insert_one_file(
            args.original_rom,
            work / "rom" / "prefix_safe_id_c80e.nds",
            args.compiled_prefix,
            append_entries(original_registry, "SE_CHUN", ["SE_V0000"]),
            "se_v0000.se",
            prefixed_safe_id,
        )
        insert_one_file(
            args.original_rom,
            work / "rom" / "prefix_a01_safe_id_c80e.nds",
            args.compiled_prefix,
            append_entries(original_registry, "SE_A01 3等船室", ["SE_V0000"]),
            "se_v0000.se",
            prefixed_safe_id,
        )
        insert_one_file(
            args.original_rom,
            work / "rom" / "safe_id_c80e.nds",
            work / "compiled" / "safe_id" / "a01b.fsb",
            append_entries(original_registry, "SE_CHUN", ["V0000"]),
            "v0000.se",
            safe_id,
        )

        stock_rom = ndspy.rom.NintendoDSRom.fromFile(str(args.original_rom))
        stock_script_id = stock_rom.filenames.idOf("scr/a01b.fsb")
        stock_sound_id = stock_rom.filenames.idOf("sound/se_a01b_wake.se")
        if stock_script_id is None or stock_sound_id is None:
            raise ValueError("original ROM lacks the stock control resources")
        stock_rom.files[stock_script_id] = bytearray(
            (work / "compiled" / "overwrite_stock" / "a01b.fsb").read_bytes()
        )
        stock_rom.files[stock_sound_id] = bytearray(overwrite_stock)
        (work / "rom" / "overwrite_stock.nds").write_bytes(
            stock_rom.save(updateDeviceCapacity=True)
        )
        replace_one_file(
            args.original_rom,
            work / "rom" / "stock_name_safe_id_c80e.nds",
            work / "compiled" / "overwrite_stock" / "a01b.fsb",
            original_registry,
            "sound/se_a01b_wake.se",
            "se_a01b_wake.se",
            stock_name_safe_id,
        )
        replace_one_file(
            args.original_rom,
            work / "rom" / "new_name_stock_id_6604.nds",
            args.compiled_prefix,
            replace_registry_symbol(original_registry, "SE_A01B_WAKE", "SE_V0000"),
            "sound/se_a01b_wake.se",
            "se_v0000.se",
            new_name_stock_id,
        )
        print("assembled=9")


if __name__ == "__main__":
    sys.exit(main())
