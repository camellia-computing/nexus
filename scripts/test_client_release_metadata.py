#!/usr/bin/env python3
"""Regression tests for the native release metadata contract."""

from __future__ import annotations

import argparse
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

import client_release_metadata as metadata


VERSION = "1.2.3"
COMMIT = "a" * 40


def build(
    platform: str,
    architecture: str,
    native: str,
    trust: str,
    identity: str | None,
    artifact_signing: dict[str, str] | None = None,
) -> dict[str, Any]:
    return {
        "schemaVersion": metadata.SCHEMA_VERSION,
        "product": metadata.PRODUCT,
        "version": VERSION,
        "buildId": VERSION,
        "commit": COMMIT,
        "platform": platform,
        "architecture": architecture,
        "nativeSigning": native,
        "distributionTrust": trust,
        "identity": identity,
        "artifactSigning": artifact_signing
        or {
            "scheme": "none",
            "trust": "none",
        },
        "delivery": "installable",
    }


def valid_builds() -> list[dict[str, Any]]:
    return [
        build(
            "linux",
            "x64",
            "not-applicable",
            "not-applicable",
            None,
            {
                "scheme": "openpgp-detached",
                "trust": "platform-key",
                "fingerprint": "B" * 40,
            },
        ),
        build(
            "macos",
            "arm64",
            "notarized",
            "public-trust",
            "Developer ID Application: Camellia Computing (TEAMID1234)",
        ),
        build(
            "macos",
            "x64",
            "signed",
            "private-trust",
            "Camellia Development Code Signing",
        ),
        build(
            "windows",
            "x64",
            "signed",
            "private-trust",
            "C" * 40,
        ),
    ]


class ClientReleaseMetadataTests(unittest.TestCase):
    def test_aggregate_and_report_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths: list[Path] = []
            for index, record in enumerate(reversed(valid_builds())):
                path = root / f"{index}.json"
                path.write_text(json.dumps(record) + "\n", encoding="utf-8")
                paths.append(path)
            output = root / "RELEASE-METADATA.json"
            report = root / "NATIVE-SIGNING.md"
            metadata.aggregate(
                argparse.Namespace(
                    version=VERSION,
                    commit=COMMIT,
                    output=output,
                    report=report,
                    metadata=paths,
                )
            )
            release = metadata.load_json(output)
            metadata.validate_release(release, VERSION, COMMIT)
            self.assertEqual(
                [
                    (item["platform"], item["architecture"])
                    for item in release["builds"]
                ],
                sorted(metadata.EXPECTED_BUILDS),
            )
            self.assertEqual(report.read_text(), metadata.render_report(release))
            metadata.validate(
                argparse.Namespace(
                    version=VERSION,
                    commit=COMMIT,
                    input=output,
                    report=report,
                )
            )

    def test_unsigned_build_cannot_claim_distribution_trust(self) -> None:
        record = build(
            "windows",
            "x64",
            "unsigned",
            "public-trust",
            None,
        )
        with self.assertRaisesRegex(ValueError, "claims trust"):
            metadata.validate_build(record, VERSION, COMMIT, "fixture")

    def test_windows_identity_must_be_canonical(self) -> None:
        record = build(
            "windows",
            "x64",
            "signed",
            "private-trust",
            "c" * 40,
        )
        with self.assertRaisesRegex(ValueError, "signer identity"):
            metadata.validate_build(record, VERSION, COMMIT, "fixture")

    def test_notarization_requires_public_trust(self) -> None:
        record = build(
            "macos",
            "arm64",
            "notarized",
            "private-trust",
            "Private Developer Identity",
        )
        with self.assertRaisesRegex(ValueError, "notarized"):
            metadata.validate_build(record, VERSION, COMMIT, "fixture")

    def test_duplicate_json_keys_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "duplicate.json"
            path.write_text('{"schemaVersion":3,"schemaVersion":2}\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
                metadata.load_json(path)

    def test_tampered_human_report_is_rejected(self) -> None:
        release = {
            "schemaVersion": metadata.SCHEMA_VERSION,
            "product": metadata.PRODUCT,
            "version": VERSION,
            "commit": COMMIT,
            "builds": valid_builds(),
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release_path = root / "RELEASE-METADATA.json"
            report_path = root / "NATIVE-SIGNING.md"
            release_path.write_text(
                json.dumps(release, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            report_path.write_text("incorrect report\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "does not match metadata"):
                metadata.validate(
                    argparse.Namespace(
                        version=VERSION,
                        commit=COMMIT,
                        input=release_path,
                        report=report_path,
                    )
                )


if __name__ == "__main__":
    unittest.main()
