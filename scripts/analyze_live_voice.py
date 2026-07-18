#!/usr/bin/env python3
"""Find known Japanese DS voice clips in a melonDS application-audio capture.

The comparison is deliberately waveform based.  The reference WAV files are
decoded from the exact .se resources embedded in the ROM, while the capture is
the digital application output collected with ScreenCaptureKit.  A detected
match therefore proves that melonDS actually played the resource; it is not a
text/alignment or container-only check.
"""

from __future__ import annotations

import argparse
import json
import math
import shutil
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path

import numpy as np
from scipy.signal import butter, fftconvolve, resample_poly, sosfiltfilt


ANALYSIS_RATE = 16_384
# ScreenCaptureKit's 48 kHz clock and melonDS' emulated 16.384 kHz clock can
# differ by a few tenths of a percent.  Long clips decorrelate sharply when
# the grid is too coarse (a measured 4.58 s clip peaks at 0.9965x), so sample
# the small plausible range densely rather than testing only nominal speed.
SPEED_FACTORS = tuple(0.994 + 0.0005 * index for index in range(19))


@dataclass(frozen=True)
class Match:
    reference: str
    channel: str
    speed_factor: float
    start_seconds: float
    end_seconds: float
    correlation: float
    absolute_correlation: float
    null_median: float
    null_p99: float
    null_p999: float
    peak_to_p999: float
    decision: str


def decode_audio(path: Path, channels: int) -> np.ndarray:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise RuntimeError("ffmpeg is required")
    command = [
        ffmpeg,
        "-v",
        "error",
        "-i",
        str(path),
        "-map",
        "0:a:0",
        "-ac",
        str(channels),
        "-ar",
        str(ANALYSIS_RATE),
        "-f",
        "f32le",
        "pipe:1",
    ]
    completed = subprocess.run(command, check=True, stdout=subprocess.PIPE)
    decoded = np.frombuffer(completed.stdout, dtype="<f4").astype(np.float64)
    if decoded.size == 0 or decoded.size % channels:
        raise RuntimeError(f"could not decode audio from {path}")
    return decoded.reshape(-1, channels)


def prepare(signal: np.ndarray) -> np.ndarray:
    """Suppress DC/BGM lows and emphasize the voice waveform's fine detail."""
    signal = np.asarray(signal, dtype=np.float64)
    if signal.size < 64:
        raise ValueError("audio is too short")
    high_pass = butter(4, 180, btype="highpass", fs=ANALYSIS_RATE, output="sos")
    filtered = sosfiltfilt(high_pass, signal)
    # Pre-emphasis reduces accidental correlation with sustained music.
    filtered = np.concatenate(([filtered[0]], filtered[1:] - 0.97 * filtered[:-1]))
    return filtered - filtered.mean()


def speed_adjust(reference: np.ndarray, factor: float) -> np.ndarray:
    if factor == 1.0:
        return reference
    denominator = 4000
    numerator = int(round(factor * denominator))
    # A factor > 1 makes the reference longer.  This models a capture whose
    # emulated audio clock is fractionally slower than the decoded resource.
    return resample_poly(reference, numerator, denominator)


def normalized_correlation(capture: np.ndarray, reference: np.ndarray) -> tuple[np.ndarray, int]:
    reference = prepare(reference)
    capture = prepare(capture)
    length = reference.size
    if capture.size < length:
        raise ValueError("capture is shorter than reference")
    reference_energy = float(np.dot(reference, reference))
    if reference_energy <= 1e-16:
        raise ValueError("reference is silent")
    numerator = fftconvolve(capture, reference[::-1], mode="valid")
    local_energy = fftconvolve(capture * capture, np.ones(length), mode="valid")
    denominator = np.sqrt(np.maximum(local_energy * reference_energy, 1e-30))
    return numerator / denominator, length


def classify(peak: float, null_p999: float) -> str:
    # Empirical no-voice captures peak below 0.06.  Requiring both an absolute
    # threshold and a multiple of the capture's own null distribution makes the
    # decision robust to unrelated BGM/SFX and capture duration.
    ratio = peak / max(null_p999, 1e-12)
    if peak >= 0.20 and ratio >= 2.5:
        return "detected"
    if peak >= 0.12 and ratio >= 1.8:
        return "probable"
    return "not_detected"


def best_match(capture_channels: dict[str, np.ndarray], reference: np.ndarray, label: str) -> Match:
    candidates: list[Match] = []
    for channel_name, channel in capture_channels.items():
        for speed_factor in SPEED_FACTORS:
            adjusted = speed_adjust(reference, speed_factor)
            correlation, length = normalized_correlation(channel, adjusted)
            absolute = np.abs(correlation)
            peak_index = int(np.argmax(absolute))
            peak = float(absolute[peak_index])
            percentiles = np.percentile(absolute, [50.0, 99.0, 99.9])
            p50, p99, p999 = (float(value) for value in percentiles)
            candidates.append(
                Match(
                    reference=label,
                    channel=channel_name,
                    speed_factor=speed_factor,
                    start_seconds=peak_index / ANALYSIS_RATE,
                    end_seconds=(peak_index + length) / ANALYSIS_RATE,
                    correlation=float(correlation[peak_index]),
                    absolute_correlation=peak,
                    null_median=p50,
                    null_p99=p99,
                    null_p999=p999,
                    peak_to_p999=peak / max(p999, 1e-12),
                    decision=classify(peak, p999),
                )
            )
    return max(candidates, key=lambda candidate: candidate.absolute_correlation)


def parse_reference(raw: str) -> tuple[str, Path]:
    if "=" not in raw:
        raise argparse.ArgumentTypeError("reference must be LABEL=PATH")
    label, path = raw.split("=", 1)
    if not label or not path:
        raise argparse.ArgumentTypeError("reference must be LABEL=PATH")
    return label, Path(path).expanduser().resolve()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path)
    parser.add_argument(
        "--reference",
        action="append",
        required=True,
        type=parse_reference,
        metavar="LABEL=PATH",
    )
    parser.add_argument("--json", type=Path, dest="json_output")
    arguments = parser.parse_args()

    capture_path = arguments.capture.expanduser().resolve()
    stereo = decode_audio(capture_path, channels=2)
    channels = {
        "left": stereo[:, 0],
        "right": stereo[:, 1],
        "mid": (stereo[:, 0] + stereo[:, 1]) * 0.5,
    }

    results: list[Match] = []
    for label, reference_path in arguments.reference:
        reference = decode_audio(reference_path, channels=1)[:, 0]
        results.append(best_match(channels, reference, label))

    payload = {
        "capture": str(capture_path),
        "analysis_rate": ANALYSIS_RATE,
        "duration_seconds": stereo.shape[0] / ANALYSIS_RATE,
        "matches": [asdict(result) for result in results],
    }
    rendered = json.dumps(payload, indent=2, ensure_ascii=False)
    print(rendered)
    if arguments.json_output:
        arguments.json_output.parent.mkdir(parents=True, exist_ok=True)
        arguments.json_output.write_text(rendered + "\n", encoding="utf-8")

    return 0 if any(result.decision == "detected" for result in results) else 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
