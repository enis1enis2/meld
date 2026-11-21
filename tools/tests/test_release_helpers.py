#!/usr/bin/env python3
"""Tests for the release workflow helper scripts."""
from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
TOOLS = ROOT / "tools"


class ExtractChangelogSectionTests(unittest.TestCase):
    def run_extract(self, changelog: str, version: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as td:
            source = Path(td) / "CHANGELOG.md"
            output = Path(td) / "notes.md"
            source.write_text(changelog, encoding="utf-8")
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPTS / "extract_changelog_section.py"),
                    "--version",
                    version,
                    "--input",
                    str(source),
                    "--output",
                    str(output),
                ],
                capture_output=True,
                text=True,
            )
            notes = output.read_text(encoding="utf-8") if output.is_file() else None
            return result, notes

    def test_extracts_only_the_requested_section(self):
        changelog = (
            "# Changelog\n\n"
            "## [Unreleased]\n\n- future\n\n"
            "## [1.2.3] - 2026-01-01\n\n- shipped\n- fixed\n\n"
            "## [1.2.2] - 2025-12-01\n\n- old\n"
        )
        result, notes = self.run_extract(changelog, "1.2.3")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(notes, "- shipped\n- fixed\n")

    def test_missing_section_fails_closed(self):
        result, notes = self.run_extract("## [1.2.2]\n\n- old\n", "1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIsNone(notes)
        self.assertIn("no changelog section", result.stderr)


class CheckReleaseStateTests(unittest.TestCase):
    def run_check(self, state: dict, notes: str, assets: list[str], expect_latest: str = "no"):
        with tempfile.TemporaryDirectory() as td:
            state_path = Path(td) / "state.json"
            notes_path = Path(td) / "notes.md"
            state_path.write_text(json.dumps(state), encoding="utf-8")
            notes_path.write_text(notes, encoding="utf-8")
            command = [
                "python3",
                str(SCRIPTS / "check_release_state.py"),
                "--state",
                str(state_path),
                "--notes",
                str(notes_path),
                "--expect-latest",
                expect_latest,
            ]
            for asset in assets:
                command += ["--asset", asset]
            return subprocess.run(command, capture_output=True, text=True)

    def test_reports_missing_assets_from_consistent_existing_release(self):
        state = {
            "isLatest": False,
            "isPrerelease": False,
            "body": "- shipped\n",
            "assets": [{"name": "a.zip"}],
        }
        result = self.run_check(state, "- shipped\n", ["a.zip", "b.zip"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "b.zip\n")

    def test_prerelease_existing_release_is_rejected(self):
        state = {
            "isLatest": False,
            "isPrerelease": True,
            "body": "- shipped\n",
            "assets": [],
        }
        result = self.run_check(state, "- shipped\n", [])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("prerelease", result.stderr)


class T3GuestDigestTests(unittest.TestCase):
    def test_recorded_digest_matches_committed_component(self):
        result = subprocess.run(
            ["python3", str(TOOLS / "check_t3_guest_digest.py"), "--check"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("digest OK", result.stdout)


if __name__ == "__main__":
    unittest.main()
