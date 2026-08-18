# Binary and metadata formats

All integer fields described below are little-endian unless stated otherwise.
The parsers enforce bounds, count limits, hashes, and exact magic/version
values. Parser validation is authoritative.

## Voice pack (`NVPACK01`)

Voice packs contain generated DS `.se` files without recompression. Version 1
is used by the established Japanese and English packs. Version 2 binds the
ordered entries to a voice-profile catalogue and is required for French packs.

The common header prefix is:

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x00` | 8 | ASCII magic `NVPACK01` |
| `0x08` | 4 | Version (`1` or `2`) |
| `0x0c` | 4 | Entry count |
| `0x10` | 4 | Language: `jp\0\0`, `en\0\0`, or `fr\0\0` |
| `0x14` | 4 | Reserved, zero |
| `0x18` | 8 | Entry-table offset (`0x70` for v1, `0x90` for v2) |
| `0x20` | 8 | Payload offset |
| `0x28` | 8 | Payload byte length |
| `0x30` | 32 | Authenticated entry-table digest described below |
| `0x50` | 32 | SHA-256 of the complete payload |

The v1 header ends at `0x70`; its table digest is the SHA-256 of the raw entry
table. The v2 header adds:

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x70` | 32 | Voice-catalogue SHA-256 |

For v2, the digest at `0x30` is:

```text
SHA-256("NVPACK02-INDEX\0" || catalogue_sha256 || raw_entry_table)
```

This binds the table and the catalogue digest in one authenticated value.

Each 40-byte entry contains:

| Offset | Size | Field |
| ---: | ---: | --- |
| `0x00` | 4 | Payload size |
| `0x04` | 2 | Flags, zero |
| `0x06` | 2 | Reserved, zero |
| `0x08` | 32 | SHA-256 of this `.se` payload |

The payload starts at the next 16-byte boundary after the table. Entries are
concatenated in `se_v0000.se`, `se_v0001.se`, … order. There are no filenames
inside the pack because the stable index defines both the generated symbol and
NitroFS basename. At patch time, a French pack must be v2 and its catalogue
digest must equal the digest recomputed from the selected profile.

## Voice profile (`voice-profile.json`)

The public profile is JSON version 1:

```json
{
  "version": 1,
  "game_code": "BSKE",
  "voice_count": 6475,
  "scripts": [
    {
      "path": "scr/a...fsb",
      "structural_sha256": "...",
      "alternative_structural_sha256": ["..."],
      "set_text_count": 123,
      "voices": [
        {
          "ordinal": 7,
          "symbol": "SE_V0000",
          "target_text_sha256": []
        }
      ]
    }
  ],
  "exact_repairs": []
}
```

Symbols must be a complete, ordered `SE_V0000` sequence. Script paths must be
unique safe `scr/*.fsb` paths. A structural hash is computed from parsed script
structure while excluding replaceable text payloads. `set_text_count` and
ordinals provide an independent drift check.

`target_text_sha256` is optional. An empty or omitted array makes the target
unconditional. A non-empty array contains lowercase SHA-256 values of the
exact raw `setText` payload bytes accepted for that target. The voice is
injected when any listed hash matches; otherwise that line remains untouched.

An exact repair names a damaged full-file SHA-256, the expected repaired
SHA-256, accepted already-clean hashes, and offset/expected/replacement byte
triples. A repair runs only when the complete input fingerprint and every
preimage agree.

### Voice-catalogue digest

The catalogue digest is independent of profile JSON formatting. It is SHA-256
over the following byte sequence:

```text
"NVPACK-CATALOG-V1\0"
voice_count as u64
for every voice in profile script order:
  symbol UTF-8 byte length as u64, then symbol bytes
  script-path UTF-8 byte length as u64, then path bytes
  setText ordinal as u64
```

When any voice has a raw-text condition, the following suffix is included:

```text
"NVPACK-TARGET-TEXT-SHA256-V1\0"
voice_count as u64
for every voice in profile script order:
  sorted hash count as u64
  each 32-byte SHA-256 value in sorted order
```

The length and integer fields are little-endian. Both the dubbing project and
French NVPACK v2 store this digest, so harmless profile JSON reformatting does
not change the catalogue identity. Projects also retain the raw profile-file
hash as provenance.

## Dubbing project (`project.nvdub.json`)

The project manifest is JSON version 1 with camel-case field names. Its root
contains:

- project name and creation/update timestamps;
- source-ROM filename, game code, and logical base-ROM SHA-256;
- raw profile-file SHA-256 and the stable voice-catalogue SHA-256;
- a `targets` object keyed by the complete `SE_V0000` through `SE_V6474`
  sequence.

Each target stores its structural `targetId` (`script_path#ordinal`), status,
notes, active-take ID, take list, gain, trim values, and optional approval
digest. Each take stores a project-relative WAV path, recording dimensions,
level statistics, creation time, and the SHA-256 of the committed file.

The manifest intentionally contains no dialogue, surrounding context, speaker
text, ROM bytes, or profile copy. The application reconstructs the 6,475 cues
and their same-function context from the selected ROM whenever the project is
opened.

The normal directory layout is:

```text
project/
├── .project.nvdub.lock
├── project.nvdub.json
├── recordings/
│   └── SE_V####/
│       └── take-<timestamp>-<sequence>.wav
└── builds/
    └── voices-fr-preview.nvpack
```

The hidden lock file coordinates manifest updates across local application
instances and command-line builders. A builder holds it from manifest snapshot
through atomic publication of its pack and any report or test ROM, so project
edits cannot invalidate an output before publication completes. It carries no
project data and may be recreated when no application has the project open.

Committed masters are 16-bit PCM WAV files at the capture-device sample rate.
The stored SHA-256 and dimensions are checked before reading, selection,
approval, or export. An approval digest is:

```text
SHA-256(
  "NVDUB_APPROVAL_V1\0" ||
  lowercase_take_sha256_ascii ||
  gain_db_f32_bits_as_u32 ||
  trim_start_ms_as_u64 ||
  trim_end_ms_as_u64
)
```

Numeric fields in that digest are little-endian. Selecting another take or
changing bound processing settings therefore requires a new approval.

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
| `0x23c` | 4 | Language (`jp\0\0`, `en\0\0`, or `fr\0\0`) |
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
