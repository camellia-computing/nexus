#!/usr/bin/env python3
"""Build the canonical organization release evidence for Nexus packages."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from typing import Any

import client_release_metadata


POLICY_REVISION = "2026-07-31.1"
SIGNING_REGISTRY_REVISION = "2026-07-31.1"
PRODUCT_FILES = {
    ("linux", "x64"): (".AppImage", ".deb", ".tar.gz"),
    ("macos", "arm64"): (".dmg", ".tar.gz"),
    ("macos", "x64"): (".dmg", ".tar.gz"),
    ("windows", "x64"): (".msi", "-portable.zip"),
}
ARCHITECTURES = {
    "x64": "x86-64",
    "arm64": "arm64",
}
COMMIT = re.compile(r"^[0-9a-f]{40}$")


def load_validator() -> ModuleType:
    path = Path(__file__).with_name("validate-release-evidence.py")
    spec = importlib.util.spec_from_file_location("release_evidence_validator", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("unable to load the release evidence validator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def regular_file(path: Path, label: str) -> Path:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} must be a regular file: {path}")
    if path.stat().st_size < 1:
        raise ValueError(f"{label} must not be empty: {path}")
    return path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def evidence_file(path: Path) -> dict[str, str]:
    regular_file(path, "evidence input")
    return {
        "name": path.name,
        "sha256": sha256(path),
    }


def native_signing(
    build: dict[str, Any],
    *,
    artifact: Path,
    metadata_name: str,
    report_name: str,
) -> dict[str, Any]:
    platform = build["platform"]
    native = build["nativeSigning"]
    trust = build["distributionTrust"]
    artifact_scheme = build["artifactSigning"]["scheme"]

    if platform == "linux":
        if artifact_scheme == "openpgp-detached":
            signature = regular_file(
                artifact.with_name(f"{artifact.name}.asc"),
                "OpenPGP signature",
            )
            key = regular_file(
                artifact.with_name(
                    f"camellia-nexus-{build['version']}-linux-x64.signing-key.asc"
                ),
                "OpenPGP verification key",
            )
            return {
                "category": "platform-key",
                "verification": "verified",
                "verifier": "openpgp",
                "timestamp": "not-applicable",
                "distribution": "installable",
                "evidence": sorted(
                    [key.name, metadata_name, report_name, signature.name]
                ),
            }
        return {
            "category": "unsigned",
            "verification": "not-present",
            "verifier": "none",
            "timestamp": "not-applicable",
            "distribution": "installable",
            "evidence": [],
        }

    common_evidence = sorted([metadata_name, report_name])
    if platform == "windows" and native == "signed":
        return {
            "category": trust,
            "verification": "verified",
            "verifier": "authenticode",
            "timestamp": "verified",
            "distribution": (
                "installable" if trust == "public-trust" else "restricted"
            ),
            "evidence": common_evidence,
        }
    if platform == "macos" and native in {"signed", "notarized"}:
        return {
            "category": trust,
            "verification": "verified",
            "verifier": "apple-codesign",
            "timestamp": "verified" if native == "notarized" else "missing",
            "distribution": (
                "installable" if trust == "public-trust" else "restricted"
            ),
            "evidence": common_evidence,
        }
    if platform == "macos" and native == "ad-hoc":
        return {
            "category": "ad-hoc",
            "verification": "verified",
            "verifier": "apple-codesign",
            "timestamp": "not-applicable",
            "distribution": "restricted",
            "evidence": common_evidence,
        }
    return {
        "category": "unsigned",
        "verification": "not-present",
        "verifier": "none",
        "timestamp": "not-applicable",
        "distribution": "restricted",
        "evidence": [],
    }


def build_evidence(args: argparse.Namespace) -> dict[str, Any]:
    if not COMMIT.fullmatch(args.commit):
        raise ValueError("commit must be a full lowercase SHA")
    if args.validation_run_id < 1:
        raise ValueError("validation run ID must be positive")
    assets = args.assets.resolve()
    if not assets.is_dir() or assets.is_symlink():
        raise ValueError("assets must be a real directory")

    metadata_path = regular_file(args.metadata.resolve(), "release metadata")
    report_path = regular_file(args.report.resolve(), "native signing report")
    sbom_path = regular_file(args.sbom.resolve(), "SBOM")
    provenance_path = regular_file(args.provenance.resolve(), "provenance")
    metadata = client_release_metadata.load_json(metadata_path)
    client_release_metadata.validate_release(metadata, args.version, args.commit)
    expected_report = client_release_metadata.render_report(metadata)
    if report_path.read_text(encoding="utf-8") != expected_report:
        raise ValueError("native signing report does not match release metadata")

    sbom = evidence_file(sbom_path)
    provenance = evidence_file(provenance_path)
    files: list[dict[str, Any]] = []
    for build in metadata["builds"]:
        key = (build["platform"], build["architecture"])
        for suffix in PRODUCT_FILES[key]:
            name = (
                f"camellia-nexus-{args.version}-{build['platform']}-"
                f"{build['architecture']}{suffix}"
            )
            artifact = regular_file(assets / name, "release artifact")
            files.append(
                {
                    "name": name,
                    "sha256": sha256(artifact),
                    "size_bytes": artifact.stat().st_size,
                    "platform": build["platform"],
                    "architecture": ARCHITECTURES[build["architecture"]],
                    "sbom": sbom,
                    "provenance": provenance,
                    "signing": native_signing(
                        build,
                        artifact=artifact,
                        metadata_name=metadata_path.name,
                        report_name=report_path.name,
                    ),
                }
            )

    generated_at = args.generated_at or datetime.now(timezone.utc).isoformat(
        timespec="seconds"
    ).replace("+00:00", "Z")
    result = {
        "schema_version": 1,
        "repository": "nexus-client",
        "version": args.version,
        "source": {
            "commit": args.commit,
            "ref": f"refs/tags/v{args.version}",
            "validation_run_id": args.validation_run_id,
        },
        "release_kind": "formal",
        "generated_at": generated_at,
        "policy": {
            "repository_policy_revision": POLICY_REVISION,
            "signing_registry_revision": SIGNING_REGISTRY_REVISION,
            "exceptions": [],
        },
        "dependencies": [],
        "files": sorted(files, key=lambda item: item["name"]),
        "images": [],
    }
    load_validator().validate_release_evidence(result)
    return result


def write_json(path: Path, value: dict[str, Any]) -> None:
    if path.exists() and (not path.is_file() or path.is_symlink()):
        raise ValueError("output must be a regular file path")
    if not path.parent.is_dir() or path.parent.is_symlink():
        raise ValueError("output parent must be a real directory")
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser()
    root.add_argument("--assets", required=True, type=Path)
    root.add_argument("--metadata", required=True, type=Path)
    root.add_argument("--report", required=True, type=Path)
    root.add_argument("--sbom", required=True, type=Path)
    root.add_argument("--provenance", required=True, type=Path)
    root.add_argument("--version", required=True)
    root.add_argument("--commit", required=True)
    root.add_argument("--validation-run-id", required=True, type=int)
    root.add_argument("--generated-at")
    root.add_argument("--output", required=True, type=Path)
    return root


def main() -> None:
    args = parser().parse_args()
    write_json(args.output, build_evidence(args))


if __name__ == "__main__":
    main()
