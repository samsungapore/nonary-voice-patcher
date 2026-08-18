# French Dubbing Studio

The Dubbing Studio turns the existing 999 voice-injection pipeline into a
line-by-line French recording workflow. It reads the dialogue and nearby
context from a compatible Nintendo DS ROM, keeps the recorded masters in a
separate project folder, and exports a French voice pack that the patcher can
apply to a test ROM.

The voice catalogue contains 6,475 dialogue-only cues. Narration and other
non-recordable text may appear as context, but they are not assigned a voice
slot and are never turned into recording targets.

One M10 cue is tied to the exact raw text used by the reviewed French
translation. It remains part of the ordered recording catalogue, but the
patcher skips its voice on a ROM whose text does not match that condition.

## Prerequisites

For the desktop workflow, you need:

- a compatible USA 999 ROM or a compatible translation based on it;
- a build of Nonary Voice Patcher that includes the Dubbing Studio;
- a microphone available to the operating system;
- permission for the application to use that microphone;
- enough free space for the project WAV masters and generated test ROMs.

For the Python export fallback, also install:

- Python 3.11 or newer;
- the dependencies from `requirements-dev.txt`;
- the headless patcher CLI, if you want to turn the generated pack into a ROM.

Run repository commands below from the repository root.

## Create a project

1. Open **Dubbing Studio** in the desktop application.
2. Select **New project**.
3. Enter a project name.
4. Select the reference ROM.
5. Select a new, dedicated project folder.
6. Create the project and wait for the 6,475 cues to be indexed.

Use a dedicated folder rather than a general documents or downloads folder.
The studio creates `project.nvdub.json` and a `recordings` tree inside the
selected location. It refuses to replace an existing project manifest.

The project is bound to the exact logical source ROM and the ordered target
catalogue. It also records the bundled profile-file hash for provenance. A ROM
produced by this patcher can be selected in the desktop app when it contains a
valid restoration receipt: the clean base is restored in memory before its hash
and scripts are checked.

## Open an existing project

Select **Open project**, then choose:

1. the folder containing `project.nvdub.json`;
2. the same reference ROM, or another byte-identical copy of its logical base.

Opening a project validates the ROM hash, compatible profile structure, target
catalogue, manifest, every take path, each WAV file, and each recorded SHA-256.
A project created with another ROM revision or an incompatible target catalogue
does not open.

The ROM remains an external input. Moving the project folder is safe; moving or
renaming individual files below `recordings` is not.

## Work through the script

The cue list can be searched by text, speaker, script location, or `SE_V####`
symbol and filtered by status. Selecting a cue displays:

- the speaker and line to record;
- nearby decoded script lines;
- whether each context line is recordable or context only;
- the script path, function, ordinal, and stable target symbol;
- all saved takes, the active take, direction notes, and review status.

Context contains up to two preceding and two following text lines from the
same script function. It never crosses a function boundary. This is useful for
delivery and continuity, but it is not a branch-aware reconstruction of the
whole scene.

Keyboard shortcuts are disabled while typing in a field:

| Shortcut | Action |
| --- | --- |
| `R` | Start recording; press again to stop and save the take |
| `Space` | Play or pause the active take |
| Left or Up arrow | Select the previous cue in the current filtered list |
| Right or Down arrow | Select the next cue in the current filtered list |

## Record and review takes

Choose the microphone before recording. The studio records through the native
audio input API, downmixes the selected device input to mono, and stores a
16-bit PCM WAV at the device sample rate. A take is limited to 45 seconds; the
interface normally stops slightly earlier. Empty, interrupted, overlong, or
overflowed captures are discarded. Clipping is reported with the saved take so
it can be reviewed.

Each successful recording creates a new take and makes it active. Earlier
takes remain available and can be played or selected again. The application
never edits a committed WAV master in place. Selecting another take returns the
cue to **Recorded**, because an approval applies to one specific performance.

Use the statuses consistently:

| Status | Meaning |
| --- | --- |
| **Missing** | No accepted recording has been made yet. |
| **Recorded** | An active take exists and has not completed review. New recordings enter this state automatically. |
| **Needs review** | An active take exists, but direction, quality, pronunciation, or continuity still needs a decision. |
| **Approved** | The active take is accepted for a production export. Approval is bound to the take hash and current trim/gain settings. |
| **Skipped** | Work is intentionally deferred. It remains valid for preview work but does not satisfy a production export. |

Add direction or continuity notes before changing cues, then save the line
details. **Recorded**, **Needs review**, and **Approved** all require an active
take. Changing the active take or processing settings invalidates the previous
approval.

## Project contents and integrity

A normal project has this layout:

```text
999-french-dub/
├── .project.nvdub.lock
├── project.nvdub.json
├── recordings/
│   ├── SE_V0000/
│   │   ├── take-<timestamp>-000.wav
│   │   └── take-<timestamp>-001.wav
│   ├── SE_V0001/
│   │   └── take-<timestamp>-000.wav
│   └── ...
└── builds/
    └── voices-fr-preview.nvpack
```

`builds` is created when an export is made and may be deleted and regenerated.
The hidden lock file serializes local manifest changes and contains no project
content. Native and Python exports keep this lock from manifest validation
through the last atomic output publication, so another local process cannot
turn a successful export into a stale snapshot. Project edits wait for the
export to finish. Do not remove the lock file while the project is open.
Temporary `.capture-*.wav` files are used only while a recording is in
progress. The native preview builder does not write a JSON sidecar.

`project.nvdub.json` persists project state, not game content. It contains:

- the project name and timestamps;
- source ROM filename, game code, and SHA-256;
- voice-profile and target-catalogue SHA-256 values;
- stable target IDs and `SE_V####` symbols;
- statuses, notes, active-take selection, and processing settings;
- take paths, durations, sample rates, level statistics, and SHA-256 values;
- an integrity value for each approval.

The manifest does **not** contain the ROM, script text, surrounding dialogue,
or a copy of the voice profile. Script text and context are decoded again from
the selected ROM each time the project is opened.

Treat committed WAV files as immutable masters. Editing, replacing, or
transcoding one outside the studio changes its hash and causes project
validation to fail. Make a new take instead.

## Build a preview test ROM

The native desktop build path performs the preview export and applies it to a
separate test ROM. It uses each cue's active take and leaves the project WAV
masters unchanged. A cue without an active take receives 80 ms of silence, so
the 6,475-entry voice bank remains contiguous and the partial project can be
tested in game. Preview builds may include active takes that are not approved.

After at least one take has been recorded, select **Build test ROM**, choose a
new `.nds` destination, and wait for encoding, pack verification, ROM patching,
and final verification to finish. The derived pack is replaced atomically at
`builds/voices-fr-preview.nvpack`; the ROM is written at the destination you
selected. The manifest remains locked until both outputs are complete. **Show**
reveals the completed ROM in Finder or Explorer.

Recording, resampling, Nintendo DS audio encoding, pack creation, and ROM
patching are native in the desktop build. This path does not invoke Python,
FFmpeg, or the Python audio dependencies.

The output ROM contains a restoration receipt and can later be reset or used as
input to another patch operation. The native builder also accepts a ROM already
patched by this tool: it verifies and restores the receipt-bearing base before
building. Keep the reference ROM and test-ROM output as separate files.

## Build a French pack with Python

The Python path is useful for automation and for checking the same project
outside the desktop application.

Create the environment on macOS or Linux:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -r requirements-dev.txt
```

On Windows PowerShell, activate it with:

```powershell
py -3.11 -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -r requirements-dev.txt
```

Build a preview pack:

```sh
python scripts/build_dubbing_project.py \
  "/absolute/path/to/999-french-dub" \
  "/absolute/path/to/reference-restored.nds" \
  "/absolute/path/to/999-french-dub/builds/voices-fr-preview-python.nvpack" \
  --profile "src-tauri/resources/voice-profile.json" \
  --scope preview
```

The positional arguments are, in order:

1. the folder containing `project.nvdub.json`;
2. the exact clean or restored ROM used to create the project;
3. the output `NVPACK01` file.

The fallback builder compares the literal ROM file SHA-256 with the project.
Unlike the desktop project loader, it does not restore a patched ROM in memory.
If necessary, reset a receipt-bearing ROM first:

```sh
src-tauri/target/release/nonary-voice-patcher-cli \
  reset "/absolute/path/to/patched.nds" \
  "/absolute/path/to/reference-restored.nds"
```

The builder converts active masters to mono 16,384 Hz PCM, applies the stored
trim and gain, creates disposable DS `.se` derivatives, and writes an
authenticated French pack bound to the project's target catalogue. Preview
uses 80 ms silence for every missing active take. A JSON report is written by
appending `.json` to the pack filename. For the example above, it is
`voices-fr-preview-python.nvpack.json`. This report belongs only to the Python
workflow; the native Studio build does not create one.

Apply the generated pack with the CLI:

```sh
src-tauri/target/release/nonary-voice-patcher-cli \
  --resources-dir "src-tauri/resources" \
  apply "/absolute/path/to/reference-restored.nds" \
  "/absolute/path/to/999-french-preview.nds" \
  --language fr \
  --voice-pack "/absolute/path/to/999-french-dub/builds/voices-fr-preview-python.nvpack"
```

`--resources-dir` must contain the matching `voice-profile.json`. French uses
the explicit `--voice-pack`; Japanese and English packs are not required for
this command. French packs are NVPACK version 2 and embed the target-catalogue
digest; the patcher rejects an older or differently ordered pack.

If the CLI has not been built yet:

```sh
cargo build \
  --manifest-path src-tauri/Cargo.toml \
  --release \
  --locked \
  --no-default-features \
  --features cli \
  --bin nonary-voice-patcher-cli
```

## Build a production pack

Production scope is deliberately strict. Every one of the 6,475 targets must
have an active take and status **Approved**. **Missing**, **Recorded**, **Needs
review**, and **Skipped** all block the build; production never substitutes
silence. Invalid trims, unsafe audio dimensions, changed WAV hashes, stale
approval digests, and post-gain clipping also block production.

```sh
python scripts/build_dubbing_project.py \
  "/absolute/path/to/999-french-dub" \
  "/absolute/path/to/reference-restored.nds" \
  "/absolute/path/to/999-french-dub/builds/voices-fr-production.nvpack" \
  --profile "src-tauri/resources/voice-profile.json" \
  --scope production
```

Apply the resulting pack with the same CLI command shown above, changing only
the pack and output paths.

## Backups and collaboration

Back up the whole project folder, including `project.nvdub.json` and
`recordings`, while no recording is in progress. The manifest is replaced
atomically, and local application instances serialize their writes. Copies on
different computers or cloud-sync clients still do not form a multi-writer
database.

For a team workflow:

- designate one current project copy as authoritative;
- avoid opening and saving the same project from two machines at once;
- exchange complete project snapshots rather than loose WAV files;
- verify that the recipient has the matching reference ROM and voice profile;
- keep generated `builds` outputs separate from irreplaceable masters;
- run a preview build after importing a collaborator's snapshot.

Cloud-sync folders can work when only one person writes at a time. Wait for the
sync to finish before opening the project on another machine. For long-term
archives, store a checksum of the entire project backup alongside it.

## Troubleshooting

### The microphone list is empty

Grant microphone access to Nonary Voice Patcher in the operating-system
privacy settings, reconnect the device, restart the application, and refresh
the device list. Confirm that another application is not holding the device in
an exclusive mode.

### A take is discarded when recording stops

The native recorder rejects empty captures, input-stream failures, writer
overflow, and recordings beyond 45 seconds. Shorten the take, close heavy audio
applications, and record again. Clipping is reported rather than hidden; lower
the input gain and replace a clipped performance.

### The project requires another ROM hash

Select the same clean/restored ROM used at project creation. A filename match
is not enough. If the available ROM was patched by this patcher, reset it to a
new file and use that restored output.

### A WAV fails its integrity check

Restore the complete project from backup. If only that performance is damaged,
record a new take in the studio. Do not update the manifest hash by hand.

### Production reports missing or unapproved targets

Filter the studio by **Missing**, **Recorded**, **Needs review**, and
**Skipped**. Production succeeds only when all 6,475 cues have an active,
approved take.

### The Python builder cannot import an audio module

Activate the project virtual environment and install `requirements-dev.txt`.
The builder requires `numpy`, `soundfile`, `soxr`, and `ndspy` through that
requirements file.

### The generated pack is rejected by the CLI

Use a `voice-profile.json` with the same target catalogue for project creation,
pack export, and ROM patching; the bundled file is the normal choice. The French
pack embeds its language and catalogue digest, and a mismatch is rejected before
the output ROM is published.

## Privacy and distribution

Project data stays on the local filesystem. The studio does not upload the ROM,
script context, microphone audio, notes, hashes, or generated pack.

Record only performers who have agreed to the project and its intended release
terms. Keep agreements and credit names with the production archive. Do not
commit or publish ROM files, game-derived assets, private voice packs, or
recording masters unless the project has the rights to distribute them. The
source project can be shared without embedding script text or ROM data; each
collaborator supplies their own compatible ROM.
