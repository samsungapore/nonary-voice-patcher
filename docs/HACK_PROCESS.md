# Voice-hack process

This document records the research pipeline used to derive the final
6,475-target patch. ROM and PC archive inputs are not distributed with the
project.

## Runtime build path

Generating the two voice packs from personally obtained game files follows
sections 1, 2, 6, and 9. Sections 3 through 5 document how the alignment was
researched; they are not required for a normal pack build. The reviewed runtime
mapping is tracked at `research/alignment/final_voice_alignment.tsv`.

`voice-profile.json` is already tracked in `src-tauri/resources/`. Its optional
exact-regeneration path uses the text-free reviewed compatibility manifest
tracked under `research/compatibility/`.

## 1. Required inputs

Runtime pack generation requires:

- the US Nintendo DS ROM with game code `BSKE`;
- `ze1_data.bin` from the PC release of *Zero Escape: The Nonary Games*;
- Python 3.11 and the dependencies pinned in
  `requirements-research-lock.txt`.

Exact `voice-profile.json` regeneration also requires the stable Rust toolchain
because its builder reuses the released Rust parser.

The complete alignment research additionally used:

- a decompilation of the DS `scr/*.fsb` files;
- ZeroEscapeScript under Crossover/Wine and melonDS.

The raw French-patcher FSB output is only needed to audit the tracked
compatibility manifest against its original source.

The three human-review inputs are versioned in the repository:

```text
research/reviews/borderline_voice_review.tsv
research/reviews/alignment_extended_candidates.tsv
research/reviews/reviewed_alignment_overrides.tsv
```

The ignored workspace layout is:

```text
original/       private ROM and PC archive
work/romfs/     extracted DS filesystem and decompiled scripts
work/manifests/ generated alignment data
build/          generated audio, scripts, maps, and reports
```

Create the research environment from the repository root:

```sh
python3.11 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -r requirements-research-lock.txt
```

On Windows PowerShell, activate the environment with
`.\.venv\Scripts\Activate.ps1` before running the same `python -m pip` command.
The remaining examples use POSIX `python3` and `\` line continuations. In
PowerShell, use `python` (or `py -3.11`) and either place each command on one
line or replace each trailing `\` with a PowerShell backtick.

For the reference build, the owned inputs were:

| Input | Bytes | SHA-256 |
| --- | ---: | --- |
| US `BSKE` ROM | 134,217,728 | `05843a3ee682bc388299ce35b9347d9d1bdda4d3890fcffbde6de785a5de5e15` |
| Steam `ze1_data.bin` | 2,659,212,416 | `3e9685e65956d4fa3e6f1832a3ca4e25df84d1e4ee16441df7eebb6b57e73691` |

Other revisions may still parse, but the reference output hashes below apply
to these inputs. Verify them with `shasum -a 256` on macOS/Linux or
`Get-FileHash -Algorithm SHA256` in PowerShell.

Steam normally installs the PC archive at:

```text
Windows:
C:\Program Files (x86)\Steam\steamapps\common\Zero Escape The Nonary Games\ze1_data.bin

CrossOver:
~/Library/Application Support/CrossOver/Bottles/<bottle>/drive_c/Program Files (x86)/Steam/steamapps/common/Zero Escape The Nonary Games/ze1_data.bin
```

Place the private inputs at `original/999-us.nds` and
`original/ze1_data.bin`, then extract the ROM filesystem:

```sh
python3 scripts/extract_nds_romfs.py \
  original/999-us.nds work/romfs
```

This extracts the 78 stock `scr/*.fsb` files plus `etc/sound.dat`,
`sound/se_sys.se`, and the `sound/se_a01b_wake.se` voice-bank template. The
extractor intentionally skips unrelated assets whose raw NitroFS names are not
portable to Windows, and refuses a non-empty output directory.

## 2. Reconstruct the PC voice manifest

The PC archive stores encrypted payloads and case-insensitive path hashes, not
plain filenames. `pc_archive_manifest.py` decrypts the archive index using the
observed `0xFABACEDA` stream key, scans SIR1 dialogue records for message IDs,
then reconstructs paths of the form:

```text
/sound/voice/<MESSAGE_ID>_<segment>.ogg
```

Each proposed path is accepted only when its archive hash resolves to an Ogg
entry. The manifest therefore joins message ID, speaker, Japanese/English text,
source segment, archive offset, size, and audio metadata without guessing file
contents.

```sh
python3 scripts/pc_archive_manifest.py \
  original/ze1_data.bin \
  work/manifests/ze1_jp_dialogue_voice_manifest.tsv
```

Regenerate this manifest from the exact `ze1_data.bin` that the voice-bank
builder will read. The manifest contains absolute archive offsets; a manifest
from another game revision or modified installation can point at unrelated
data even when the reconstructed Ogg paths have the same names.

The reference manifest has 9,823 data rows, 2,290,693 bytes, and SHA-256
`4d3f90018c765fb99590dbe8382697d35ad8823e27808c8aace1aa872160553c`.

The English dub uses the contiguous `/sound/voice_us` archive entries. The
Japanese and English banks share the same reviewed DS target map.

## 3. Decompile the DS scripts

Extract the ROM files as shown in section 1 and decompile all 78 `scr/*.fsb`
files to sidecars named like `a01b.fsb.txt`. Do not edit these decompiled source
files. The reviewed build used ZeroEscapeScript commit
`79a9d761af952cc1776140dbca60e4d2e1568a53`.

Build the Windows x64 tool from its GPL-3.0 source with the .NET 8 SDK:

```sh
git clone https://github.com/onepiecefreak3/ZeroEscapeScript.git tools/ZeroEscapeScript
git -C tools/ZeroEscapeScript checkout 79a9d761af952cc1776140dbca60e4d2e1568a53
dotnet publish tools/ZeroEscapeScript/ZeroEscapeScript/ZeroEscapeScript.csproj \
  --configuration Release \
  --runtime win-x64 \
  --self-contained true \
  -p:PublishSingleFile=true \
  --output tools/ZeroEscapeScript-bin
```

On macOS, run the deterministic batch wrapper through the CrossOver bottle that
contains the PC game:

```sh
python3 scripts/decompile_scripts.py \
  work/romfs/scr work/romfs/scr \
  --compiler tools/ZeroEscapeScript-bin/ZeroEscapeScript.exe \
  --bottle Steam --jobs 4
```

On native Windows, the tool accepts the whole directory directly:

```powershell
.\tools\ZeroEscapeScript-bin\ZeroEscapeScript.exe -o e -f .\work\romfs\scr
```

Both commands create 78 `.fsb.txt` sidecars. The CrossOver wrapper refuses to
replace an existing decompilation so reviewed source evidence is not silently
overwritten.

Keep an untouched decompilation as the line-number authority. Never align
against a tree that already contains injected `PlaySE` or `WaitSE` statements;
those extra lines would move all later source targets.

## 4. Build the first-pass alignment

```sh
python3 scripts/align_dialogue.py \
  work/manifests/ze1_jp_dialogue_voice_manifest.tsv \
  work/romfs/scr \
  work/manifests/alignment_full.tsv \
  --max-per-line 12 \
  --scene-max-per-line a01e:050=6 \
  --rejects-tsv work/manifests/alignment_full.rejects.tsv
```

The aligner normalizes equivalent punctuation, custom glyphs, whitespace, and
control markers. It then:

1. finds speaker-and-text pairs that occur exactly once on both platforms;
2. retains a monotonic longest-increasing subsequence of those anchors;
3. aligns gaps with character 2–5 gram TF-IDF cosine scores and a weighted,
   monotonic longest-common-subsequence dynamic program;
4. emits low-confidence sequence fallbacks only as review candidates, never as
   automatic production mappings.

The monotonic constraint matters because repeated lines such as short replies
can have an excellent text score at the wrong scene. Speaker equality and
scene boundaries stop those local similarities from crossing story order.

See [ALIGNMENT.md](ALIGNMENT.md) for the confidence policy and the meaning of
the 3,303 initially rejected rows.

## 5. Select the safe baseline and extend it cautiously

`select_voice_alignment.py` accepts exact/high-confidence rows and the
explicitly approved borderline review table. It rejects speaker mismatches,
manual rejections, low-confidence fallbacks, reused message IDs, and duplicate
DS targets.

The first manual input, `borderline_voice_review.tsv`, has exactly three
tab-separated columns:

```text
pc_message_id    confidence    decision
```

It contains one `accept` or `reject` decision for every `medium` or `review`
row in `alignment_full.tsv`; confidence must match the source row. The selector
fails if a decision is missing, duplicated, or added for a non-borderline row.
The reviewed production ledger has 230 data rows and SHA-256
`7a2d6ccf5123ca09a08444587c4832a659d63b49534f79ef8d25552ad36d53e7`.

```sh
python3 scripts/select_voice_alignment.py \
  --alignment work/manifests/alignment_full.tsv \
  --review research/reviews/borderline_voice_review.tsv \
  --output work/manifests/alignment_safe.tsv
```

The selected safe manifest has SHA-256
`362951c551fc39d02cfec95e6a358fd4eb40eab2617374bd0002bc5697d3f345`.

The safe baseline contained 6,397 targets. Short-window comparison and manual
review recovered additional candidates. `build_extended_alignment.py` emits
the strict automatic manifest, its additions, and an accounting report. The
later human-review ledger is a separate input.

```sh
python3 scripts/build_extended_alignment.py
```

The strict extended manifest has SHA-256
`7d7e33553f52547dfcce75de8c7096d147dd2b771d3dba225d2c3f2057ce8bbb`;
its 420-row additions file has SHA-256
`94bad92ef024550270c662f3f3157373ced99c036f2be05045ad634e97d18ca6`.
The corresponding report records 4,206 raw candidates, 692 eligible
unambiguous candidates, 380 selected windows, and 420 added DS targets.

Reviewers then inspected unresolved candidates in complete scene order. They
checked message reuse, speaker identity, negation, quantities, direction, and
split/merge boundaries, then recorded the accepted mappings in
`research/reviews/alignment_extended_candidates.tsv`. Its required columns are
enforced by `merge_extended_alignment.py`; dialogue text is intentionally
omitted. The reviewed production ledger has 242 data rows and SHA-256
`d0971200b2a2ee88fc973fa7909737181d21d8b2fa4ea5cb4039d3aba5fb4433`.

The 242-row ledger was produced by manual scene-order review using
`alignment_full.rejects.tsv`, the PC manifest, and decompiled DS context. It is
the recorded human-decision input; the scripts can validate and apply those
decisions but cannot infer them again automatically.

All 242 rows were accepted during manual review. In the broad diagnostic merge,
conflict resolution kept 144 of them alongside 420 strict automatic additions,
producing 564 additions in total: 547 `novel_semantic` targets and 17
same-speaker targets. The final same-speaker merge discarded all 420 strict
automatic narration rows and 225 reviewed narration rows, leaving the 17
reviewed named-speaker rows.

The scripts validate and deterministically merge those judgments, but cannot
recreate a human decision from source data alone. The production merge uses
`--speaker-policy same`, deliberately discarding unnamed `NOVEL`/`HERO`
targets even when their text similarity is strong.

```sh
python3 scripts/merge_extended_alignment.py \
  --strict work/manifests/alignment_extended.tsv \
  --reviewed research/reviews/alignment_extended_candidates.tsv \
  --speaker-policy same \
  --output work/manifests/alignment_extended_dialogue_only.tsv \
  --additions-output work/manifests/alignment_extended_dialogue_only_additions.tsv \
  --report work/manifests/alignment_extended_dialogue_only_report.json
```

That policy added 17 reviewed, same-speaker targets and produced the
conservative 6,414-target baseline. The final additions file has SHA-256
`9cd74115919c9a37de038ffdb8429b9d7bd7d8f28b49d0ace8ab6f391ad9d8e7`;
the merged alignment has SHA-256
`72b4c72a598c139a38b905ef55692d0171a841c0b98665bfeca12df0cc0967b6`.
The same-speaker policy prevents Junpei's remake narration from playing during
bottom-screen DS narration.

The final review stage applies cases that must be stated explicitly rather
than inferred by a broader matching rule:

```sh
python3 scripts/apply_reviewed_alignment_overrides.py \
  --alignment work/manifests/alignment_extended_dialogue_only.tsv \
  --pc-manifest work/manifests/ze1_jp_dialogue_voice_manifest.tsv \
  --ds-scripts work/romfs/scr \
  --ledger research/reviews/reviewed_alignment_overrides.tsv \
  --output work/manifests/alignment_with_reviewed_overrides.tsv
```

`reviewed_alignment_overrides.tsv` has 65 component rows for 63 exact DS
targets, arranged into 25 multi-page groups. Two baseline targets are replaced
as part of those reviewed sequences, so the target count rises by 61 rather
than 63. Two targets combine ordered pieces from multiple PC messages. The
ledger also records ten Bed4 speaker-label equivalents, one A01 dancer-label
equivalent, and one M10 target restricted to the exact raw text bytes found in
the reviewed French translation. Its SHA-256 is
`abee0b504ab42e85e165b2e131b9d7c7cc2be687a17b129990f3406e53bf6f6a`.

The stage verifies the exact script, text ordinal, DS speaker, PC message,
PC speaker, component order, replacement identity, and both language-specific
intervals. Reused audio must cover consecutive targets with ordered,
non-overlapping cuts. A missing or changed input stops the build; no numeric
speaker suffix is removed as a general matching rule.

The ledger uses these fields:

| Field | Meaning |
| --- | --- |
| `override_group` | Stable name shared by every component in one reviewed case |
| `component_order` | Zero-based playback position within one DS target |
| `target_action` | `add`, or `replace` for an occupied baseline target |
| `replace_pc_message_id` | Exact baseline PC message required by `replace` |
| `pc_message_id` | Exact PC source message for this component |
| `ds_script`, `ds_settext_ordinal` | Exact DS destination |
| `ds_speaker`, `pc_speaker` | Exact speaker labels verified on both inputs |
| `target_text_sha256` | Optional accepted hashes of the raw `setText` payload, separated by `|` |
| `jp_start_ms`, `jp_end_ms` | Japanese source interval; blank end means the source end |
| `en_start_ms`, `en_end_ms` | English source interval; blank end means the source end |
| `review_basis` | `speaker_alias`, `multi_page_split`, `multi_page_composite`, or `translated_dialogue` |

The tracked runtime projection removes dialogue text while preserving every
field needed to build the voice banks:

```sh
python3 scripts/export_voice_alignment.py \
  --source work/manifests/alignment_with_reviewed_overrides.tsv \
  --output research/alignment/final_voice_alignment.tsv \
  --expected-rows 6475

python3 scripts/export_voice_alignment.py --check --expected-rows 6475
```

`research/alignment/final_voice_alignment.tsv` has 6,475 data rows and SHA-256
`c1c1f6fdd9100c6fb7f1e88e43ccb2473ea4becb3c9402da9c7abf43b0367acc`.
Pack generation consumes this tracked projection and does not depend on
rerunning the alignment research.

## 6. Generate DS audio resources

`build_voice_bank.py` decrypts only the mapped Ogg entries, mixes to mono,
resamples to 16,384 Hz, and encodes Nintendo DS IMA ADPCM inside validated
SIR0/SWDL/SEDL `.se` containers. Reviewed intervals are applied separately to
the Japanese and English source messages before any composite is assembled.
A newly cut edge receives a deterministic 4 ms fade; an uncut source remains
unchanged before encoding. Separate components keep a 65 ms gap. The builder
also allocates internal DSE IDs that do not collide with retail sound resources.

```sh
python3 scripts/build_voice_bank.py \
  original/ze1_data.bin \
  work/manifests/ze1_jp_dialogue_voice_manifest.tsv \
  research/alignment/final_voice_alignment.tsv \
  work/romfs/sound/se_a01b_wake.se \
  build/voices-jp \
  --language jp \
  --id-source-rom original/999-us.nds \
  --map-output build/voice_map_extended_dialogue_only.tsv \
  --symbols-output build/voice-symbols.txt

python3 scripts/build_voice_bank.py \
  original/ze1_data.bin \
  work/manifests/ze1_jp_dialogue_voice_manifest.tsv \
  research/alignment/final_voice_alignment.tsv \
  work/romfs/sound/se_a01b_wake.se \
  build/voices-en \
  --language en \
  --id-source-rom original/999-us.nds \
  --map-output build/voice-map-en.tsv \
  --symbols-output build/voice-symbols-en.txt
```

Use the manifest regenerated from the same `original/ze1_data.bin` supplied to
these commands. A normal build refuses to start if its output directory already
contains `SE_V` files. Before writing the first voice, it records
`.voice-bank-build.json` with the language, generation settings, pipeline
version, and SHA-256 identity of the alignment, PC archive, Japanese manifest,
SE template, and source ROM.

If an identical build is interrupted, rerun the same command with `--resume`.
Resume stops before generation when the sidecar is missing, invalid, or does
not match every recorded input. It also rejects voice filenames outside the
current symbol range. Each existing expected file is kept only when its full
contents match the resource regenerated from the current inputs; a partial or
damaged file is replaced. After changing any recorded input or setting, use a
new empty output directory without `--resume`.

Run only one builder per output directory, and do not edit or replace its input
files while it is running. The sidecar detects changes between runs; it does not
coordinate simultaneous writers or files modified during a run.

`--allow-narration-voices` disables the same-speaker guard and is limited to
research builds.

Sample the generated bank with `verify_voice_bank.py`, which resolves the
source clips again, rebuilds the expected PCM, decodes the generated ADPCM, and
reports signal-to-noise and structural agreement.

## 7. Inject and compile scripts

`inject_voice_calls.py` inserts this sequence at each reviewed `setText` line:

```text
Sound::PlaySE(":SE_V####", 127f, 0f);
setText("...");
Sound::WaitSE(":SE_V####");
```

`WaitSE` is required because character windows are sometimes non-blocking. A
stop immediately after `setText` truncates those voices, while no wait allows a
run of clips to overlap.

For the reviewed M10 translation-only target, the injector reconstructs the
compiler's exact raw CP932 `setText` bytes and checks their SHA-256 before
adding either call. A mismatch leaves the original line untouched and is
reported as `rejected_by_text_hash`, matching the released Rust engine's
fail-closed behavior.

```sh
python3 scripts/inject_voice_calls.py \
  build/voice_map_extended_dialogue_only.tsv work/romfs/scr build/scripts-source

python3 scripts/compile_scripts.py \
  build/scripts-source build/scripts-compiled \
  --compiler tools/ZeroEscapeScript-bin/ZeroEscapeScript.exe \
  --bottle Steam --jobs 4
```

The released Rust engine performs equivalent FSB injection directly from
`voice-profile.json`; it does not invoke Crossover at patch time.

## 8. Extend the sound registry and silence text bleeps

The generated `SE_V####` names must be appended to the existing `SE_CHUN`
category in `etc/sound.dat`. `sound_registry.py` reparses every SIR0 pointer and
relocation after rebuilding the registry.

```sh
python3 scripts/sound_registry.py patch \
  work/romfs/etc/sound.dat build/sound.dat \
  --names-file build/voice-symbols.txt
```

The original text sound would play on top of speech. `silence_text_bleeps.py`
identifies the ten routed volume operands in `sound/se_sys.se` and changes only
their `0x7f` volume byte to zero. It rejects an unknown bank layout instead of
searching and replacing similar bytes globally.

```sh
python3 scripts/silence_text_bleeps.py patch \
  work/romfs/sound/se_sys.se build/se_sys.silenced.se
```

## 9. Build runtime packs and compatibility profile

The `.se` files are already compressed; `NVPACK01` stores them byte-for-byte
with per-entry and aggregate SHA-256 values. This lets the Rust engine validate
metadata first and stream hundreds of megabytes without loading the entire
pack into memory.

```sh
python3 scripts/build_voice_pack.py \
  build/voices-jp src-tauri/resources/voices-jp.nvpack --language jp
python3 scripts/build_voice_pack.py \
  build/voices-en src-tauri/resources/voices-en.nvpack --language en
```

These two generated packs can be placed beside the tracked
`src-tauri/resources/voice-profile.json`; profile regeneration is not required
to build or use the patcher.

`build_voice_profile_resource.py` invokes the Rust profile builder. The profile
stores control-flow fingerprints, text ordinals, voice symbols, alternative
reviewed structures, and hash-pinned French pointer repairs. It intentionally
does not store translated text. The tracked compatibility manifest contains
only the 21 alternative structural hashes and five exact repair records needed
to reproduce the bundled profile.

```sh
python3 scripts/build_voice_profile_resource.py \
  --voice-map build/voice_map_extended_dialogue_only.tsv \
  --stock-romfs work/romfs

python3 scripts/build_voice_profile_resource.py --check \
  --voice-map build/voice_map_extended_dialogue_only.tsv \
  --stock-romfs work/romfs
```

`research/compatibility/voice-profile-compatibility.json` has SHA-256
`7cf04371a71b06138bfc4fa3eea6a48c04ac266de436a44d0eb28201c71c1dfd`.
To audit that derived manifest from the original French-patcher output, replace
the default with `--french-fsb-root <compiled-fsb-directory>`. The raw directory
supplies the alternative structural fingerprints and exact before-images for
the five known pointer-repair files. The remaining 12 repair locations are
derived from relocation metadata rather than a broad byte replacement.

The verified production artifacts are:

| File | SHA-256 |
| --- | --- |
| Japanese voice map | `df3caea4638a4d6457fc1bd4d8a02e7d8bf7b2597d71f74fe973e7e0743b79fd` |
| English voice map | `fddee09d40bdf9bfd9d3ff1f875ded805ea5aac48857ebc2d6b03287b3c5fa92` |
| `voice-profile.json` | `d9df04cb61b17c55a3b1681750136df43bc615c096cd6b9c70cadc7936c0ca2c` |
| `voices-jp.nvpack` | `0af77bb2af5abcd12ac1bc157ce9d61e249dfbd01aa4dfa43a3f8eaadd6c5db6` |
| `voices-en.nvpack` | `ed7db368102877734d7fa9aab1dcb7b91c9298bfdeba5da8324d2d48ffcfc734` |

The profile's target-catalogue SHA-256 is
`1539086a40a22f7ed5402a8ee4b253716491a0b25e095b132cdfbc2d2ccc4497`.
These values come from one fresh build using the reference archive manifest
above. Pack hashes identify one verified build and can differ when another
supported PC archive contains different encoded audio.

## 10. Patch, verify, and test in melonDS

The prototype `rom_build.py` can assemble a research ROM with ndspy. The
released engine instead retains original file payloads, appends replacement
payloads and voices, appends new FNT/FAT tables, updates only required metadata,
and writes a restoration receipt. The receipt stores preimages for the few
original-range edits so exact reset remains reconstructible.

Run `verify_rom.py` against research builds, then run the Rust integration
audits and melonDS scenarios in [TESTING.md](TESTING.md). The acceptance bar is:

- 6,475 contiguous voice resources and 51 patched scripts;
- no DSE ID collisions, broken SIR0 relocations, or invalid NitroFS paths;
- every original `setText` byte unchanged;
- no narration-only target in the production map;
- byte-exact reset for stock and translated inputs;
- audible JP and EN voices on hardware-equivalent emulator audio.
