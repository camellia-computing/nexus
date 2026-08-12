#!/usr/bin/env python3
"""Build and validate the offline Core release/configuration-surface catalog.

Normal checks are deterministic and network-free.  Maintainers explicitly pass
``--rebuild --mirror-root`` to inspect local mirrors and ``--write`` to replace
the checked-in catalog.  A rebuild never executes upstream code.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
CATALOG_PATH = ROOT / "crates" / "camellia-nexus-core" / "core-compatibility-catalog.json"
SCHEMA_VERSION = 1
EXTRACTOR_REVISION = "core-history-v1-20260811"
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
UTC_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
FIELD_PATTERN = re.compile(r"^\s*(?P<field>[A-Za-z_][A-Za-z0-9_]*)\s+.*`(?P<tags>[^`]*)`")
SERDE_TAG_PATTERN = re.compile(r"(?:^|\s)(?P<encoding>json|yaml):\"(?P<value>[^\"]*)\"")
SEMVERISH_PATTERN = re.compile(
    r"^[vV]?(?P<major>\d+)\.(?P<minor>\d+)(?:\.(?P<patch>\d+))?"
    r"(?:-(?P<pre>[0-9A-Za-z.-]+))?(?:\+(?P<build>[0-9A-Za-z.-]+))?$"
)
COMPACT_PRERELEASE_PATTERN = re.compile(r"^(?P<label>[A-Za-z]+)(?P<number>\d+)$")


@dataclass(frozen=True)
class ProgramSource:
    program: str
    repository: str
    mirror_name: str
    development_ref: str
    path_prefixes: tuple[str, ...]


PROGRAMS = (
    ProgramSource("xray", "XTLS/Xray-core", "xray.git", "main", ("infra/conf/",)),
    ProgramSource(
        "mihomo",
        "MetaCubeX/mihomo",
        "mihomo.git",
        "Alpha",
        (
            "adapter/",
            "component/",
            "config/",
            "dns/",
            "listener/",
            "rules/",
            "transport/",
            "tunnel/",
        ),
    ),
    ProgramSource("singBox", "SagerNet/sing-box", "sing-box.git", "testing", ("option/",)),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rebuild", action="store_true", help="rebuild from local bare mirrors")
    parser.add_argument("--write", action="store_true", help="write the rebuilt catalog")
    parser.add_argument("--mirror-root", type=Path, help="directory containing the three bare mirrors")
    return parser.parse_args()


def git(mirror: Path, *args: str, allow_failure: bool = False) -> str:
    process = subprocess.run(
        ["git", "-C", str(mirror), *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if process.returncode and not allow_failure:
        raise RuntimeError(
            f"git {' '.join(args)} failed for {mirror}: {process.stderr.strip()}"
        )
    return process.stdout if process.returncode == 0 else ""


def normalize_version_tag(tag: str) -> str | None:
    candidate = tag.strip()
    match = SEMVERISH_PATTERN.fullmatch(candidate)
    if not match:
        return None
    major = int(match.group("major"))
    minor = int(match.group("minor"))
    patch = int(match.group("patch") or 0)
    pre = match.group("pre")
    if pre:
        parts: list[str] = []
        for part in pre.split("."):
            compact = COMPACT_PRERELEASE_PATTERN.fullmatch(part)
            if compact:
                parts.extend((compact.group("label").lower(), str(int(compact.group("number")))))
            else:
                parts.append(part.lower())
        pre = ".".join(parts)
    normalized = f"{major}.{minor}.{patch}"
    if pre:
        normalized += f"-{pre}"
    if match.group("build"):
        normalized += f"+{match.group('build').lower()}"
    return normalized


def version_sort_key(version: str) -> tuple[Any, ...]:
    main, _, suffix = version.partition("-")
    major, minor, patch = (int(part) for part in main.split("."))
    if not suffix:
        prerelease: tuple[Any, ...] = ((2, ""),)
    else:
        prerelease = tuple(
            (0, int(part)) if part.isdigit() else (1, part)
            for part in suffix.split("+")[0].split(".")
        ) + ((-1, ""),)
    return major, minor, patch, prerelease


def utc_second(value: str) -> str:
    parsed = datetime.fromisoformat(value.strip().replace("Z", "+00:00"))
    return parsed.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def version_records(source: ProgramSource, mirror: Path) -> tuple[list[dict[str, Any]], list[str]]:
    tags = sorted(
        line.strip()
        for line in git(mirror, "for-each-ref", "--format=%(refname:short)", "refs/tags").splitlines()
        if line.strip()
    )
    records: list[dict[str, Any]] = []
    excluded: list[str] = []
    for tag in tags:
        normalized = normalize_version_tag(tag)
        if normalized is None:
            excluded.append(tag)
            continue
        commit = git(mirror, "rev-parse", f"{tag}^{{commit}}").strip()
        if not SHA_PATTERN.fullmatch(commit):
            raise RuntimeError(f"{source.program} tag {tag} did not peel to a commit")
        timestamp = utc_second(git(mirror, "show", "-s", "--format=%cI", commit))
        records.append(
            {
                "tag": tag,
                "normalizedVersion": normalized,
                "commitSha": commit,
                "sourceTimestamp": timestamp,
                "sourceUrl": f"https://github.com/{source.repository}/tree/{tag}",
                "prerelease": "-" in normalized,
            }
        )
    records.sort(key=lambda value: (version_sort_key(value["normalizedVersion"]), value["tag"]))
    return records, excluded


def config_surface_at(mirror: Path, tag: str, prefixes: tuple[str, ...]) -> dict[str, dict[str, str]]:
    output = git(
        mirror,
        "grep",
        "-n",
        "-E",
        r"(json|yaml):\"",
        tag,
        "--",
        "*.go",
        allow_failure=True,
    )
    observed: dict[str, dict[str, str]] = {}
    for raw_line in output.splitlines():
        parts = raw_line.split(":", 3)
        if len(parts) != 4:
            continue
        _, path, line_number, content = parts
        if not path.startswith(prefixes):
            continue
        field_match = FIELD_PATTERN.match(content)
        if not field_match:
            continue
        for tag_match in SERDE_TAG_PATTERN.finditer(field_match.group("tags")):
            encoded = tag_match.group("value").split(",", 1)[0]
            if not encoded or encoded == "-":
                continue
            encoding = tag_match.group("encoding")
            go_field = field_match.group("field")
            identity = f"{encoding}|{path}|{go_field}|{encoded}"
            observed[identity] = {
                "encoding": encoding,
                "name": encoded,
                "goField": go_field,
                "sourcePath": path,
                "sourceLine": line_number,
            }
    return observed


def surface_records(
    source: ProgramSource,
    mirror: Path,
    versions: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    definitions: dict[str, dict[str, str]] = {}
    events: dict[str, list[dict[str, Any]]] = defaultdict(list)
    previous: set[str] = set()
    for index, version in enumerate(versions):
        current_records = config_surface_at(mirror, version["tag"], source.path_prefixes)
        current = set(current_records)
        for identity, definition in current_records.items():
            definitions.setdefault(identity, definition)
        for identity in sorted(current - previous):
            events[identity].append(
                {"version": version["normalizedVersion"], "tag": version["tag"], "state": "present"}
            )
        for identity in sorted(previous - current):
            events[identity].append(
                {"version": version["normalizedVersion"], "tag": version["tag"], "state": "absent"}
            )
        previous = current
        if index and index % 50 == 0:
            print(f"  {source.program}: inspected {index}/{len(versions)} versions", file=sys.stderr)
    records = []
    for identity in sorted(definitions):
        definition = definitions[identity]
        digest = hashlib.sha256(f"{source.program}|{identity}".encode()).hexdigest()[:20]
        records.append(
            {
                "id": f"{source.program}.surface.{digest}",
                **definition,
                "events": events[identity],
            }
        )
    return records


def manual_semantic_features() -> list[dict[str, Any]]:
    ids = (
        ("core.cli.nativeValidation", "cli"),
        ("core.cli.generatedSchema", "cli"),
        ("proxy.outbound.vless", "protocol"),
        ("proxy.outbound.shadowsocks", "protocol"),
        ("proxy.outbound.hysteria2", "protocol"),
        ("proxy.outbound.tuicV5", "protocol"),
        ("config.logging.level", "guided"),
        ("config.dns.strategy", "guided"),
        ("config.routing.autoDetectInterface", "guided"),
        ("config.routing.domainStrategy", "guided"),
        ("config.tun.strictRoute", "guided"),
        ("runtime.dashboard", "runtime"),
    )
    return [
        {
            "id": feature_id,
            "domain": domain,
            "defaultAvailability": "unknown",
            "events": [],
            "evidenceRevision": "semantic-overlay-v1-20260811",
        }
        for feature_id, domain in ids
    ]


def rebuild(mirror_root: Path) -> dict[str, Any]:
    programs: list[dict[str, Any]] = []
    latest_timestamp = "1970-01-01T00:00:00Z"
    for source in PROGRAMS:
        mirror = mirror_root / source.mirror_name
        if not mirror.is_dir():
            raise RuntimeError(f"missing bare mirror: {mirror}")
        print(f"Inspecting {source.repository}", file=sys.stderr)
        versions, excluded = version_records(source, mirror)
        if not versions:
            raise RuntimeError(f"{source.program} has no version records")
        latest_timestamp = max(latest_timestamp, *(value["sourceTimestamp"] for value in versions))
        programs.append(
            {
                "program": source.program,
                "repository": source.repository,
                "developmentRef": source.development_ref,
                "versions": versions,
                "excludedTags": excluded,
                "surface": surface_records(source, mirror, versions),
            }
        )
    return {
        "schemaVersion": SCHEMA_VERSION,
        "extractorRevision": EXTRACTOR_REVISION,
        "sourceSnapshotAt": latest_timestamp,
        "programs": programs,
        "semanticFeatures": manual_semantic_features(),
    }


def expect_exact_keys(errors: list[str], value: Any, expected: set[str], label: str) -> bool:
    if not isinstance(value, dict):
        errors.append(f"{label} must be an object")
        return False
    actual = set(value)
    if actual != expected:
        errors.append(
            f"{label} keys differ: missing={sorted(expected - actual)}, unknown={sorted(actual - expected)}"
        )
        return False
    return True


def validate_catalog(catalog: Any) -> list[str]:
    errors: list[str] = []
    root_keys = {"schemaVersion", "extractorRevision", "sourceSnapshotAt", "programs", "semanticFeatures"}
    if not expect_exact_keys(errors, catalog, root_keys, "catalog"):
        return errors
    if catalog["schemaVersion"] != SCHEMA_VERSION:
        errors.append(f"catalog schemaVersion must be {SCHEMA_VERSION}")
    if catalog["extractorRevision"] != EXTRACTOR_REVISION:
        errors.append(f"catalog extractorRevision must be {EXTRACTOR_REVISION}")
    if not UTC_PATTERN.fullmatch(str(catalog["sourceSnapshotAt"])):
        errors.append("catalog sourceSnapshotAt must be second-precision UTC")
    seen_programs: set[str] = set()
    for index, program in enumerate(catalog.get("programs", [])):
        label = f"programs[{index}]"
        keys = {"program", "repository", "developmentRef", "versions", "excludedTags", "surface"}
        if not expect_exact_keys(errors, program, keys, label):
            continue
        seen_programs.add(program["program"])
        versions: set[str] = set()
        for version_index, version in enumerate(program["versions"]):
            version_label = f"{label}.versions[{version_index}]"
            version_keys = {
                "tag",
                "normalizedVersion",
                "commitSha",
                "sourceTimestamp",
                "sourceUrl",
                "prerelease",
            }
            if not expect_exact_keys(errors, version, version_keys, version_label):
                continue
            normalized = version["normalizedVersion"]
            if normalize_version_tag(normalized) != normalized:
                errors.append(f"{version_label}.normalizedVersion is not canonical")
            versions.add(normalized)
            if not SHA_PATTERN.fullmatch(str(version["commitSha"])):
                errors.append(f"{version_label}.commitSha is invalid")
            if not UTC_PATTERN.fullmatch(str(version["sourceTimestamp"])):
                errors.append(f"{version_label}.sourceTimestamp is invalid")
        surface_ids: set[str] = set()
        for surface_index, surface in enumerate(program["surface"]):
            surface_label = f"{label}.surface[{surface_index}]"
            surface_keys = {"id", "encoding", "name", "goField", "sourcePath", "sourceLine", "events"}
            if not expect_exact_keys(errors, surface, surface_keys, surface_label):
                continue
            if surface["id"] in surface_ids:
                errors.append(f"{label} contains duplicate surface id {surface['id']}")
            surface_ids.add(surface["id"])
            if surface["encoding"] not in {"json", "yaml"}:
                errors.append(f"{surface_label}.encoding is invalid")
            for event in surface["events"]:
                if event.get("version") not in versions or event.get("state") not in {"present", "absent"}:
                    errors.append(f"{surface_label} has an invalid event")
    expected_programs = {source.program for source in PROGRAMS}
    if seen_programs != expected_programs:
        errors.append(f"catalog program set must be {sorted(expected_programs)}")
    feature_ids: set[str] = set()
    for index, feature in enumerate(catalog.get("semanticFeatures", [])):
        label = f"semanticFeatures[{index}]"
        keys = {"id", "domain", "defaultAvailability", "events", "evidenceRevision"}
        if not expect_exact_keys(errors, feature, keys, label):
            continue
        if feature["id"] in feature_ids:
            errors.append(f"duplicate semantic feature {feature['id']}")
        feature_ids.add(feature["id"])
        if feature["defaultAvailability"] not in {"supported", "unsupported", "unknown"}:
            errors.append(f"{label}.defaultAvailability is invalid")
    return errors


def load_catalog() -> Any:
    try:
        return json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read {CATALOG_PATH}: {error}") from error


def canonical_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=False) + "\n"


def main() -> int:
    args = parse_args()
    if args.write and not args.rebuild:
        print("error: --write requires --rebuild", file=sys.stderr)
        return 2
    try:
        if args.rebuild:
            if args.mirror_root is None:
                raise RuntimeError("--rebuild requires --mirror-root")
            catalog = rebuild(args.mirror_root.resolve())
            content = canonical_json(catalog)
            if args.write:
                CATALOG_PATH.write_text(content, encoding="utf-8", newline="\n")
            elif CATALOG_PATH.exists() and CATALOG_PATH.read_text(encoding="utf-8") != content:
                raise RuntimeError("rebuilt compatibility catalog differs from the checked-in file")
        else:
            catalog = load_catalog()
        errors = validate_catalog(catalog)
        if errors:
            for error in errors:
                print(f"error: {error}", file=sys.stderr)
            return 1
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "Core compatibility catalog passed: "
        + ", ".join(
            f"{program['program']}={len(program['versions'])} versions/{len(program['surface'])} fields"
            for program in catalog["programs"]
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
