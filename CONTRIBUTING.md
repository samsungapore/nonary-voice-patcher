# Contributing

Thank you for helping improve Nonary Voice Patcher. Keep pull requests focused,
explain the user-visible effect, and include the verification you performed.

## Ground rules

- Do not upload ROMs, saves, extracted voice audio, PC game archives, voice
  packs, Dubbing Studio projects, recorded WAV masters, build reports, or
  additional extracted game payloads. The reviewed alignment inputs under
  `research/reviews/` are the only versioned review datasets.
- Write documentation, code comments, commit messages, and public diagnostics
  in English.
- Comments should explain a constraint, trade-off, or invariant. Avoid comments
  that merely restate the next line of code.
- Preserve translated text and unrelated ROM assets. A compatibility change
  must include a structural validation argument and a regression test.
- Do not weaken hash, bounds, receipt, SIR0 relocation, or post-write checks to
  make an unsupported ROM pass.

## Setup and checks

Follow [docs/BUILDING.md](docs/BUILDING.md). Before opening a pull request, run:

```sh
npm ci
npm run build
python3 -m unittest discover -s tests -v
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets --all-features
```

Fixture-dependent tests are ignored in a clean checkout. If you change the ROM
engine, also run the private acceptance suite described in
[docs/TESTING.md](docs/TESTING.md) and summarize the results without publishing
the fixtures. Changes to project, recording, or French-build code must also
cover the relevant Dubbing Studio acceptance cases without checking in the
generated project or pack.

## Pull requests

Include:

1. the problem and why the chosen design is safe;
2. tests for new behavior or a reason a test is not practical;
3. screenshots for interface changes;
4. updated public contracts when CLI JSON, formats, or compatibility changes;
5. no generated `dist/`, `target/`, `release/`, Dubbing Studio project,
   recording, build-report, ROM, or voice-pack files.
