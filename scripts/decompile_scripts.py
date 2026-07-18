#!/usr/bin/env python3
"""Decompile extracted 999 FSB scripts through CrossOver Wine."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path


def to_wine_path(path: Path) -> str:
    resolved = path.resolve()
    home = Path.home().resolve()
    try:
        relative = resolved.relative_to(home)
        return "Y:\\" + str(relative).replace("/", "\\")
    except ValueError:
        return "Z:" + str(resolved).replace("/", "\\")


def decompile_one(
    source: Path,
    destination: Path,
    wine: Path,
    compiler: Path,
    bottle: str,
) -> tuple[str, int]:
    generated = Path(str(source) + ".txt")
    if generated.exists() or (destination != generated and destination.exists()):
        raise FileExistsError(f"refusing to replace an existing decompilation: {destination}")

    environment = os.environ.copy()
    environment["CX_BOTTLE"] = bottle
    process = subprocess.run(
        [str(wine), str(compiler), "-o", "e", "-f", to_wine_path(source)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        errors="replace",
        env=environment,
    )
    if process.returncode or not generated.is_file():
        raise RuntimeError(
            f"Decompilation failed for {source} (exit {process.returncode}):\n"
            f"{process.stdout}"
        )
    size = generated.stat().st_size
    if size == 0:
        generated.unlink()
        raise ValueError(f"Decompiler produced an empty script for {source}")

    if destination != generated:
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(generated, destination)
        generated.unlink()
    return source.name, size


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument(
        "--wine",
        type=Path,
        default=Path(
            "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine"
        ),
    )
    parser.add_argument("--bottle", default="Steam")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()

    sources = sorted(args.source_dir.glob("*.fsb"))
    if not sources:
        raise ValueError(f"No .fsb sources in {args.source_dir}")
    if not args.compiler.is_file() or not args.wine.is_file():
        raise FileNotFoundError("CrossOver Wine or ZeroEscapeScript is missing")

    results = []
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = {
            pool.submit(
                decompile_one,
                source,
                args.output_dir / f"{source.name}.txt",
                args.wine,
                args.compiler,
                args.bottle,
            ): source
            for source in sources
        }
        for future in as_completed(futures):
            results.append(future.result())
    print(f"decompiled={len(results)}")
    print(f"bytes={sum(size for _, size in results)}")


if __name__ == "__main__":
    main()
