# Dialogue alignment

The PC remake and DS game do not expose a shared numeric dialogue key. The PC
archive has message IDs and voiced segments; the DS scripts have ordered
`setSpeaker`/`setText` operations. The mapping is therefore evidence-based and
monotonic rather than a filename lookup.

## Direct matches

A direct match is not based on the Japanese waveform. It is established from
the English text and speaker metadata present beside the PC Japanese voice:

1. normalize platform-specific glyphs, punctuation, controls, case, and space;
2. pair the normalized text with the speaker;
3. require that pair to occur exactly once in both the PC scene and DS script;
4. retain only a longest-increasing sequence in story order.

These `unique_exact_anchor` rows have score `1.0`. The increasing-subsequence
step is essential: a pair can be unique inside extracted data yet still land
outside the physically corresponding DS scene if scene numbering differs.

## Approximate comparison

Unmatched gaps between exact anchors are compared with character TF-IDF:

- analyzer: character n-grams inside word boundaries;
- n-gram sizes: 2 through 5;
- sublinear term frequency and L2 normalization;
- pass-one minimum cosine: `0.34`;
- speaker equality is mandatory;
- matches must remain monotonic inside the anchored gap.

A weighted LCS dynamic program maximizes the sum of squared cosine scores. The
square rewards a few strong matches over many weak matches. Sequence similarity
and exact-normalized equality are recorded separately so review does not rely
on one opaque score.

When the remake splits one DS line into several messages, the extension pass
compares windows of up to four consecutive PC messages against up to three DS
lines. Default automatic thresholds are:

| Policy | Combined cosine | Sequence ratio |
| --- | ---: | ---: |
| Same named speaker | 0.85 | 0.80 |
| Unnamed `NOVEL` semantic candidate | 0.93 | 0.90 |
| Each local partition | 0.72 | 0.65 |

Repeated or near-tied windows are rejected through an ambiguity margin even if
their absolute score is high.

## Confidence and review

The full table contains exact, high, medium/review, and low/fallback rows. The
safe selector accepts strong rows automatically, requires a complete explicit
decision for every borderline row, and rejects:

- low-confidence sequence fallbacks;
- speaker mismatches;
- manual rejections;
- duplicate DS targets;
- reused PC message IDs.

This fail-closed policy means missing review data is an error, not an implicit
acceptance.

## What the 3,303 rejected rows mean

The initial full alignment had 9,700 candidate rows. The safe baseline retained
6,397, leaving 3,303 rows outside production. They are **not** simply “3,303
voiced remake lines that have no DS equivalent.” They are rejected alignment
rows, a mixed population that includes:

- low-confidence sequence placements;
- repeated text with no unambiguous occurrence;
- remake/DS split and merge differences;
- lines whose speaker evidence disagrees;
- remake narration or inner monologue mapped to unnamed DS prose;
- additional or reordered remake material;
- candidates displaced by a stronger one-target/one-message assignment.

An excluded row may have audio and a plausible DS sentence while still lacking
enough evidence to inject it safely.

## Extended-coverage experiment

The automatic short-window pass considered 4,206 raw candidates and found 692
eligible unambiguous candidates. Score and ambiguity gates selected 380 windows,
which produced 420 DS targets because one accepted window can cover more than
one DS line. A broad diagnostic merge added 144 non-conflicting reviewed
targets, producing a 6,961-target coverage experiment.

That broad profile was rejected for release because 547 of its 564 additions
used the `novel_semantic` policy. In practice, this voiced Junpei during
bottom-screen narration, reproducing a behavior the DS version never had.

The final merge reran the accepted set with `--speaker-policy same`:

| Set | Targets | Added targets | Speaker policy |
| --- | ---: | ---: | --- |
| Safe baseline | 6,397 | — | Named-speaker safe set |
| Broad experiment | 6,961 | 564 | Same + `NOVEL` semantic |
| Conservative dialogue baseline | 6,414 | 17 | Exact same speaker only |
| Production profile | 6,475 | 61 net | Exact reviewed targets; no narration |

The first 17 additions are reviewed named-character dialogue. No `NOVEL` or
`HERO` target is permitted in the production voice bank.

## Reviewed dialogue overrides

Some voiced PC messages span two consecutive DS text boxes, and a few speaker
labels differ without changing the character. These cases cannot be accepted
by weakening the automatic matcher globally. They are recorded instead in
`research/reviews/reviewed_alignment_overrides.tsv` as exact PC-message and DS
target pairs.

The ledger contains 65 ordered components for 63 DS targets. Two targets
replace an existing baseline assignment, so applying the ledger to the 6,414
target conservative baseline produces 6,475 targets. Its reviewed cases are:

- 25 groups in which one voiced PC message spans consecutive DS text boxes;
- two composite targets, in A41d and Aed1, assembled from ordered pieces of
  more than one PC message;
- ten Bed4 speaker-label equivalents and one A01 dancer-label equivalent;
- one M10 target enabled only when the raw `setText` payload matches the
  reviewed French translation.

Japanese and English use separate millisecond boundaries. The builder converts
each boundary to a deterministic 16,384 Hz sample index, applies a 4 ms fade
only at newly cut edges, and inserts 65 ms of silence between components of a
composite target. Audio without a reviewed cut remains byte-identical before DS
encoding.

The override stage stops on an unknown target, changed speaker, missing PC
message, occupied add target, unexpected replacement, non-consecutive reuse,
overlapping interval, or malformed raw-text hash. Speaker equivalents are tied
to one exact target and message; the pipeline never removes numeric suffixes
from speaker names globally.

## Reproducing the reports

Run the alignment commands in [HACK_PROCESS.md](HACK_PROCESS.md). The scripts
write deterministic TSV output and SHA-256-stamped JSON accounting reports.
The reviewed decisions are versioned under `research/reviews/` with row counts
and SHA-256 values recorded in that directory's README.
