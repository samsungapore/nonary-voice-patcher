# Voice-hack process

This document records the research pipeline used to derive the final
6,414-target patch. ROM and PC archive inputs are not distributed with the
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

The two human-review inputs are versioned in the repository:

```text
research/reviews/borderline_voice_review.tsv
research/reviews/alignment_extended_candidates.tsv
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

That policy added 17 reviewed, same-speaker targets and produced the final
6,414-target map. The final additions file has SHA-256
`9cd74115919c9a37de038ffdb8429b9d7bd7d8f28b49d0ace8ab6f391ad9d8e7`;
the merged alignment has SHA-256
`72b4c72a598c139a38b905ef55692d0171a841c0b98665bfeca12df0cc0967b6`.
The same-speaker policy prevents Junpei's remake narration from playing during
bottom-screen DS narration.

The tracked runtime projection removes dialogue text while preserving every
field needed to build the voice banks:

```sh
python3 scripts/export_voice_alignment.py --check
```

`research/alignment/final_voice_alignment.tsv` has 6,414 data rows and SHA-256
`192246fefa8ab2e08fe13c3fc518f0792a2f15431c4d5072680bfccb3a9a0cbd`.
Pack generation consumes this tracked projection and does not depend on
rerunning the alignment research.

## 6. Generate DS audio resources

`build_voice_bank.py` decrypts only the mapped Ogg entries, concatenates grouped
segments with 65 ms of silence, mixes to mono, resamples to 16,384 Hz, and
encodes Nintendo DS IMA ADPCM inside validated SIR0/SWDL/SEDL `.se` containers.
It allocates internal DSE IDs that do not collide with retail sound resources.

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

The reference production hashes are:

| File | SHA-256 |
| --- | --- |
| `voices-jp.nvpack` | `797b611507682e7df8b73a8d23c98f47405dc5a059c36a8e8d3b2f262c50cae9` |
| `voices-en.nvpack` | `5278646961d82e8161528db63d75ae817599746bebb89c9356d40a64d12e0c7a` |

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

The text-free voice map produced by the commands above contains 6,414 data rows
and has SHA-256
`2b0bd3dc9cb1c89f1fb86b5b32e2770567d0dd535231ec39f709b20ef7631ad6`.
The resulting public profile has SHA-256
`905bc7e27c16c9b9c3302e49425adcdb70d6b8f9b5708970c22aeb151ef9c6c5`.
Pack hashes identify a build but do not contain pack data.

## 10. Patch, verify, and test in melonDS

The prototype `rom_build.py` can assemble a research ROM with ndspy. The
released engine instead retains original file payloads, appends replacement
payloads and voices, appends new FNT/FAT tables, updates only required metadata,
and writes a restoration receipt. The receipt stores preimages for the few
original-range edits so exact reset remains reconstructible.

Run `verify_rom.py` against research builds, then run the Rust integration
audits and melonDS scenarios in [TESTING.md](TESTING.md). The acceptance bar is:

- 6,414 contiguous voice resources and 51 patched scripts;
- no DSE ID collisions, broken SIR0 relocations, or invalid NitroFS paths;
- every original `setText` byte unchanged;
- no narration-only target in the production map;
- byte-exact reset for stock and translated inputs;
- audible JP and EN voices on hardware-equivalent emulator audio.
