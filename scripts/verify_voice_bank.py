#!/usr/bin/env python3
"""Rebuild selected source waveforms and compare them with generated DS ADPCM."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path

import numpy as np

from build_voice_bank import (
    VOICE_LANGUAGE_SPECS,
    decode_pcm,
    decrypt_ogg,
    load_archive_voice_index,
    load_manifest,
    resolve_voice_records,
)
from voice_audio import (
    CHUNK_HEADER_SIZE,
    SAMPLE_RATE,
    decode_nds_ima,
    signal_to_noise_db,
    validate_se,
)


def parse_indices(specification: str, count: int) -> list[int]:
    if specification == "spread":
        return sorted({0, 1, count // 4, count // 2, 3 * count // 4, count - 1})
    result = sorted({int(value) for value in specification.split(",")})
    if any(value < 0 or value >= count for value in result):
        raise ValueError(f"sample index outside 0..{count - 1}")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ze1_data", type=Path)
    parser.add_argument("pc_manifest", type=Path)
    parser.add_argument("voice_map", type=Path)
    parser.add_argument("voice_dir", type=Path)
    parser.add_argument(
        "--language",
        choices=sorted(VOICE_LANGUAGE_SPECS),
        default="jp",
    )
    parser.add_argument(
        "--indices", default="spread", help="comma-separated or 'spread'"
    )
    parser.add_argument("--silence-ms", type=float, default=65.0)
    args = parser.parse_args()

    manifest = load_manifest(args.pc_manifest) if args.language == "jp" else None
    with args.voice_map.open(encoding="utf-8", newline="") as stream:
        rows = list(csv.DictReader(stream, delimiter="\t"))
    indices = parse_indices(args.indices, len(rows))
    silence = np.zeros(round(SAMPLE_RATE * args.silence_ms / 1000.0), dtype=np.int16)

    minimum_snr = float("inf")
    with args.ze1_data.open("rb") as archive:
        archive_index = (
            load_archive_voice_index(archive) if args.language == "en" else None
        )
        for index in indices:
            row = rows[index]
            chunks: list[np.ndarray] = []
            records = resolve_voice_records(
                row,
                args.language,
                manifest=manifest,
                archive_index=archive_index,
            )
            resolved_paths = [path for path, _ in records]
            mapped_paths = row["jp_ogg_paths"].split(" | ")
            if resolved_paths != mapped_paths:
                raise ValueError(
                    f"voice-map source paths disagree at {row['symbol']}: "
                    f"{mapped_paths!r} vs {resolved_paths!r}"
                )
            for _path, record in records:
                if chunks:
                    chunks.append(silence)
                chunks.append(decode_pcm(decrypt_ogg(archive, record)))
            reference = np.concatenate(chunks).astype(np.int16).tolist()

            symbol = row["symbol"]
            payload = (args.voice_dir / f"{symbol.lower()}.se").read_bytes()
            info = validate_se(payload, symbol, int(row["internal_id_hex"], 16))
            start = info.pcmd_offset + CHUNK_HEADER_SIZE
            decoded = decode_nds_ima(payload[start : start + info.pcmd_size])
            reference.extend([reference[-1]] * (len(decoded) - len(reference)))
            snr = signal_to_noise_db(reference, decoded)
            minimum_snr = min(minimum_snr, snr)
            correlation = float(np.corrcoef(reference, decoded)[0, 1])
            print(
                f"index={index} symbol={symbol} samples={len(decoded)} "
                f"snr_db={snr:.4f} correlation={correlation:.8f}"
            )
    print(f"minimum_snr_db={minimum_snr:.4f}")
    print("status=OK")


if __name__ == "__main__":
    main()
