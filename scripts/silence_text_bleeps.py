#!/usr/bin/env python3
"""Silence 999's ten text-bleep sequences in ``sound/se_sys.se``.

The system SE bank labels the character-writing sounds as
``SE_SYS_MESS_0`` through ``SE_SYS_MESS_9`` in its MCRL chunk.  Each label
identifies a sequence whose first track-volume event is ``E0 7F``.  This tool
changes only the volume operand to zero (``E0 00``), leaving UI sounds and all
sample/sequence data otherwise byte-identical.

Commands are intentionally strict:

* ``inspect`` accepts an original, silenced, or mixed file and reports every
  resolved label/sequence/track/volume offset.
* ``patch`` requires all ten volumes to be ``7F``, writes a new file, and
  proves that exactly ten bytes changed.
* ``verify`` requires all ten volumes to be ``00`` and reparses the complete
  routing chain rather than trusting fixed offsets.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Sequence


SIR0_HEADER_SIZE = 0x10
CHUNK_HEADER_SIZE = 0x10
MESS_LABELS = tuple(f"SE_SYS_MESS_{index}" for index in range(10))


class BleepPatchError(RuntimeError):
    """The SE bank is malformed or is not in the required patch state."""


@dataclass(frozen=True)
class BleepRoute:
    label: str
    sequence_id: int
    sequence_offset: int
    sequence_end: int
    track_offset: int
    track_payload_size: int
    volume_opcode_offset: int
    volume_value_offset: int
    volume: int


@dataclass(frozen=True)
class ParsedBank:
    file_size: int
    swdl_offset: int
    sedl_offset: int
    sequence_chunk_offset: int
    macro_chunk_offset: int
    sequence_count: int
    routes: tuple[BleepRoute, ...]

    @property
    def state(self) -> str:
        values = {route.volume for route in self.routes}
        if values == {0x7F}:
            return "audible"
        if values == {0x00}:
            return "silenced"
        return "mixed"


def u16(data: bytes | bytearray, offset: int) -> int:
    if offset < 0 or offset + 2 > len(data):
        raise BleepPatchError(f"uint16 out of range at 0x{offset:x}")
    return struct.unpack_from("<H", data, offset)[0]


def u32(data: bytes | bytearray, offset: int) -> int:
    if offset < 0 or offset + 4 > len(data):
        raise BleepPatchError(f"uint32 out of range at 0x{offset:x}")
    return struct.unpack_from("<I", data, offset)[0]


def sha256(data: bytes | bytearray) -> str:
    return hashlib.sha256(data).hexdigest()


def find_unique(data: bytes, needle: bytes, start: int, end: int, description: str) -> int:
    if not 0 <= start <= end <= len(data):
        raise BleepPatchError(f"invalid search range for {description}")
    first = data.find(needle, start, end)
    if first < 0:
        raise BleepPatchError(f"missing {description} between 0x{start:x} and 0x{end:x}")
    if data.find(needle, first + 1, end) >= 0:
        raise BleepPatchError(f"ambiguous {description} between 0x{start:x} and 0x{end:x}")
    return first


def chunk_end(data: bytes, offset: int, boundary: int, name: bytes) -> int:
    if data[offset : offset + 4] != name:
        raise BleepPatchError(f"expected {name!r} at 0x{offset:x}")
    end = offset + CHUNK_HEADER_SIZE + u32(data, offset + 0x0C)
    if end > boundary:
        raise BleepPatchError(
            f"{name.decode('ascii', 'replace').strip()} chunk at 0x{offset:x} "
            f"ends beyond 0x{boundary:x}"
        )
    return end


def decode_sir0_relocations(data: bytes, offset: int) -> tuple[int, ...]:
    if not 0 <= offset < len(data):
        raise BleepPatchError("SIR0 relocation pointer is out of range")
    locations: list[int] = []
    location = 0
    delta = 0
    cursor = offset
    while cursor < len(data):
        value = data[cursor]
        cursor += 1
        if value == 0:
            if delta:
                raise BleepPatchError("truncated SIR0 relocation varint")
            return tuple(locations)
        delta = (delta << 7) | (value & 0x7F)
        if not value & 0x80:
            location += delta
            if location >= len(data):
                raise BleepPatchError("SIR0 relocation field is out of range")
            locations.append(location)
            delta = 0
    raise BleepPatchError("unterminated SIR0 relocation table")


def parse_label_record(data: bytes, mcrl_payload: int, mcrl_end: int, label: str) -> int:
    encoded = label.encode("ascii")
    name_offset = find_unique(data, encoded, mcrl_payload, mcrl_end, f"MCRL label {label}")
    record_offset = name_offset - 4
    if record_offset < mcrl_payload or record_offset & 1:
        raise BleepPatchError(f"misaligned MCRL record for {label}")
    record_size = u16(data, record_offset + 2)
    expected_size = 4 + len(encoded) + 1
    expected_size += expected_size & 1
    if record_size != expected_size or record_offset + record_size > mcrl_end:
        raise BleepPatchError(
            f"invalid MCRL record size for {label}: {record_size}, expected {expected_size}"
        )
    if data[name_offset + len(encoded)] != 0:
        raise BleepPatchError(f"MCRL label {label} is not NUL-terminated")
    record_name = data[name_offset : record_offset + record_size].split(b"\0", 1)[0]
    if record_name != encoded:
        raise BleepPatchError(f"MCRL record for {label} contains trailing name bytes")
    return u16(data, record_offset)


def next_sequence_boundary(
    sequence_payload: int,
    sequence_chunk_end: int,
    pointers: Sequence[int],
    current: int,
) -> int:
    later = [pointer for pointer in pointers if pointer > current]
    relative_end = min(later) if later else sequence_chunk_end - sequence_payload
    absolute_end = sequence_payload + relative_end
    if absolute_end > sequence_chunk_end:
        raise BleepPatchError("sequence pointer ends beyond the SEQ chunk")
    return absolute_end


def parse_bank(data: bytes) -> ParsedBank:
    if len(data) < 0x100 or len(data) % 0x10 or data[:4] != b"SIR0":
        raise BleepPatchError("input is not an aligned SIR0 file")

    subheader = u32(data, 4)
    relocation_offset = u32(data, 8)
    if subheader + 8 > len(data):
        raise BleepPatchError("SIR0 subheader is out of range")
    relocations = decode_sir0_relocations(data, relocation_offset)
    expected_relocations = (4, 8, subheader, subheader + 4)
    if relocations != expected_relocations:
        raise BleepPatchError(
            f"unexpected SIR0 relocations {relocations!r}; expected {expected_relocations!r}"
        )

    swdl = u32(data, subheader)
    sedl = u32(data, subheader + 4)
    if data[swdl : swdl + 4] != b"swdl" or data[sedl : sedl + 4] != b"sedl":
        raise BleepPatchError("SIR0 subheader does not point to SWDL and SEDL")
    if not (SIR0_HEADER_SIZE <= swdl < sedl < subheader):
        raise BleepPatchError("SWDL/SEDL/subheader order is invalid")

    sedl_end = sedl + u32(data, sedl + 8)
    if sedl_end > subheader or sedl_end - CHUNK_HEADER_SIZE < sedl:
        raise BleepPatchError("SEDL declared size is invalid")
    if data[sedl_end - CHUNK_HEADER_SIZE : sedl_end - CHUNK_HEADER_SIZE + 4] != b"eod ":
        raise BleepPatchError("SEDL is not terminated by an EOD chunk")

    sequence_chunk = find_unique(data, b"seq ", sedl + 0x30, sedl_end, "SEQ chunk")
    macro_chunk = find_unique(data, b"mcrl", sequence_chunk + 4, sedl_end, "MCRL chunk")
    sequence_end = chunk_end(data, sequence_chunk, macro_chunk, b"seq ")
    macro_end = chunk_end(data, macro_chunk, sedl_end, b"mcrl")
    if sequence_end > macro_chunk:
        raise BleepPatchError("SEQ and MCRL chunks overlap")

    sequence_count = u16(data, sedl + 0x30)
    if sequence_count <= max(range(len(MESS_LABELS))):
        raise BleepPatchError(f"invalid SEDL sequence count: {sequence_count}")
    sequence_payload = sequence_chunk + CHUNK_HEADER_SIZE
    if sequence_payload + 2 * sequence_count > sequence_end:
        raise BleepPatchError("SEQ pointer table is truncated")
    pointers = tuple(u16(data, sequence_payload + 2 * index) for index in range(sequence_count))
    minimum_pointer = 2 * sequence_count
    for sequence_id, pointer in enumerate(pointers):
        if pointer and not minimum_pointer <= pointer < sequence_end - sequence_payload:
            raise BleepPatchError(
                f"sequence {sequence_id} pointer 0x{pointer:x} is outside the SEQ payload"
            )

    routes: list[BleepRoute] = []
    used_sequence_ids: set[int] = set()
    used_volume_offsets: set[int] = set()
    macro_payload = macro_chunk + CHUNK_HEADER_SIZE
    for label in MESS_LABELS:
        sequence_id = parse_label_record(data, macro_payload, macro_end, label)
        if sequence_id >= sequence_count:
            raise BleepPatchError(f"{label} maps to out-of-range sequence {sequence_id}")
        if sequence_id in used_sequence_ids:
            raise BleepPatchError(f"multiple MESS labels map to sequence {sequence_id}")
        used_sequence_ids.add(sequence_id)

        pointer = pointers[sequence_id]
        if pointer == 0:
            raise BleepPatchError(f"{label} maps to empty sequence {sequence_id}")
        sequence_offset = sequence_payload + pointer
        sequence_boundary = next_sequence_boundary(
            sequence_payload, sequence_end, pointers, pointer
        )
        if sequence_boundary <= sequence_offset:
            raise BleepPatchError(f"invalid bounds for {label} sequence {sequence_id}")

        track = find_unique(data, b"trk ", sequence_offset, sequence_boundary, f"{label} TRK")
        track_end = chunk_end(data, track, sequence_boundary, b"trk ")
        track_payload = track + CHUNK_HEADER_SIZE
        track_payload_size = track_end - track_payload
        payload = data[track_payload:track_end]
        first_volume_opcode = payload.find(b"\xE0")
        if first_volume_opcode < 0 or first_volume_opcode + 1 >= len(payload):
            raise BleepPatchError(f"{label} has no complete E0 track-volume event")
        volume_opcode_offset = track_payload + first_volume_opcode
        volume_value_offset = volume_opcode_offset + 1
        volume = data[volume_value_offset]
        if volume not in (0x00, 0x7F):
            raise BleepPatchError(
                f"{label} first E0 operand is 0x{volume:02x}, expected 00 or 7F"
            )
        if volume_value_offset in used_volume_offsets:
            raise BleepPatchError(f"duplicate volume operand at 0x{volume_value_offset:x}")
        used_volume_offsets.add(volume_value_offset)

        routes.append(
            BleepRoute(
                label=label,
                sequence_id=sequence_id,
                sequence_offset=sequence_offset,
                sequence_end=sequence_boundary,
                track_offset=track,
                track_payload_size=track_payload_size,
                volume_opcode_offset=volume_opcode_offset,
                volume_value_offset=volume_value_offset,
                volume=volume,
            )
        )

    if len(routes) != 10 or len(used_volume_offsets) != 10:
        raise BleepPatchError("did not resolve ten distinct MESS volume operands")
    return ParsedBank(
        file_size=len(data),
        swdl_offset=swdl,
        sedl_offset=sedl,
        sequence_chunk_offset=sequence_chunk,
        macro_chunk_offset=macro_chunk,
        sequence_count=sequence_count,
        routes=tuple(routes),
    )


def render_report(path: Path, data: bytes, parsed: ParsedBank) -> dict[str, object]:
    return {
        "file": str(path),
        "size": len(data),
        "sha256": sha256(data),
        "state": parsed.state,
        "swdl_offset": f"0x{parsed.swdl_offset:x}",
        "sedl_offset": f"0x{parsed.sedl_offset:x}",
        "sequence_chunk_offset": f"0x{parsed.sequence_chunk_offset:x}",
        "macro_chunk_offset": f"0x{parsed.macro_chunk_offset:x}",
        "sequence_count": parsed.sequence_count,
        "routes": [
            {
                **asdict(route),
                "sequence_offset": f"0x{route.sequence_offset:x}",
                "sequence_end": f"0x{route.sequence_end:x}",
                "track_offset": f"0x{route.track_offset:x}",
                "volume_opcode_offset": f"0x{route.volume_opcode_offset:x}",
                "volume_value_offset": f"0x{route.volume_value_offset:x}",
                "volume": f"0x{route.volume:02x}",
            }
            for route in parsed.routes
        ],
    }


def print_report(report: dict[str, object]) -> None:
    print(json.dumps(report, indent=2, ensure_ascii=False))


def inspect_command(arguments: argparse.Namespace) -> int:
    data = arguments.input.read_bytes()
    parsed = parse_bank(data)
    print_report(render_report(arguments.input, data, parsed))
    return 0


def patch_command(arguments: argparse.Namespace) -> int:
    source = arguments.input.read_bytes()
    parsed_source = parse_bank(source)
    if parsed_source.state != "audible":
        raise BleepPatchError(
            f"patch input state is {parsed_source.state}, expected audible (all E0 7F)"
        )
    if arguments.output.resolve() == arguments.input.resolve():
        raise BleepPatchError("refusing to overwrite the input file; choose a distinct output")

    patched = bytearray(source)
    for route in parsed_source.routes:
        if patched[route.volume_opcode_offset : route.volume_value_offset + 1] != b"\xE0\x7F":
            raise BleepPatchError(f"unexpected bytes at 0x{route.volume_opcode_offset:x}")
        patched[route.volume_value_offset] = 0x00
    output = bytes(patched)

    changed_offsets = [
        index for index, (before, after) in enumerate(zip(source, output, strict=True))
        if before != after
    ]
    expected_offsets = [route.volume_value_offset for route in parsed_source.routes]
    if changed_offsets != expected_offsets:
        raise BleepPatchError(
            f"changed offsets {changed_offsets!r} do not match expected {expected_offsets!r}"
        )
    parsed_output = parse_bank(output)
    if parsed_output.state != "silenced":
        raise BleepPatchError("patched output did not reparse as fully silenced")
    if len(output) != len(source):
        raise BleepPatchError("patch unexpectedly changed the file size")

    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_bytes(output)
    report = render_report(arguments.output, output, parsed_output)
    report.update(
        {
            "source_sha256": sha256(source),
            "changed_byte_count": len(changed_offsets),
            "changed_offsets": [f"0x{offset:x}" for offset in changed_offsets],
        }
    )
    print_report(report)
    return 0


def verify_command(arguments: argparse.Namespace) -> int:
    data = arguments.input.read_bytes()
    parsed = parse_bank(data)
    if parsed.state != "silenced":
        raise BleepPatchError(
            f"verification state is {parsed.state}, expected silenced (all E0 00)"
        )
    report = render_report(arguments.input, data, parsed)
    report["verification"] = "OK"
    print_report(report)
    return 0


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    inspect_parser = subparsers.add_parser("inspect", help="inspect and report MESS routes")
    inspect_parser.add_argument("input", type=Path)
    inspect_parser.set_defaults(handler=inspect_command)

    patch_parser = subparsers.add_parser("patch", help="write a silenced se_sys.se copy")
    patch_parser.add_argument("input", type=Path)
    patch_parser.add_argument("output", type=Path)
    patch_parser.set_defaults(handler=patch_command)

    verify_parser = subparsers.add_parser("verify", help="strictly verify a silenced copy")
    verify_parser.add_argument("input", type=Path)
    verify_parser.set_defaults(handler=verify_command)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = make_parser()
    arguments = parser.parse_args(argv)
    try:
        return int(arguments.handler(arguments))
    except (BleepPatchError, OSError) as error:
        parser.error(str(error))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
