#!/usr/bin/env python3
"""Regression tests for Nexus organization release evidence."""

from __future__ import annotations

import argparse
import json
import tempfile
import unittest
from pathlib import Path

import build_release_evidence as evidence
import client_release_metadata
from test_client_release_metadata import COMMIT, VERSION, valid_builds


class BuildReleaseEvidenceTests(unittest.TestCase):
    def fixture(self, root: Path) -> argparse.Namespace:
        assets = root / "assets"
        assets.mkdir()
        metadata = {
            "schemaVersion": client_release_metadata.SCHEMA_VERSION,
            "product": client_release_metadata.PRODUCT,
            "version": VERSION,
            "commit": COMMIT,
            "builds": valid_builds(),
        }
        metadata_path = assets / "RELEASE-METADATA.json"
        metadata_path.write_text(
            json.dumps(metadata, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        report_path = assets / "NATIVE-SIGNING.md"
        report_path.write_text(
            client_release_metadata.render_report(metadata),
            encoding="utf-8",
        )
        for build in metadata["builds"]:
            for suffix in evidence.PRODUCT_FILES[
                (build["platform"], build["architecture"])
            ]:
                path = assets / (
                    f"camellia-nexus-{VERSION}-{build['platform']}-"
                    f"{build['architecture']}{suffix}"
                )
                path.write_bytes(f"fixture:{path.name}\n".encode())
                if (
                    build["platform"] == "linux"
                    and build["artifactSigning"]["scheme"] == "openpgp-detached"
                ):
                    path.with_name(f"{path.name}.asc").write_text(
                        "signature\n",
                        encoding="utf-8",
                    )
        (assets / f"camellia-nexus-{VERSION}-linux-x64.signing-key.asc").write_text(
            "key\n",
            encoding="utf-8",
        )
        sbom = assets / "SBOM.spdx.json"
        sbom.write_text('{"spdxVersion":"SPDX-2.3"}\n', encoding="utf-8")
        provenance = assets / "PROVENANCE.intoto.jsonl"
        provenance.write_text('{"mediaType":"fixture"}\n', encoding="utf-8")
        return argparse.Namespace(
            assets=assets,
            metadata=metadata_path,
            report=report_path,
            sbom=sbom,
            provenance=provenance,
            version=VERSION,
            commit=COMMIT,
            validation_run_id=42,
            generated_at="2026-07-31T00:00:00Z",
            output=assets / "release-evidence.json",
        )

    def test_complete_matrix_and_native_categories(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            args = self.fixture(Path(temporary))
            value = evidence.build_evidence(args)
            self.assertEqual(len(value["files"]), 9)
            categories = {
                item["platform"]: item["signing"]["category"]
                for item in value["files"]
            }
            self.assertEqual(categories["linux"], "platform-key")
            self.assertEqual(categories["windows"], "private-trust")
            self.assertIn(
                "public-trust",
                {
                    item["signing"]["category"]
                    for item in value["files"]
                    if item["platform"] == "macos"
                },
            )

    def test_missing_detached_signature_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            args = self.fixture(Path(temporary))
            signature = next(args.assets.glob("*.AppImage.asc"))
            signature.unlink()
            with self.assertRaisesRegex(ValueError, "OpenPGP signature"):
                evidence.build_evidence(args)

    def test_tampered_human_report_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            args = self.fixture(Path(temporary))
            args.report.write_text("tampered\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "does not match"):
                evidence.build_evidence(args)


if __name__ == "__main__":
    unittest.main()
