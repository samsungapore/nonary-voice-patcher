# Testing and validation

Testing consists of synthetic CI coverage and production acceptance tests using
local fixtures. Public CI requires no ROM, PC archive, extracted audio, or
generated voice pack.

## Public checks

```sh
npm ci
npm run build

cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml \
  --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml \
  --all-targets --all-features

python3 -m unittest discover -s tests -v
```

The Rust suite keeps synthetic NitroFS, receipt, and CLI tests active. Tests
that require a stock ROM, French output, extracted FSB files, or real voice
packs are marked `ignored` with their prerequisite. A clean checkout currently
runs 13 Rust tests and ignores 21 private-fixture tests.

The Python tests cover window enumeration and scoring, grouped voice-source
resolution, duplicate/conflict rejection, and same-speaker filtering.

## Private fixture tests

Place private inputs in the ignored workspace paths documented by each test,
then run:

```sh
cargo test --manifest-path src-tauri/Cargo.toml \
  --all-targets --all-features -- --ignored --nocapture
```

Do not change an ignored production test into an unconditional public test;
use a synthetic fixture when possible.

## Production acceptance matrix

Run both voice languages against:

| Input | Inspect | Apply JP | Apply EN | Exact reset |
| --- | --- | --- | --- | --- |
| Reviewed US retail ROM | `clean` | required | required | required |
| Reviewed French output | `clean` | required | required | required |
| JP output | `japanese` | optional reapply | switch to EN | required |
| EN output | `english` | switch to JP | optional reapply | required |
| Unknown structural edit | `unsupported` | reject | reject | n/a |
| Voice files without receipt | `legacy_voice_patch` | reject | reject | reject |

For translated inputs, additionally assert:

- all 14,610 `setText` payloads remain byte-identical;
- all 17 reviewed pointer fields are repaired in the voiced output;
- reset returns the exact pre-repair translated input hash;
- JP and EN contain the same 6,414 targets and 51 scripts.

## Static ROM audit

`scripts/verify_rom.py` verifies generated symbol order, sound-registry
relocations, internal DSE IDs, voice payloads, compiled scripts, and the
anti-bleep bank:

```sh
python3 scripts/verify_rom.py output.nds build/voice_map_extended_dialogue_only.tsv \
  --compiled-dir build/scripts-compiled \
  --voice-dir build/voices-jp \
  --se-sys build/se_sys.silenced.se
```

The Rust `audit_voice_patch_preservation` developer binary performs the
runtime-engine preservation audit from private fixtures.

## melonDS smoke test on macOS

The helpers were validated with melonDS 1.1. Launch the executable directly so
the boot log is retained:

```sh
ROM=/absolute/path/to/voiced.nds
/usr/bin/script -qF /tmp/melonds.log \
  /Applications/melonDS.app/Contents/MacOS/melonDS -b always "$ROM"
```

A healthy boot log contains:

```text
Inserted cart with game code: BSKE
Secure area decryption OK
Game is now booting
```

Use a temporary ROM copy when the test must not create a `.sav` beside a build.

### Reach the first exact voice

The first high-confidence target is `Ow!` in `a01b.fsb`. Starting from a fresh
boot:

1. tap the opening montage's **skip** area;
2. tap **Start**;
3. dismiss the save-data notice and fiction disclaimer;
4. advance the first two narration windows one at a time;
5. capture audio, then tap once to advance to `Ow!`.

```sh
xcrun swift scripts/melonds_input.swift touch bottom 248 180
xcrun swift scripts/melonds_input.swift touch bottom 128 108

xcrun swift scripts/capture_melonds_audio.swift \
  --duration 7 --output /tmp/melonds-ow.m4a --force &
CAPTURE_PID=$!
sleep 1.5
xcrun swift scripts/melonds_input.swift touch bottom 128 100
wait "$CAPTURE_PID"
```

ScreenCaptureKit records the melonDS application audio, not the microphone.
macOS may require Accessibility and Screen & System Audio Recording permission.

Inspect the capture:

```sh
ffprobe -v error \
  -show_entries format=duration,size:stream=codec_name,sample_rate,channels \
  -of default=nw=1 /tmp/melonds-ow.m4a

ffmpeg -hide_banner -i /tmp/melonds-ow.m4a \
  -af volumedetect -f null - 2>&1 | rg 'Duration|mean_volume|max_volume'
```

Repeat for JP and EN. Also advance through consecutive short character lines to
confirm `WaitSE` prevents overlap, and through bottom-screen narration to
confirm no Junpei voice is injected there.

## Release evidence

Release evidence records tool versions, source commit, platform, commands,
pass/fail counts, artifact sizes, and SHA-256 values. CI artifacts and public
issues exclude ROMs, saves, decoded audio, voice packs, and dialogue text.
