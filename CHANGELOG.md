# Changelog

All notable changes are documented here. The format follows Keep a Changelog;
the project uses semantic versioning for public releases.

## [Unreleased]

### Added

- A French Dubbing Studio with 6,475 ROM-derived dialogue cues, same-function
  context, native microphone capture, immutable takes, review statuses, notes,
  playback, filtering, and keyboard navigation.
- Text-free dubbing projects bound to the logical source ROM, target catalogue,
  WAV hashes, and take/processing approval hashes, with the profile hash kept
  for provenance.
- A native preview builder that resamples and encodes recorded WAV masters,
  fills missing cues with 80 ms silence, creates
  `builds/voices-fr-preview.nvpack`, and applies it to a reversible test ROM.
- Python preview and strict production export for French projects, including a
  machine-readable build report.
- French CLI application through `--language fr --voice-pack <FILE>`.
- NVPACK version 2 with an authenticated target-catalogue digest; French packs
  must use this format and match the selected profile.
- English architecture, hack-process, alignment, format, CLI, build, test,
  release, compatibility, and troubleshooting documentation.
- End-to-end CLI compilation and usage instructions for macOS, Linux, and
  Windows, including portable resource staging, inspection, patching, reset,
  JSON automation, exit codes, and troubleshooting.
- A current English desktop-interface screenshot in the README.
- Reproducible research scripts and Python unit tests.
- The reviewed baseline and extended-alignment decision ledgers required to
  reproduce the conservative 6,414-target mapping.
- A separate reviewed-override ledger that adds or replaces 63 DS targets from
  65 exact components, including 25 multi-page groups, two composite targets,
  explicit speaker aliases, and one French-only raw-text condition.
- A text-free production alignment, portable ROMFS extractor, pinned Python
  environment, and CrossOver decompilation wrapper for reproducing the hack.
- A text-free compatibility source manifest for exact `voice-profile.json`
  regeneration without the raw French-patcher files.
- CI, contribution and security policies, and legal notices.

### Changed

- The desktop window now opens at 1360×860 and supports the patcher and Dubbing
  Studio as separate workspaces.
- Dialogue context is limited to two neighboring text lines in the same script
  function.
- Project builds authenticate active WAV masters before encoding; production
  also requires every target to have an approved active take.
- Reduced interface copy to the decisions and status needed to patch a ROM.
- Refined the bilingual hero text to describe the 999 dubbing patch directly.
- Standardized backend diagnostics and code comments in English.
- Documented GUI and CLI resource layouts, including bundled and local runtime
  pack requirements.
- Release CLI builds no longer retain a builder-specific source-tree resource
  fallback.
- Updated the frontend toolchain to Vite 8, `@vitejs/plugin-react` 6, and
  TypeScript 7, and synchronized the Tauri dialog bindings at 2.7.2.
- Updated the checkout, Node.js, and Python setup actions used by CI.
- Expanded the production profile from 6,414 to 6,475 ordered voice slots while
  keeping bottom-screen narration excluded.

### Fixed

- Dubbing project paths, manifest size, WAV dimensions, gain, trim, take
  identity, and output collisions are validated before derived files are
  published.
- Project manifest updates use a cross-process lock and compare-and-swap check;
  native and Python builders retain that lock through output publication, and
  committed take links are flushed before the manifest can reference them.
- Patch outputs cannot replace the selected voice profile or voice pack, and
  Python exports authenticate the exact bounded audio bytes they encode.
- Selecting a new take or changing its bound processing values invalidates a
  stale approval instead of carrying it to a different performance.
- French voice packs cannot be applied to a reordered or unrelated voice
  catalogue.
- The result action now reveals and selects the generated ROM in Finder or
  Explorer using the permission granted to the application.
- Large hashing and voice-streaming buffers now use heap storage so the
  Windows MSVC CLI stays within the default console stack reserve.
- The Python CI job now keys its dependency cache from
  `requirements-dev.txt`.
- PC lines that span consecutive DS text boxes are cut at reviewed,
  language-specific boundaries. Newly cut edges receive a deterministic 4 ms
  fade, and composite targets keep a 65 ms gap between source messages.
- Speaker-name differences are accepted only for the exact reviewed DS target
  and PC message pair; numeric suffixes are never removed globally.
- Interrupted voice-bank builds can resume only when an atomic provenance
  record matches every source input and generation setting; normal builds
  reject stale `SE_V` files instead of overwriting or mixing them.

### Removed

- The experimental multi-profile save modification and every related GUI, CLI,
  resource, migration, and test path.
- The framed Zero robot overlay from the hero banner.
