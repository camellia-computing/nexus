#!/usr/bin/env python3
"""Validate and aggregate Camellia Nexus native release metadata."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 3
PRODUCT = "Camellia Nexus"
BUILD_FIELDS = {
    "schemaVersion",
    "product",
    "version",
    "buildId",
    "commit",
    "platform",
    "architecture",
    "nativeSigning",
    "distributionTrust",
    "identity",
    "artifactSigning",
    "delivery",
}
RELEASE_FIELDS = {
    "schemaVersion",
    "product",
    "version",
    "commit",
    "builds",
}
EXPECTED_BUILDS = {
    ("linux", "x64"),
    ("macos", "arm64"),
    ("macos", "x64"),
    ("windows", "x64"),
}
SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
WINDOWS_THUMBPRINT = re.compile(r"^[0-9A-F]{40}$")
OPENPGP_FINGERPRINT = re.compile(r"^(?:[0-9A-F]{40}|[0-9A-F]{64})$")


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path: Path) -> dict[str, Any]:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"metadata input must be a regular file: {path}")
    value = json.loads(
        path.read_text(encoding="utf-8"),
        object_pairs_hook=unique_object,
    )
    if not isinstance(value, dict):
        raise ValueError(f"metadata root must be an object: {path}")
    return value


def require_exact_fields(value: dict[str, Any], expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise ValueError(
            f"{label} fields differ: expected {sorted(expected)}, "
            f"found {sorted(value)}"
        )


def require_string(value: dict[str, Any], name: str, label: str) -> str:
    item = value.get(name)
    if not isinstance(item, str) or not item:
        raise ValueError(f"{label}.{name} must be a non-empty string")
    return item


def validate_artifact_signing(
    signing: Any,
    platform: str,
    label: str,
) -> None:
    if not isinstance(signing, dict):
        raise ValueError(f"{label}.artifactSigning must be an object")
    scheme = signing.get("scheme")
    if scheme == "none":
        if signing != {"scheme": "none", "trust": "none"}:
            raise ValueError(f"{label} has invalid unsigned artifact metadata")
        return
    if scheme == "openpgp-detached":
        if set(signing) != {"scheme", "trust", "fingerprint"}:
            raise ValueError(f"{label} has invalid OpenPGP metadata fields")
        fingerprint = signing.get("fingerprint")
        if (
            platform != "linux"
            or signing.get("trust") != "platform-key"
            or not isinstance(fingerprint, str)
            or not OPENPGP_FINGERPRINT.fullmatch(fingerprint)
        ):
            raise ValueError(f"{label} has invalid OpenPGP signing identity")
        return
    raise ValueError(f"{label} has unsupported artifact signing scheme: {scheme}")


def validate_build(
    build: dict[str, Any],
    expected_version: str,
    expected_commit: str,
    label: str,
) -> tuple[str, str]:
    require_exact_fields(build, BUILD_FIELDS, label)
    if build.get("schemaVersion") != SCHEMA_VERSION:
        raise ValueError(f"{label} has an unsupported schema version")
    if build.get("product") != PRODUCT:
        raise ValueError(f"{label} has an unexpected product")
    if build.get("version") != expected_version or build.get("buildId") != expected_version:
        raise ValueError(f"{label} version/build ID does not match {expected_version}")
    if build.get("commit") != expected_commit:
        raise ValueError(f"{label} commit does not match {expected_commit}")

    platform = require_string(build, "platform", label)
    architecture = require_string(build, "architecture", label)
    key = (platform, architecture)
    if key not in EXPECTED_BUILDS:
        raise ValueError(f"{label} has an unsupported platform/architecture: {key}")
    if build.get("delivery") != "installable":
        raise ValueError(f"{label} must use delivery=installable")

    native = require_string(build, "nativeSigning", label)
    trust = require_string(build, "distributionTrust", label)
    identity = build.get("identity")
    if identity is not None and (
        not isinstance(identity, str)
        or not identity
        or any(ord(character) < 0x20 for character in identity)
    ):
        raise ValueError(f"{label}.identity must be null or a printable string")

    validate_artifact_signing(build.get("artifactSigning"), platform, label)
    artifact_scheme = build["artifactSigning"]["scheme"]
    if platform != "linux" and artifact_scheme != "none":
        raise ValueError(f"{label} may not use detached artifact signing")

    if platform == "linux":
        if native != "not-applicable" or trust != "not-applicable" or identity is not None:
            raise ValueError(f"{label} has invalid Linux native trust metadata")
    elif platform == "windows":
        if artifact_scheme != "none":
            raise ValueError(f"{label} has invalid Windows artifact signing")
        if native == "unsigned":
            if trust != "none" or identity is not None:
                raise ValueError(f"{label} unsigned Windows metadata claims trust")
        elif native == "signed":
            if (
                trust not in {"private-trust", "public-trust"}
                or not isinstance(identity, str)
                or not WINDOWS_THUMBPRINT.fullmatch(identity)
            ):
                raise ValueError(f"{label} has invalid Windows signer identity/trust")
        else:
            raise ValueError(f"{label} has invalid Windows native signing mode")
    elif platform == "macos":
        if artifact_scheme != "none":
            raise ValueError(f"{label} has invalid macOS artifact signing")
        if native in {"unsigned", "ad-hoc"}:
            if trust != "none" or identity is not None:
                raise ValueError(f"{label} unsigned/ad-hoc macOS metadata claims trust")
        elif native == "signed":
            if trust not in {"private-trust", "public-trust"} or identity is None:
                raise ValueError(f"{label} has invalid macOS signer identity/trust")
        elif native == "notarized":
            if trust != "public-trust" or identity is None:
                raise ValueError(f"{label} has invalid notarized macOS identity/trust")
        else:
            raise ValueError(f"{label} has invalid macOS native signing mode")
    return key


def validate_release(
    release: dict[str, Any],
    expected_version: str,
    expected_commit: str,
) -> None:
    require_exact_fields(release, RELEASE_FIELDS, "release")
    if release.get("schemaVersion") != SCHEMA_VERSION:
        raise ValueError("release has an unsupported schema version")
    if release.get("product") != PRODUCT:
        raise ValueError("release has an unexpected product")
    if release.get("version") != expected_version:
        raise ValueError("release version does not match")
    if release.get("commit") != expected_commit:
        raise ValueError("release commit does not match")
    builds = release.get("builds")
    if not isinstance(builds, list) or len(builds) != len(EXPECTED_BUILDS):
        raise ValueError("release must contain exactly four native build records")
    keys: list[tuple[str, str]] = []
    for index, build in enumerate(builds):
        if not isinstance(build, dict):
            raise ValueError(f"release build {index} must be an object")
        keys.append(
            validate_build(
                build,
                expected_version,
                expected_commit,
                f"release.builds[{index}]",
            )
        )
    if keys != sorted(keys) or set(keys) != EXPECTED_BUILDS:
        raise ValueError("release build identities must be complete, unique, and sorted")


def markdown_cell(value: str) -> str:
    return value.replace("\\", "\\\\").replace("|", "\\|")


def render_report(release: dict[str, Any]) -> str:
    lines = [
        "# Native signing status",
        "",
        "Checksums and keyless GitHub/Sigstore bundles apply to every asset. "
        "Native publisher and detached-signature trust are reported separately.",
        "",
        "| Platform | Architecture | Native mode | Distribution trust | Identity | Artifact signing | Delivery |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for build in release["builds"]:
        identity = markdown_cell(build["identity"] or "none")
        artifact = build["artifactSigning"]["scheme"]
        if artifact == "openpgp-detached":
            artifact = f"{artifact}:{build['artifactSigning']['fingerprint']}"
        artifact = markdown_cell(artifact)
        lines.append(
            f"| {build['platform']} | {build['architecture']} | "
            f"{build['nativeSigning']} | {build['distributionTrust']} | "
            f"{identity} | {artifact} | {build['delivery']} |"
        )
    lines.extend(
        [
            "",
            "The current reviewed public identities and rotation states are maintained in "
            "the [organization signing registry]"
            "(https://github.com/camellia-computing/.github/blob/main/"
            "docs/SIGNING_IDENTITY_REGISTRY.md).",
            "",
        ]
    )
    return "\n".join(lines)


def write_text(path: Path, content: str) -> None:
    if path.exists() and (path.is_symlink() or not path.is_file()):
        raise ValueError(f"output must be a regular file path: {path}")
    if not path.parent.is_dir() or path.parent.is_symlink():
        raise ValueError(f"output parent must be a real directory: {path.parent}")
    path.write_text(content, encoding="utf-8")


def aggregate(args: argparse.Namespace) -> None:
    if not SEMVER.fullmatch(args.version):
        raise ValueError("version must be stable SemVer")
    if not COMMIT.fullmatch(args.commit):
        raise ValueError("commit must be a full lowercase SHA")
    builds = [load_json(path) for path in args.metadata]
    keys = [
        validate_build(build, args.version, args.commit, f"input[{index}]")
        for index, build in enumerate(builds)
    ]
    if len(keys) != len(EXPECTED_BUILDS) or set(keys) != EXPECTED_BUILDS:
        raise ValueError("input metadata must contain the exact native build matrix")
    release = {
        "schemaVersion": SCHEMA_VERSION,
        "product": PRODUCT,
        "version": args.version,
        "commit": args.commit,
        "builds": sorted(
            builds,
            key=lambda build: (build["platform"], build["architecture"]),
        ),
    }
    validate_release(release, args.version, args.commit)
    write_text(
        args.output,
        json.dumps(release, indent=2, sort_keys=True) + "\n",
    )
    write_text(args.report, render_report(release))


def validate(args: argparse.Namespace) -> None:
    if not SEMVER.fullmatch(args.version):
        raise ValueError("version must be stable SemVer")
    if not COMMIT.fullmatch(args.commit):
        raise ValueError("commit must be a full lowercase SHA")
    release = load_json(args.input)
    validate_release(release, args.version, args.commit)
    if args.report is not None:
        if not args.report.is_file() or args.report.is_symlink():
            raise ValueError("release signing report must be a regular file")
        if args.report.read_text(encoding="utf-8") != render_report(release):
            raise ValueError("release signing report does not match metadata")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    subcommands = root.add_subparsers(dest="command", required=True)

    aggregate_parser = subcommands.add_parser("aggregate")
    aggregate_parser.add_argument("--version", required=True)
    aggregate_parser.add_argument("--commit", required=True)
    aggregate_parser.add_argument("--output", required=True, type=Path)
    aggregate_parser.add_argument("--report", required=True, type=Path)
    aggregate_parser.add_argument("metadata", nargs="+", type=Path)
    aggregate_parser.set_defaults(handler=aggregate)

    validate_parser = subcommands.add_parser("validate")
    validate_parser.add_argument("--version", required=True)
    validate_parser.add_argument("--commit", required=True)
    validate_parser.add_argument("--input", required=True, type=Path)
    validate_parser.add_argument("--report", type=Path)
    validate_parser.set_defaults(handler=validate)
    return root


def main() -> None:
    args = parser().parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
