# Building from source

## Repository root

Run all commands from the `nonary-voice-patcher` repository root.

## Prerequisites

- Node.js 20.19 or newer and npm
- stable Rust and Cargo
- Python 3.11 or newer for research scripts
- Tauri 2 platform prerequisites
- a working native audio-input backend for the Dubbing Studio (CoreAudio on
  macOS, WASAPI on Windows, or the CPAL-supported backend for the target Linux
  distribution)
- optional research tools: ZeroEscapeScript, Crossover/Wine, FFmpeg,
  vgmstream, melonDS, and 999-tools

Install JavaScript and Python dependencies:

```sh
npm ci
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -r requirements-dev.txt
```

## Frontend-only build

```sh
npm run build
npm run dev
```

Vite can render the interface in a browser, but ROM, project, microphone, and
build commands require the Tauri backend. Development-only
`?qa-language=en` or `?qa-language=fr` parameters skip the startup language
choice. Add `&qa-view=dubbing` to open the Dubbing Studio directly, for example:

```text
http://localhost:1420/?qa-language=en&qa-view=dubbing
```

These parameters are ignored by production builds.

## Desktop application

```sh
npm run tauri dev
npm run tauri build
```

The configured desktop window opens at 1360×860 and can shrink to 860×680.
GUI builds enable the native recorder, WAV writer, and high-quality resampler;
the native Studio path does not require Python or FFmpeg.

`tauri.conf.json` bundles the `src-tauri/resources/` directory. A clean clone
can compile because the public profile is present, but applying voices requires
these ignored local files:

```text
src-tauri/resources/voices-jp.nvpack
src-tauri/resources/voices-en.nvpack
```

Voice packs are generated separately using
[HACK_PROCESS.md](HACK_PROCESS.md) and are excluded from Git. Missing packs
produce a runtime resource error only when the corresponding Japanese or
English patch is applied. Project creation, microphone recording, and a French
preview test-ROM build use the public profile plus the selected ROM and project
WAV files; they do not require either private JP/EN pack. See
[DUBBING_STUDIO.md](DUBBING_STUDIO.md).

The Python preview/production fallback is run from the same development
environment:

```sh
python scripts/build_dubbing_project.py --help
```

It creates an explicit French NVPACK v2 and a `.json` report. The native GUI
instead writes `builds/voices-fr-preview.nvpack` with no report sidecar and
immediately applies it to the selected test-ROM destination.

## Headless CLI

```sh
cargo build \
  --manifest-path src-tauri/Cargo.toml \
  --release \
  --locked \
  --no-default-features \
  --features cli \
  --bin nonary-voice-patcher-cli
```

This target omits Tauri, Wry, and platform webview dependencies. See
[CLI.md](CLI.md) for packaging and resource discovery.

## Rust developer tools

The profile builder and audits are excluded from normal binaries:

```sh
cargo build \
  --manifest-path src-tauri/Cargo.toml \
  --no-default-features \
  --features dev-tools \
  --bins
```

They consume private research fixtures and are not expected to run in CI.

## Regenerating interface assets

`scripts/build_tauri_game_assets.py` reads selected backgrounds and music from
a private US ROM. It expects the external 999-tools checkout at
`tools/999-tools/` and optionally `vgmstream-cli` plus FFmpeg:

```sh
python3 scripts/build_tauri_game_assets.py \
  --rom original/999-us.nds \
  --output src/assets/999 \
  --icon-source src-tauri/icons/icon-source.png

npm exec tauri icon src-tauri/icons/icon-source.png
```

The generator records filenames and hashes without storing local absolute
paths.

Game-derived visual/audio assets are not covered by the source-code license.
See [NOTICE.md](../NOTICE.md).

## Reproducible clean-checkout checks

These commands must pass without either voice pack:

```sh
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml \
  --locked --no-default-features --features cli --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml \
  --locked --no-default-features --features cli --all-targets
python3 -m unittest discover -s tests -v
```

An all-features Cargo build also compiles without packs; fixture-based tests
remain ignored without their private inputs.
