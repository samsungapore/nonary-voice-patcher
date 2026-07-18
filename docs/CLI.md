# CLI reference

`nonary-voice-patcher-cli` exposes the same Rust engine as the desktop app
without linking Tauri or a webview. It is intended for terminal use and for
embedding in another patcher.

## Build

```sh
cargo build \
  --manifest-path src-tauri/Cargo.toml \
  --release \
  --locked \
  --no-default-features \
  --features cli \
  --bin nonary-voice-patcher-cli
```

The binary is written to `src-tauri/target/release/`.

## Syntax

```text
nonary-voice-patcher-cli [--json] [--resources-dir DIR] <COMMAND>

Commands:
  inspect <INPUT_ROM>
  apply <INPUT_ROM> <OUTPUT_ROM> --language <jp|en|fr> [--voice-pack FILE]
  reset <PATCHED_ROM> <OUTPUT_ROM>
```

The global options may appear before or after a subcommand.

### `inspect`

Parses the ROM, verifies any restoration receipt, restores the logical base for
compatibility checks, and reports one of these states:

- `clean`
- `japanese`
- `english`
- `french`
- `legacy_voice_patch`
- `unsupported`

`inspect` needs `voice-profile.json` but does not open a voice pack.

```sh
nonary-voice-patcher-cli \
  --resources-dir ./resources \
  inspect game.nds
```

### `apply`

Creates a new Japanese-, English-, or French-voiced ROM. If the input contains
a valid receipt from this patcher, the engine first restores its base; applying
`en` to a JP-patched ROM therefore switches the voice language without stacking
two patches.

```sh
nonary-voice-patcher-cli --resources-dir ./resources \
  apply source.nds output.nds --language jp

nonary-voice-patcher-cli --resources-dir ./resources \
  apply source.nds output.nds --language en

nonary-voice-patcher-cli --resources-dir ./resources \
  apply source.nds output.nds --language fr \
  --voice-pack ./studio-export/voices-fr.nvpack
```

`apply` needs the profile and a matching pack. Japanese and English use the
pack selected from the resource directory. French studio exports are not
bundled resources, so `--voice-pack FILE` is required for `--language fr`.
The pack's embedded language code is checked before any output is written.

### `reset`

Restores the exact source ROM recorded by a valid patch receipt. It does not
need a resource directory.

```sh
nonary-voice-patcher-cli reset voiced.nds restored.nds
```

The source and destination must be distinct after canonicalization. Parent
directories for CLI output are created as needed. A destination is published
only after the temporary ROM passes structural and byte-exact restoration
checks.

## Resource discovery

The recommended integration always passes an absolute `--resources-dir`.
That directory has this layout:

```text
resources/
├── voice-profile.json
├── voices-jp.nvpack
└── voices-en.nvpack
```

Without the option, the CLI checks these locations in order and uses the first
directory containing `voice-profile.json`:

1. `NONARY_VOICE_PATCHER_RESOURCES`;
2. `resources/` beside the executable;
3. the executable directory;
4. macOS bundle `Resources/resources` and `Resources` locations;
5. AppImage and system package locations under `usr/lib` on Linux;
6. `../share/nonary-voice-patcher/resources` beside the binary;
7. debug builds only: the development `src-tauri/resources` directory.

An explicitly supplied directory is never silently replaced by a fallback.

## Human output

Human mode writes the final report to stdout and progress/errors to stderr.
Fields include the ROM title, game code, byte size, SHA-256, patch state, and
compatibility for `inspect`. Apply reports include language, voice count,
script count, and restoration status; reset reports zero voice/script counts
and an exact-restoration status without a language line.

## JSON mode

`--json` is the stable automation interface:

- stdout contains exactly one result or error JSON object;
- stderr contains zero or more newline-delimited progress objects;
- Clap `--help` and `--version` remain terminal text.

### Successful inspection

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "inspect",
  "result": {
    "path": "source.nds",
    "title": "999HRPERDOOR",
    "gameCode": "BSKE",
    "bytes": 134217728,
    "sha256": "...",
    "state": "clean",
    "compatible": true,
    "detail": "Compatible ROM with no voice patch"
  }
}
```

### Successful apply

```json
{
  "schemaVersion": 1,
  "ok": true,
  "command": "apply",
  "result": {
    "outputPath": "output.nds",
    "bytes": 314159265,
    "sha256": "...",
    "language": "japanese",
    "voices": 6414,
    "scripts": 51,
    "resetExact": false
  }
}
```

A reset result uses `"language": null`, zero voices/scripts, and
`"resetExact": true`.

### Progress event

```json
{"schemaVersion":1,"event":"progress","command":"apply","progress":{"stage":"write","completed":1048576,"total":183500800,"message":"Writing sound/se_v0032.se"}}
```

Known stages are `inspect`, `read`, `scripts`, `write`, `verify`, and `done`.
Consumers must not infer completion from a progress event; the process
exit code and stdout envelope are authoritative.

### Error envelope

```json
{
  "schemaVersion": 1,
  "ok": false,
  "command": "apply",
  "error": {
    "code": "input_error",
    "message": "the output file must be different from the source ROM",
    "exitCode": 4
  }
}
```

## Exit codes

| Code | JSON code | Meaning |
| ---: | --- | --- |
| `0` | — | Success, help, or version output |
| `2` | `usage_error` | Invalid syntax or option value |
| `3` | `resource_error` | Missing/corrupt pack or a profile validation/compatibility failure during apply |
| `4` | `input_error` | Invalid NDS layout, receipt, legacy patch, ROM-size limit, or path conflict |
| `5` | `io_error` | Filesystem access failure |
| `6` | `operation_error` | Patch, registry, FSB, or final verification failure |
| `70` | `internal_error` | JSON result serialization failure |

## Embedding checklist

1. Launch with `--json` and an absolute `--resources-dir`.
2. Read stdout and stderr concurrently to avoid pipe backpressure.
3. Parse each stderr line independently and tolerate future progress stages.
4. Wait for process termination before parsing the single stdout object.
5. Require exit code `0`, `ok: true`, and supported `schemaVersion`.
6. Treat output paths and SHA-256 values as untrusted data until validated.
7. Never pass the same canonical path for input and output.
