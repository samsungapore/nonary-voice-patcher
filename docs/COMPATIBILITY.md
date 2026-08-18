# Compatibility

## Supported base

The profile targets the US Nintendo DS release identified by game code `BSKE`.
The reviewed retail image has SHA-256:

```text
05843a3ee682bc388299ce35b9347d9d1bdda4d3890fcffbde6de785a5de5e15
```

The engine does not rely on that whole-ROM hash alone. It validates NDS
metadata and every affected script's structural fingerprint, allowing reviewed
translations that change text but preserve the required control flow.

## French translation workflow

The validated order is:

1. apply the French translation to a clean US ROM;
2. inspect the translated result with Nonary Voice Patcher;
3. apply Japanese or English voices to that result.

The reviewed raw French output has SHA-256:

```text
a89d7756ce0a9216f9ff0e157fd56a4f0b22ac9f778bd7ddcd27287b3903f818
```

That translation's legacy global accent-marker substitution damages 17 pointer
fields. Twelve are recovered while structurally parsing the voiced scripts.
Five occur in other scripts and are repaired only when path, complete damaged
file hash, offset, and expected bytes all match the profile. No global byte
substitution is performed.

The engine then confirms that all 14,610 parsed text entries are byte-identical
to the translated input. Reset reconstructs the raw French-patcher output,
including its original pointer bytes, because reversibility takes precedence
over retaining forward-only repairs.

One reviewed M10 voice target exists only for this translation. The profile
stores the SHA-256 of the exact raw `setText` payload bytes. That voice is
injected only when the payload matches; the same slot is safely skipped on the
stock US text or an unreviewed translation.

Applying the voice patch first and then running an unrelated translation
patcher is unsupported. A later tool may rewrite headers, truncate the appended
receipt, or treat added files as an unexpected ROM layout.

## Other translations and modifications

A modified ROM can pass when all of these remain true:

- game code is `BSKE`;
- the NDS header, FNT, FAT, and referenced data are structurally valid;
- all 51 affected scripts match a reviewed structural fingerprint;
- each script has the expected number and order of `setText` operations;
- `etc/sound.dat` and `sound/se_sys.se` match supported structures;
- no generated `sound/se_v0000.se` file exists without a valid receipt;
- voice insertion would not shift an overlay's raw FAT reference.

Text, graphics, music, and unrelated NitroFS files may differ. A script-control
change is intentionally rejected even if its displayed text looks correct,
because the same ordinal could now represent another speaker or scene.

## Already patched ROMs

A ROM produced by this tool is inspectable as `japanese` or `english`. Applying
again restores the verified base internally and creates one fresh patch, so JP
and EN can be switched without nesting receipts. `reset` writes the exact base.

A legacy voice-patched ROM with `se_v####.se` files but no valid receipt is
rejected. The engine cannot prove what its original bytes were or safely
distinguish compatible script injections.

## Hardware and emulator limits

The Nintendo DS file format uses 32-bit offsets and a maximum 512 MiB ROM image.
The planner updates the device-capacity byte and CRC16 and rejects any plan over
that limit. Production testing uses melonDS, but the output is standard NDS
NitroFS and does not depend on emulator-specific filesystem hooks.
