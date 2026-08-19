# Production voice alignment

`final_voice_alignment.tsv` is the runtime-facing projection of the reviewed
6,475-target map. It contains DS targets, PC message identifiers, Japanese Ogg
paths, speakers, confidence, exact raw-text conditions, and reviewed audio-cut
metadata, but no dialogue text.

| File | Data rows | SHA-256 |
| --- | ---: | --- |
| `final_voice_alignment.tsv` | 6,475 | `c1c1f6fdd9100c6fb7f1e88e43ccb2473ea4becb3c9402da9c7abf43b0367acc` |

`scripts/build_voice_bank.py` consumes this file directly when generating the
Japanese or English voice banks. The three ledgers under `research/reviews/`
preserve the human decisions behind the final map.

After rerunning the complete alignment research pipeline, validate the tracked
projection with:

```sh
python3 scripts/apply_reviewed_alignment_overrides.py \
  --alignment work/manifests/alignment_extended_dialogue_only.tsv \
  --pc-manifest work/manifests/ze1_jp_dialogue_voice_manifest.tsv \
  --ds-scripts work/romfs/scr \
  --ledger research/reviews/reviewed_alignment_overrides.tsv \
  --output work/manifests/alignment_with_reviewed_overrides.tsv

python3 scripts/export_voice_alignment.py --check --expected-rows 6475
```

The PC manifest must be regenerated from the same `ze1_data.bin` used to build
the Japanese bank because its archive offsets are input-specific.
