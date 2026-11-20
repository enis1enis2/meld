#!/usr/bin/env python3
"""Regression tests for the tests/examples/tools hardening pass.

Run from the project root with:
    python3 -m unittest discover -s tools/tests -v

The tests are read-only with respect to checked-in files: all generated files
live in temporary directories, and example checks only parse/render in memory.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"
EXAMPLES = ROOT / "examples"
PYDEPS = ROOT.parent / "pydeps"
if PYDEPS.is_dir() and str(PYDEPS) not in sys.path:
    sys.path.insert(0, str(PYDEPS))
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

try:
    import packtool  # noqa: E402
except ModuleNotFoundError:  # pragma: no cover - depends on PYTHONPATH
    packtool = None  # type: ignore[assignment]

import gen_pack  # noqa: E402
import generate_third_party_notices as notices  # noqa: E402
import migrate_index  # noqa: E402
import reference_splitter  # noqa: E402
import sync_grammars  # noqa: E402


@unittest.skipIf(packtool is None, "cryptography is not importable")
class ChecksumByteTests(unittest.TestCase):
    def test_checksums_are_bytes_and_reject_crlf(self):
        with tempfile.TemporaryDirectory() as td:
            pack = Path(td) / "demo"
            pack.mkdir()
            (pack / "pack.toml").write_text(
                'id = "demo"\nversion = "0.1.0"\ntier = "L0"\nlevel = "L0"\n',
                encoding="utf-8",
            )
            (pack / "payload.txt").write_text("hello\n", encoding="utf-8")
            packtool.write_checksums(pack)
            data = (pack / "checksums.sha256").read_bytes()
            self.assertNotIn(b"\r", data)
            self.assertEqual(packtool.read_checksums(pack)["payload.txt"],
                             packtool._sha256_file(pack / "payload.txt"))

            (pack / "checksums.sha256").write_bytes(data.replace(b"\n", b"\r\n"))
            with self.assertRaisesRegex(packtool.PackToolError, "LF line endings"):
                packtool.read_checksums(pack)


class MigrateIndexPathTests(unittest.TestCase):
    def test_attested_sibling_path_allowed_but_escapes_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "packs" / "registry"
            base.mkdir(parents=True)
            (base.parent / "bash").mkdir()
            self.assertEqual(
                migrate_index.safe_path("../bash", base, allow_parent=True), "../bash"
            )
            for bad in ("..\\bash", "C:\\evil", "../../../outside", "", "/etc/passwd"):
                with self.subTest(bad=bad), self.assertRaises(SystemExit):
                    migrate_index.safe_path(bad, base, allow_parent=True)
            with self.assertRaises(SystemExit):
                migrate_index.safe_path("../bash", base, allow_parent=False)

    def test_symlink_components_are_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "registry"
            base.mkdir()
            outside = Path(td) / "outside"
            outside.mkdir()
            (outside / "secret.txt").write_text("secret", encoding="utf-8")
            (base / "link").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(SystemExit, "symlink in path"):
                migrate_index.safe_path("link/secret.txt", base, allow_parent=False)


class GrammarPinTests(unittest.TestCase):
    def test_gen_pack_rejects_malformed_or_unknown_pins(self):
        self.assertEqual(gen_pack.validate_grammar_rev("pinned:shell-v1", "tree-sitter-shell", ROOT),
                         "pinned:shell-v1")
        self.assertEqual(gen_pack.validate_grammar_rev("=0.25.0", "tree-sitter-python", ROOT),
                         "=0.25.0")
        for bad in ("", "pinned:", "pinned:../escape", "pinned:has space", "main",
                    "pinned:0.0.0"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                gen_pack.validate_grammar_rev(bad, "tree-sitter-python", ROOT)

    def test_sync_reads_overrides_and_pinned_versions(self):
        crates, subdirs, fixups = sync_grammars.load_overrides(ROOT)
        self.assertEqual(crates["tree-sitter-markdown"], "tree-sitter-md")
        self.assertEqual(subdirs["tree-sitter-md"], "tree-sitter-markdown")
        self.assertIn("highlights.scm", fixups["tree-sitter-solidity"])

        versions = sync_grammars.load_pinned_versions(ROOT)
        self.assertIn("0.25.0", versions["tree-sitter-python"])
        self.assertEqual(
            sync_grammars.resolve_rev("pinned:0.25.0", "tree-sitter-python", versions),
            "0.25.0",
        )
        self.assertEqual(
            sync_grammars.resolve_rev("=0.25.0", "tree-sitter-python", versions),
            "0.25.0",
        )
        for bad in ("pinned:missing", "pinned:", "main"):
            with self.subTest(bad=bad), self.assertRaises(SystemExit):
                sync_grammars.resolve_rev(bad, "tree-sitter-python", versions)


class SpdxExpressionTests(unittest.TestCase):
    def setUp(self):
        doc = tomllib.loads((ROOT / "about.toml").read_text(encoding="utf-8"))
        self.accepted = set(doc["accepted"])

    def test_slash_shorthand_is_normalized_to_or(self):
        self.assertEqual(notices.normalize_spdx("MIT/Apache-2.0"), "MIT OR Apache-2.0")
        self.assertEqual(notices.normalize_spdx("Apache-2.0 / MIT"), "Apache-2.0 OR MIT")
        self.assertTrue(notices.licenses_ok("MIT/Apache-2.0", self.accepted))

    def test_parentheses_and_with_are_preserved(self):
        expression = "(Apache-2.0 OR MIT) AND BSD-3-Clause"
        self.assertEqual(notices.normalize_spdx(expression), expression)
        self.assertEqual(notices.normalize_spdx("Apache-2.0 WITH LLVM-exception"),
                         "Apache-2.0 WITH LLVM-exception")
        self.assertTrue(notices.licenses_ok(expression, self.accepted))
        self.assertTrue(notices.licenses_ok("GPL-2.0-only OR BSD-3-Clause", self.accepted))
        self.assertFalse(notices.licenses_ok("GPL-2.0-only", self.accepted))

    def test_unknown_forms_fail_closed(self):
        for bad in ("", "MIT//Apache-2.0", "MIT AND", "MIT OR", "(MIT OR Apache-2.0) WITH X",
                    "see LICENSE in archive"):
            with self.subTest(bad=bad), self.assertRaises(notices.SpdxExpressionError):
                notices.normalize_spdx(bad)


EXAMPLE_COUNTS = {
    "01-hello.meld": 8,
    "02-bridge.meld": 6,
    "03-fenced.meld": 6,
    "04-highlight.meld": 31,
    "05-t2-run.meld": 6,
    "06-bridge.meld": 8,
    "07-t3-bridge.meld": 4,
    "08-shell-pack.meld": 2,
    "09-smoke.meld": 13,
    "10-more-languages.meld": 32,
}
RUNTIME_EXAMPLES = {"05-t2-run.meld", "07-t3-bridge.meld", "08-shell-pack.meld"}


class ExampleSnapshotTests(unittest.TestCase):
    def test_all_examples_parse_render_and_match_region_counts(self):
        paths = sorted(EXAMPLES.glob("*.meld"))
        self.assertEqual({p.name for p in paths}, set(EXAMPLE_COUNTS))
        for path in paths:
            with self.subTest(example=path.name):
                src = path.read_text(encoding="utf-8")
                doc = reference_splitter.split(src)
                self.assertEqual(reference_splitter.render(doc), src)
                self.assertEqual(len(doc["regions"]), EXAMPLE_COUNTS[path.name])
                self.assertTrue(doc["regions"])
                self.assertTrue(all(r["confidence"] >= 0.75 for r in doc["regions"]))

    @unittest.skipUnless(os.environ.get("MELD_BIN"),
                         "MELD_BIN is not set; runtime examples need a Meld binary")
    def test_runtime_examples_when_meld_is_available(self):
        binary = os.environ["MELD_BIN"]
        for name in sorted(RUNTIME_EXAMPLES):
            with self.subTest(example=name):
                subprocess.run([binary, "check", str(EXAMPLES / name)], check=True)


class ReferenceHarnessTests(unittest.TestCase):
    def test_harnesses_do_not_import_fuzz_splitter(self):
        for name in ("meld_check_ref.py", "highlight_ref.py"):
            source = (TOOLS / name).read_text(encoding="utf-8")
            self.assertNotIn("fuzz_splitter", source)
            self.assertIn("from reference_splitter import split", source)

    def test_reference_harnesses_still_pass(self):
        for name in ("meld_check_ref.py", "highlight_ref.py"):
            with self.subTest(harness=name):
                result = subprocess.run(
                    [sys.executable, str(TOOLS / name)], cwd=ROOT,
                    text=True, capture_output=True,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_external_harnesses_have_prerequisite_probes(self):
        t2 = (TOOLS / "t2_ref.py").read_text(encoding="utf-8")
        self.assertIn("not on PATH", t2)
        self.assertIn("SKIPPED (no T2 runtimes available)", t2)
        result = subprocess.run(
            ["sh", str(TOOLS / "build_t3_guest.sh"), "--check"], cwd=ROOT,
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue("READY" in result.stdout or "SKIP" in result.stdout)


class ManualSmokeTests(unittest.TestCase):
    def test_manual_tool_smoke_script_is_read_only(self):
        result = subprocess.run(
            [sys.executable, str(TOOLS / "smoke_manual_tools.py")], cwd=ROOT,
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("manual tool smoke: PASS", result.stdout)


if __name__ == "__main__":
    unittest.main()
