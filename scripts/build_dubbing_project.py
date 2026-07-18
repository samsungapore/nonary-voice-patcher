#!/usr/bin/env python3
"""Build a French voice pack from a Nonary Dubbing Studio project.

The project keeps lossless WAV masters. This builder creates disposable DS
``.se`` derivatives and an authenticated ``NVPACK01`` pack without modifying
the masters or the source ROM.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterator
from contextlib import contextmanager
import errno
import hashlib
import io
import json
import math
import os
from pathlib import Path
import stat
import struct
import tempfile
import time
from typing import Any

import numpy as np
import soundfile as sf
import soxr

from build_voice_bank import allocate_internal_ids, collect_retail_internal_ids
from build_voice_pack import build_pack
from voice_audio import SAMPLE_RATE, build_se, read_template, validate_se


PROJECT_FILE = "project.nvdub.json"
PROJECT_LOCK_FILE = ".project.nvdub.lock"
PROJECT_VERSION = 1
MAX_DURATION_SECONDS = 45.0
MAX_JSON_BYTES = 64 * 1024 * 1024
MAX_ROM_BYTES = 512 * 1024 * 1024
MAX_MASTER_BYTES = 64 * 1024 * 1024
MAX_MASTER_CHANNELS = 8
MAX_MASTER_SAMPLE_RATE = 384_000
PREVIEW_SILENCE_MS = 80.0
CATALOG_DOMAIN = b"NVPACK-CATALOG-V1\0"
APPROVAL_DOMAIN = b"NVDUB_APPROVAL_V1\0"
VALID_STATUSES = {"missing", "recorded", "needs_review", "approved", "skipped"}
PCM_CLIP_LIMIT = 0.999969


def _lock_descriptor(descriptor: int) -> None:
    if os.name == "nt":
        import msvcrt

        retryable = {errno.EACCES, errno.EAGAIN, getattr(errno, "EDEADLK", errno.EACCES)}
        while True:
            os.lseek(descriptor, 0, os.SEEK_SET)
            try:
                msvcrt.locking(descriptor, msvcrt.LK_NBLCK, 1)
                return
            except OSError as error:
                if error.errno not in retryable:
                    raise
                time.sleep(0.05)
    else:
        import fcntl

        fcntl.flock(descriptor, fcntl.LOCK_EX)


def _unlock_descriptor(descriptor: int) -> None:
    if os.name == "nt":
        import msvcrt

        os.lseek(descriptor, 0, os.SEEK_SET)
        msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)
    else:
        import fcntl

        fcntl.flock(descriptor, fcntl.LOCK_UN)


@contextmanager
def project_mutation_lock(project_dir: Path) -> Iterator[None]:
    try:
        project_metadata = project_dir.lstat()
    except OSError as error:
        raise ValueError(f"Cannot inspect project directory {project_dir}: {error}") from error
    if stat.S_ISLNK(project_metadata.st_mode) or not stat.S_ISDIR(
        project_metadata.st_mode
    ):
        raise ValueError(
            f"Project directory must be a real directory, not a symbolic link: {project_dir}"
        )

    lock_path = project_dir / PROJECT_LOCK_FILE
    try:
        lock_metadata = lock_path.lstat()
    except FileNotFoundError:
        lock_metadata = None
    except OSError as error:
        raise ValueError(f"Cannot inspect project lock {lock_path}: {error}") from error
    if lock_metadata is not None and (
        stat.S_ISLNK(lock_metadata.st_mode) or not stat.S_ISREG(lock_metadata.st_mode)
    ):
        raise ValueError(f"Project lock must be a regular file: {lock_path}")

    flags = os.O_RDWR | os.O_CREAT
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(lock_path, flags, 0o600)
    except OSError as error:
        raise ValueError(f"Cannot open project lock {lock_path}: {error}") from error
    locked = False
    try:
        opened_metadata = os.fstat(descriptor)
        current_metadata = lock_path.lstat()
        if not stat.S_ISREG(opened_metadata.st_mode) or not stat.S_ISREG(
            current_metadata.st_mode
        ):
            raise ValueError(f"Project lock must be a regular file: {lock_path}")
        if (opened_metadata.st_dev, opened_metadata.st_ino) != (
            current_metadata.st_dev,
            current_metadata.st_ino,
        ):
            raise ValueError(f"Project lock changed while being opened: {lock_path}")
        # Builders and editors share this guard so an output can never be
        # published from a manifest snapshot that ceased to be current midway.
        _lock_descriptor(descriptor)
        locked = True
        yield
    finally:
        if locked:
            _unlock_descriptor(descriptor)
        os.close(descriptor)


def read_limited_regular_bytes(path: Path, limit: int, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ValueError(f"Cannot inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ValueError(f"{label} must be a regular file, not a symbolic link: {path}")
    if metadata.st_size > limit:
        raise ValueError(f"{label} exceeds the {limit}-byte safety limit: {path}")
    try:
        with path.open("rb") as stream:
            data = stream.read(limit + 1)
    except OSError as error:
        raise ValueError(f"Cannot read {label} {path}: {error}") from error
    if len(data) > limit:
        raise ValueError(f"{label} exceeds the {limit}-byte safety limit: {path}")
    return data


def load_json(path: Path) -> dict[str, Any]:
    try:
        raw = read_limited_regular_bytes(path, MAX_JSON_BYTES, "JSON file")
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"Cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"JSON root must be an object: {path}")
    return value


def profile_targets(profile: dict[str, Any]) -> list[tuple[str, str, int]]:
    if profile.get("version") != 1:
        raise ValueError(f"Unsupported voice profile version: {profile.get('version')!r}")
    result: list[tuple[str, str, int]] = []
    for script in profile.get("scripts", []):
        path = script.get("path")
        if not isinstance(path, str) or not path.startswith("scr/"):
            raise ValueError(f"Invalid script path in voice profile: {path!r}")
        for voice in script.get("voices", []):
            symbol = voice.get("symbol")
            ordinal = voice.get("ordinal")
            expected = f"SE_V{len(result):04d}"
            if symbol != expected or not isinstance(ordinal, int) or ordinal < 0:
                raise ValueError(
                    f"Invalid voice target {symbol!r}/{ordinal!r}; expected {expected}"
                )
            result.append((symbol, path, ordinal))
    if len(result) != profile.get("voice_count") or not result:
        raise ValueError(
            f"Profile declares {profile.get('voice_count')!r} voices but contains {len(result)}"
        )
    return result


def catalog_sha256(targets: list[tuple[str, str, int]]) -> bytes:
    digest = hashlib.sha256()
    digest.update(CATALOG_DOMAIN)
    digest.update(struct.pack("<Q", len(targets)))
    for symbol, script_path, ordinal in targets:
        symbol_bytes = symbol.encode("utf-8")
        path_bytes = script_path.encode("utf-8")
        digest.update(struct.pack("<Q", len(symbol_bytes)))
        digest.update(symbol_bytes)
        digest.update(struct.pack("<Q", len(path_bytes)))
        digest.update(path_bytes)
        digest.update(struct.pack("<Q", ordinal))
    return digest.digest()


def valid_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and value == value.lower()
        and all(character in "0123456789abcdef" for character in value)
    )


def approval_sha256(target: dict[str, Any], take: dict[str, Any]) -> str:
    gain_db = float(target.get("gainDb", 0.0))
    trim_start = target.get("trimStartMs", 0)
    trim_end = target.get("trimEndMs", 0)
    take_digest = take.get("sha256")
    if not valid_sha256(take_digest):
        raise ValueError("Approval references an invalid take SHA-256")
    if not math.isfinite(gain_db) or not -60.0 <= gain_db <= 24.0:
        raise ValueError(f"Approval uses invalid gain {gain_db!r} dB")
    if (
        isinstance(trim_start, bool)
        or not isinstance(trim_start, int)
        or isinstance(trim_end, bool)
        or not isinstance(trim_end, int)
        or not 0 <= trim_start <= 45_000
        or not 0 <= trim_end <= 45_000
    ):
        raise ValueError("Approval uses invalid trim values")
    digest = hashlib.sha256()
    digest.update(APPROVAL_DOMAIN)
    digest.update(take_digest.encode("ascii"))
    # Rust stores gain as f32 in the manifest contract, so approval binds the
    # same single-precision value in both implementations.
    digest.update(struct.pack("<f", gain_db))
    digest.update(struct.pack("<Q", trim_start))
    digest.update(struct.pack("<Q", trim_end))
    return digest.hexdigest()


def safe_project_file(project_dir: Path, relative: str) -> Path:
    if not relative or Path(relative).is_absolute():
        raise ValueError(f"Take path must be relative to the project: {relative!r}")
    root = project_dir.resolve()
    candidate = root / relative
    cursor = root
    for component in Path(relative).parts:
        cursor /= component
        if cursor.is_symlink():
            raise ValueError(f"Take path contains a symbolic link: {relative!r}")
    resolved = candidate.resolve()
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"Take path escapes the project: {relative!r}") from error
    return resolved


def validate_output_pack_path(
    output: Path,
    project_dir: Path,
    manifest_path: Path,
    profile_path: Path,
    rom_path: Path,
) -> None:
    resolved = output.resolve()
    protected = {
        manifest_path.resolve(),
        (project_dir / PROJECT_LOCK_FILE).resolve(),
        profile_path.resolve(),
        rom_path.resolve(),
    }
    if resolved in protected:
        raise ValueError(f"Output pack would overwrite a required input: {resolved}")
    recordings = (project_dir / "recordings").resolve()
    try:
        resolved.relative_to(recordings)
    except ValueError:
        return
    raise ValueError("Output pack cannot be written inside the recordings directory")


def validate_project(
    project: dict[str, Any],
    project_dir: Path,
    profile_path: Path,
    profile: dict[str, Any],
    targets: list[tuple[str, str, int]],
    rom_path: Path,
    raw_rom: bytes,
) -> None:
    if project.get("version") != PROJECT_VERSION:
        raise ValueError(f"Unsupported dubbing project version: {project.get('version')!r}")
    if not valid_sha256(project.get("profileSha256")):
        raise ValueError("The project contains an invalid profile audit fingerprint")
    expected_catalog = catalog_sha256(targets).hex()
    if project.get("profileCatalogSha256") != expected_catalog:
        raise ValueError(
            "The project voice catalogue does not match the selected voice profile"
        )
    if project.get("sourceRomSha256") != hashlib.sha256(raw_rom).hexdigest():
        raise ValueError(
            "The selected ROM is not the exact clean/restored ROM used to create the project"
        )
    if len(raw_rom) < 0x10:
        raise ValueError("The source ROM is too short")
    game_code = raw_rom[0x0C:0x10].decode("ascii", "replace")
    if game_code != project.get("gameCode") or game_code != profile.get("game_code"):
        raise ValueError(
            f"ROM game code {game_code!r} does not match the project/profile"
        )
    progress = project.get("targets")
    if not isinstance(progress, dict) or len(progress) != len(targets):
        raise ValueError("The project target table does not match the voice profile")
    for symbol, script, ordinal in targets:
        target = progress.get(symbol)
        if not isinstance(target, dict):
            raise ValueError(f"Project is missing {symbol}")
        expected_id = f"{script}#{ordinal}"
        if target.get("targetId") != expected_id:
            raise ValueError(
                f"{symbol} points to {target.get('targetId')!r}; expected {expected_id!r}"
            )
        takes = target.get("takes", [])
        if not isinstance(takes, list):
            raise ValueError(f"{symbol} has an invalid take list")
        seen: dict[str, dict[str, Any]] = {}
        for take in takes:
            if not isinstance(take, dict) or not isinstance(take.get("id"), str):
                raise ValueError(f"{symbol} has invalid take metadata")
            take_id = take["id"]
            if take_id in seen:
                raise ValueError(f"{symbol} repeats take ID {take['id']!r}")
            expected_file = f"recordings/{symbol}/{take_id}.wav"
            if take.get("file") != expected_file:
                raise ValueError(
                    f"{symbol} take {take_id!r} must use {expected_file!r}"
                )
            expected_take_hash = take.get("sha256")
            if not valid_sha256(expected_take_hash):
                raise ValueError(f"{symbol} take {take_id!r} has an invalid SHA-256")
            path = safe_project_file(project_dir, take.get("file", ""))
            if not path.is_file():
                raise FileNotFoundError(f"Take file is missing: {path}")
            actual_take_hash = hashlib.sha256(
                read_limited_regular_bytes(path, MAX_MASTER_BYTES, "WAV master")
            ).hexdigest()
            if actual_take_hash != expected_take_hash:
                raise ValueError(
                    f"{symbol} take {take_id!r} failed its SHA-256 integrity check"
                )
            seen[take_id] = take
        active = target.get("activeTake")
        if active is not None and active not in seen:
            raise ValueError(f"{symbol} selects missing take {active!r}")
        status = target.get("status", "missing")
        if status not in VALID_STATUSES:
            raise ValueError(f"{symbol} has invalid status {status!r}")
        if status in {"recorded", "needs_review", "approved"} and active is None:
            raise ValueError(f"{symbol} requires an active take in status {status!r}")
        approval = target.get("approvalSha256")
        if status == "approved":
            if active is None:
                raise AssertionError("approved status passed active-take validation")
            expected_approval = approval_sha256(target, seen[active])
            if approval != expected_approval:
                raise ValueError(
                    f"{symbol} approval does not match its active take and processing settings"
                )
        elif approval is not None:
            raise ValueError(
                f"{symbol} retains an approval signature outside approved status"
            )


def active_take(
    project_dir: Path, target: dict[str, Any]
) -> tuple[Path, dict[str, Any]] | None:
    active = target.get("activeTake")
    if not isinstance(active, str):
        return None
    for take in target.get("takes", []):
        if take.get("id") == active:
            return safe_project_file(project_dir, take["file"]), take
    raise AssertionError("project validation did not catch a missing active take")


def decode_master(
    path: Path,
    target: dict[str, Any],
    take: dict[str, Any],
    scope: str,
) -> np.ndarray:
    raw_master = read_limited_regular_bytes(path, MAX_MASTER_BYTES, "WAV master")
    if not raw_master:
        raise ValueError(f"WAV master is empty: {path}")
    if hashlib.sha256(raw_master).hexdigest() != take.get("sha256"):
        raise ValueError(f"WAV master changed after project validation: {path}")
    try:
        info = sf.info(io.BytesIO(raw_master))
    except (RuntimeError, OSError) as error:
        raise ValueError(f"Cannot inspect WAV master {path}: {error}") from error
    if (
        info.frames <= 0
        or not 0 < info.channels <= MAX_MASTER_CHANNELS
        or not 0 < info.samplerate <= MAX_MASTER_SAMPLE_RATE
        or info.frames / info.samplerate > MAX_DURATION_SECONDS
    ):
        raise ValueError(f"WAV master has unsafe dimensions: {path}")
    try:
        samples, rate = sf.read(
            io.BytesIO(raw_master), dtype="float32", always_2d=True
        )
    except (RuntimeError, OSError) as error:
        raise ValueError(f"Cannot decode WAV master {path}: {error}") from error
    if samples.size == 0 or rate <= 0:
        raise ValueError(f"WAV master is empty: {path}")
    mono = samples.mean(axis=1, dtype=np.float32)
    if rate != SAMPLE_RATE:
        mono = soxr.resample(mono, rate, SAMPLE_RATE, quality="HQ")

    trim_start = round(float(target.get("trimStartMs", 0)) * SAMPLE_RATE / 1000.0)
    trim_end = round(float(target.get("trimEndMs", 0)) * SAMPLE_RATE / 1000.0)
    if trim_start < 0 or trim_end < 0 or trim_start + trim_end >= len(mono):
        raise ValueError(f"Invalid trim range for {path}")
    if trim_end:
        mono = mono[trim_start:-trim_end]
    else:
        mono = mono[trim_start:]

    gain_db = float(target.get("gainDb", 0.0))
    if not math.isfinite(gain_db) or not -60.0 <= gain_db <= 24.0:
        raise ValueError(f"Invalid gain {gain_db!r} dB for {path}")
    if gain_db:
        mono = mono * np.float32(10.0 ** (gain_db / 20.0))
    peak = float(np.max(np.abs(mono)))
    if not math.isfinite(peak):
        raise ValueError(f"WAV master produces non-finite samples after gain: {path}")
    if scope == "production" and peak > PCM_CLIP_LIMIT:
        raise ValueError(
            f"Gain would clip WAV master {path} (post-gain peak {peak:.6f})"
        )
    duration = len(mono) / SAMPLE_RATE
    if not 0 < duration <= MAX_DURATION_SECONDS:
        raise ValueError(
            f"{path} is {duration:.3f}s after trimming; expected 0-{MAX_DURATION_SECONDS:.0f}s"
        )
    return np.rint(np.clip(mono, -PCM_CLIP_LIMIT, PCM_CLIP_LIMIT) * 32767.0).astype(np.int16)


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, raw_temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(raw_temporary)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            json.dump(value, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def build(arguments: argparse.Namespace) -> dict[str, Any]:
    project_dir = arguments.project_dir.resolve()
    # The sidecar is part of the publication contract, so the lock outlives
    # both pack creation and report replacement rather than only validation.
    with project_mutation_lock(project_dir):
        return _build_locked(arguments, project_dir)


def _build_locked(
    arguments: argparse.Namespace, project_dir: Path
) -> dict[str, Any]:
    manifest_path = project_dir / PROJECT_FILE
    profile_path = arguments.profile.resolve()
    rom_path = arguments.rom.resolve()
    output_pack = arguments.output_pack.resolve()
    output_report = output_pack.with_suffix(output_pack.suffix + ".json")
    project = load_json(manifest_path)
    profile = load_json(profile_path)
    raw_rom = read_limited_regular_bytes(rom_path, MAX_ROM_BYTES, "source ROM")
    targets = profile_targets(profile)
    catalog_digest = catalog_sha256(targets)
    validate_project(
        project, project_dir, profile_path, profile, targets, rom_path, raw_rom
    )
    validate_output_pack_path(
        output_pack, project_dir, manifest_path, profile_path, rom_path
    )
    validate_output_pack_path(
        output_report, project_dir, manifest_path, profile_path, rom_path
    )
    if len(targets) > 10_000:
        raise ValueError("SE_V#### supports at most 10,000 voices")

    if arguments.scope == "production":
        incomplete = [
            symbol
            for symbol, _script, _ordinal in targets
            if project["targets"][symbol].get("status") != "approved"
            or active_take(project_dir, project["targets"][symbol]) is None
        ]
        if incomplete:
            raise ValueError(
                f"Production build requires {len(targets)} approved active takes; "
                f"{len(incomplete)} targets are missing or unapproved "
                f"(first: {incomplete[0]})"
            )

    # `.se` derivatives are private scratch data.  A new temporary directory
    # prevents a failed build from deleting caller-owned files or reusing stale
    # silence/audio from an earlier project state.
    build_root = project_dir / "builds"
    build_root.mkdir(parents=True, exist_ok=True)
    temporary_voices = tempfile.TemporaryDirectory(
        prefix=".voices-fr-", dir=build_root
    )
    voices_dir = Path(temporary_voices.name)
    rom_snapshot = voices_dir / ".source-rom.nds"
    rom_snapshot.write_bytes(raw_rom)

    retail_ids = set(collect_retail_internal_ids(rom_snapshot).values())
    internal_ids = allocate_internal_ids(retail_ids, len(targets))
    template = read_template(None, rom_snapshot)
    validate_se(template, expected_internal_id=0x6604)
    silence = np.zeros(round(PREVIEW_SILENCE_MS * SAMPLE_RATE / 1000.0), dtype=np.int16)

    recorded = 0
    approved = 0
    missing: list[str] = []
    total_samples = 0
    total_bytes = 0
    for index, (symbol, _script, _ordinal) in enumerate(targets):
        target = project["targets"][symbol]
        selected_take = active_take(project_dir, target)
        if selected_take is None:
            missing.append(symbol)
            if arguments.scope == "production":
                continue
            pcm = silence
        else:
            source, take = selected_take
            recorded += 1
            if target.get("status") == "approved":
                approved += 1
            elif arguments.scope == "production":
                missing.append(symbol)
                continue
            pcm = decode_master(source, target, take, arguments.scope)

        identifier = internal_ids[index]
        output = voices_dir / f"{symbol.lower()}.se"
        encoded = build_se(template, pcm, SAMPLE_RATE, identifier, symbol)
        info = validate_se(encoded, symbol, identifier)
        temporary = output.with_name(f".{output.name}.tmp-{os.getpid()}")
        temporary.write_bytes(encoded)
        temporary.replace(output)
        total_samples += len(pcm)
        total_bytes += info.file_size
        if (index + 1) % 100 == 0 or index + 1 == len(targets):
            print(
                f"progress={index + 1}/{len(targets)} "
                f"recorded={recorded} se_mib={total_bytes / 1048576:.2f}",
                flush=True,
            )

    if arguments.scope == "production" and missing:
        raise AssertionError("production preflight accepted an incomplete project")
    expected_files = [f"se_v{index:04d}.se" for index in range(len(targets))]
    actual_files = sorted(path.name.lower() for path in voices_dir.glob("se_v*.se"))
    if actual_files != expected_files:
        raise ValueError(
            f"Generated voice bank is not contiguous: expected {len(expected_files)}, "
            f"found {len(actual_files)}"
        )

    voices, payload_bytes, pack_sha256 = build_pack(
        voices_dir, output_pack, "fr", catalog_digest
    )
    report = {
        "version": 1,
        "scope": arguments.scope,
        "project": str(manifest_path),
        "projectName": project.get("name"),
        "sourceRomSha256": project["sourceRomSha256"],
        "profileSha256": project["profileSha256"],
        "profileCatalogSha256": catalog_digest.hex(),
        "voices": voices,
        "recorded": recorded,
        "approved": approved,
        "silentPreviewEntries": len(targets) - recorded,
        "durationSeconds": total_samples / SAMPLE_RATE,
        "payloadBytes": payload_bytes,
        "pack": str(output_pack),
        "packSha256": pack_sha256,
        "firstInternalId": f"{internal_ids[0]:04x}",
        "lastInternalId": f"{internal_ids[-1]:04x}",
    }
    atomic_json(output_report, report)
    temporary_voices.cleanup()
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project_dir", type=Path)
    parser.add_argument("rom", type=Path, help="clean/restored ROM used by the project")
    parser.add_argument("output_pack", type=Path)
    parser.add_argument(
        "--profile",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "src-tauri"
        / "resources"
        / "voice-profile.json",
    )
    parser.add_argument("--scope", choices=("preview", "production"), default="preview")
    arguments = parser.parse_args()
    try:
        report = build(arguments)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(f"voices={report['voices']}")
    print(f"recorded={report['recorded']}")
    print(f"silent_preview_entries={report['silentPreviewEntries']}")
    print(f"payload_bytes={report['payloadBytes']}")
    print(f"sha256={report['packSha256']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
