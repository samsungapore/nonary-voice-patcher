#!/usr/bin/env python3
"""Insert Japanese voice calls before the DS ``setText`` lines in a map.

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
import re
from collections import defaultdict
from pathlib import Path


SET_TEXT = re.compile(r'^(?P<indent>\s*)setText\("(?:[^"\\]|\\.)*"\);\s*$')
SYMBOL = re.compile(r"SE_V[0-9]{4}")


def load_map(path: Path) -> dict[str, dict[int, str]]:
    result: dict[str, dict[int, str]] = defaultdict(dict)
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter="\t")
        required = {"symbol", "ds_script", "ds_source_line"}
        if not reader.fieldnames or not required.issubset(reader.fieldnames):
            raise ValueError(f"{path} is missing columns: {sorted(required - set(reader.fieldnames or []))}")
        for row in reader:
            symbol = row["symbol"].upper()
            if not SYMBOL.fullmatch(symbol):
                raise ValueError(f"Invalid voice symbol: {symbol!r}")
            script = Path(row["ds_script"]).name
            line = int(row["ds_source_line"])
            previous = result[script].setdefault(line, symbol)
            if previous != symbol:
                raise ValueError(f"Two voice symbols target {script}:{line}: {previous}, {symbol}")
    if not result:
        raise ValueError(f"Voice map is empty: {path}")
    return result


def inject_file(source: Path, output: Path, targets: dict[int, str]) -> int:
    lines = source.read_text(encoding="utf-8").splitlines()
    output_lines: list[str] = []
    injected = 0
    for number, line in enumerate(lines, 1):
        symbol = targets.get(number)
        if symbol is None:
            output_lines.append(line)
            continue
        match = SET_TEXT.match(line)
        if not match:
            raise ValueError(f"Mapped line is not a setText call: {source}:{number}: {line!r}")
        indent = match.group("indent")
        output_lines.append(f'{indent}Sound::PlaySE(":{symbol}", 127f, 0f);')
        output_lines.append(line)
        output_lines.append(f'{indent}Sound::WaitSE(":{symbol}");')
        injected += 1
    missing = sorted(set(targets) - set(range(1, len(lines) + 1)))
    if missing:
        raise ValueError(f"Mapped lines outside {source}: {missing[:10]}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(output_lines) + "\n", encoding="utf-8", newline="\n")
    return injected


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("voice_map", type=Path)
    parser.add_argument("source_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()

    mapping = load_map(args.voice_map)
    total = 0
    for script, targets in sorted(mapping.items()):
        source = args.source_dir / script
        if not source.is_file():
            raise FileNotFoundError(source)
        total += inject_file(source, args.output_dir / script, targets)
    print(f"scripts={len(mapping)}")
    print(f"injected_lines={total}")


if __name__ == "__main__":
    main()
