#!/usr/bin/env python3
"""Validate the pinned dual-channel Core manifest, optionally against GitHub.

The script is deliberately read-only.  It never rewrites the manifest because
an upstream movement must be reviewed together with capability code, fixtures,
and the append-only update record described in docs/core-upstream-tracking.md.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = ROOT / "crates" / "camellia-nexus-core" / "core-upstream-versions.json"
EXPECTED = {
    "xray": ("XTLS/Xray-core", "main"),
    "mihomo": ("MetaCubeX/mihomo", "Alpha"),
    "singBox": ("SagerNet/sing-box", "testing"),
}
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
UTC_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--live",
        action="store_true",
        help="also compare every symbolic branch/latest policy with the current GitHub API result",
    )
    return parser.parse_args()


def fail(errors: list[str], message: str) -> None:
    errors.append(message)


def validate_resolution(
    errors: list[str],
    label: str,
    repository: str,
    resolution: Any,
    *,
    development_ref: str | None,
) -> None:
    if not isinstance(resolution, dict):
        fail(errors, f"{label} must be an object")
        return
    required = {
        "policy",
        "trackedRef",
        "resolvedRef",
        "commitSha",
        "sourceTimestamp",
        "sourceUrl",
    }
    missing = sorted(required - resolution.keys())
    unexpected = sorted(set(resolution) - required)
    if missing:
        fail(errors, f"{label} is missing {', '.join(missing)}")
    if unexpected:
        fail(errors, f"{label} contains unknown fields: {', '.join(unexpected)}")
    if missing:
        return
    if development_ref is not None:
        if resolution["policy"] != "branchHead":
            fail(errors, f"{label}.policy must be branchHead")
        if resolution["trackedRef"] != development_ref:
            fail(errors, f"{label}.trackedRef must be case-exact {development_ref}")
        if resolution["resolvedRef"] != resolution["commitSha"]:
            fail(errors, f"{label}.resolvedRef must equal its immutable commitSha")
    else:
        if resolution["policy"] != "latestStableRelease":
            fail(errors, f"{label}.policy must be latestStableRelease")
        if resolution["trackedRef"] != "latest":
            fail(errors, f"{label}.trackedRef must be latest")
        if "-alpha" in resolution["resolvedRef"].lower() or "-beta" in resolution["resolvedRef"].lower() or "-rc" in resolution["resolvedRef"].lower():
            fail(errors, f"{label}.resolvedRef must not be a prerelease tag")
    if not SHA_PATTERN.fullmatch(str(resolution["commitSha"])):
        fail(errors, f"{label}.commitSha must be a lowercase 40-character SHA")
    if not UTC_PATTERN.fullmatch(str(resolution["sourceTimestamp"])):
        fail(errors, f"{label}.sourceTimestamp must be second-precision UTC RFC 3339")
    if not str(resolution["sourceUrl"]).startswith(f"https://github.com/{repository}/"):
        fail(errors, f"{label}.sourceUrl must belong to {repository}")


def load_and_validate() -> tuple[dict[str, Any], list[str]]:
    errors: list[str] = []
    try:
        manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return {}, [f"cannot read {MANIFEST_PATH}: {error}"]
    if set(manifest) != {"schemaVersion", "observedAt", "compatibilityCatalogRevision", "programs"}:
        fail(
            errors,
            "manifest root must contain only schemaVersion, observedAt, compatibilityCatalogRevision, and programs",
        )
    if manifest.get("schemaVersion") != 2:
        fail(errors, "manifest schemaVersion must be 2")
    if not isinstance(manifest.get("compatibilityCatalogRevision"), str) or not manifest["compatibilityCatalogRevision"].strip():
        fail(errors, "manifest compatibilityCatalogRevision must be non-empty")
    if not UTC_PATTERN.fullmatch(str(manifest.get("observedAt", ""))):
        fail(errors, "manifest observedAt must be second-precision UTC RFC 3339")
    programs = manifest.get("programs")
    if not isinstance(programs, list):
        return manifest, errors + ["manifest programs must be an array"]
    seen: set[str] = set()
    for index, program in enumerate(programs):
        label = f"programs[{index}]"
        if not isinstance(program, dict):
            fail(errors, f"{label} must be an object")
            continue
        if set(program) != {"program", "repository", "development", "stable"}:
            fail(errors, f"{label} has missing or unknown fields")
            continue
        kind = program["program"]
        if kind in seen:
            fail(errors, f"duplicate program {kind}")
        seen.add(kind)
        if kind not in EXPECTED:
            fail(errors, f"unsupported tracked program {kind}")
            continue
        repository, development_ref = EXPECTED[kind]
        if program["repository"] != repository:
            fail(errors, f"{kind}.repository must be {repository}")
        validate_resolution(
            errors,
            f"{kind}.development",
            repository,
            program["development"],
            development_ref=development_ref,
        )
        validate_resolution(
            errors,
            f"{kind}.stable",
            repository,
            program["stable"],
            development_ref=None,
        )
    if seen != set(EXPECTED):
        fail(errors, f"manifest program set must be {', '.join(EXPECTED)}")
    return manifest, errors


def github_json(path: str) -> dict[str, Any]:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "camellia-nexus-core-upstream-check",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(f"https://api.github.com{path}", headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, json.JSONDecodeError) as error:
        raise RuntimeError(f"GitHub API request failed for {path}: {error}") from error


def compare_field(errors: list[str], label: str, field: str, recorded: Any, live: Any) -> None:
    if recorded != live:
        fail(errors, f"{label}.{field} is stale: recorded {recorded!r}, live {live!r}")


def validate_live(manifest: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    for program in manifest["programs"]:
        kind = program["program"]
        repository = program["repository"]
        development = program["development"]
        branch_name = urllib.parse.quote(development["trackedRef"], safe="")
        branch = github_json(f"/repos/{repository}/branches/{branch_name}")
        branch_commit = branch["commit"]
        label = f"{kind}.development"
        compare_field(errors, label, "resolvedRef", development["resolvedRef"], branch_commit["sha"])
        compare_field(errors, label, "commitSha", development["commitSha"], branch_commit["sha"])
        compare_field(
            errors,
            label,
            "sourceTimestamp",
            development["sourceTimestamp"],
            branch_commit["commit"]["committer"]["date"],
        )
        compare_field(errors, label, "sourceUrl", development["sourceUrl"], branch_commit["html_url"])

        stable = program["stable"]
        release = github_json(f"/repos/{repository}/releases/latest")
        release_tag = release["tag_name"]
        commit = github_json(
            f"/repos/{repository}/commits/{urllib.parse.quote(release_tag, safe='')}"
        )
        label = f"{kind}.stable"
        if release.get("draft") or release.get("prerelease"):
            fail(errors, f"{label} GitHub latest unexpectedly resolved to a draft/prerelease")
        compare_field(errors, label, "resolvedRef", stable["resolvedRef"], release_tag)
        compare_field(errors, label, "commitSha", stable["commitSha"], commit["sha"])
        compare_field(errors, label, "sourceTimestamp", stable["sourceTimestamp"], release["published_at"])
        compare_field(errors, label, "sourceUrl", stable["sourceUrl"], release["html_url"])
    return errors


def main() -> int:
    args = parse_args()
    manifest, errors = load_and_validate()
    if not errors and args.live:
        try:
            errors.extend(validate_live(manifest))
        except RuntimeError as error:
            errors.append(str(error))
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    scope = "offline structure and live GitHub refs" if args.live else "offline structure"
    print(f"Core upstream manifest passed {scope}: {MANIFEST_PATH.relative_to(ROOT)}")
    for program in manifest["programs"]:
        print(
            f"  {program['program']}: "
            f"{program['development']['trackedRef']}@{program['development']['commitSha'][:12]} + "
            f"latest={program['stable']['resolvedRef']}@{program['stable']['commitSha'][:12]}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
