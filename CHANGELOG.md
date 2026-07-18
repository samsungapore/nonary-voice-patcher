# Changelog

All notable changes are documented here. The format follows Keep a Changelog;
the project uses semantic versioning for public releases.

## [Unreleased]

### Added

- English architecture, hack-process, alignment, format, CLI, build, test,
  release, compatibility, and troubleshooting documentation.
- End-to-end CLI compilation and usage instructions for macOS, Linux, and
  Windows, including portable resource staging, inspection, patching, reset,
  JSON automation, exit codes, and troubleshooting.
- A current English desktop-interface screenshot in the README.
- Reproducible research scripts and Python unit tests.
- The reviewed baseline and extended-alignment decision ledgers required to
  reproduce the final 6,414-target mapping.
- A text-free production alignment, portable ROMFS extractor, pinned Python
  environment, and CrossOver decompilation wrapper for reproducing the hack.
- A text-free compatibility source manifest for exact `voice-profile.json`
  regeneration without the raw French-patcher files.
- CI, contribution and security policies, and legal notices.

### Changed

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

### Fixed

- The result action now reveals and selects the generated ROM in Finder or
  Explorer using the permission granted to the application.
- Large hashing and voice-streaming buffers now use heap storage so the
  Windows MSVC CLI stays within the default console stack reserve.
- The Python CI job now keys its dependency cache from
  `requirements-dev.txt`.

### Removed

- The experimental multi-profile save modification and every related GUI, CLI,
  resource, migration, and test path.
- The framed Zero robot overlay from the hero banner.
