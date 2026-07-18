#!/usr/bin/env python3
"""Compile injected ZeroEscapeScript text files through CrossOver Wine."""

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


def compile_one(source: Path, destination: Path, wine: Path, compiler: Path, bottle: str) -> tuple[str, int]:
    generated = Path(str(source) + ".fsb")
    if generated.exists():
        generated.unlink()
    environment = os.environ.copy()
    environment["CX_BOTTLE"] = bottle
    process = subprocess.run(
        [str(wine), str(compiler), "-o", "c", "-f", to_wine_path(source)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=environment,
    )
    if process.returncode or not generated.is_file():
        raise RuntimeError(
            f"Compilation failed for {source} (exit {process.returncode}):\n{process.stdout}"
        )
    data = generated.read_bytes()
    if data[:4] != b"SIR0":
        raise ValueError(f"Compiler produced an invalid FSB for {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(generated, destination)
    generated.unlink()
    return source.name, len(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument(
        "--wine",
        type=Path,
        default=Path("/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine"),
    )
    parser.add_argument("--bottle", default="Steam")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()

    sources = sorted(args.source_dir.glob("*.fsb.txt"))
    if not sources:
        raise ValueError(f"No .fsb.txt sources in {args.source_dir}")
    if not args.compiler.is_file() or not args.wine.is_file():
        raise FileNotFoundError("CrossOver Wine or ZeroEscapeScript compiler is missing")

    results = []
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = {
            pool.submit(
                compile_one,
                source,
                args.output_dir / source.name.removesuffix(".txt"),
                args.wine,
                args.compiler,
                args.bottle,
            ): source
            for source in sources
        }
        for future in as_completed(futures):
            results.append(future.result())
    print(f"compiled={len(results)}")
    print(f"bytes={sum(size for _, size in results)}")


if __name__ == "__main__":
    main()
