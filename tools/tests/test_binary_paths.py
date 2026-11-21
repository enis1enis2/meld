#!/usr/bin/env python3
"""Tests for the local-path leak guard used by release builds."""
from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools" / "check_binary_paths.py"


class BinaryPathGuardTests(unittest.TestCase):
    def run_guard(self, data: bytes, prefix: str = "D:\\a\\meld\\meld\\target") -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as td:
            path = pathlib.Path(td) / "meld.exe"
            path.write_bytes(data)
            return subprocess.run(
                [sys.executable, str(SCRIPT), "--prefix", prefix, str(path)],
                capture_output=True,
                text=True,
            )

    def test_clean_binary_passes(self):
        result = self.run_guard(b"no local paths here")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("PASS", result.stdout)

    def test_native_and_slash_paths_are_rejected(self):
        for payload in (
            b"prefix D:\\a\\meld\\meld\\target\\x86_64\\out/file.rs",
            b"prefix D:/a/meld/meld/target/x86_64/out/file.rs",
            "prefix D:\\a\\meld\\meld\\target\\out.rs".encode("utf-16-le"),
        ):
            with self.subTest(payload=payload[:24]):
                result = self.run_guard(payload)
                self.assertEqual(result.returncode, 1)
                self.assertIn("embeds local path prefix", result.stderr)

    def test_unrelated_prefix_does_not_fail(self):
        result = self.run_guard(b"D:\\other\\place\\target\\out.rs")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
