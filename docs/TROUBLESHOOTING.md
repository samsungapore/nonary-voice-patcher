# Troubleshooting

## “resources were not found”

The profile or selected voice pack is absent. Pass an explicit directory:

```sh
nonary-voice-patcher-cli --resources-dir /absolute/path/to/resources \
  apply source.nds output.nds --language jp
```

`inspect` needs only `voice-profile.json`; `apply` also needs the requested
`voices-jp.nvpack` or `voices-en.nvpack`. Voice packs are excluded by
`.gitignore`.

## The ROM is unsupported

Confirm that it is the US `BSKE` release and inspect the CLI detail. A
translation may alter control flow even when the title screen looks normal.
Do not add its hash to the profile until every affected FSB has been reviewed
and the full text-preservation audit passes.

## A French ROM is rejected

Apply the French patch to a clean US ROM first, then apply voices. Compare its
SHA-256 with the reviewed value in [COMPATIBILITY.md](COMPATIBILITY.md). A newer
French-patcher build may need new structural fingerprints and exact repair
preimages; never bypass those checks with global byte replacement.

## “legacy voice patch” is detected

The ROM already contains generated voice files but no valid restoration
receipt. Return to the clean or translated pre-voice ROM. This tool cannot
prove how a legacy image was built and will not stack onto it.

## Output path equals input

Choose another file. The patcher resolves aliases, symbolic links, relative
paths, and case variants where the platform permits, so changing only the
spelling is not enough.

## Reset says no patch was detected

Reset accepts only a complete, unmodified receipt produced by this engine. A
later patcher, trimmer, or ROM manager may have removed the appended marker or
changed the signature pointer. In that state, reset requires the original
pre-voice ROM because receipt metadata cannot be reconstructed safely.

## The voice pack is corrupt or has the wrong language

Rebuild it with `scripts/build_voice_pack.py` from the matching generated bank.
Do not rename an EN pack to the JP filename: language, entry-table hash,
payload hash, entry count, and every entry digest are independently checked.

## Voices are missing or overlap

Verify that:

- the profile and pack both contain 6,475 contiguous entries;
- scripts contain matching `SE_V####` operations;
- `etc/sound.dat` contains the complete symbol sequence;
- internal DSE IDs do not collide with retail resources;
- each injected line has `PlaySE`, `setText`, then `WaitSE`;
- emulator audio is enabled and not muted.

Run `scripts/verify_rom.py` and the first-voice melonDS scenario from
[TESTING.md](TESTING.md).

## Junpei speaks during bottom-screen narration

The build is using the broad experimental alignment or a bank created with
`--allow-narration-voices`. Rebuild the conservative alignment with
`--speaker-policy same`, apply `reviewed_alignment_overrides.tsv`, and export
the tracked `final_voice_alignment.tsv`. Do not enable the narration override.
The production profile must report exactly 6,475 targets.

## Japanese source audio does not decrypt

Regenerate `ze1_jp_dialogue_voice_manifest.tsv` from the exact
`ze1_data.bin` passed to `build_voice_bank.py`. The manifest stores archive
offsets, so a manifest from another game revision or modified installation can
resolve a familiar path to the wrong bytes. Resume accepts only the exact
archive, manifest, alignment, template, source ROM, language, and generation
settings recorded in `.voice-bank-build.json`. If any of them changed, use an
empty voice output directory and omit `--resume`.

## Text bleeps still play over voices

Verify `sound/se_sys.se` with:

```sh
python3 scripts/silence_text_bleeps.py verify path/to/se_sys.se
```

A mixed or unknown bank must be investigated structurally. Do not search the
file for every `E0 7F` byte; only the ten verified `SE_SYS_MESS_*` routes are
safe targets.

## Script compilation fails under Crossover

Confirm the compiler path, bottle name, and that the source is the untouched
decompilation plus one injection pass. `compile_scripts.py --jobs 1` provides a
simpler diagnostic order. Crossover/Wine is used only for research compilation;
released patching performs direct Rust FSB injection.

## macOS blocks the application

Unsigned or ad-hoc-signed macOS builds may trigger Gatekeeper. Local builds can
be opened through Finder or System Settings. Developer ID signing and
notarization prevent the standard Gatekeeper warning for distributed builds.

## JSON integration hangs

Read stdout and stderr concurrently. Progress is written to stderr while the
single final envelope is written to stdout; waiting on one full pipe before
reading the other can deadlock an embedding process.
