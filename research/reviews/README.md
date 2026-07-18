# Reviewed alignment inputs

These TSV files preserve the human decisions used by the deterministic
alignment pipeline:

- `borderline_voice_review.tsv` contains one accept/reject decision for every
  medium-confidence or review-confidence row in the full alignment;
- `alignment_extended_candidates.tsv` contains the manually reviewed
  extension candidates considered after the safe baseline.

The production merge applies `--speaker-policy same`, so only 17 named-speaker
rows from the extended ledger enter the final 6,414-target map. Narration rows
remain in the evidence ledger but are filtered from production output.

| File | Data rows | SHA-256 |
| --- | ---: | --- |
| `borderline_voice_review.tsv` | 230 | `7a2d6ccf5123ca09a08444587c4832a659d63b49534f79ef8d25552ad36d53e7` |
| `alignment_extended_candidates.tsv` | 242 | `d0971200b2a2ee88fc973fa7909737181d21d8b2fa4ea5cb4039d3aba5fb4433` |

`scripts/select_voice_alignment.py` and
`scripts/merge_extended_alignment.py` use these tracked paths by default.
The complete reconstruction sequence is documented in
[`docs/HACK_PROCESS.md`](../../docs/HACK_PROCESS.md).

Both ledgers are text-free. They preserve the decisions and stable identifiers
needed by the pipeline without redistributing dialogue excerpts.
