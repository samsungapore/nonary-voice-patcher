# Runtime resources

`voice-profile.json` is versioned because it contains compatibility metadata,
script ordinals, structural hashes, and exact repair rules, but no ROM or voice
payloads.

The two runtime voice packs are absent from Git:

```text
voices-jp.nvpack
voices-en.nvpack
```

Voice packs are generated locally from the PC release using
[`docs/HACK_PROCESS.md`](../../docs/HACK_PROCESS.md). Place them here for GUI
builds, or select another resource directory with the CLI's `--resources-dir`
option.

French packs are project outputs, not installed resources. The Dubbing Studio
writes `builds/voices-fr-preview.nvpack`, and the CLI accepts a French pack
explicitly with `apply ... --language fr --voice-pack FILE`.
