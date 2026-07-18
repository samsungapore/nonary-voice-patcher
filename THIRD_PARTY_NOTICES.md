# Third-party notices

This repository uses third-party libraries through Cargo, npm, and Python.
Their licenses remain their authors' licenses; consult `src-tauri/Cargo.lock`,
`package-lock.json`, and the installed Python package metadata for the exact
dependency graph of a build.

Research and asset-generation workflows can also use these external tools:

- [999-tools](https://github.com/PhoenixBound/999-tools), MIT License, for
  decoding selected DS background resources.
- [ZeroEscapeScript](https://github.com/onepiecefreak3/ZeroEscapeScript), GPL-3.0,
  for decompiling and compiling Spike Chunsoft script files.
- [999_nds_french](https://gitlab.com/ekyard/999_nds_french) or a compatible
  translation toolchain, under its own terms, for French-patch compatibility
  research.
- `vgmstream-cli`, FFmpeg, melonDS, and Crossover/Wine, each under its own
  license, for local extraction, conversion, compilation, and testing.

No source from those projects is vendored here. Installing or invoking an
external program does not relicense it under this project's MIT license.
