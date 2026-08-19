#!/usr/bin/env python3
"""Apply narrowly reviewed dialogue mappings after automatic alignment.

The automatic matcher remains deliberately conservative.  This stage accepts
only exact PC-message/DS-target pairs recorded in a review ledger.  A ledger
row may also restrict a target to exact raw ``setText`` SHA-256 values and may
select a language-specific interval from one PC voice.  Reusing a PC message
is allowed only for a validated group of consecutive, non-overlapping slices.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ALIGNMENT = PROJECT_ROOT / "work/manifests/alignment_extended_dialogue_only.tsv"
DEFAULT_PC_MANIFEST = PROJECT_ROOT / "work/manifests/ze1_jp_dialogue_voice_manifest.tsv"
DEFAULT_DS_SCRIPTS = PROJECT_ROOT / "work/romfs/scr"
DEFAULT_LEDGER = PROJECT_ROOT / "research/reviews/reviewed_alignment_overrides.tsv"
DEFAULT_OUTPUT = PROJECT_ROOT / "work/manifests/alignment_with_reviewed_overrides.tsv"

HASH = re.compile(r"[0-9a-f]{64}")
GROUP = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")
SCRIPT = re.compile(r"[^/\\]+\.fsb\.txt", re.IGNORECASE)
WILDCARDS = frozenset("*?[]")
NATURAL_PART = re.compile(r"(\d+)")
REVIEW_BASES = frozenset(
    {
        "speaker_alias",
        "multi_page_split",
        "multi_page_composite",
        "translated_dialogue",
    }
)
MATERIALIZED_REVIEW_BASIS = "materialized_runtime"

LEDGER_FIELDS = (
    "override_group",
    "component_order",
    "target_action",
    "replace_pc_message_id",
    "pc_message_id",
    "ds_script",
    "ds_settext_ordinal",
    "ds_speaker",
    "pc_speaker",
    "target_text_sha256",
    "jp_start_ms",
    "jp_end_ms",
    "en_start_ms",
    "en_end_ms",
    "review_basis",
)
RUNTIME_FIELDS = (
    "reviewed_override_group",
    "target_text_sha256",
    "jp_start_ms",
    "jp_end_ms",
    "en_start_ms",
    "en_end_ms",
)


@dataclass(frozen=True)
class AudioInterval:
    start_ms: int
    end_ms: int | None


@dataclass(frozen=True)
class ReviewedOverride:
    group: str
    review_basis: str
    component_order: int
    target_action: str
    replace_pc_message_id: str
    pc_message_id: str
    ds_script: str
    ds_settext_ordinal: int
    ds_speaker: str
    pc_speaker: str
    target_text_sha256: tuple[str, ...]
    jp_interval: AudioInterval | None
    en_interval: AudioInterval | None

    def interval(self, language: str) -> AudioInterval | None:
        if language == "jp":
            return self.jp_interval
        if language == "en":
            return self.en_interval
        raise ValueError(f"Unsupported voice language: {language!r}")


def natural_key(value: str) -> list[str | int]:
    return [
        int(part) if part.isdigit() else part.casefold()
        for part in NATURAL_PART.split(value)
    ]


def message_scene(message_id: str) -> str:
    parts = message_id.split("_")
    if len(parts) < 2 or not parts[1].isdigit():
        raise ValueError(f"Cannot determine scene from message ID: {message_id}")
    return parts[1]


def raw_settext_sha256(payload: bytes) -> str:
    """Hash the exact payload bytes supplied by the binary FSB parser."""
    return hashlib.sha256(payload).hexdigest()


def parse_hashes(value: str, *, label: str) -> tuple[str, ...]:
    if not value.strip():
        return ()
    values = tuple(part.strip() for part in value.split("|"))
    if any(not HASH.fullmatch(item) for item in values):
        raise ValueError(f"{label} must contain lowercase SHA-256 values")
    if len(values) != len(set(values)):
        raise ValueError(f"{label} repeats a SHA-256 value")
    return values


def parse_milliseconds(value: str, *, label: str) -> int | None:
    value = value.strip()
    if not value:
        return None
    if not value.isascii() or not value.isdecimal():
        raise ValueError(
            f"{label} must be a non-negative integer number of milliseconds"
        )
    return int(value)


def parse_interval(
    row: Mapping[str, str], language: str, *, label: str
) -> AudioInterval | None:
    start = parse_milliseconds(
        row[f"{language}_start_ms"], label=f"{label} {language}_start_ms"
    )
    end = parse_milliseconds(
        row[f"{language}_end_ms"], label=f"{label} {language}_end_ms"
    )
    if start is None and end is None:
        return None
    if start is None:
        raise ValueError(f"{label} {language}_end_ms requires {language}_start_ms")
    if end is not None and end <= start:
        raise ValueError(f"{label} {language}_end_ms must be greater than its start")
    return AudioInterval(start, end)


def parse_override_row(
    row: Mapping[str, str], row_number: int, *, materialized: bool = False
) -> ReviewedOverride:
    label = f"override row {row_number}"
    group = row["override_group"].strip()
    if not GROUP.fullmatch(group):
        raise ValueError(f"{label} has an invalid override_group")
    review_basis = row["review_basis"].strip()
    accepted_bases = (
        frozenset({MATERIALIZED_REVIEW_BASIS}) if materialized else REVIEW_BASES
    )
    if review_basis not in accepted_bases:
        raise ValueError(
            f"{label} review_basis must be one of {sorted(accepted_bases)}"
        )
    component_order_text = row["component_order"].strip()
    if not component_order_text.isascii() or not component_order_text.isdecimal():
        raise ValueError(f"{label} has an invalid component_order")
    component_order = int(component_order_text)
    target_action = row["target_action"].strip().casefold()
    if target_action not in {"add", "replace"}:
        raise ValueError(f"{label} target_action must be 'add' or 'replace'")
    replace_pc_message_id = row["replace_pc_message_id"].strip()
    if target_action == "add" and replace_pc_message_id:
        raise ValueError(f"{label} cannot replace a PC message when target_action=add")
    if target_action == "replace" and not replace_pc_message_id:
        raise ValueError(
            f"{label} target_action=replace requires one exact baseline PC message"
        )
    if any(character in replace_pc_message_id for character in WILDCARDS):
        raise ValueError(
            f"{label} replacement PC message cannot contain wildcard syntax"
        )
    message_id = row["pc_message_id"].strip()
    if not message_id or any(character in message_id for character in WILDCARDS):
        raise ValueError(f"{label} must name one exact PC message ID")
    script = row["ds_script"].strip()
    if not SCRIPT.fullmatch(script) or Path(script).name != script:
        raise ValueError(f"{label} must name one exact .fsb.txt script basename")
    ordinal_text = row["ds_settext_ordinal"].strip()
    if not ordinal_text.isascii() or not ordinal_text.isdecimal():
        raise ValueError(f"{label} has an invalid DS setText ordinal")
    ordinal = int(ordinal_text)
    ds_speaker = row["ds_speaker"]
    pc_speaker = row["pc_speaker"]
    if not ds_speaker or not pc_speaker:
        raise ValueError(f"{label} must contain both exact speaker names")
    if any(character in ds_speaker + pc_speaker for character in WILDCARDS):
        raise ValueError(f"{label} speaker names cannot contain wildcard syntax")
    jp_interval = parse_interval(row, "jp", label=label)
    en_interval = parse_interval(row, "en", label=label)
    if (jp_interval is None) != (en_interval is None):
        raise ValueError(f"{label} must define slices for both JP and EN or neither")
    return ReviewedOverride(
        group=group,
        review_basis=review_basis,
        component_order=component_order,
        target_action=target_action,
        replace_pc_message_id=replace_pc_message_id,
        pc_message_id=message_id,
        ds_script=script,
        ds_settext_ordinal=ordinal,
        ds_speaker=ds_speaker,
        pc_speaker=pc_speaker,
        target_text_sha256=parse_hashes(
            row["target_text_sha256"], label=f"{label} target_text_sha256"
        ),
        jp_interval=jp_interval,
        en_interval=en_interval,
    )


def runtime_component_values(
    row: Mapping[str, str], field: str, component_count: int, *, label: str
) -> list[str]:
    """Parse one value per ordered component without losing empty positions."""
    raw = row.get(field, "")
    if component_count == 1:
        return [raw.strip()]
    if not raw.strip():
        if row.get("alignment_method", "").strip().casefold() == "reviewed_override":
            raise ValueError(
                f"{label} must encode {component_count} component values with '|' separators"
            )
        return [""] * component_count
    values = [value.strip() for value in raw.split("|")]
    if len(values) != component_count:
        raise ValueError(
            f"{label} encodes {len(values)} component values, expected {component_count}"
        )
    return values


def overrides_from_runtime_row(
    row: Mapping[str, str], row_number: int
) -> list[ReviewedOverride]:
    """Expand the reviewed components carried by one materialized target row."""
    missing = [field for field in RUNTIME_FIELDS if field not in row]
    if missing:
        raise ValueError(
            f"alignment row {row_number} lacks reviewed override columns: {missing}"
        )
    raw_ids = (
        row.get("pc_message_ids", "").strip() or row.get("pc_message_id", "").strip()
    )
    message_ids = [part.strip() for part in raw_ids.split("|") if part.strip()]
    if not message_ids or len(message_ids) != len(set(message_ids)):
        raise ValueError(
            f"alignment row {row_number} has invalid reviewed component IDs"
        )
    declared_count = row.get("message_count", "").strip()
    if not declared_count.isascii() or not declared_count.isdecimal():
        raise ValueError(f"alignment row {row_number} has invalid message_count")
    if int(declared_count) != len(message_ids):
        raise ValueError(
            f"alignment row {row_number} declares {declared_count} reviewed components "
            f"but contains {len(message_ids)} IDs"
        )
    component_fields = {
        field: runtime_component_values(
            row,
            field,
            len(message_ids),
            label=f"alignment row {row_number} {field}",
        )
        for field in ("jp_start_ms", "jp_end_ms", "en_start_ms", "en_end_ms")
    }
    result = []
    for component_order, message_id in enumerate(message_ids):
        ledger_row = {
            "override_group": row["reviewed_override_group"],
            "review_basis": MATERIALIZED_REVIEW_BASIS,
            "component_order": str(component_order),
            # Replacement claims are consumed while materializing the ledger;
            # carrying them into the runtime map would grant no extra safety.
            "target_action": "add",
            "replace_pc_message_id": "",
            "pc_message_id": message_id,
            "ds_script": row["ds_script"],
            "ds_settext_ordinal": row["ds_settext_ordinal"],
            "ds_speaker": row["ds_speaker"],
            "pc_speaker": row["pc_speaker"],
            "target_text_sha256": row["target_text_sha256"],
            **{
                field: values[component_order]
                for field, values in component_fields.items()
            },
        }
        result.append(parse_override_row(ledger_row, row_number, materialized=True))
    return result


def override_from_runtime_row(
    row: Mapping[str, str], row_number: int
) -> ReviewedOverride:
    """Parse a legacy single-component reviewed target."""
    result = overrides_from_runtime_row(row, row_number)
    if len(result) != 1:
        raise ValueError(
            f"alignment row {row_number} contains {len(result)} reviewed components"
        )
    return result[0]


def _validate_language_slices(
    label: str, rows: Sequence[ReviewedOverride], language: str
) -> None:
    intervals = [row.interval(language) for row in rows]
    if any(interval is None for interval in intervals):
        raise ValueError(f"{label} lacks {language.upper()} slice metadata")
    concrete = [interval for interval in intervals if interval is not None]
    if concrete[0].start_ms != 0:
        raise ValueError(f"{label} must start its {language.upper()} audio at 0 ms")
    for index, (left, right) in enumerate(zip(concrete, concrete[1:])):
        if left.end_ms is None:
            raise ValueError(
                f"Only the final {language.upper()} slice in {label} may have an open end"
            )
        if right.start_ms < left.end_ms:
            raise ValueError(f"{label} has overlapping {language.upper()} slices")
        if index + 1 < len(concrete) - 1 and right.end_ms is None:
            raise ValueError(
                f"Only the final {language.upper()} slice in {label} may have an open end"
            )


def validate_overrides(rows: Sequence[ReviewedOverride]) -> None:
    if not rows:
        raise ValueError("Reviewed override ledger is empty")
    by_target: dict[tuple[str, int], list[ReviewedOverride]] = defaultdict(list)
    by_group: dict[str, list[ReviewedOverride]] = defaultdict(list)
    groups_by_message: dict[str, set[str]] = defaultdict(set)
    for row in rows:
        target = (row.ds_script.casefold(), row.ds_settext_ordinal)
        by_target[target].append(row)
        by_group[row.group].append(row)
        groups_by_message[row.pc_message_id].add(row.group)

    for target, components in by_target.items():
        ordered = sorted(components, key=lambda row: row.component_order)
        orders = [row.component_order for row in ordered]
        if orders != list(range(len(ordered))):
            raise ValueError(
                f"Reviewed override target {target} component_order values must be 0.."
                f"{len(ordered) - 1}"
            )
        if len({row.pc_message_id for row in ordered}) != len(ordered):
            raise ValueError(
                f"Reviewed override target {target} repeats a PC component"
            )
        target_metadata = {
            (
                row.group,
                row.review_basis,
                row.ds_speaker,
                row.pc_speaker,
                row.target_text_sha256,
                row.target_action,
                row.replace_pc_message_id,
            )
            for row in ordered
        }
        if len(target_metadata) != 1:
            raise ValueError(
                f"Reviewed override target {target} has inconsistent component metadata"
            )
        first = ordered[0]
        if first.target_action == "replace" and first.replace_pc_message_id not in {
            component.pc_message_id for component in ordered
        }:
            raise ValueError(
                f"Reviewed replacement target {target} must retain its exact baseline "
                f"message {first.replace_pc_message_id!r} as a component"
            )

    for message_id, groups in groups_by_message.items():
        if len(groups) != 1:
            raise ValueError(
                f"PC message {message_id!r} is reused across override groups: {sorted(groups)}"
            )

    for group, members in by_group.items():
        unique_targets = sorted(
            {(row.ds_script.casefold(), row.ds_settext_ordinal) for row in members}
        )
        scripts = {script for script, _ordinal in unique_targets}
        ordinals = [ordinal for _script, ordinal in unique_targets]
        if len(scripts) != 1 or any(
            right != left + 1 for left, right in zip(ordinals, ordinals[1:])
        ):
            raise ValueError(
                f"Override group {group!r} must target consecutive ordinals in one DS script"
            )
        bases = {row.review_basis for row in members}
        if len(bases) != 1:
            raise ValueError(f"Override group {group!r} mixes review_basis values")
        basis = next(iter(bases))
        if basis == MATERIALIZED_REVIEW_BASIS:
            continue
        target_sizes = [len(by_target[target]) for target in unique_targets]
        message_counts = defaultdict(int)
        for row in members:
            message_counts[row.pc_message_id] += 1
        if basis == "speaker_alias":
            if len(members) != 1 or members[0].ds_speaker == members[0].pc_speaker:
                raise ValueError(
                    f"Speaker-alias group {group!r} must contain one exact differing "
                    "speaker pair"
                )
            if members[0].jp_interval is not None:
                raise ValueError(f"Speaker-alias group {group!r} cannot slice audio")
        elif basis == "multi_page_split":
            if any(size != 1 for size in target_sizes) or not any(
                count > 1 for count in message_counts.values()
            ):
                raise ValueError(
                    f"Multi-page split group {group!r} must reuse one sliced message "
                    "across single-component targets"
                )
            if any(row.target_action != "add" for row in members):
                raise ValueError(
                    f"Multi-page split group {group!r} cannot replace a baseline target"
                )
        elif basis == "multi_page_composite":
            if not any(size > 1 for size in target_sizes) or not any(
                row.target_action == "replace" for row in members
            ):
                raise ValueError(
                    f"Multi-page composite group {group!r} must compose messages and "
                    "replace an exact baseline target"
                )
        elif basis == "translated_dialogue":
            if len(members) != 1 or not members[0].target_text_sha256:
                raise ValueError(
                    f"Translated-dialogue group {group!r} must contain one "
                    "target restricted by a raw text hash"
                )

    by_message: dict[str, list[ReviewedOverride]] = defaultdict(list)
    for row in rows:
        by_message[row.pc_message_id].append(row)
    for message_id, members in by_message.items():
        if len(members) == 1:
            continue
        ordered = sorted(
            members,
            key=lambda row: (
                row.ds_script.casefold(),
                row.ds_settext_ordinal,
                row.component_order,
            ),
        )
        targets = [
            (row.ds_script.casefold(), row.ds_settext_ordinal) for row in ordered
        ]
        if len(set(targets)) != len(targets):
            raise ValueError(
                f"Split PC message {message_id!r} may occur only once per DS target"
            )
        scripts = {script for script, _ordinal in targets}
        ordinals = [ordinal for _script, ordinal in targets]
        if len(scripts) != 1 or any(
            right != left + 1 for left, right in zip(ordinals, ordinals[1:])
        ):
            raise ValueError(
                f"Split PC message {message_id!r} must target consecutive DS ordinals"
            )
        _validate_language_slices(f"split PC message {message_id!r}", ordered, "jp")
        _validate_language_slices(f"split PC message {message_id!r}", ordered, "en")


def read_overrides(path: Path) -> list[ReviewedOverride]:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        fields = reader.fieldnames
        if not fields:
            raise ValueError(f"Empty reviewed override ledger: {path}")
        missing = set(LEDGER_FIELDS) - set(fields)
        if missing:
            raise ValueError(f"{path} lacks columns: {sorted(missing)}")
        rows = [parse_override_row(row, number) for number, row in enumerate(reader, 2)]
    validate_overrides(rows)
    return rows


def interval_fields(override: ReviewedOverride) -> dict[str, str]:
    result: dict[str, str] = {}
    for language in ("jp", "en"):
        interval = override.interval(language)
        result[f"{language}_start_ms"] = (
            "" if interval is None else str(interval.start_ms)
        )
        result[f"{language}_end_ms"] = (
            "" if interval is None or interval.end_ms is None else str(interval.end_ms)
        )
    return result


def component_interval_fields(
    components: Sequence[ReviewedOverride],
) -> dict[str, str]:
    """Serialize one interval slot per component in deterministic order."""
    values: dict[str, list[str]] = {
        "jp_start_ms": [],
        "jp_end_ms": [],
        "en_start_ms": [],
        "en_end_ms": [],
    }
    for component in components:
        current = interval_fields(component)
        for field in values:
            values[field].append(current[field])
    return {
        field: values_for_field[0]
        if len(values_for_field) == 1
        else " | ".join(values_for_field)
        for field, values_for_field in values.items()
    }


def _message_ids(row: Mapping[str, str]) -> list[str]:
    raw = row.get("pc_message_ids", "").strip() or row.get("pc_message_id", "").strip()
    return [part.strip() for part in raw.split("|") if part.strip()]


def _target(row: Mapping[str, str]) -> tuple[str, int]:
    return (Path(row["ds_script"]).name.casefold(), int(row["ds_settext_ordinal"]))


def apply_overrides(
    base_rows: Sequence[Mapping[str, str]],
    overrides: Sequence[ReviewedOverride],
    pc_by_id: Mapping[str, Mapping[str, Any]],
    ds_by_group: Mapping[str, Sequence[Mapping[str, Any]]],
) -> list[dict[str, str]]:
    """Return the baseline plus strictly resolved reviewed overrides."""
    validate_overrides(overrides)
    base_by_target: dict[tuple[str, int], Mapping[str, str]] = {}
    base_targets_by_message: dict[str, set[tuple[str, int]]] = defaultdict(set)
    for row in base_rows:
        target = _target(row)
        if target in base_by_target:
            raise ValueError(f"Baseline alignment repeats DS target {target}")
        base_by_target[target] = row
        for message_id in _message_ids(row):
            base_targets_by_message[message_id].add(target)

    components_by_target: dict[tuple[str, int], list[ReviewedOverride]] = defaultdict(
        list
    )
    for override in overrides:
        components_by_target[
            (override.ds_script.casefold(), override.ds_settext_ordinal)
        ].append(override)
    for components in components_by_target.values():
        components.sort(key=lambda component: component.component_order)

    replacement_by_target: dict[tuple[str, int], str] = {}
    for target, components in components_by_target.items():
        first = components[0]
        baseline = base_by_target.get(target)
        if first.target_action == "add":
            if baseline is not None:
                raise ValueError(
                    f"Reviewed add targets an occupied DS line: {first.ds_script}:"
                    f"{first.ds_settext_ordinal}"
                )
            continue
        if baseline is None:
            raise ValueError(
                f"Reviewed replacement target is absent from baseline: {first.ds_script}:"
                f"{first.ds_settext_ordinal}"
            )
        baseline_messages = _message_ids(baseline)
        if baseline_messages != [first.replace_pc_message_id]:
            raise ValueError(
                f"Reviewed replacement expected baseline PC message "
                f"{first.replace_pc_message_id!r} at {first.ds_script}:"
                f"{first.ds_settext_ordinal}, found {baseline_messages!r}"
            )
        replacement_by_target[target] = first.replace_pc_message_id

    override_targets_by_message: dict[str, set[tuple[str, int]]] = defaultdict(set)
    for target, components in components_by_target.items():
        for component in components:
            override_targets_by_message[component.pc_message_id].add(target)
    for message_id, baseline_targets in base_targets_by_message.items():
        if message_id not in override_targets_by_message:
            continue
        for target in baseline_targets:
            if replacement_by_target.get(target) != message_id:
                raise ValueError(
                    f"Reviewed override reuses baseline PC message {message_id!r} "
                    f"without replacing its exact target {target}"
                )
            if target not in override_targets_by_message[message_id]:
                raise ValueError(
                    f"Reviewed replacement removes PC message {message_id!r} without "
                    "retaining it at the replaced target"
                )

    result = [
        dict(row) for row in base_rows if _target(row) not in replacement_by_target
    ]
    for target, components in components_by_target.items():
        first = components[0]
        group = first.ds_script[: -len(".fsb.txt")].casefold()
        ds_records = ds_by_group.get(group)
        if ds_records is None or first.ds_settext_ordinal >= len(ds_records):
            raise ValueError(
                f"Reviewed override target does not exist: {first.ds_script}:"
                f"{first.ds_settext_ordinal}"
            )
        ds = ds_records[first.ds_settext_ordinal]
        if int(ds.get("ordinal", first.ds_settext_ordinal)) != first.ds_settext_ordinal:
            raise ValueError(
                f"DS manifest ordinal mismatch at {first.ds_script}:"
                f"{first.ds_settext_ordinal}"
            )
        if ds["speaker"] != first.ds_speaker:
            raise ValueError(
                f"Reviewed override DS speaker changed at {first.ds_script}:"
                f"{first.ds_settext_ordinal}: expected {first.ds_speaker!r}, "
                f"found {ds['speaker']!r}"
            )
        pc_records = []
        paths = []
        for component in components:
            pc = pc_by_id.get(component.pc_message_id)
            if pc is None:
                raise ValueError(
                    f"Reviewed override PC message does not exist: "
                    f"{component.pc_message_id}"
                )
            if pc["speaker"] != component.pc_speaker:
                raise ValueError(
                    f"Reviewed override PC speaker changed for {component.pc_message_id}: "
                    f"expected {component.pc_speaker!r}, found {pc['speaker']!r}"
                )
            component_paths = list(pc["paths"])
            if not component_paths:
                raise ValueError(
                    f"Reviewed override has no JP voice: {component.pc_message_id}"
                )
            pc_records.append(pc)
            paths.extend(component_paths)
        scenes = {message_scene(component.pc_message_id) for component in components}
        if len(scenes) != 1:
            raise ValueError(
                f"Reviewed override target {target} combines PC messages from "
                f"different scenes: {sorted(scenes)}"
            )
        row = {
            "ds_script": first.ds_script,
            "ds_source_line": str(ds["line"]),
            "ds_settext_ordinal": str(first.ds_settext_ordinal),
            "ds_speaker": first.ds_speaker,
            "ds_text": str(ds["text"]),
            "pc_message_id": " | ".join(
                component.pc_message_id for component in components
            ),
            "pc_speaker": first.pc_speaker,
            "pc_en_text": " || ".join(str(pc["en_text"]) for pc in pc_records),
            "pc_jp_speaker_and_text": " || ".join(str(pc["jp"]) for pc in pc_records),
            "jp_ogg_paths": " | ".join(paths),
            "ogg_segment_count": str(len(paths)),
            "alignment_method": "reviewed_override",
            "normalized_text_exact": "reviewed",
            "char_tfidf_cosine": "reviewed",
            "sequence_ratio": "reviewed",
            "confidence": "review",
            "pc_scene": next(iter(scenes)),
            "scene_range_source": "reviewed_override",
            "scene_max_per_line": "1",
            "selection_status": "selected",
            "message_count": str(len(components)),
            "speaker_policy": (
                "same"
                if first.ds_speaker == first.pc_speaker
                else "reviewed_equivalent"
            ),
            "reviewed_override_group": first.group,
            "target_text_sha256": " | ".join(first.target_text_sha256),
            **component_interval_fields(components),
        }
        result.append(row)

    result.sort(
        key=lambda row: (
            Path(row["ds_script"]).name.casefold(),
            int(row["ds_settext_ordinal"]),
            natural_key(row["pc_message_id"]),
        )
    )
    return result


def read_tsv(path: Path) -> tuple[list[str], list[dict[str, str]]]:
    with path.open(encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source, delimiter="\t")
        if not reader.fieldnames:
            raise ValueError(f"Empty TSV: {path}")
        return list(reader.fieldnames), list(reader)


def write_tsv(
    path: Path, fields: Sequence[str], rows: Iterable[Mapping[str, str]]
) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as destination:
        writer = csv.DictWriter(
            destination,
            fieldnames=list(fields),
            delimiter="\t",
            lineterminator="\n",
            extrasaction="ignore",
        )
        writer.writeheader()
        writer.writerows(rows)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    # Keeping the TF-IDF aligner import out of library initialization lets the
    # bank builder validate override metadata without requiring scikit-learn.
    from align_dialogue import read_ds_scripts, read_pc_manifest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--alignment", type=Path, default=DEFAULT_ALIGNMENT)
    parser.add_argument("--pc-manifest", type=Path, default=DEFAULT_PC_MANIFEST)
    parser.add_argument("--ds-scripts", type=Path, default=DEFAULT_DS_SCRIPTS)
    parser.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()

    fields, baseline = read_tsv(args.alignment)
    pc_by_group = read_pc_manifest(args.pc_manifest)
    pc_by_id = {
        item["id"]: item for messages in pc_by_group.values() for item in messages
    }
    if len(pc_by_id) != sum(len(messages) for messages in pc_by_group.values()):
        raise ValueError("PC manifest repeats a message ID")
    ds_by_group, _ = read_ds_scripts(args.ds_scripts)
    overrides = read_overrides(args.ledger)
    rows = apply_overrides(baseline, overrides, pc_by_id, ds_by_group)
    output_fields = [
        *fields,
        *(field for field in RUNTIME_FIELDS if field not in fields),
    ]
    digest = write_tsv(args.output, output_fields, rows)
    report = {
        "baseline_targets": len(baseline),
        "reviewed_override_targets": len(
            {(row.ds_script.casefold(), row.ds_settext_ordinal) for row in overrides}
        ),
        "reviewed_override_components": len(overrides),
        "split_groups": sum(
            len(
                {
                    (row.ds_script.casefold(), row.ds_settext_ordinal)
                    for row in overrides
                    if row.group == group
                }
            )
            > 1
            for group in {row.group for row in overrides}
        ),
        "composite_targets": sum(
            count > 1
            for count in (
                sum(
                    other.ds_script.casefold() == row.ds_script.casefold()
                    and other.ds_settext_ordinal == row.ds_settext_ordinal
                    for other in overrides
                )
                for row in overrides
                if row.component_order == 0
            )
        ),
        "total_targets": len(rows),
        "output_sha256": digest,
    }
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as error:
        raise SystemExit(f"error: {error}") from error
