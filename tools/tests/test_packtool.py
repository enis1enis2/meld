#!/usr/bin/env python3
"""Unit/CLI tests for tools/packtool.py Ed25519 signing and registry tooling.

Run from the project root with:
    python3 -m unittest discover -s tools/tests -v

JUDGMENT CALL: tests drive the CLI through subprocesses where the contract is
user-visible (exit codes, messages) and use the module API only to set up
fixtures. Every key pair is generated into a temporary directory by packtool;
no private key material is written into this repository or this test file.
Nothing in a pack is imported or executed by packtool; the generated worker
file writes a marker file if anything ever did execute it, and tests assert
that the marker is absent after install.

HMAC was removed: the old symmetric ``signature.hmac`` format is rejected, and
``--key`` now exits with an explicit migration error instead of checking a MAC.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "packtool.py"
FIXTURE = ROOT / "tests" / "fixtures" / "pack-third-party"
PYDEPS = ROOT.parent / "pydeps"
if PYDEPS.is_dir() and str(PYDEPS) not in sys.path:
    sys.path.insert(0, str(PYDEPS))
sys.path.insert(0, str(ROOT / "tools"))
import packtool  # noqa: E402

CHILD_ENV = os.environ.copy()
if PYDEPS.is_dir():
    existing = CHILD_ENV.get("PYTHONPATH", "")
    CHILD_ENV["PYTHONPATH"] = os.pathsep.join(
        [str(PYDEPS), existing] if existing else [str(PYDEPS)]
    )


def cli(*args: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(TOOL), *(str(a) for a in args)],
        cwd=ROOT, text=True, capture_output=True, env=CHILD_ENV,
    )


def key_pair(base: Path, name: str = "signer") -> tuple[Path, Path]:
    private = base / f"{name}.private.pem"
    public = base / f"{name}.public.pem"
    result = cli("keygen", "--private-key", private, "--public-key", public)
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    return private, public


def trust_roots(base: Path, public: Path, name: str = "trust-roots.toml",
                label: str = "Test Ed25519 key") -> Path:
    target = base / name
    result = cli(
        "trust-roots", "init", "--trust-roots", target, "--public-key", public,
        "--label", label, "--created", "2026-11-20",
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    return target


def revocations(base: Path, name: str = "revocations.json") -> Path:
    target = base / name
    result = cli("revocations", "init", "--revocations", target)
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    return target


class PackToolTests(unittest.TestCase):
    def make_pack(self, base: str, *, lang: str = "demo", strict: bool = False,
                  version: str = "0.1.0", aliases: list[str] | None = None,
                  signed: bool = False, private_key: Path | None = None):
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
        key = private_key
        if signed:
            if key is None:
                key, _public = key_pair(Path(base))
            result = cli("sign", pack, "--private-key", key)
            if result.returncode != 0:
                raise AssertionError(result.stderr)
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

    def test_sign_refuses_stale_checksums(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            private, _public = key_pair(base)
            pack, _ = self.make_pack(base, private_key=private)
            (pack / "worker.py").write_text("# changed\n", encoding="utf-8")
            result = cli("sign", pack, "--private-key", private)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("refusing to sign stale checksums", result.stderr)
            self.assertIn("checksum mismatch: worker.py", result.stderr)
            self.assertFalse((pack / packtool.SIGNATURE_FILE).exists())

    def test_ed25519_sign_verify_unknown_key_revocation_and_tamper(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            private, public = key_pair(base)
            roots = trust_roots(base, public)
            revoked = revocations(base)
            pack, _ = self.make_pack(base, strict=True, signed=True, private_key=private)

            signature = (pack / packtool.SIGNATURE_FILE).read_text(encoding="utf-8")
            parts = signature.split()
            self.assertEqual(parts[0], "meld-pack/ed25519/v1")
            self.assertEqual(len(parts[2]), 128)
            self.assertEqual(signature, signature.lower())

            result = cli("verify", pack, "--trust-roots", roots, "--revocations", revoked)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("signature: verified", result.stdout)
            self.assertIn("not verified", cli("verify", pack).stdout)

            # A trust root for a different key must not authenticate this pack.
            _other_private, other_public = key_pair(base, "other")
            other_roots = trust_roots(base, other_public, "other-roots.toml")
            result = cli("verify", pack, "--trust-roots", other_roots)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is not in the trust roots", result.stderr)

            # Revocation is checked before signature verification.
            result = cli("revocations", "revoke", "--revocations", revoked,
                         "--key-id", parts[1], "--reason", "compromised",
                         "--revoked", "2026-11-20")
            self.assertEqual(result.returncode, 0, result.stderr)
            result = cli("verify", pack, "--trust-roots", roots, "--revocations", revoked)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is revoked", result.stderr)

            result = cli("verify", pack, "--require-trusted")
            self.assertEqual(result.returncode, 1)
            self.assertIn("trusted Ed25519 signature required", result.stderr)

            # Re-sign after a payload change, then tamper with the signed
            # checksums file: the exact signed message no longer matches.
            (pack / "worker.py").write_text("# changed\n", encoding="utf-8")
            packtool.write_checksums(pack)
            result = cli("verify", pack, "--trust-roots", roots)
            self.assertEqual(result.returncode, 1)
            self.assertIn("signature mismatch", result.stderr)

    def test_strict_requirements_and_key_placement(self):
        with tempfile.TemporaryDirectory() as td:
            pack, _ = self.make_pack(td, strict=True, signed=False)
            registry = Path(td) / "registry"
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("strict pack requires signature.ed25519", result.stderr)
            result = cli("publish", pack, "--registry", registry)
            self.assertEqual(result.returncode, 1)
            self.assertIn("strict pack requires signature.ed25519", result.stderr)
            self.assertFalse(registry.exists())

            (pack / "checksums.sha256").unlink()
            result = cli("verify", pack)
            self.assertEqual(result.returncode, 1)
            self.assertIn("requires checksums.sha256", result.stderr)

            key_inside = pack / "secret.key"
            key_inside.write_text("not a real key", encoding="utf-8")
            result = cli("sign", pack, "--private-key", key_inside)
            self.assertEqual(result.returncode, 1)
            self.assertIn("private key file must not be inside the pack", result.stderr)
            key_inside.unlink()

            result = cli("sign", pack, "--key", key_inside)
            self.assertEqual(result.returncode, 1)
            self.assertIn("HMAC --key was removed", result.stderr)

    def test_exact_crate_version_counts_as_a_pin(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            private, public = key_pair(base)
            roots = trust_roots(base, public)
            pack, _ = self.make_pack(base, lang="grammar", strict=True,
                                     private_key=private, signed=False)
            (pack / "pack.toml").write_text(
                (pack / "pack.toml").read_text(encoding="utf-8").replace(
                    'rev = "pinned:demo-v1"', 'rev = "0.25.0"'
                ),
                encoding="utf-8",
            )
            packtool.write_checksums(pack)
            result = cli("sign", pack, "--private-key", private)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = cli("verify", pack, "--trust-roots", roots)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("strict: true", result.stdout)
            self.assertIn("signature: verified", result.stdout)

    def test_publish_list_install_round_trip(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            private, public = key_pair(base)
            roots = trust_roots(base, public)
            revoked = revocations(base)
            pack, _ = self.make_pack(
                td, strict=True, private_key=private, signed=True,
                aliases=["d", "demo-alias"],
            )
            registry = base / "registry"
            result = cli("publish", pack, "--registry", registry,
                         "--trust-roots", roots, "--revocations", revoked)
            self.assertEqual(result.returncode, 0, result.stderr)
            index = json.loads((registry / "index.json").read_text(encoding="utf-8"))
            self.assertEqual(index["schema"], "meld-pack-registry/1")
            self.assertEqual(len(index["packs"]), 1)
            entry = index["packs"][0]
            self.assertEqual(entry["id"], "demo")
            self.assertEqual(entry["path"], "packs/demo/0.1.0")
            self.assertEqual(entry["signature"]["scheme"], "meld-pack/ed25519/v1")
            self.assertEqual(len(entry["signature"]["signature"]), 128)
            self.assertTrue((registry / entry["path"] / "worker.py").is_file())

            result = cli("publish", pack, "--registry", registry,
                         "--trust-roots", roots, "--revocations", revoked)
            self.assertEqual(result.returncode, 0, result.stderr)

            result = cli("list", "--registry", registry)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("demo 0.1.0 tier=T2 level=L1 strict=true signed", result.stdout)
            result = cli("list", "--registry", registry, "--json")
            self.assertEqual(json.loads(result.stdout), index)

            into = base / "installed"
            result = cli("install", "demo-alias", "--registry", registry,
                         "--into", into, "--trust-roots", roots,
                         "--revocations", revoked, "--require-trusted")
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = into / "demo"
            self.assertTrue((installed / "pack.toml").is_file())
            result = cli("verify", installed, "--trust-roots", roots)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((installed / "EXECUTED").exists())
            self.assertFalse((into / "EXECUTED").exists())

            result = cli("install", "demo", "--registry", registry, "--into", into,
                         "--trust-roots", roots, "--require-trusted")
            self.assertEqual(result.returncode, 1)
            self.assertIn("install target already exists", result.stderr)

    def test_unsigned_non_strict_and_index_tamper(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            pack, _ = self.make_pack(td, strict=False, signed=False, lang="plain")
            registry = base / "registry"
            result = cli("publish", pack, "--registry", registry)
            self.assertEqual(result.returncode, 0, result.stderr)
            entry = json.loads((registry / "index.json").read_text(encoding="utf-8"))["packs"][0]
            self.assertIsNone(entry["signature"])
            into = base / "installed"
            result = cli("install", "plain", "--registry", registry, "--into", into)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = cli("verify", into / "plain")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("signature: absent", result.stdout)

            result = cli("install", "no-such-lang", "--registry", registry,
                         "--into", base / "nope")
            self.assertEqual(result.returncode, 1)
            self.assertIn("unknown language or alias", result.stderr)

            index_path = registry / "index.json"
            doc = json.loads(index_path.read_text(encoding="utf-8"))
            doc["packs"][0]["checksums_sha256"] = "0" * 64
            index_path.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            result = cli("install", "plain", "--registry", registry,
                         "--into", base / "tampered")
            self.assertEqual(result.returncode, 1)
            self.assertIn("registry index checksum does not match", result.stderr)

    def test_exact_id_wins_over_a_colliding_alias(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            self.make_pack(base, lang="bash", aliases=["shell", "sh"])
            self.make_pack(base, lang="shell", aliases=["sh"])
            registry = base / "registry"
            for lang in ("bash", "shell"):
                result = cli("publish", base / lang, "--registry", registry)
                self.assertEqual(result.returncode, 0, result.stderr)

            installed = cli("install", "shell", "--registry", registry,
                            "--into", base / "by-id")
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertEqual(
                packtool.load_manifest(base / "by-id" / "shell")["id"], "shell"
            )

            ambiguous = cli("install", "sh", "--registry", registry,
                            "--into", base / "by-alias")
            self.assertEqual(ambiguous.returncode, 1)
            self.assertIn("ambiguous alias 'sh'", ambiguous.stderr)

    def test_registry_attestation_binding_and_revocation(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td)
            private, public = key_pair(base)
            roots = trust_roots(base, public)
            revoked = revocations(base)
            registry = base / "registry"
            registry.mkdir()
            (registry / "index.json").write_text(
                json.dumps({"schema": "meld-pack-registry/1", "packs": []},
                           indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            (registry / "trust-roots.toml").write_bytes(roots.read_bytes())
            (registry / "revocations.json").write_bytes(revoked.read_bytes())
            result = cli("mirrors", "--registry", registry,
                         "--mirror", "https://mirror.example/meld")
            self.assertEqual(result.returncode, 0, result.stderr)
            result = cli("attest", "--registry", registry, "--private-key", private)
            self.assertEqual(result.returncode, 0, result.stderr)

            result = cli("verify-registry", registry, "--trust-roots", roots,
                         "--revocations", revoked)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("attestation: verified", result.stdout)
            self.assertIn("OK", result.stdout)

            # An unsigned registry index change breaks the attestation.
            index_path = registry / "index.json"
            doc = json.loads(index_path.read_text(encoding="utf-8"))
            doc["packs"].append({
                "id": "ghost",
                "version": "0.1.0",
                "aliases": [],
                "tier": "T2",
                "level": "L1",
                "strict": False,
                "path": "packs/ghost/0.1.0",
                "checksums_sha256": "0" * 64,
                "signature": None,
            })
            index_path.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            result = cli("verify-registry", registry, "--trust-roots", roots,
                         "--revocations", revoked)
            self.assertEqual(result.returncode, 1)
            self.assertIn("attestation mismatch", result.stderr)

            # Restoring the index and revoking the attestation key also fails.
            doc["packs"] = []
            index_path.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            key_id = packtool._key_id(packtool._read_public_bytes(public))
            result = cli("revocations", "revoke", "--revocations", revoked,
                         "--key-id", key_id, "--revoked", "2026-11-20")
            self.assertEqual(result.returncode, 0, result.stderr)
            (registry / "revocations.json").write_bytes(revoked.read_bytes())
            result = cli("verify-registry", registry, "--trust-roots", roots,
                         "--revocations", revoked)
            self.assertEqual(result.returncode, 1)
            self.assertIn("is revoked", result.stderr)

    def test_fixture_is_complete_and_data_driven(self):
        for name in ("pack.toml", "runner.toml", "toolchain.toml", "worker.sh",
                     "README.md", "checksums.sha256", "signature.ed25519",
                     "tests/conformance.json", "tests/feed.meld"):
            self.assertTrue((FIXTURE / name).is_file(), name)
        self.assertFalse((FIXTURE / "signature.hmac").exists())
        cases = json.loads((FIXTURE / "tests" / "conformance.json").read_text())
        self.assertGreaterEqual(len(cases["cases"]), 3)
        self.assertTrue(all("code" in case for case in cases["cases"]))
        self.assertEqual(packtool.load_manifest(FIXTURE)["id"], "shell")


if __name__ == "__main__":
    unittest.main()
