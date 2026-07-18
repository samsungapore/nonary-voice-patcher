# Release procedure

## 1. Release contents

Release artifacts exclude ROMs, saves, PC archives, extracted audio, generated
voice packs, and binaries embedding those packs unless covered by explicit
redistribution rights. Interface images, music, and icons require separate
licensing. See [NOTICE.md](../NOTICE.md).

Source-only releases omit runtime voice packs. Exact production-pack
regeneration uses the tracked review inputs and private game files documented
in [HACK_PROCESS.md](HACK_PROCESS.md).

## 2. Prepare metadata

Update the same version in:

- `package.json` and `package-lock.json`;
- `src-tauri/Cargo.toml` and `Cargo.lock`;
- `src-tauri/tauri.conf.json`;
- `CHANGELOG.md`.

Release candidates require English user-facing documentation and comments. CLI
schema changes require matching updates to [CLI.md](CLI.md).

## 3. Validate a clean clone

Run the public checks in [TESTING.md](TESTING.md) from a new clone with no
ignored files. Confirm `git status --short` is empty after the test. Scan for
private paths and prohibited payloads:

```sh
rg -n '/Users/|[A-Z]:\\\\' . \
  --glob '!package-lock.json' --glob '!src-tauri/Cargo.lock'
find . -type f \( -name '*.nds' -o -name '*.sav' -o -name '*.nvpack' \)
```

## 4. Run private acceptance

In a separate private workspace, rebuild the profile and both packs, run all
ignored Rust tests, exercise the CLI matrix, verify French text preservation,
perform exact resets, and complete JP/EN melonDS smoke tests. Publish only the
counts and cryptographic hashes that do not expose game data.

## 5. Build distributable code

Source archive:

```sh
git archive --format=tar.gz \
  --prefix=nonary-voice-patcher-VERSION/ \
  -o nonary-voice-patcher-VERSION.tar.gz HEAD
```

Headless CLI:

```sh
cargo build --manifest-path src-tauri/Cargo.toml --release \
  --no-default-features --features cli --bin nonary-voice-patcher-cli
```

Desktop bundles, when their resource distribution is authorized:

```sh
npm ci
npm run tauri build
```

Use platform code signing and notarization where applicable. An ad-hoc macOS
signature is useful for local verification but is not a substitute for
Developer ID signing and notarization.

Generate a complete dependency-license inventory from the exact Cargo and npm
lock files for every binary release. `THIRD_PARTY_NOTICES.md` is a source-tree
overview, not a substitute for shipping the required license texts.

## 6. Verify artifacts

For every publishable artifact:

- inspect its contents for forbidden game/private inputs;
- test it on a clean machine or VM;
- verify CLI `--help`, human mode, and JSON mode;
- verify the desktop application starts with both interface languages;
- compute SHA-256 and attach a checksum file;
- state clearly whether runtime voice packs are included or must be built.

Example checksum generation:

```sh
shasum -a 256 nonary-voice-patcher-VERSION.tar.gz \
  > SHA256SUMS.txt
shasum -a 256 -c SHA256SUMS.txt
```

## 7. Publish

Create an annotated tag, push it, and draft the GitHub release from the
changelog. Release notes include supported ROM/language scope, breaking CLI or
format changes, known limitations, verification results, and licensing
information. Private validation fixtures are excluded from release assets.
