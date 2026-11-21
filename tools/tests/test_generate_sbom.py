#!/usr/bin/env python3
"""Tests for deterministic CycloneDX SBOM generation."""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "generate_sbom.sh"
EPOCH = 1795292305
COMMIT = "0123456789abcdef0123456789abcdef01234567"


def raw_sbom(shuffle: bool = False) -> dict:
    components = [
        {"bom-ref": f"C-{i}", "name": f"crate-{i}", "type": "library", "version": "1.0.0"}
        for i in range(4)
    ]
    dependencies = [
        {"ref": "C-0", "dependsOn": ["C-3", "C-1", "C-2"]},
        {"ref": "C-1", "dependsOn": ["C-2"]},
        {"ref": "C-2", "dependsOn": []},
        {"ref": "C-3", "dependsOn": []},
    ]
    if shuffle:
        components.reverse()
        dependencies.reverse()
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "serialNumber": "urn:uuid:00000000-0000-0000-0000-000000000000",
        "metadata": {
            "timestamp": "2020-01-01T00:00:00Z",
            "authors": [{"name": "b"}, {"name": "a"}],
            "component": {"bom-ref": "root", "name": "meld-lang", "type": "application"},
            "tools": [{"vendor": "other", "version": "9"}],
        },
        "components": components,
        "dependencies": dependencies,
    }


class GenerateSbomTests(unittest.TestCase):
    def run_script(self, document: dict, out: pathlib.Path) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td)
            raw = root / "raw.json"
            raw.write_text(json.dumps(document), encoding="utf-8")
            fake = root / "bin"
            fake.mkdir()
            cargo = fake / "cargo"
            cargo.write_text(
                "#!/bin/sh\n"
                'if [ "$1" = sbom ] && [ "$2" = --help ]; then exit 0; fi\n'
                'if [ "$1" = sbom ]; then cat "$FAKE_SBOM"; exit 0; fi\n'
                "exit 1\n",
                encoding="utf-8",
            )
            cargo.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = f"{fake}:{env['PATH']}"
            env["FAKE_SBOM"] = str(raw)
            env["SOURCE_DATE_EPOCH"] = str(EPOCH)
            return subprocess.run(
                ["sh", str(SCRIPT), "--version", "0.2.0", "--out", str(out)],
                capture_output=True,
                text=True,
                env=env,
            )

    def test_ordering_is_canonical(self):
        with tempfile.TemporaryDirectory() as td:
            out = pathlib.Path(td) / "sbom.json"
            result = self.run_script(raw_sbom(), out)
            self.assertEqual(result.returncode, 0, result.stderr)
            doc = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(
                [c["bom-ref"] for c in doc["components"]], ["C-0", "C-1", "C-2", "C-3"]
            )
            self.assertEqual([d["ref"] for d in doc["dependencies"]], ["C-0", "C-1", "C-2", "C-3"])
            self.assertEqual(doc["dependencies"][0]["dependsOn"], ["C-1", "C-2", "C-3"])
            self.assertEqual(
                [a["name"] for a in doc["metadata"]["authors"]], ["a", "b"]
            )
            self.assertEqual(doc["metadata"]["timestamp"], "2026-11-21T20:18:25Z")

    def test_permuted_input_produces_identical_bytes(self):
        with tempfile.TemporaryDirectory() as td:
            first, second = pathlib.Path(td) / "one.json", pathlib.Path(td) / "two.json"
            self.assertEqual(self.run_script(raw_sbom(), first).returncode, 0)
            self.assertEqual(self.run_script(raw_sbom(shuffle=True), second).returncode, 0)
            self.assertEqual(first.read_bytes(), second.read_bytes())


if __name__ == "__main__":
    unittest.main()
