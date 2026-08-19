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

The Rust suite covers synthetic NitroFS and FSB parsing, receipts, CLI
contracts, NVPACK v1/v2 validation, catalogue hashing, project integrity,
approval invalidation, WAV inspection, energy-gated active-block measurement,
reference matching, audio conversion, DS encoding and decoding, and
native-builder path safety. Tests that require a stock ROM, French translation,
extracted FSB files, or real voice packs are marked `ignored` with their
prerequisite. Test totals are intentionally not pinned in this document.

The Python suite covers the research alignment tools plus catalogue hashing,
project validation, approval binding, WAV conversion, output-path safety, and
preview/production French-pack generation contracts.

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

Run every patch language against:

| Input | Inspect | Apply JP | Apply EN | Apply FR v2 | Exact reset |
| --- | --- | --- | --- | --- | --- |
| Reviewed US retail ROM | `clean` | required | required | required | n/a |
| Reviewed French translation ROM | `clean` | required | required | required | n/a |
| JP output | `japanese` | optional reapply | switch to EN | switch to FR | required |
| EN output | `english` | switch to JP | optional reapply | switch to FR | required |
| FR output | `french` | switch to JP | switch to EN | optional reapply | required |
| Unknown structural edit | `unsupported` | reject | reject | reject | n/a |
| Voice files without receipt | `legacy_voice_patch` | reject | reject | reject | reject |

French applies always use `--language fr --voice-pack <FILE>`. Test rejection
of a v1 French pack, a v2 pack with another catalogue digest, a wrong-language
pack, a changed entry, and a changed aggregate payload.

For translated inputs, additionally assert:

- all 14,610 `setText` payloads remain byte-identical;
- all 17 reviewed pointer fields are repaired in the voiced output;
- reset returns the exact pre-repair translated input hash;
- JP, EN, and FR contain the same 6,475 ordered voice slots and 51 scripts;
- the French M10 voice is injected only for its reviewed raw-text SHA-256 and
  is safely skipped on the stock text;
- all reviewed split and composite targets preserve their component order and
  language-specific cut boundaries.

## Dubbing Studio acceptance matrix

| Scenario | Expected result |
| --- | --- |
| Create from a compatible ROM | Exactly 6,475 unique contiguous targets are indexed |
| Inspect cue context | At most two lines before and after; no context crosses a script-function boundary |
| Record multiple takes | Each capture becomes an immutable mono PCM16 WAV with a distinct ID and SHA-256 |
| Select another take | The selected take becomes active and any prior approval is cleared |
| Compare against JP, then EN | Each analysis uses the explicitly selected original voice pack and the matching `SE_V####` entry |
| Match a normal take | Stored gain matches the active-block level where possible, causes no pre-IMA clipping, and keeps the measured Final DS peak at or below -1 dBFS |
| Match a very quiet take | Gain stops at +24 dB, reports the limit, and recommends recording a stronger clean signal |
| Match a transient-heavy take | Gain is reduced when necessary to preserve -1 dBFS headroom even if active RMS remains below the reference |
| Match all recorded | Every measurable active take is updated independently; unrecorded cues remain outside the batch and active-but-unmeasurable takes are reported as skipped |
| A/B preview | Raw take plays the committed master, Final DS includes saved processing and the DS codec path, and Original reference plays the selected source cue |
| Approve, then change bound settings | Approval digest no longer validates until the cue is approved again |
| Replace or symlink a WAV | Project open/build rejects the take before encoding |
| Preview with partial recordings | Active takes are used; every missing cue receives 80 ms silence |
| Native preview output | `builds/voices-fr-preview.nvpack` is v2, no JSON sidecar is created, and the selected ROM output verifies |
| Edit from another process during export | The edit waits until the pack and report or test ROM have been atomically published |
| Production with any incomplete cue | Python preflight rejects before audio encoding |
| Production with 6,475 approved cues | The Python builder creates a complete catalogue-bound v2 pack and JSON report |
| Reset the preview ROM | Output is byte-identical to the logical source ROM |

The private end-to-end fixture should also exercise a complete 6,475-entry
native preview build, reopen the generated pack, apply it, and verify exact
receipt-based restoration. The all-silence fixture is suitable for structural
coverage; at least one real recorded cue is still required for listening tests.

## Microphone smoke test

On each desktop platform:

1. grant microphone access and confirm the intended input appears;
2. record a short take and stop it normally;
3. play it back, record a second take, and switch between both;
4. confirm duration, sample rate, peak level, clipping count, and SHA-256 are
   present after reopening the project;
5. start another capture and navigate away or cancel, then confirm no committed
   take was added;
6. build a preview ROM and confirm the UI reports the final path and hash.

## Voice-level matching smoke test

Use a real recorded cue and locally supplied Japanese and English voice packs:

1. choose **Japanese**, select **Compare levels**, and record the displayed
   take, reference, and Final DS active RMS values, the suggested gain, and the
   measured Final DS peak;
2. choose **English**, compare again, and confirm the reference values change
   without changing the recorded WAV or saved gain;
3. choose the intended language and select **Match reference**;
4. confirm the stored gain follows the active-RMS difference rounded downward
   to a safe 0.1 dB step, unless the codec/headroom check or the -60 dB to
   +24 dB range limits it further;
5. alternate **Raw take**, **Final DS**, and **Original reference** and confirm
   that Raw plays the unchanged master while Final DS includes saved processing
   and DS codec coloration;
6. adjust **Dubbing gain** manually, save it, select **Compare levels** again,
   and confirm the displayed Final DS peak and preview update while the
   committed WAV hash does not;
7. if the cue was approved, confirm a gain change returns it to **Needs
   review**;
8. run **Match all recorded** in a disposable project containing loud, quiet,
   active-but-unmeasurable, and unrecorded examples, then verify the matched,
   headroom-limited, gain-limited, and skipped counts; confirm that the
   unrecorded cue is outside those counts;
9. build a test ROM and listen to the cue in context with music and sound
   effects. Treat this in-game pass, not the isolated preview, as final mix
   acceptance.

No step should reveal compression, limiting, automatic gain movement, or a
changed master WAV. The matcher stores one constant per-cue gain, caps its own
recommendation, and leaves a too-quiet source for the performer to record
again.

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

Repeat for JP, EN, and a French preview containing a real take at the target.
Also advance through consecutive short character lines to confirm `WaitSE`
prevents overlap, and through bottom-screen narration to confirm no Junpei
voice is injected there.

## Release evidence

Release evidence records tool versions, source commit, platform, commands,
pass/fail counts, artifact sizes, and SHA-256 values. CI artifacts and public
issues exclude ROMs, saves, decoded audio, Dubbing Studio projects and
recordings, voice packs, and dialogue text.
