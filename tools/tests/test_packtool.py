#!/usr/bin/env python3
"""Unit/CLI tests for tools/packtool.py (Phase 7 registry tooling).

Run from the project root with:
    python3 -m unittest discover -s tools/tests -v

JUDGMENT CALL: tests drive the CLI through subprocesses where the contract is
user-visible (exit codes, messages), and use the module API only to set up
fixtures. Nothing in a pack is imported or executed by packtool; the generated
worker file writes a marker file if anything ever did execute it, and tests
assert that the marker is absent after install.
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import packtool  # noqa: E402

TOOL = ROOT / "tools" / "packtool.py"
FIXTURE = ROOT / "tests" / "fixtures" / "pack-third-party"


def cli(*args: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(TOOL), *(str(a) for a in args)],
        cwd=ROOT, text=True, capture_output=True,
    )


class PackToolTests(unittest.TestCase):
    def make_pack(self, base: str, *, lang: str = "demo", strict: bool = False,
                  version: str = "0.1.0", aliases: list[str] | None = None,
                  signed: bool = False):
        pack = Path(base) / lang
        pack.mkdir(parents=True)
        (pack / "tests").mkdir()
        (pack / "pack.toml").write_text(
            f'id = "{lang}"\n'
            f'version = "{version}"\n'
            f'aliases = {json.dumps(aliases or [])}\n'
            'tier = "T2"\n'
            'level = "L1"\n'
            'capabilities = ["stdout"]\n'
            'grammar_rev = "pinned:demo-v1"\n'
            f'strict = {str(strict).lower()}\n'
            '\n[tree_sitter]\n'
            'crate = "tree-sitter-demo"\n'
            'rev = "pinned:demo-v1"\n',
            encoding="utf-8",
        )
        # Marker is only written if someone executes pack code. packtool must
        # not: it copies bytes and verifies hashes, nothing more.
        (pack / "worker.py").write_text(
            "from pathlib import Path\nPath('EXECUTED').write_text('yes')\n",
            encoding="utf-8",
        )
        (pack / "tests" / "case.json").write_text('{"case": "hello"}\n', encoding="utf-8")
        packtool.write_checksums(pack)
        key = None
        if signed:
            key = Path(base) / "private.key"
            key.write_bytes(b"k" * 32)
            packtool.sign_pack(pack, key)
        return pack, key

    def test_verify_ok_and_demo_fixture(self):
        with tempfile.TemporaryDirectory() as td:
            pack, _ = self.make_pack(td)
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("3 file(s) verified", result.stdout)
            self.assertIn("OK", result.stdout)
        result = cli("verify", FIXTURE)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("strict: true", result.stdout)
        self.assertIn("shell", result.stdout)

    def test_tamper_detection(self):
        with tempfile.TemporaryDirectory() as td:
            pack, _ = self.make_pack(td)
            worker = pack / "worker.py"
            original = worker.read_text(encoding="utf-8")
            worker.write_text(original + "# tampered\n", encoding="utf-8")
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("checksum mismatch: worker.py", result.stderr)

            worker.write_text(original, encoding="utf-8")
            extra = pack / "extra.txt"
            extra.write_text("unexpected\n", encoding="utf-8")
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("unexpected file(s): extra.txt", result.stderr)

            extra.unlink()
            worker.unlink()
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("missing file(s): worker.py", result.stderr)

            # Non-canonical checksum files are rejected too.
            self.make_pack(td, lang="canonical")
            checksums = Path(td) / "canonical" / "checksums.sha256"
            lines = checksums.read_text(encoding="utf-8").splitlines()
            checksums.write_text("\n".join(reversed(lines)) + "\n", encoding="utf-8")
            result = cli("verify", Path(td) / "canonical")
            self.assertEqual(result.returncode, 1)
            self.assertIn("not canonical", result.stderr)

    def test_sign_and_verify(self):
        with tempfile.TemporaryDirectory() as td:
            pack, key = self.make_pack(td, strict=True, signed=True)
            result = cli("verify", pack, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("signature: verified", result.stdout)

            result = cli("verify", pack)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("not verified", result.stdout)

            wrong = Path(td) / "wrong.key"
            wrong.write_bytes(b"x" * 32)
            result = cli("verify", pack, "--key", wrong)
            self.assertEqual(result.returncode, 1)
            self.assertIn("key-id does not match", result.stderr)

            # Recompute checksums after a payload change: the old MAC is stale.
            (pack / "worker.py").write_text("# changed\n", encoding="utf-8")
            packtool.write_checksums(pack)
            result = cli("verify", pack, "--key", key)
            self.assertEqual(result.returncode, 1)
            self.assertIn("signature mismatch", result.stderr)

    def test_strict_requirements_and_key_placement(self):
        with tempfile.TemporaryDirectory() as td:
            pack, _ = self.make_pack(td, strict=True, signed=False)
            registry = Path(td) / "registry"
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("strict pack requires signature.hmac", result.stderr)
            result = cli("publish", pack, "--registry", registry)
            self.assertEqual(result.returncode, 1)
            self.assertIn("strict pack requires signature.hmac", result.stderr)
            self.assertFalse(registry.exists())

            (pack / "checksums.sha256").unlink()
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("requires checksums.sha256", result.stderr)

            key_inside = pack / "secret.key"
            key_inside.write_bytes(b"z" * 32)
            result = cli("sign", pack, "--key", key_inside)
            self.assertEqual(result.returncode, 1)
            self.assertIn("key file must not be inside the pack", result.stderr)

    def test_exact_crate_version_counts_as_a_pin(self):
        with tempfile.TemporaryDirectory() as td:
            pack, key = self.make_pack(td, lang="grammar", strict=True, signed=True)
            # Checked-in grammar packs pin `rev` to an exact crate version;
            # sync_grammars.py consumes that value directly, so it must verify.
            (pack / "pack.toml").write_text(
                (pack / "pack.toml").read_text().replace(
                    'rev = "pinned:demo-v1"', 'rev = "0.25.0"'
                ),
                encoding="utf-8",
            )
            packtool.write_checksums(pack)
            packtool.sign_pack(pack, key)
            result = cli("verify", pack, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("strict: true", result.stdout)
            self.assertIn("signature: verified", result.stdout)

    def test_publish_list_install_round_trip(self):
        with tempfile.TemporaryDirectory() as td:
            pack, key = self.make_pack(
                td, strict=True, signed=True, aliases=["d", "demo-alias"],
            )
            registry = Path(td) / "registry"
            result = cli("publish", pack, "--registry", registry, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)
            index = json.loads((registry / "index.json").read_text(encoding="utf-8"))
            self.assertEqual(index["schema"], "meld-pack-registry/1")
            self.assertEqual(len(index["packs"]), 1)
            entry = index["packs"][0]
            self.assertEqual(entry["id"], "demo")
            self.assertEqual(entry["path"], "packs/demo/0.1.0")
            self.assertEqual(entry["signature"]["scheme"], "hmac-sha256")
            self.assertTrue((registry / entry["path"] / "worker.py").is_file())

            # Idempotent re-publish of identical bytes.
            result = cli("publish", pack, "--registry", registry, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)

            result = cli("list", "--registry", registry)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("demo 0.1.0 tier=T2 level=L1 strict=true signed", result.stdout)
            result = cli("list", "--registry", registry, "--json")
            self.assertEqual(json.loads(result.stdout), index)

            into = Path(td) / "installed"
            result = cli("install", "demo-alias", "--registry", registry,
                         "--into", into, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = into / "demo"
            self.assertTrue((installed / "pack.toml").is_file())
            result = cli("verify", installed, "--key", key)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((installed / "EXECUTED").exists())
            self.assertFalse((into / "EXECUTED").exists())

            result = cli("install", "demo", "--registry", registry, "--into", into)
            self.assertEqual(result.returncode, 1)
            self.assertIn("install target already exists", result.stderr)

    def test_unsigned_non_strict_and_index_tamper(self):
        with tempfile.TemporaryDirectory() as td:
            pack, _ = self.make_pack(td, strict=False, signed=False, lang="plain")
            registry = Path(td) / "registry"
            result = cli("publish", pack, "--registry", registry)
            self.assertEqual(result.returncode, 0, result.stderr)
            entry = json.loads((registry / "index.json").read_text())["packs"][0]
            self.assertIsNone(entry["signature"])
            into = Path(td) / "installed"
            result = cli("install", "plain", "--registry", registry, "--into", into)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = cli("verify", into / "plain")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("signature: absent", result.stdout)

            result = cli("install", "no-such-lang", "--registry", registry,
                         "--into", Path(td) / "nope")
            self.assertEqual(result.returncode, 1)
            self.assertIn("unknown language or alias", result.stderr)

            index_path = registry / "index.json"
            doc = json.loads(index_path.read_text(encoding="utf-8"))
            doc["packs"][0]["checksums_sha256"] = "0" * 64
            index_path.write_text(json.dumps(doc), encoding="utf-8")
            result = cli("install", "plain", "--registry", registry,
                         "--into", Path(td) / "tampered")
            self.assertEqual(result.returncode, 1)
            self.assertIn("registry index checksum does not match", result.stderr)

    def test_fixture_is_complete_and_data_driven(self):
        for name in ("pack.toml", "runner.toml", "toolchain.toml", "worker.sh",
                     "README.md", "checksums.sha256", "signature.hmac",
                     "tests/conformance.json", "tests/feed.meld"):
            self.assertTrue((FIXTURE / name).is_file(), name)
        cases = json.loads((FIXTURE / "tests" / "conformance.json").read_text())
        self.assertGreaterEqual(len(cases["cases"]), 3)
        self.assertTrue(all("code" in case for case in cases["cases"]))
        self.assertEqual(packtool.load_manifest(FIXTURE)["id"], "shell")


if __name__ == "__main__":
    unittest.main()
