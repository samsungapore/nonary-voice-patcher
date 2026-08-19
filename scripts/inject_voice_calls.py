#!/usr/bin/env python3
"""Insert reviewed voice calls before the DS ``setText`` lines in a map.

Character-window ``setText`` calls in 999 are sometimes non-blocking: the
script immediately continues to a blocking narration window on the other DS
screen.  Stopping the sound directly after such a call truncates it in the
same frame, while merely starting the next clip can overlap a whole run of
non-blocking lines.  ``WaitSE`` therefore serializes each generated clip.  On
a blocking text line it only waits for any audio left after the player's
advance; on a non-blocking line it keeps the script from racing ahead.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path


SET_TEXT = re.compile(r'^(?P<indent>\s*)setText\("(?P<text>(?:[^"\\]|\\.)*)"\);\s*$')
SYMBOL = re.compile(r"SE_V[0-9]{4}")
SHA256 = re.compile(r"[0-9a-fA-F]{64}")

# The French translation build replaces these two-byte ASCII markers across
# the compiled FSB. Reproducing that final representation here is necessary:
# the runtime condition intentionally authenticates the raw bytes seen by the
# game, not a visually normalized rendering of the text.
ACCENT_MARKERS = {
    marker: bytes((0x84, 0xBF + index))
    for index, marker in enumerate("0123456789DEFGHI")
}


@dataclass(frozen=True)
class VoiceTarget:
    symbol: str
    target_text_sha256: tuple[str, ...]


@dataclass(frozen=True)
class InjectionResult:
    injected: int
    rejected_by_text_hash: int


def parse_target_text_hashes(value: str | None, *, label: str) -> tuple[str, ...]:
    if value is None:
        raise ValueError(f"Missing target-text SHA-256 value for {label}")
    hashes: list[str] = []
    for part in value.split("|"):
        candidate = part.strip()
        if not candidate:
            continue
        if not SHA256.fullmatch(candidate):
            raise ValueError(f"Invalid target-text SHA-256 for {label}: {candidate!r}")
        hashes.append(candidate.lower())
    if len(hashes) != len(set(hashes)):
        raise ValueError(f"Duplicate target-text SHA-256 for {label}")
    return tuple(sorted(hashes))


def raw_set_text_bytes(match: re.Match[str]) -> bytes:
    # ZeroEscapeScript only unescapes quotes in string literals. Matching that
    # narrow rule prevents this audit path from hashing a different byte string
    # than the compiler for sequences such as a literal backslash followed by n.
    text = match.group("text").replace(r"\"", '"')
    try:
        encoded = text.encode("cp932")
    except UnicodeEncodeError as error:
        raise ValueError(
            f"setText literal is not encodable as CP932: {text!r}"
        ) from error
    for marker, replacement in ACCENT_MARKERS.items():
        encoded = encoded.replace(f"~{marker}".encode("ascii"), replacement)
    return encoded


def target_accepts_text(target: VoiceTarget, raw_text: bytes) -> bool:
    return not target.target_text_sha256 or hashlib.sha256(raw_text).hexdigest() in (
        target.target_text_sha256
    )


def load_map(path: Path) -> dict[str, dict[int, VoiceTarget]]:
    result: dict[str, dict[int, VoiceTarget]] = defaultdict(dict)
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {"symbol", "ds_script", "ds_source_line"}
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(
                f"{path} is missing columns: {sorted(required - set(reader.fieldnames or []))}"
            )
        for row in reader:
            symbol = row["symbol"].upper()
            if not SYMBOL.fullmatch(symbol):
                raise ValueError(f"Invalid voice symbol: {symbol!r}")
            script = Path(row["ds_script"]).name
            line = int(row["ds_source_line"])
            label = f"{script}:{line} ({symbol})"
            hashes = parse_target_text_hashes(
                row.get("target_text_sha256", ""), label=label
            )
            target = VoiceTarget(symbol=symbol, target_text_sha256=hashes)
            previous = result[script].setdefault(line, target)
            if previous != target:
                raise ValueError(
                    f"Two voice targets disagree at {script}:{line}: "
                    f"{previous}, {target}"
                )
    if not result:
        raise ValueError(f"Voice map is empty: {path}")
    return result


def inject_file(
    source: Path, output: Path, targets: dict[int, VoiceTarget]
) -> InjectionResult:
    lines = source.read_text(encoding="utf-8").splitlines()
    output_lines: list[str] = []
    injected = 0
    rejected_by_text_hash = 0
    for number, line in enumerate(lines, 1):
        target = targets.get(number)
        if target is None:
            output_lines.append(line)
            continue
        match = SET_TEXT.match(line)
        if not match:
            raise ValueError(
                f"Mapped line is not a setText call: {source}:{number}: {line!r}"
            )
        if not target_accepts_text(target, raw_set_text_bytes(match)):
            output_lines.append(line)
            rejected_by_text_hash += 1
            continue
        indent = match.group("indent")
        output_lines.append(f'{indent}Sound::PlaySE(":{target.symbol}", 127f, 0f);')
        output_lines.append(line)
        output_lines.append(f'{indent}Sound::WaitSE(":{target.symbol}");')
        injected += 1
    missing = sorted(set(targets) - set(range(1, len(lines) + 1)))
    if missing:
        raise ValueError(f"Mapped lines outside {source}: {missing[:10]}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write("\n".join(output_lines) + "\n")
    return InjectionResult(
        injected=injected, rejected_by_text_hash=rejected_by_text_hash
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("voice_map", type=Path)
    parser.add_argument("source_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()

    mapping = load_map(args.voice_map)
    total = 0
    rejected_by_text_hash = 0
    for script, targets in sorted(mapping.items()):
        source = args.source_dir / script
        if not source.is_file():
            raise FileNotFoundError(source)
        result = inject_file(source, args.output_dir / script, targets)
        total += result.injected
        rejected_by_text_hash += result.rejected_by_text_hash
    print(f"scripts={len(mapping)}")
    print(f"injected_lines={total}")
    print(f"rejected_by_text_hash={rejected_by_text_hash}")


if __name__ == "__main__":
    main()
