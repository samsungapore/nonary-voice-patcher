#!/usr/bin/env python3
"""Inspect and extend Zero Escape 999's ``etc/sound.dat`` registry.

``sound.dat`` is a SIR0 container. Its sub-header is a null-terminated array
of category pairs::

    uint32 category_name_pointer
    uint32 entry_pointer_table

Each entry table is itself a null-terminated array of pointers to Shift-JIS
symbol strings. SIR0's relocation list records every field containing a
pointer. The game resolves sound symbols independently of NitroFS file order.

For Japanese voice effects, append symbols to the existing ``SE_CHUN``
overflow category (the default) and add matching files named
``sound/<symbol.lower()>.se`` to the ROM. Do not include the leading ``:``
used by script calls in registry symbols.

Examples::

    python3 scripts/sound_registry.py inspect work/etc/sound.dat --entries
    python3 scripts/sound_registry.py patch \
        work/etc/sound.dat work/etc/sound.patched.dat \
        --names-file work/manifests/voice_symbols.txt
    python3 scripts/sound_registry.py verify work/etc/sound.patched.dat
"""

from __future__ import annotations

import argparse
import re
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence


SIR0_MAGIC = b"SIR0"
SIR0_HEADER_SIZE = 0x10
DEFAULT_SE_CATEGORY = "SE_CHUN"
SYMBOL_RE = re.compile(r"^[A-Z0-9_]+$")


@dataclass(frozen=True)
class Category:
    """One category pair and its resolved symbol strings."""

    pair_offset: int
    name: str
    table_offset: int
    entries: tuple[str, ...]


class RegistryError(ValueError):
    """Raised when a sound registry is malformed or cannot be patched safely."""


def read_u32(data: bytes | bytearray, offset: int) -> int:
    """Read a little-endian unsigned 32-bit integer with bounds checking."""

    if offset < 0 or offset + 4 > len(data):
        raise RegistryError(f"u32 offset 0x{offset:x} is outside the file")
    return struct.unpack_from("<I", data, offset)[0]


def write_u32(data: bytearray, offset: int, value: int) -> None:
    """Write a little-endian unsigned 32-bit integer with bounds checking."""

    if offset < 0 or offset + 4 > len(data):
        raise RegistryError(f"u32 offset 0x{offset:x} is outside the file")
    if not 0 <= value <= 0xFFFFFFFF:
        raise RegistryError(f"u32 value 0x{value:x} is out of range")
    struct.pack_into("<I", data, offset, value)


def read_c_string(data: bytes | bytearray, offset: int) -> str:
    """Resolve one null-terminated Shift-JIS string."""

    if not 0 <= offset < len(data):
        raise RegistryError(f"string offset 0x{offset:x} is outside the file")
    try:
        end = data.index(0, offset)
    except ValueError as exc:
        raise RegistryError(f"unterminated string at 0x{offset:x}") from exc
    try:
        return bytes(data[offset:end]).decode("shift_jis")
    except UnicodeDecodeError as exc:
        raise RegistryError(f"invalid Shift-JIS string at 0x{offset:x}") from exc


def decode_relocations(data: bytes | bytearray, offset: int) -> list[int]:
    """Decode a SIR0 delta/variable-length pointer-position list."""

    if not 0 <= offset < len(data):
        raise RegistryError(f"relocation list offset 0x{offset:x} is outside the file")
    positions: list[int] = []
    absolute = 0
    while True:
        delta = 0
        groups = 0
        while True:
            if offset >= len(data):
                raise RegistryError("unterminated SIR0 relocation list")
            byte = data[offset]
            offset += 1
            if byte == 0 and groups == 0:
                return positions
            delta = (delta << 7) | (byte & 0x7F)
            groups += 1
            if groups > 4:
                raise RegistryError("SIR0 relocation delta uses more than four groups")
            if not byte & 0x80:
                break
        if delta == 0:
            raise RegistryError("SIR0 relocation positions are not strictly increasing")
        absolute += delta
        if absolute + 4 > len(data):
            raise RegistryError(f"relocation position 0x{absolute:x} is outside the file")
        positions.append(absolute)


def encode_relocations(positions: Iterable[int]) -> bytes:
    """Encode sorted absolute pointer positions in SIR0's delta format."""

    output = bytearray()
    previous = 0
    for absolute in sorted(set(positions)):
        if absolute <= previous:
            raise RegistryError("SIR0 relocation positions must be positive and increasing")
        delta = absolute - previous
        if delta > 0x0FFFFFFF:
            raise RegistryError(f"SIR0 relocation delta 0x{delta:x} exceeds 28 bits")
        previous = absolute

        groups = [delta & 0x7F]
        delta >>= 7
        while delta:
            groups.append(delta & 0x7F)
            delta >>= 7
        groups.reverse()
        for index, group in enumerate(groups):
            output.append(group | (0x80 if index + 1 < len(groups) else 0))
    output.append(0)
    return bytes(output)


def parse_categories(data: bytes | bytearray) -> list[Category]:
    """Parse the SIR0 sound-registry sub-header and all entry tables."""

    if data[:4] != SIR0_MAGIC or len(data) < SIR0_HEADER_SIZE:
        raise RegistryError("not a SIR0 file")

    root = read_u32(data, 4)
    relocation_offset = read_u32(data, 8)
    if root < SIR0_HEADER_SIZE or root >= relocation_offset:
        raise RegistryError(f"invalid registry root 0x{root:x}")

    categories: list[Category] = []
    pair_offset = root
    while True:
        name_pointer = read_u32(data, pair_offset)
        if name_pointer == 0:
            break
        table_offset = read_u32(data, pair_offset + 4)
        if not 0 < table_offset < relocation_offset:
            raise RegistryError(
                f"invalid table pointer 0x{table_offset:x} at 0x{pair_offset + 4:x}"
            )

        entries: list[str] = []
        entry_offset = table_offset
        while True:
            string_pointer = read_u32(data, entry_offset)
            if string_pointer == 0:
                break
            entries.append(read_c_string(data, string_pointer))
            entry_offset += 4
            if entry_offset >= relocation_offset:
                raise RegistryError(f"unterminated entry table at 0x{table_offset:x}")

        categories.append(
            Category(
                pair_offset=pair_offset,
                name=read_c_string(data, name_pointer),
                table_offset=table_offset,
                entries=tuple(entries),
            )
        )
        pair_offset += 8
        if pair_offset >= relocation_offset:
            raise RegistryError("unterminated category array")
    return categories


def validate_registry(data: bytes | bytearray) -> tuple[list[Category], list[int]]:
    """Fully parse a registry and check all SIR0 relocation fields."""

    categories = parse_categories(data)
    relocation_offset = read_u32(data, 8)
    relocations = decode_relocations(data, relocation_offset)
    required = {4, 8}
    root = read_u32(data, 4)

    for category in categories:
        required.update((category.pair_offset, category.pair_offset + 4))
        entry_offset = category.table_offset
        while read_u32(data, entry_offset):
            required.add(entry_offset)
            entry_offset += 4

    category_terminator = root + len(categories) * 8
    self_pointer_offset = category_terminator + 4
    if read_u32(data, self_pointer_offset) == root:
        required.add(self_pointer_offset)

    missing = sorted(required - set(relocations))
    if missing:
        rendered = ", ".join(f"0x{position:x}" for position in missing[:10])
        raise RegistryError(f"relocation list is missing required pointer fields: {rendered}")

    for position in relocations:
        pointer = read_u32(data, position)
        if position == 8:
            if pointer != relocation_offset:
                raise RegistryError(
                    f"header relocation pointer 0x{pointer:x} does not match 0x{relocation_offset:x}"
                )
            continue
        if pointer >= relocation_offset:
            raise RegistryError(
                f"pointer 0x{pointer:x} at relocation 0x{position:x} reaches relocation data"
            )
    return categories, relocations


def validate_new_symbols(categories: Sequence[Category], symbols: Sequence[str]) -> None:
    """Reject names that could collide or produce invalid NitroFS filenames."""

    existing = {entry.casefold() for category in categories for entry in category.entries}
    seen: set[str] = set()
    for symbol in symbols:
        try:
            encoded = symbol.encode("ascii")
        except UnicodeEncodeError as exc:
            raise RegistryError(f"symbol is not ASCII: {symbol!r}") from exc
        if not SYMBOL_RE.fullmatch(symbol):
            raise RegistryError(
                f"invalid symbol {symbol!r}; expected uppercase ASCII letters, digits, or underscores"
            )
        folded = symbol.casefold()
        if folded in existing or folded in seen:
            raise RegistryError(f"duplicate registry symbol: {symbol}")
        if len(encoded) + len(".se") > 127:
            raise RegistryError(f"NitroFS filename would exceed 127 bytes: {symbol}")
        seen.add(folded)


def rebuild_registry(
    categories: Sequence[tuple[str, Sequence[str]]],
) -> bytes:
    """Serialize a registry in the canonical layout used by the retail ROM.

    The game file places every category/entry string first, every pointer
    table second, and the category root after all of those tables.  Keeping
    newly added data before the SIR0 root matters for the retail loader; a
    structurally valid pointer to data appended after the root is not a safe
    representation for this format.
    """

    if not categories:
        raise RegistryError("cannot serialize an empty sound registry")

    output = bytearray(SIR0_HEADER_SIZE)
    string_pointers: list[tuple[int, list[int]]] = []
    for category_name, entries in categories:
        try:
            name_pointer = len(output)
            output.extend(category_name.encode("shift_jis") + b"\0")
        except UnicodeEncodeError as exc:
            raise RegistryError(
                f"category name is not Shift-JIS encodable: {category_name!r}"
            ) from exc

        entry_pointers: list[int] = []
        for entry in entries:
            try:
                encoded = entry.encode("shift_jis")
            except UnicodeEncodeError as exc:
                raise RegistryError(
                    f"registry symbol is not Shift-JIS encodable: {entry!r}"
                ) from exc
            entry_pointers.append(len(output))
            output.extend(encoded + b"\0")
        string_pointers.append((name_pointer, entry_pointers))

    while len(output) % 4:
        output.append(0xAA)

    table_pointers: list[int] = []
    relocation_fields: list[int] = [4, 8]
    for _name_pointer, entry_pointers in string_pointers:
        table_pointers.append(len(output))
        for pointer in entry_pointers:
            relocation_fields.append(len(output))
            output.extend(struct.pack("<I", pointer))
        output.extend(bytes(4))

    root = len(output)
    for (name_pointer, _entry_pointers), table_pointer in zip(
        string_pointers, table_pointers, strict=True
    ):
        relocation_fields.extend((len(output), len(output) + 4))
        output.extend(struct.pack("<II", name_pointer, table_pointer))
    output.extend(bytes(4))
    relocation_fields.append(len(output))
    output.extend(struct.pack("<I", root))
    while len(output) % 16:
        output.append(0xAA)

    relocation_offset = len(output)
    output.extend(encode_relocations(relocation_fields))
    while len(output) % 16:
        output.append(0xAA)

    output[:4] = SIR0_MAGIC
    write_u32(output, 4, root)
    write_u32(output, 8, relocation_offset)
    serialized = bytes(output)
    parsed, _relocations = validate_registry(serialized)
    expected = [(name, tuple(entries)) for name, entries in categories]
    actual = [(category.name, category.entries) for category in parsed]
    if actual != expected:
        raise RegistryError("canonical registry did not round-trip")
    return serialized


def append_entries(data: bytes, category_name: str, symbols: Sequence[str]) -> bytes:
    """Return a registry with ``symbols`` appended to one existing category.

    The result is rebuilt in the retail file's canonical order (all strings
    and entry tables before the category root), then reparsed in full.
    """

    categories, _relocations = validate_registry(data)
    matches = [category for category in categories if category.name == category_name]
    if len(matches) != 1:
        raise RegistryError(
            f"expected one category named {category_name!r}, found {len(matches)}"
        )
    if not symbols:
        raise RegistryError("no symbols were supplied")
    validate_new_symbols(categories, symbols)
    category = matches[0]
    rebuilt_categories = [
        (
            item.name,
            (*item.entries, *symbols) if item is category else item.entries,
        )
        for item in categories
    ]
    output = rebuild_registry(rebuilt_categories)

    patched_categories, _ = validate_registry(output)
    patched = next(item for item in patched_categories if item.name == category_name)
    expected = (*category.entries, *symbols)
    if patched.entries != expected:
        raise RegistryError("post-write verification found an unexpected category table")
    return output


def load_symbols(arguments: argparse.Namespace) -> list[str]:
    """Collect positional and one-per-line symbols without silently sorting them."""

    symbols = list(arguments.symbols)
    if arguments.names_file:
        for line in arguments.names_file.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if stripped and not stripped.startswith("#"):
                symbols.append(stripped)
    return symbols


def command_inspect(arguments: argparse.Namespace) -> int:
    data = arguments.registry.read_bytes()
    categories, relocations = validate_registry(data)
    print(
        f"file={arguments.registry} size={len(data)} "
        f"root=0x{read_u32(data, 4):x} reloc=0x{read_u32(data, 8):x} "
        f"categories={len(categories)} entries={sum(len(item.entries) for item in categories)} "
        f"relocations={len(relocations)}"
    )
    for category in categories:
        print(
            f"0x{category.pair_offset:04x} {category.name:<20} "
            f"table=0x{category.table_offset:04x} count={len(category.entries)}"
        )
        if arguments.entries:
            for index, entry in enumerate(category.entries):
                print(f"  {index:5d} {entry}")
    return 0


def command_patch(arguments: argparse.Namespace) -> int:
    if arguments.input.resolve() == arguments.output.resolve():
        raise RegistryError("input and output paths must differ")
    symbols = load_symbols(arguments)
    original = arguments.input.read_bytes()
    patched = append_entries(original, arguments.category, symbols)
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_bytes(patched)

    categories, relocations = validate_registry(patched)
    category = next(item for item in categories if item.name == arguments.category)
    print(
        f"wrote={arguments.output} size={len(patched)} category={arguments.category!r} "
        f"added={len(symbols)} new_count={len(category.entries)} "
        f"relocations={len(relocations)}"
    )
    return 0


def command_verify(arguments: argparse.Namespace) -> int:
    data = arguments.registry.read_bytes()
    categories, relocations = validate_registry(data)
    print(
        f"OK file={arguments.registry} size={len(data)} "
        f"categories={len(categories)} entries={sum(len(item.entries) for item in categories)} "
        f"relocations={len(relocations)}"
    )
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    inspect_parser = subparsers.add_parser("inspect", help="show registry structure")
    inspect_parser.add_argument("registry", type=Path)
    inspect_parser.add_argument("--entries", action="store_true", help="also list symbols")
    inspect_parser.set_defaults(handler=command_inspect)

    patch_parser = subparsers.add_parser("patch", help="append symbols to a category")
    patch_parser.add_argument("input", type=Path)
    patch_parser.add_argument("output", type=Path)
    patch_parser.add_argument("symbols", nargs="*", help="symbols without the script ':' prefix")
    patch_parser.add_argument(
        "--category",
        default=DEFAULT_SE_CATEGORY,
        help=f"exact existing category label (default: {DEFAULT_SE_CATEGORY})",
    )
    patch_parser.add_argument(
        "--names-file",
        type=Path,
        help="UTF-8 text file with one additional symbol per line; # comments are ignored",
    )
    patch_parser.set_defaults(handler=command_patch)

    verify_parser = subparsers.add_parser("verify", help="validate registry pointers and tables")
    verify_parser.add_argument("registry", type=Path)
    verify_parser.set_defaults(handler=command_verify)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    arguments = parser.parse_args(argv)
    try:
        return arguments.handler(arguments)
    except (OSError, RegistryError) as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    sys.exit(main())
