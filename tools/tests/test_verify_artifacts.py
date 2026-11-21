#!/usr/bin/env python3
"""Offline regression tests for scripts/verify_artifacts.sh.

These tests build a synthetic release directory with fake (but structurally
valid) cosign bundles in both the DSSE and legacy base64Signature shapes. They
exercise integrity and policy checks without requiring cosign, OIDC or network
access.
"""
from __future__ import annotations

import base64
import hashlib
import io
import json
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VERIFY = ROOT / "scripts" / "verify_artifacts.sh"
VERSION = "1.2.3"
TARGET = "x86_64-pc-windows-msvc"
ARCHIVE = f"meld-lang-{VERSION}-{TARGET}.zip"
SBOM = f"meld-lang-{VERSION}-sbom.cdx.json"
MANIFEST = f"meld-lang-{VERSION}.release-manifest.json"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def archive_bytes() -> bytes:
    prefix = f"meld-lang-{VERSION}-{TARGET}"
    members = [
        f"{prefix}/bin/meld.exe",
        f"{prefix}/bin/meld-lsp.exe",
        f"{prefix}/bin/meld-pack.exe",
        f"{prefix}/packs/python/pack.toml",
        f"{prefix}/packs/bash/pack.toml",
        f"{prefix}/share/doc/meld-lang/LICENSE-APACHE",
        f"{prefix}/share/doc/meld-lang/LICENSE-MIT",
        f"{prefix}/share/doc/meld-lang/THIRD-PARTY-NOTICES.md",
        f"{prefix}/share/doc/meld-lang/CHANGELOG.md",
    ]
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as zf:
        for name in members:
            zf.writestr(name, name.encode("utf-8"))
    return buffer.getvalue()


def write_fake_signatures(root: Path, name: str) -> None:
    raw = b"synthetic-signature:" + name.encode("utf-8")
    encoded = base64.b64encode(raw).decode("ascii")
    (root / f"{name}.sig").write_text(encoded + "\n", encoding="ascii")
    bundle = {
        "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.1",
        "dsseEnvelope": {
            "payloadType": "application/vnd.in-toto+json",
            "payload": base64.b64encode(b"{}").decode("ascii"),
            "signatures": [{"keyid": "", "sig": encoded}],
        },
    }
    (root / f"{name}.bundle").write_text(json.dumps(bundle), encoding="utf-8")


def rewrite_legacy_bundles(root: Path) -> None:
    """Convert all bundles to the shape cosign emits for keyless blob signing."""
    for name in (ARCHIVE, SBOM, MANIFEST, "SHA256SUMS"):
        encoded = (root / f"{name}.sig").read_text(encoding="ascii").strip()
        bundle = {
            "base64Signature": encoded,
            "cert": "synthetic-certificate",
        }
        (root / f"{name}.bundle").write_text(json.dumps(bundle), encoding="utf-8")


def build_release(root: Path) -> None:
    root.mkdir(parents=True, exist_ok=True)
    archive_data = archive_bytes()
    (root / ARCHIVE).write_bytes(archive_data)
    sbom_data = json.dumps({"bomFormat": "CycloneDX", "specVersion": "1.6"}).encode("utf-8")
    (root / SBOM).write_bytes(sbom_data)

    manifest = {
        "schema": "meld-release-manifest/1",
        "product": "meld-lang",
        "version": VERSION,
        "semver_spec": "2.0.0",
        "expected_targets": [TARGET],
        "checksums": {
            "file": "SHA256SUMS",
            "generated_after_manifest": True,
            "includes_manifest": True,
        },
        "signature_policy": {
            "scheme": "cosign-keyless-plus-github-build-provenance",
            "expected_files_per_artifact": [".sig", ".bundle"],
            "embedded_signatures": False,
        },
        "artifacts": [
            {"name": ARCHIVE, "sha256": sha256(archive_data), "size": len(archive_data), "kind": "platform-archive"},
            {"name": SBOM, "sha256": sha256(sbom_data), "size": len(sbom_data), "kind": "sbom"},
        ],
    }
    manifest_data = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    (root / MANIFEST).write_bytes(manifest_data)

    lines = [
        f"{sha256(archive_data)}  {ARCHIVE}",
        f"{sha256(sbom_data)}  {SBOM}",
        f"{sha256(manifest_data)}  {MANIFEST}",
    ]
    (root / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")

    for name in (ARCHIVE, SBOM, MANIFEST, "SHA256SUMS"):
        write_fake_signatures(root, name)


class VerifyArtifactsOfflineTests(unittest.TestCase):
    def run_verifier(self, root: Path, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["sh", str(VERIFY), "--dir", str(root), "--tag", f"v{VERSION}", "--offline", *extra],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    def test_offline_verification_accepts_consistent_release(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            result = self.run_verifier(root)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("PASS expected target set", result.stdout)
            self.assertIn("equal signature bytes", result.stdout)
            self.assertIn("OFFLINE", result.stdout)

    def test_legacy_cosign_bundle_signature_is_accepted(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            rewrite_legacy_bundles(root)
            result = self.run_verifier(root)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("equal signature bytes", result.stdout)

    def test_signature_byte_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            (root / f"{ARCHIVE}.sig").write_text(base64.b64encode(b"tampered").decode("ascii") + "\n", encoding="ascii")
            result = self.run_verifier(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("signature bytes", result.stderr)

    def test_unexpected_expected_target_is_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            result = self.run_verifier(root, "--expected-target", "aarch64-apple-darwin")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("expected targets", result.stderr)

    def test_require_attestation_cannot_be_used_offline(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            result = self.run_verifier(root, "--require-attestation")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("--require-attestation cannot be combined with --offline", result.stderr)

    def test_path_traversal_entry_is_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td) / "release"
            build_release(root)
            # Rebuild the archive with an unsafe member, then refresh checksums
            # and the manifest so only the archive-entry gate can fail.
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as zf:
                zf.writestr("../escape", b"escape")
            archive_data = buffer.getvalue()
            (root / ARCHIVE).write_bytes(archive_data)
            manifest_path = root / MANIFEST
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            for item in manifest["artifacts"]:
                if item["name"] == ARCHIVE:
                    item["sha256"] = sha256(archive_data)
                    item["size"] = len(archive_data)
            manifest_data = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
            manifest_path.write_bytes(manifest_data)
            entries = {
                ARCHIVE: sha256(archive_data),
                SBOM: sha256((root / SBOM).read_bytes()),
                MANIFEST: sha256(manifest_data),
            }
            (root / "SHA256SUMS").write_text(
                "".join(f"{digest}  {name}\n" for name, digest in sorted(entries.items())),
                encoding="utf-8",
            )
            result = self.run_verifier(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe archive entry", result.stderr)


if __name__ == "__main__":
    unittest.main()
