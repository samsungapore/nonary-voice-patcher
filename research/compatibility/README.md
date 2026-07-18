# Voice-profile compatibility input

`voice-profile-compatibility.json` is the text-free source manifest used to
regenerate the bundled `voice-profile.json`. It records 21 reviewed alternative
script-structure hashes and five exact pointer-repair profiles for the supported
French translation.

| File | SHA-256 |
| --- | --- |
| `voice-profile-compatibility.json` | `7cf04371a71b06138bfc4fa3eea6a48c04ac266de436a44d0eb28201c71c1dfd` |

The normal builder consumes this manifest directly. A maintainer with the raw
French-patcher FSB output can independently audit it with the
`--french-fsb-root` option documented in
[`docs/HACK_PROCESS.md`](../../docs/HACK_PROCESS.md).
