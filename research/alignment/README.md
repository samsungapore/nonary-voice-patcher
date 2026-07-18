# Production voice alignment

`final_voice_alignment.tsv` is the runtime-facing projection of the reviewed
6,414-target map. It contains DS targets, PC message identifiers, Japanese Ogg
paths, speakers, confidence, and selection policy, but no dialogue text.

| File | Data rows | SHA-256 |
| --- | ---: | --- |
| `final_voice_alignment.tsv` | 6,414 | `192246fefa8ab2e08fe13c3fc518f0792a2f15431c4d5072680bfccb3a9a0cbd` |

`scripts/build_voice_bank.py` consumes this file directly when generating the
Japanese or English voice banks. The two ledgers under `research/reviews/`
preserve the human decisions behind the final map.

After rerunning the complete alignment research pipeline, validate the tracked
projection with:

```sh
python3 scripts/export_voice_alignment.py --check
```
