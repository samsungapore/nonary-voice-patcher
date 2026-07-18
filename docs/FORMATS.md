# Binary and metadata formats

All integer fields described below are little-endian unless stated otherwise.
The parsers enforce bounds, count limits, hashes, and exact magic/version
values. Parser validation is authoritative.

## Voice pack (`NVPACK01`)

Voice packs contain generated DS `.se` files without recompression. Version 1
has a fixed 0x70-byte header:

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x00` | 8 | ASCII magic `NVPACK01` |
| `0x08` | 4 | Version (`1`) |
| `0x0c` | 4 | Entry count |
| `0x10` | 4 | Language: `jp\0\0` or `en\0\0` |
| `0x14` | 4 | Reserved, zero |
| `0x18` | 8 | Entry-table offset (`0x70`) |
| `0x20` | 8 | Payload offset |
| `0x28` | 8 | Payload byte length |
| `0x30` | 32 | SHA-256 of the entry table |
| `0x50` | 32 | SHA-256 of the complete payload |

Each 40-byte entry contains:

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x00` | 4 | Payload size |
| `0x04` | 2 | Flags, zero in version 1 |
| `0x06` | 2 | Reserved, zero |
| `0x08` | 32 | SHA-256 of this `.se` payload |

The payload starts at the next 16-byte boundary after the table. Entries are
concatenated in `se_v0000.se`, `se_v0001.se`, … order. There are no filenames
inside the pack because the stable index defines both the generated symbol and
NitroFS basename.

## Voice profile (`voice-profile.json`)

The public profile is JSON version 1:

```json
{
  "version": 1,
  "game_code": "BSKE",
  "voice_count": 6414,
  "scripts": [
    {
      "path": "scr/a...fsb",
      "structural_sha256": "...",
      "alternative_structural_sha256": ["..."],
      "set_text_count": 123,
      "voices": [{"ordinal": 7, "symbol": "SE_V0000"}]
    }
  ],
  "exact_repairs": []
}
```

Symbols must be a complete, ordered `SE_V0000` sequence. Script paths must be
unique safe `scr/*.fsb` paths. A structural hash is computed from parsed script
structure while excluding replaceable text payloads. `set_text_count` and
ordinals provide an independent drift check.

An exact repair names a damaged full-file SHA-256, the expected repaired
SHA-256, accepted already-clean hashes, and offset/expected/replacement byte
triples. A repair runs only when the complete input fingerprint and every
preimage agree.

## Restoration receipt (`NVPBASE1` / `NVPRCP01`)

The receipt body is 580 bytes (`0x244`):

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x000` | 8 | ASCII magic `NVPBASE1` |
| `0x008` | 4 | Version (`1`) |
| `0x00c` | 4 | Reserved, zero |
| `0x010` | 8 | Original ROM length |
| `0x018` | 32 | Original ROM SHA-256 |
| `0x038` | 512 | Original NDS header preimage |
| `0x238` | 4 | Original bytes at ROM offset `0x1000` |
| `0x23c` | 4 | Language (`jp\0\0` or `en\0\0`) |
| `0x240` | 4 | Original game code |

It is followed by a 16-byte marker: `NVPRCP01` and the 64-bit receipt length.
Padding before the body makes the following DS signature 16-byte aligned. The
patched header's RSA-signature pointer (`0x80`) points immediately after the
marker, which lets inspection locate the receipt without scanning.

The reserved word must remain zero. Unknown flags or versions are rejected.
Reset copies the recorded prefix, restores the two preimages, and verifies the
full original SHA-256 before publishing any output.

## Generated DS sound effect

Each voice resource is an aligned SIR0 container holding:

- one SWDL sample bank;
- one SEDL sequence;
- a WAVI entry describing the sample;
- a PCMD chunk with mono Nintendo DS IMA ADPCM;
- BNKL, track, and MCRL records using the same internal bank ID and symbol;
- a complete SIR0 relocation table.

Audio is 16,384 Hz mono. The ADPCM stream begins with a four-byte
predictor/index preamble and is padded to whole 4-byte / 8-sample blocks. The
encoder clamps the negative predictor at `-32767`, matching the DS variant.

The MCRL label is indexed by the sum of ASCII symbol bytes modulo 256. Updating
the visible string without moving its hash-bucket entry makes the retail engine
fail to resolve the sound, so the generator verifies both the record and bucket
chain after renaming.

## SIR0 relocation encoding

SIR0 stores sorted pointer-field offsets as positive deltas. Each delta is a
big-endian sequence of 7-bit groups with the high bit marking continuation; a
zero byte terminates the table. Parsers require unique increasing fields and
validate every relocated pointer against the non-relocation data region.

## NitroFS output layout

The output file is conceptually:

```text
[base ROM bytes with patched header/auxiliary metadata]
[aligned replacement payloads]
[aligned voice payloads]
[new FNT]
[new FAT]
[padding]
[receipt body][receipt marker][retained 0x88-byte signature]
```

Original payload data remains present but becomes unreachable after its FAT
entry points to the appended version. The receipt retains preimages for the
small original-range metadata edits. Together, that redundancy and those
preimages make exact reset possible without shipping a reverse patch.
