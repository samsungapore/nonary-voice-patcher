# Reviewed alignment inputs

These TSV files preserve the human decisions used by the deterministic
alignment pipeline:

- `borderline_voice_review.tsv` contains one accept/reject decision for every
  medium-confidence or review-confidence row in the full alignment;
- `alignment_extended_candidates.tsv` contains the manually reviewed
  extension candidates considered after the safe baseline;
- `reviewed_alignment_overrides.tsv` records exact speaker equivalents,
  multi-page audio cuts and composites, and one translation-specific target.

The conservative merge applies `--speaker-policy same`, so only 17
named-speaker rows from the extended ledger enter its 6,414-target output.
Narration rows remain in the evidence ledger but are filtered from production.
The reviewed-override stage then adds or replaces 63 exact DS targets from 65
ordered components. Two replacements make the final total 6,475.

| File | Data rows | SHA-256 |
| --- | ---: | --- |
| `borderline_voice_review.tsv` | 230 | `7a2d6ccf5123ca09a08444587c4832a659d63b49534f79ef8d25552ad36d53e7` |
| `alignment_extended_candidates.tsv` | 242 | `d0971200b2a2ee88fc973fa7909737181d21d8b2fa4ea5cb4039d3aba5fb4433` |
| `reviewed_alignment_overrides.tsv` | 65 | `abee0b504ab42e85e165b2e131b9d7c7cc2be687a17b129990f3406e53bf6f6a` |

`scripts/select_voice_alignment.py`, `scripts/merge_extended_alignment.py`,
and `scripts/apply_reviewed_alignment_overrides.py` use these tracked paths by
default.
The complete reconstruction sequence is documented in
[`docs/HACK_PROCESS.md`](../../docs/HACK_PROCESS.md).

All three ledgers are text-free. They preserve decisions and stable identifiers
needed by the pipeline without redistributing dialogue excerpts.

The override ledger contains 25 multi-page groups, including two targets that
combine ordered pieces from multiple PC messages. It also contains ten exact
Bed4 speaker-label equivalents, one exact A01 dancer-label equivalent, and one
M10 entry restricted to the SHA-256 of the reviewed French raw `setText`
payload. Every row records a closed review basis; missing targets, changed
speakers, conflicting replacements, overlapping cuts, and malformed hashes are
rejected instead of being guessed.
