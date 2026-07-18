#!/usr/bin/env python3
"""Build Nintendo DS 999 ``.se`` voice files from ordinary audio.

The game stores sound effects as a SIR0 wrapper containing one SWDL sample
bank and one SEDL sequence.  This tool clones the known-good
``sound/se_a01b_wake.se`` resource, replaces its PCMD sample with mono
Nintendo DS IMA ADPCM, and fixes every offset and size affected by the new
sample length.

Only the Python standard library and ``ffmpeg`` are needed when an extracted
template is supplied.  Passing ``--rom`` instead requires ``ndspy`` so the
template can be read directly from the original ROM.
"""

from __future__ import annotations

import argparse
import array
import json
import math
import re
import shutil
import struct
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable, Sequence


SAMPLE_RATE = 16_384
TEMPLATE_ROM_PATH = "sound/se_a01b_wake.se"
SIR0_HEADER_SIZE = 0x10
SWDL_HEADER_SIZE = 0x50
CHUNK_HEADER_SIZE = 0x10
WAVI_ENTRY_SIZE = 0x40
ADPCM_PREAMBLE_SIZE = 4

# Nintendo DS IMA ADPCM tables.  The DS differs from the usual IMA decoder
# only in clamping its negative predictor to -32767 instead of -32768.
IMA_INDEX_TABLE = (
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8,
)
IMA_STEP_TABLE = (
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17,
    19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358,
    5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635,
    13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086,
    29794, 32767,
)


class VoiceAudioError(RuntimeError):
    """A malformed template, audio conversion error, or invalid output."""


@dataclass(frozen=True)
class SeInfo:
    file_size: int
    swdl_offset: int
    swdl_size: int
    sedl_offset: int
    sedl_size: int
    subheader_offset: int
    relocation_offset: int
    pcmd_offset: int
    pcmd_size: int
    sample_rate: int
    sample_count: int
    duration_seconds: float
    bank_id: int
    name: str


def _u16(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def _put_u16(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", data, offset, value)


def _put_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def _align(value: int, alignment: int) -> int:
    return (value + alignment - 1) & -alignment


def _find_once(data: bytes | bytearray, needle: bytes, start: int, end: int) -> int:
    first = data.find(needle, start, end)
    if first < 0:
        raise VoiceAudioError(f"missing {needle!r} between 0x{start:x} and 0x{end:x}")
    if data.find(needle, first + 1, end) >= 0:
        raise VoiceAudioError(f"ambiguous {needle!r} between 0x{start:x} and 0x{end:x}")
    return first


def _read_fixed_ascii(data: bytes | bytearray, offset: int, size: int) -> str:
    field = bytes(data[offset : offset + size]).split(b"\0", 1)[0]
    try:
        return field.decode("ascii")
    except UnicodeDecodeError as exc:
        raise VoiceAudioError(f"non-ASCII internal name at 0x{offset:x}") from exc


def _write_fixed_ascii(data: bytearray, offset: int, size: int, text: str) -> None:
    encoded = text.encode("ascii")
    if len(encoded) >= size:
        raise VoiceAudioError(f"{text!r} does not fit in a {size}-byte NUL-terminated field")
    data[offset : offset + size] = encoded + bytes(size - len(encoded))


def _mcrl_hash(name: str) -> int:
    """Return the retail SEDL macro-label hash for one ASCII symbol.

    The sequence engine sums signed bytes modulo 256.  Generated symbols are
    restricted to 7-bit ASCII, for which that is simply the byte sum.
    """

    encoded = name.encode("ascii")
    if any(byte >= 0x80 for byte in encoded):
        raise VoiceAudioError("MCRL symbols must use 7-bit ASCII")
    return sum(encoded) & 0xFF


def validate_name(name: str) -> str:
    """Validate the compact internal name used by SWDL, SEDL, and MCRL."""
    name = name.upper()
    if not re.fullmatch(r"[A-Z0-9_]{1,12}", name):
        raise VoiceAudioError(
            "internal name must contain 1-12 ASCII letters, digits, or underscores"
        )
    return name


def read_template(template: Path | None, rom: Path | None) -> bytes:
    if (template is None) == (rom is None):
        raise VoiceAudioError("pass exactly one of --template or --rom")
    if template is not None:
        return template.read_bytes()

    try:
        from ndspy.rom import NintendoDSRom  # type: ignore[import-not-found]
    except ImportError as exc:
        raise VoiceAudioError("--rom requires ndspy (pip install ndspy)") from exc

    assert rom is not None
    nds = NintendoDSRom.fromFile(str(rom))
    file_id = nds.filenames.idOf(TEMPLATE_ROM_PATH)
    if file_id is None:
        raise VoiceAudioError(f"ROM does not contain {TEMPLATE_ROM_PATH}")
    return bytes(nds.files[file_id])


def decode_audio(audio_path: Path, ffmpeg: str, gain_db: float = 0.0) -> list[int]:
    """Decode/resample an audio file to mono signed PCM16 at 16384 Hz."""
    executable = shutil.which(ffmpeg) if not Path(ffmpeg).is_file() else ffmpeg
    if executable is None:
        raise VoiceAudioError(f"ffmpeg executable not found: {ffmpeg}")

    command = [
        str(executable),
        "-v", "error",
        "-nostdin",
        "-i", str(audio_path),
        "-map", "0:a:0",
        "-ac", "1",
        "-ar", str(SAMPLE_RATE),
    ]
    if gain_db:
        command += ["-af", f"volume={gain_db:.8g}dB"]
    command += ["-f", "s16le", "-acodec", "pcm_s16le", "-"]

    process = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if process.returncode:
        message = process.stderr.decode("utf-8", "replace").strip()
        raise VoiceAudioError(f"ffmpeg failed for {audio_path}: {message}")
    if not process.stdout or len(process.stdout) % 2:
        raise VoiceAudioError(f"ffmpeg produced invalid/empty PCM for {audio_path}")

    samples = array.array("h")
    samples.frombytes(process.stdout)
    if sys.byteorder != "little":
        samples.byteswap()
    return list(samples)


def _clamp_predictor(value: int) -> int:
    return max(-32767, min(32767, value))


def _encode_nibble(target: int, predictor: int, index: int) -> tuple[int, int, int]:
    step = IMA_STEP_TABLE[index]
    difference = target - predictor
    code = 8 if difference < 0 else 0
    magnitude = abs(difference)

    if magnitude >= step:
        code |= 4
        magnitude -= step
    if magnitude >= step >> 1:
        code |= 2
        magnitude -= step >> 1
    if magnitude >= step >> 2:
        code |= 1

    delta = step >> 3
    if code & 1:
        delta += step >> 2
    if code & 2:
        delta += step >> 1
    if code & 4:
        delta += step
    predictor = predictor - delta if code & 8 else predictor + delta
    predictor = _clamp_predictor(predictor)
    index = max(0, min(88, index + IMA_INDEX_TABLE[code]))
    return code, predictor, index


def encode_nds_ima(samples: Sequence[int]) -> tuple[bytes, list[int]]:
    """Encode mono PCM16 to DS IMA ADPCM, padding to complete 32-bit blocks.

    The returned PCM list includes at most seven repeated tail samples added so
    the PCMD payload consists of whole 4-byte/8-sample blocks.
    """
    if len(samples) == 0:
        raise VoiceAudioError("cannot encode empty PCM")
    padded = [max(-32767, min(32767, int(sample))) for sample in samples]
    padded.extend([padded[-1]] * ((-len(padded)) % 8))

    predictor = padded[0]
    index = 0
    nibbles: list[int] = []
    for target in padded:
        code, predictor, index = _encode_nibble(target, predictor, index)
        nibbles.append(code)

    payload = bytearray(struct.pack("<hH", padded[0], 0))
    payload.extend(
        nibbles[i] | (nibbles[i + 1] << 4)
        for i in range(0, len(nibbles), 2)
    )
    if len(payload) % 4:
        raise AssertionError("ADPCM encoder did not produce complete 32-bit blocks")
    return bytes(payload), padded


def encode_ima_adpcm(pcm_int16: Sequence[int]) -> bytes:
    """Public convenience API: encode PCM16 and return a complete PCMD blob."""
    encoded, _padded = encode_nds_ima(pcm_int16)
    return encoded


def decode_nds_ima(adpcm: bytes) -> list[int]:
    """Reference decoder used for round-trip verification."""
    if len(adpcm) < ADPCM_PREAMBLE_SIZE or len(adpcm) % 4:
        raise VoiceAudioError("invalid DS IMA ADPCM length")
    predictor, index = struct.unpack_from("<hH", adpcm)
    index = max(0, min(88, index))
    decoded: list[int] = []
    for packed in adpcm[ADPCM_PREAMBLE_SIZE:]:
        for code in (packed & 0x0F, packed >> 4):
            step = IMA_STEP_TABLE[index]
            delta = step >> 3
            if code & 1:
                delta += step >> 2
            if code & 2:
                delta += step >> 1
            if code & 4:
                delta += step
            predictor = predictor - delta if code & 8 else predictor + delta
            predictor = _clamp_predictor(predictor)
            index = max(0, min(88, index + IMA_INDEX_TABLE[code]))
            decoded.append(predictor)
    return decoded


def encode_sir0_relocations(pointer_fields: Iterable[int]) -> bytes:
    """Encode sorted pointer-field locations as SIR0 delta varints."""
    result = bytearray()
    previous = 0
    for pointer_field in sorted(pointer_fields):
        delta = pointer_field - previous
        if delta <= 0:
            raise VoiceAudioError("SIR0 pointer fields must be unique and increasing")
        groups = [delta & 0x7F]
        delta >>= 7
        while delta:
            groups.append(delta & 0x7F)
            delta >>= 7
        groups.reverse()
        for index, group in enumerate(groups):
            result.append(group | (0x80 if index + 1 < len(groups) else 0))
        previous = pointer_field
    result.append(0)
    return bytes(result)


def decode_sir0_relocations(data: bytes | bytearray, offset: int) -> list[int]:
    result: list[int] = []
    location = 0
    delta = 0
    while offset < len(data):
        value = data[offset]
        offset += 1
        if value == 0:
            if delta:
                raise VoiceAudioError("truncated SIR0 relocation varint")
            return result
        delta = (delta << 7) | (value & 0x7F)
        if not value & 0x80:
            location += delta
            result.append(location)
            delta = 0
    raise VoiceAudioError("unterminated SIR0 relocation table")


def _template_layout(template: bytes) -> tuple[int, int, int, int, int]:
    if len(template) < 0x100 or template[:4] != b"SIR0":
        raise VoiceAudioError("template is not a SIR0 file")
    subheader = _u32(template, 4)
    if subheader + 8 > len(template):
        raise VoiceAudioError("template SIR0 subheader is out of range")
    swdl = _u32(template, subheader)
    sedl = _u32(template, subheader + 4)
    if template[swdl : swdl + 4] != b"swdl" or template[sedl : sedl + 4] != b"sedl":
        raise VoiceAudioError("template subheader does not point to SWDL/SEDL")
    pcmd = _find_once(template, b"pcmd", swdl + SWDL_HEADER_SIZE, sedl)
    eod = _align(pcmd + CHUNK_HEADER_SIZE + _u32(template, pcmd + 12), 16)
    if template[eod : eod + 4] != b"eod ":
        raise VoiceAudioError("template SWDL has no EOD after PCMD")
    sedl_size = _u32(template, sedl + 8)
    if sedl + sedl_size != subheader:
        raise VoiceAudioError("template SEDL length does not end at the SIR0 subheader")
    return swdl, pcmd, eod, sedl, subheader


def _patch_names(data: bytearray, swdl: int, sedl: int, subheader: int, name: str) -> None:
    old_swdl_name = _read_fixed_ascii(data, swdl + 0x20, 16)
    if not old_swdl_name.endswith((".SW", ".SWD")):
        raise VoiceAudioError(f"unexpected template SWDL name: {old_swdl_name!r}")
    old_base = old_swdl_name.rsplit(".", 1)[0]
    if len(old_base) > 12:
        raise VoiceAudioError("template MCRL name slot is too small")

    _write_fixed_ascii(data, swdl + 0x20, 16, name + ".SW")
    _write_fixed_ascii(data, sedl + 0x20, 16, name + ".SE")

    mcrl = _find_once(data, b"mcrl", sedl, subheader)
    macro_name = _find_once(data, old_base.encode("ascii"), mcrl, subheader)
    if macro_name < 4 or _u16(data, macro_name - 4) != 1:
        raise VoiceAudioError("template MCRL name record is malformed")
    # The preceding uint16 is the complete, even-aligned record size (four
    # header bytes plus the NUL-terminated symbol), not the string length.
    record_size = _u16(data, macro_name - 2)
    slot_size = record_size - 4
    if record_size < 6 or macro_name + slot_size > subheader:
        raise VoiceAudioError("template MCRL name record is out of range")
    if len(name) + 1 > slot_size:
        raise VoiceAudioError("new MCRL name does not fit the template slot")

    # MCRL does not linearly scan its label records.  It first indexes a
    # 256-entry uint16 bucket table by sum(symbol bytes) modulo 256.  Renaming
    # only the visible string therefore leaves the record unreachable and the
    # retail engine enters its fatal error loop when PlaySE resolves it.
    bucket_base = mcrl + CHUNK_HEADER_SIZE
    if bucket_base + 0x200 > subheader:
        raise VoiceAudioError("template MCRL bucket table is truncated")
    record = macro_name - 4
    old_hash = _mcrl_hash(old_base)
    new_hash = _mcrl_hash(name)
    old_bucket = bucket_base + old_hash * 2
    old_value = _u16(data, old_bucket)
    if bucket_base + old_value != record:
        raise VoiceAudioError("template MCRL old-name bucket does not point to its record")
    if _u16(data, record) != 1 or _u16(data, record + 2) != record_size:
        raise VoiceAudioError("template MCRL name record header is malformed")
    if record + record_size + 2 > subheader or _u16(data, record + record_size) != 0xFFFF:
        raise VoiceAudioError("template MCRL old-name bucket is not a single-record chain")
    if new_hash != old_hash:
        new_bucket = bucket_base + new_hash * 2
        new_value = _u16(data, new_bucket)
        new_chain = bucket_base + new_value
        if new_chain + 2 > subheader or _u16(data, new_chain) != 0xFFFF:
            raise VoiceAudioError("new MCRL hash bucket is not empty")
        _put_u16(data, old_bucket, new_value)
        _put_u16(data, new_bucket, old_value)

    data[macro_name : macro_name + slot_size] = name.encode("ascii") + bytes(
        slot_size - len(name)
    )


def _patch_bank_id(data: bytearray, swdl: int, sedl: int, subheader: int, bank_id: int) -> None:
    if not 0 <= bank_id <= 0xFFFF:
        raise VoiceAudioError("bank ID must fit in 16 bits")

    # DSE headers and BNKL store the logical ID little-endian.  The A8 track
    # opcode stores the same two bytes in the opposite (big-endian) order.
    struct.pack_into("<H", data, swdl + 0x0E, bank_id)
    struct.pack_into("<H", data, sedl + 0x0E, bank_id)

    track = _find_once(data, b"trk ", sedl, subheader)
    track_end = track + CHUNK_HEADER_SIZE + _u32(data, track + 12)
    opcode = _find_once(data, b"\xA8", track + CHUNK_HEADER_SIZE, track_end)
    if opcode + 3 > track_end:
        raise VoiceAudioError("truncated A8 bank-select opcode")
    struct.pack_into(">H", data, opcode + 1, bank_id)

    bnkl = _find_once(data, b"bnkl", sedl, subheader)
    if bnkl + 0x2A > subheader:
        raise VoiceAudioError("truncated BNKL chunk")
    struct.pack_into("<H", data, bnkl + 0x28, bank_id)


def _build_se_from_adpcm(template: bytes, adpcm: bytes, name: str, bank_id: int) -> bytes:
    """Build one voice SE from template bytes and an encoded PCMD sample."""
    name = validate_name(name)
    if len(adpcm) < 8 or len(adpcm) % 4:
        raise VoiceAudioError("ADPCM must contain a preamble and complete 32-bit blocks")

    old_swdl, old_pcmd, old_eod, old_sedl, old_subheader = _template_layout(template)
    pcmd_data = old_pcmd + CHUNK_HEADER_SIZE
    old_pcmd_size = _u32(template, old_pcmd + 12)
    if pcmd_data + old_pcmd_size > old_eod:
        raise VoiceAudioError("template PCMD extends beyond its EOD")

    # Keep all known-good SWDL metadata/preset data before PCMD, then lay out
    # the variable sample, SWDL EOD, and fixed SEDL at their required
    # alignments (chunks: 16 bytes; SEDL: 64 bytes).
    output = bytearray(template[:pcmd_data])
    output.extend(adpcm)
    new_eod = _align(len(output), 16)
    output.extend(bytes([0xAA]) * (new_eod - len(output)))
    output.extend(template[old_eod : old_eod + CHUNK_HEADER_SIZE])
    new_sedl = _align(len(output), 64)
    output.extend(bytes([0xAA]) * (new_sedl - len(output)))
    output.extend(template[old_sedl:old_subheader])

    sedl_size = _u32(output, new_sedl + 8)
    new_subheader = new_sedl + sedl_size
    if len(output) != new_subheader:
        raise VoiceAudioError("copied SEDL does not end at its declared length")
    output.extend(struct.pack("<II", old_swdl, new_sedl))
    new_relocations = _align(len(output), 16)
    output.extend(bytes([0xAA]) * (new_relocations - len(output)))
    output.extend(
        encode_sir0_relocations((4, 8, new_subheader, new_subheader + 4))
    )
    output.extend(bytes([0xAA]) * (_align(len(output), 16) - len(output)))

    _put_u32(output, 4, new_subheader)
    _put_u32(output, 8, new_relocations)

    _put_u32(output, old_swdl + 8, new_eod + CHUNK_HEADER_SIZE - old_swdl)
    _put_u32(output, old_swdl + 0x40, len(adpcm))
    _put_u32(output, old_pcmd + 12, len(adpcm))

    # DSE counts ADPCM length in 32-bit words after its predictor preamble, so
    # byte-based lengths would make the engine read past the sample.
    wavi = _find_once(output, b"wavi", old_swdl + SWDL_HEADER_SIZE, old_pcmd)
    wavi_entry = wavi + 0x20
    if wavi_entry + WAVI_ENTRY_SIZE > old_pcmd:
        raise VoiceAudioError("template WAVI entry is truncated")
    _put_u32(output, wavi_entry + 0x20, SAMPLE_RATE)
    # Keeping the predictor preamble outside the playable span preserves the
    # decoder state while making duration depend only on encoded sample words.
    _put_u32(output, wavi_entry + 0x24, 0)
    _put_u32(output, wavi_entry + 0x28, 1)
    _put_u32(output, wavi_entry + 0x2C, (len(adpcm) - 4) // 4)

    _patch_names(output, old_swdl, new_sedl, new_subheader, name)
    _patch_bank_id(output, old_swdl, new_sedl, new_subheader, bank_id)
    result = bytes(output)
    inspect_se(result, expected_name=name, expected_bank_id=bank_id)
    return result


def build_se(
    template: bytes,
    pcm_int16: Sequence[int],
    sample_rate: int,
    internal_id: int,
    symbol: str,
) -> bytes:
    """Stable orchestration API used by the voice-bank builder.

    ``pcm_int16`` must already be mono PCM at 16384 Hz.  Audio-file decoding
    and resampling live in :func:`decode_audio`, keeping this function fully
    deterministic and free of external-process calls.
    """
    if sample_rate != SAMPLE_RATE:
        raise VoiceAudioError(
            f"PCM sample rate must be {SAMPLE_RATE} Hz, got {sample_rate}; resample first"
        )
    adpcm = encode_ima_adpcm(pcm_int16)
    return _build_se_from_adpcm(template, adpcm, symbol, internal_id)


def inspect_se(
    data: bytes,
    expected_name: str | None = None,
    expected_bank_id: int | None = None,
) -> SeInfo:
    """Structurally reparse a generated SE and reject inconsistent fields."""
    if len(data) < 0x100 or len(data) % 16 or data[:4] != b"SIR0":
        raise VoiceAudioError("output is not an aligned SIR0 file")
    subheader = _u32(data, 4)
    relocations = _u32(data, 8)
    if subheader + 8 > len(data) or relocations >= len(data):
        raise VoiceAudioError("SIR0 header pointer is out of range")
    swdl = _u32(data, subheader)
    sedl = _u32(data, subheader + 4)
    if data[swdl : swdl + 4] != b"swdl" or data[sedl : sedl + 4] != b"sedl":
        raise VoiceAudioError("SIR0 subheader does not point to SWDL/SEDL")
    pointer_fields = decode_sir0_relocations(data, relocations)
    if pointer_fields != [4, 8, subheader, subheader + 4]:
        raise VoiceAudioError(f"unexpected SIR0 relocations: {pointer_fields!r}")

    sedl_size = _u32(data, sedl + 8)
    if sedl + sedl_size != subheader:
        raise VoiceAudioError("SEDL length does not end at SIR0 subheader")
    pcmd = _find_once(data, b"pcmd", swdl + SWDL_HEADER_SIZE, sedl)
    pcmd_size = _u32(data, pcmd + 12)
    if pcmd_size != _u32(data, swdl + 0x40):
        raise VoiceAudioError("SWDL and PCMD sample lengths disagree")
    if pcmd_size < 8 or pcmd_size % 4:
        raise VoiceAudioError("PCMD is not complete DS ADPCM blocks")
    eod = _align(pcmd + CHUNK_HEADER_SIZE + pcmd_size, 16)
    if data[eod : eod + 4] != b"eod ":
        raise VoiceAudioError("PCMD is not followed by aligned SWDL EOD")
    swdl_size = _u32(data, swdl + 8)
    if swdl + swdl_size != eod + CHUNK_HEADER_SIZE:
        raise VoiceAudioError("SWDL declared length is inconsistent")
    if sedl != _align(eod + CHUNK_HEADER_SIZE, 64):
        raise VoiceAudioError("SEDL is not at the expected 64-byte boundary")

    wavi = _find_once(data, b"wavi", swdl + SWDL_HEADER_SIZE, pcmd)
    wavi_entry = wavi + 0x20
    if _u16(data, wavi_entry + 0x12) != 0x0200:
        raise VoiceAudioError("WAVI sample is not IMA ADPCM")
    sample_rate = _u32(data, wavi_entry + 0x20)
    sample_pos = _u32(data, wavi_entry + 0x24)
    loop_begin = _u32(data, wavi_entry + 0x28)
    loop_length = _u32(data, wavi_entry + 0x2C)
    if sample_rate != SAMPLE_RATE or sample_pos or loop_begin != 1:
        raise VoiceAudioError("unexpected WAVI rate/position/preamble fields")
    if loop_length != (pcmd_size - 4) // 4:
        raise VoiceAudioError("WAVI sample length does not match PCMD")

    name_field = _read_fixed_ascii(data, swdl + 0x20, 16)
    if not name_field.endswith(".SW"):
        raise VoiceAudioError("unexpected generated SWDL internal name")
    name = name_field[:-3]
    sedl_name = _read_fixed_ascii(data, sedl + 0x20, 16)
    if sedl_name != name + ".SE":
        raise VoiceAudioError("SWDL and SEDL internal names disagree")
    if expected_name is not None and name != validate_name(expected_name):
        raise VoiceAudioError("generated internal name does not match request")

    # The sequence player resolves the MCRL copy rather than trusting the SWDL
    # or SEDL header names, so all three must agree before the file is accepted.
    mcrl = _find_once(data, b"mcrl", sedl, subheader)
    macro_matches = 0
    for record in range(mcrl + CHUNK_HEADER_SIZE, subheader - 5, 2):
        if _u16(data, record) != 1:
            continue
        record_size = _u16(data, record + 2)
        if record_size < 6 or record_size & 1 or record + record_size > subheader:
            continue
        raw_symbol = data[record + 4 : record + record_size].split(b"\0", 1)[0]
        if raw_symbol == name.encode("ascii"):
            macro_matches += 1
    if macro_matches != 1:
        raise VoiceAudioError(
            f"expected one matching MCRL symbol record, found {macro_matches}"
        )

    bucket_base = mcrl + CHUNK_HEADER_SIZE
    bucket = bucket_base + _mcrl_hash(name) * 2
    if bucket + 2 > subheader:
        raise VoiceAudioError("MCRL hash bucket is out of range")
    record = bucket_base + _u16(data, bucket)
    if record + 4 > subheader or _u16(data, record) != 1:
        raise VoiceAudioError("MCRL hash bucket does not resolve the generated symbol")
    hashed_record_size = _u16(data, record + 2)
    if (
        hashed_record_size < 6
        or hashed_record_size & 1
        or record + hashed_record_size > subheader
    ):
        raise VoiceAudioError("MCRL hashed symbol record is malformed")
    hashed_symbol = data[record + 4 : record + hashed_record_size].split(b"\0", 1)[0]
    if hashed_symbol != name.encode("ascii"):
        raise VoiceAudioError("MCRL hash bucket resolves a different symbol")

    bank_id = _u16(data, swdl + 0x0E)
    if _u16(data, sedl + 0x0E) != bank_id:
        raise VoiceAudioError("SWDL and SEDL bank IDs disagree")
    track = _find_once(data, b"trk ", sedl, subheader)
    track_end = track + CHUNK_HEADER_SIZE + _u32(data, track + 12)
    # Match the complete bank-select instruction.  Looking for the opcode
    # byte alone becomes ambiguous whenever either byte of the generated
    # bank ID is itself 0xA8 (for example ID 0xC9A8).
    bank_select = b"\xA8" + struct.pack(">H", bank_id)
    _find_once(data, bank_select, track + CHUNK_HEADER_SIZE, track_end)
    bnkl = _find_once(data, b"bnkl", sedl, subheader)
    if _u16(data, bnkl + 0x28) != bank_id:
        raise VoiceAudioError("BNKL bank ID disagrees")
    if expected_bank_id is not None and bank_id != expected_bank_id:
        raise VoiceAudioError("generated bank ID does not match request")

    pcmd_data = pcmd + CHUNK_HEADER_SIZE
    if pcmd_data + pcmd_size > len(data):
        raise VoiceAudioError("PCMD sample data is out of range")
    if _u16(data, pcmd_data + 2) > 88:
        raise VoiceAudioError("PCMD IMA step index is invalid")
    sample_count = (pcmd_size - ADPCM_PREAMBLE_SIZE) * 2
    return SeInfo(
        file_size=len(data),
        swdl_offset=swdl,
        swdl_size=swdl_size,
        sedl_offset=sedl,
        sedl_size=sedl_size,
        subheader_offset=subheader,
        relocation_offset=relocations,
        pcmd_offset=pcmd,
        pcmd_size=pcmd_size,
        sample_rate=sample_rate,
        sample_count=sample_count,
        duration_seconds=sample_count / sample_rate,
        bank_id=bank_id,
        name=name,
    )


def validate_se(
    data: bytes,
    expected_name: str | None = None,
    expected_internal_id: int | None = None,
) -> SeInfo:
    """Public validation API; returns parsed metadata or raises on mismatch."""
    return inspect_se(data, expected_name, expected_internal_id)


def read_dse_internal_id(data: bytes, source: str = "DSE resource") -> int:
    """Read the shared SWDL/SEDL ID from any retail ``.se`` resource.

    :func:`inspect_se` intentionally validates the much narrower generated
    one-sample voice layout.  Retail effects use many different SWDL/SEDL
    layouts, so collision detection needs a small, strict parser for the
    common SIR0 wrapper instead of relying on fixed chunk contents.
    """
    if len(data) < SIR0_HEADER_SIZE or data[:4] != b"SIR0":
        raise VoiceAudioError(f"{source} is not a SIR0 resource")

    subheader = _u32(data, 4)
    relocations = _u32(data, 8)
    if not (
        SIR0_HEADER_SIZE <= subheader
        and subheader + 8 <= relocations
        and relocations < len(data)
    ):
        raise VoiceAudioError(f"{source} has invalid SIR0 header pointers")

    swdl = _u32(data, subheader)
    sedl = _u32(data, subheader + 4)
    if not (SIR0_HEADER_SIZE <= swdl < sedl < subheader):
        raise VoiceAudioError(f"{source} has invalid SWDL/SEDL pointers")
    if data[swdl : swdl + 4] != b"swdl" or data[sedl : sedl + 4] != b"sedl":
        raise VoiceAudioError(f"{source} does not contain the expected SWDL/SEDL pair")
    if swdl + 0x10 > len(data) or sedl + 0x10 > len(data):
        raise VoiceAudioError(f"{source} has truncated SWDL/SEDL headers")

    swdl_size = _u32(data, swdl + 8)
    sedl_size = _u32(data, sedl + 8)
    if swdl_size < SWDL_HEADER_SIZE or swdl + swdl_size > sedl:
        raise VoiceAudioError(f"{source} has an invalid SWDL size")
    if sedl_size < 0x30 or sedl + sedl_size > subheader:
        raise VoiceAudioError(f"{source} has an invalid SEDL size")

    pointer_fields = decode_sir0_relocations(data, relocations)
    expected_fields = [4, 8, subheader, subheader + 4]
    if pointer_fields != expected_fields:
        raise VoiceAudioError(
            f"{source} has unexpected SIR0 relocations: {pointer_fields!r}"
        )

    swdl_id = _u16(data, swdl + 0x0E)
    sedl_id = _u16(data, sedl + 0x0E)
    if swdl_id != sedl_id:
        raise VoiceAudioError(
            f"{source} has mismatched SWDL/SEDL IDs "
            f"0x{swdl_id:04x}/0x{sedl_id:04x}"
        )
    return swdl_id


def signal_to_noise_db(reference: Sequence[int], decoded: Sequence[int]) -> float:
    if len(reference) != len(decoded) or len(reference) == 0:
        raise VoiceAudioError("SNR inputs must have the same non-zero length")
    signal = sum(sample * sample for sample in reference)
    noise = sum((left - right) ** 2 for left, right in zip(reference, decoded))
    if noise == 0:
        return math.inf
    if signal == 0:
        return -math.inf
    return 10.0 * math.log10(signal / noise)


def _pcmd_from_se(data: bytes, info: SeInfo) -> bytes:
    start = info.pcmd_offset + CHUNK_HEADER_SIZE
    return data[start : start + info.pcmd_size]


def _common_arguments(parser: argparse.ArgumentParser, output_required: bool) -> None:
    parser.add_argument("--audio", type=Path, required=True, help="source audio (Ogg, WAV, etc.)")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--template", type=Path, help="extracted se_a01b_wake.se")
    source.add_argument("--rom", type=Path, help="original USA .nds; template is extracted with ndspy")
    parser.add_argument("--output", type=Path, required=output_required, help="generated .se path")
    parser.add_argument("--name", required=True, help="unique 1-12 character internal name")
    parser.add_argument("--bank-id", required=True, type=lambda value: int(value, 0), help="unique 16-bit ID")
    parser.add_argument("--gain-db", type=float, default=0.0, help="gain applied before encoding (default: 0)")
    parser.add_argument("--ffmpeg", default="ffmpeg", help="ffmpeg executable (default: ffmpeg on PATH)")


def _build_from_arguments(args: argparse.Namespace) -> tuple[bytes, list[int], SeInfo]:
    template = read_template(args.template, args.rom)
    pcm = decode_audio(args.audio, args.ffmpeg, args.gain_db)
    adpcm, padded_pcm = encode_nds_ima(pcm)
    generated = _build_se_from_adpcm(template, adpcm, args.name, args.bank_id)
    info = inspect_se(generated, args.name, args.bank_id)
    return generated, padded_pcm, info


def command_build(args: argparse.Namespace) -> int:
    generated, _pcm, info = _build_from_arguments(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(generated)
    result = asdict(info)
    result["output"] = str(args.output)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def command_test(args: argparse.Namespace) -> int:
    generated, pcm, info = _build_from_arguments(args)
    reparsed = inspect_se(generated, args.name, args.bank_id)
    decoded = decode_nds_ima(_pcmd_from_se(generated, reparsed))
    snr = signal_to_noise_db(pcm, decoded)
    if args.min_snr_db is not None and snr < args.min_snr_db:
        raise VoiceAudioError(
            f"ADPCM round-trip SNR {snr:.2f} dB is below {args.min_snr_db:.2f} dB"
        )
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(generated)
    result = asdict(info)
    result.update(
        {
            "output": str(args.output) if args.output is not None else None,
            "snr_db": snr,
            "structural_reparse": "ok",
        }
    )
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    build = subparsers.add_parser("build", help="convert audio and write one .se")
    _common_arguments(build, output_required=True)
    build.set_defaults(handler=command_build)

    test = subparsers.add_parser("test", help="build in memory, reparse, decode, and report SNR")
    _common_arguments(test, output_required=False)
    test.add_argument(
        "--min-snr-db",
        type=float,
        default=18.0,
        help="fail below this round-trip SNR (default: 18 dB)",
    )
    test.set_defaults(handler=command_test)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = make_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.handler(args))
    except (VoiceAudioError, OSError) as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
