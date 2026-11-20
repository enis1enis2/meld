#!/usr/bin/env python3
"""Local pack registry and signing CLI for Meld Lang Packs (Phase 7).

Authority: project.md §4 (Lang Pack contract), §7 (tooling), §9 (security),
§11 (Phase 7: "a third party adds a language without touching core"). The
registry is a directory of *data*; this tool never downloads, installs a
toolchain, imports pack code, or spawns a pack worker.

JUDGMENT CALL (signing): HMAC-SHA256 with a local key file is used instead of
public-key signatures. It needs only the Python standard library (no packages
may be installed here) and gives local integrity/authenticity between parties
that already share a key. Full public-key signing and key distribution are
future work, documented in spec/registry.md. Because HMAC is symmetric,
``verify`` without ``--key`` checks only that a well-formed signature *exists*;
it cannot prove authenticity. ``--key`` is therefore available on verify,
publish and install for full MAC checking.

JUDGMENT CALL (Python version): ``tomllib`` requires Python 3.11+. The project
environment has Python 3.12; no vendored TOML parser is shipped.

JUDGMENT CALL (copies): install/publish are plain filesystem copies of pack
data. They never rewrite runner.toml (including its project-relative worker
path) and never act on toolchain.toml; toolchain detection remains `meld
doctor`'s job (project.md §9).
"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

SCHEMA = "meld-pack-registry/1"
CHECKSUM_FILE = "checksums.sha256"
SIGNATURE_FILE = "signature.hmac"
KEY_ID_CHARS = 16
MIN_KEY_BYTES = 16
ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
HEX64_RE = re.compile(r"^[0-9a-f]{64}$")
HEX16_RE = re.compile(r"^[0-9a-f]{16}$")
# JUDGMENT CALL: "pinned" has two honest spellings in this project. Generated
# packs use an opaque `pinned:<ref>`; the checked-in grammar packs use exact
# crate versions because tools/sync_grammars.py consumes that value directly.
# Both are immutable. A bare `rev = "main"` or `rev = "pinned"` is not.
EXACT_VERSION_RE = re.compile(r"^=?\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$")
CONTROL_FILES = frozenset({CHECKSUM_FILE, SIGNATURE_FILE})
SKIP_DIRS = frozenset({".git", "__pycache__", ".mypy_cache", ".pytest_cache"})
SKIP_SUFFIXES = (".pyc", ".pyo")
TIERS = frozenset({"L0", "T1", "T2", "T3", "T4", "T5"})
LEVELS = frozenset({"L0", "L1", "L2", "L3"})


class PackToolError(Exception):
    """Expected failure; rendered as ``packtool: error: ...`` with exit code 1."""


@dataclass(frozen=True)
class Verification:
    pack_dir: Path
    pack_id: str
    version: str
    strict: bool
    file_count: int
    signature: str  # "absent" | "present-unverified" | "verified"
    key_id: str | None


# --- integrity primitives -------------------------------------------------

def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _iter_files(pack_dir: Path) -> list[tuple[str, Path]]:
    """Return sorted (posix-relative path, Path) pairs for pack payload files.

    JUDGMENT CALL: checksums.sha256 and signature.hmac are control files and
    cannot checksum themselves; caches and VCS metadata are excluded so a
    checkout does not change a pack's digest. Symlinks are rejected outright:
    a symlink could make a published pack read outside its own directory
    (project.md §9 supply-chain rule).
    """
    if not pack_dir.is_dir():
        raise PackToolError(f"not a pack directory: {pack_dir}")
    found: list[tuple[str, Path]] = []
    for root, dirs, files in os.walk(pack_dir, followlinks=False):
        kept = []
        for name in sorted(dirs):
            child = Path(root) / name
            if child.is_symlink():
                raise PackToolError(f"symlink in pack is not allowed: {child}")
            if name not in SKIP_DIRS:
                kept.append(name)
        dirs[:] = kept
        for name in sorted(files):
            path = Path(root) / name
            if path.is_symlink():
                raise PackToolError(f"symlink in pack is not allowed: {path}")
            if name in CONTROL_FILES or name.endswith(SKIP_SUFFIXES):
                continue
            found.append((path.relative_to(pack_dir).as_posix(), path))
    return sorted(found)


def compute_checksums(pack_dir: Path) -> dict[str, str]:
    return {rel: _sha256_file(path) for rel, path in _iter_files(pack_dir)}


def _checksums_text(entries: dict[str, str]) -> str:
    # Canonical form: lowercase hex, two spaces, posix path, LF, sorted by path.
    return "".join(f"{entries[rel]}  {rel}\n" for rel in sorted(entries))


def write_checksums(pack_dir: Path) -> Path:
    target = Path(pack_dir) / CHECKSUM_FILE
    target.write_text(_checksums_text(compute_checksums(pack_dir)), encoding="utf-8")
    return target


def read_checksums(pack_dir: Path) -> dict[str, str]:
    path = Path(pack_dir) / CHECKSUM_FILE
    if not path.is_file():
        raise PackToolError(f"missing {CHECKSUM_FILE} in {pack_dir}")
    text = path.read_text(encoding="utf-8")
    entries: dict[str, str] = {}
    for lineno, line in enumerate(text.splitlines(), 1):
        if not line:
            continue
        match = re.fullmatch(r"([0-9a-f]{64})  (\S.*)", line)
        if not match:
            raise PackToolError(f"malformed {CHECKSUM_FILE}:{lineno} in {pack_dir}")
        digest, rel = match.groups()
        if rel in entries:
            raise PackToolError(f"duplicate path in {CHECKSUM_FILE}: {rel}")
        if Path(rel).is_absolute() or ".." in Path(rel).parts or "\\" in rel:
            raise PackToolError(f"unsafe path in {CHECKSUM_FILE}: {rel}")
        entries[rel] = digest
    if text != _checksums_text(entries):
        raise PackToolError(f"{CHECKSUM_FILE} is not canonical (sorted paths, LF, two spaces)")
    return entries


def _read_key(key_path: Path) -> bytes:
    key = Path(key_path).read_bytes()
    if len(key) < MIN_KEY_BYTES:
        raise PackToolError(f"key must be at least {MIN_KEY_BYTES} bytes")
    return key


def _key_id(key: bytes) -> str:
    return hashlib.sha256(key).hexdigest()[:KEY_ID_CHARS]


def _mac(data: bytes, key: bytes) -> str:
    return hmac.new(key, data, hashlib.sha256).hexdigest()


def _signature_text(key: bytes, checksums_bytes: bytes) -> str:
    return f"hmac-sha256 {_key_id(key)} {_mac(checksums_bytes, key)}\n"


def parse_signature(text: str) -> tuple[str, str]:
    parts = text.strip().split()
    if (len(parts) != 3 or parts[0] != "hmac-sha256"
            or not HEX16_RE.fullmatch(parts[1]) or not HEX64_RE.fullmatch(parts[2])):
        raise PackToolError(f"malformed {SIGNATURE_FILE} (expected 'hmac-sha256 <key-id> <digest>')")
    return parts[1], parts[2]


def _read_signature(pack_dir: Path) -> tuple[str, str] | None:
    path = Path(pack_dir) / SIGNATURE_FILE
    if not path.is_file():
        return None
    return parse_signature(path.read_text(encoding="utf-8"))


def sign_pack(pack_dir: Path, key_path: Path) -> Path:
    pack_dir = Path(pack_dir)
    key_path = Path(key_path)
    # JUDGMENT CALL: a key inside the pack would be published with the pack and
    # would let anyone forge its signature, so refuse that combination.
    if key_path.resolve().is_relative_to(pack_dir.resolve()):
        raise PackToolError("key file must not be inside the pack directory")
    key = _read_key(key_path)
    read_checksums(pack_dir)  # never sign a pack whose checksums are stale/malformed
    checksums_bytes = (pack_dir / CHECKSUM_FILE).read_bytes()
    target = pack_dir / SIGNATURE_FILE
    target.write_text(_signature_text(key, checksums_bytes), encoding="utf-8")
    return target


# --- manifests and verification -------------------------------------------

def load_manifest(pack_dir: Path) -> dict:
    path = Path(pack_dir) / "pack.toml"
    if not path.is_file():
        raise PackToolError(f"missing pack.toml in {pack_dir}")
    try:
        with path.open("rb") as handle:
            manifest = tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        raise PackToolError(f"invalid pack.toml in {pack_dir}: {exc}") from exc
    pack_id = manifest.get("id")
    if not isinstance(pack_id, str) or not ID_RE.fullmatch(pack_id):
        raise PackToolError(f"invalid or missing pack id in {pack_dir}")
    if manifest.get("tier") not in TIERS:
        raise PackToolError(f"unknown tier in {path} (expected one of {sorted(TIERS)})")
    if manifest.get("level") not in LEVELS:
        raise PackToolError(f"unknown level in {path} (expected one of {sorted(LEVELS)})")
    return manifest


def _version(manifest: dict) -> str:
    # JUDGMENT CALL: packs written before the registry existed have no version;
    # they are published as 0.0.0 rather than being rejected.
    version = str(manifest.get("version", "0.0.0"))
    if not ID_RE.fullmatch(version):
        raise PackToolError(f"invalid version in pack.toml: {version!r}")
    return version


def _check_pinned_rev(manifest: dict, pack_dir: Path) -> None:
    rev = (manifest.get("tree_sitter") or {}).get("rev", "")
    pinned_ref = isinstance(rev, str) and rev.startswith("pinned:") and len(rev) > len("pinned:")
    exact_version = isinstance(rev, str) and EXACT_VERSION_RE.fullmatch(rev.lstrip("=")) is not None
    if manifest.get("strict") and not (pinned_ref or exact_version):
        raise PackToolError(
            f"strict pack requires a pinned [tree_sitter].rev "
            f"(`pinned:<ref>` or exact version) in {pack_dir}/pack.toml"
        )


def verify_pack(pack_dir: Path, key_path: Path | None = None) -> Verification:
    """Verify payloads against checksums, then (if present) the signature.

    Checksums are always required: without them there is nothing to verify.
    A signature is required only for ``strict = true`` packs, matching
    spec/lang-pack.md §6. A key is optional; MAC verification happens only
    when one is supplied.
    """
    pack_dir = Path(pack_dir)
    manifest = load_manifest(pack_dir)
    strict = bool(manifest.get("strict", False))
    _check_pinned_rev(manifest, pack_dir)
    if strict and not (pack_dir / CHECKSUM_FILE).is_file():
        raise PackToolError(f"strict pack requires {CHECKSUM_FILE} (missing in {pack_dir})")

    expected = read_checksums(pack_dir)
    actual = compute_checksums(pack_dir)
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    changed = sorted(rel for rel in set(expected) & set(actual) if expected[rel] != actual[rel])
    problems = []
    if missing:
        problems.append("missing file(s): " + ", ".join(missing))
    if extra:
        problems.append("unexpected file(s): " + ", ".join(extra))
    if changed:
        problems.append("checksum mismatch: " + ", ".join(changed))
    if problems:
        raise PackToolError("; ".join(problems))

    signature = _read_signature(pack_dir)
    if strict and signature is None:
        raise PackToolError(f"strict pack requires {SIGNATURE_FILE} (missing in {pack_dir})")
    state, key_id = ("absent", None) if signature is None else ("present-unverified", signature[0])
    if signature is not None and key_path is not None:
        key = _read_key(key_path)
        if signature[0] != _key_id(key):
            raise PackToolError("signature key-id does not match the supplied key")
        want = _mac((pack_dir / CHECKSUM_FILE).read_bytes(), key)
        if not hmac.compare_digest(signature[1], want):
            raise PackToolError("signature mismatch: checksums were modified or the key is wrong")
        state = "verified"
    return Verification(
        pack_dir=pack_dir,
        pack_id=str(manifest["id"]),
        version=_version(manifest),
        strict=strict,
        file_count=len(actual),
        signature=state,
        key_id=key_id,
    )


# --- registry -------------------------------------------------------------

def load_registry(registry_dir: Path, create: bool = False) -> dict:
    path = Path(registry_dir) / "index.json"
    if not path.is_file():
        if not create:
            raise PackToolError(f"registry index not found: {path}")
        return {"schema": SCHEMA, "packs": []}
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PackToolError(f"invalid registry index {path}: {exc}") from exc
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        raise PackToolError(f"unsupported registry schema in {path} (expected {SCHEMA})")
    if not isinstance(doc.get("packs"), list):
        raise PackToolError(f"invalid registry index {path}: 'packs' must be a list")
    return doc


def save_registry(registry_dir: Path, doc: dict) -> Path:
    path = Path(registry_dir) / "index.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    # JUDGMENT CALL: no timestamps in the index. Canonical, sorted JSON makes
    # repeated publishes byte-for-byte reproducible (project.md §9).
    path.write_text(json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
                    encoding="utf-8")
    return path


def _entry(pack_dir: Path, verification: Verification) -> dict:
    manifest = load_manifest(pack_dir)
    signature = _read_signature(pack_dir)
    aliases = manifest.get("aliases", [])
    if not isinstance(aliases, list) or not all(isinstance(a, str) for a in aliases):
        raise PackToolError("pack.toml aliases must be a list of strings")
    return {
        "id": verification.pack_id,
        "version": verification.version,
        "aliases": sorted(aliases),
        "tier": str(manifest.get("tier")),
        "level": str(manifest.get("level")),
        "strict": verification.strict,
        # JUDGMENT CALL: registry paths are relative to the registry directory
        # so a registry can be moved or mirrored without rewriting the index.
        "path": f"packs/{verification.pack_id}/{verification.version}",
        "checksums_sha256": _sha256_file(pack_dir / CHECKSUM_FILE),
        "signature": (None if signature is None else {
            "scheme": "hmac-sha256", "key_id": signature[0], "digest": signature[1],
        }),
    }


def _copy_pack(src: Path, dst: Path) -> None:
    shutil.copytree(src, dst, symlinks=False,
                    ignore=shutil.ignore_patterns(*SKIP_DIRS, "*.pyc", "*.pyo"))


def publish_pack(pack_dir: Path, registry_dir: Path, key_path: Path | None = None) -> dict:
    pack_dir, registry_dir = Path(pack_dir), Path(registry_dir)
    verification = verify_pack(pack_dir, key_path)
    entry = _entry(pack_dir, verification)
    target = registry_dir / entry["path"]
    doc = load_registry(registry_dir, create=True)
    existing = [e for e in doc["packs"]
                if e.get("id") == entry["id"] and e.get("version") == entry["version"]]
    if target.exists():
        # JUDGMENT CALL: re-publishing the *same* bytes is idempotent; changing
        # content under an existing id/version would break pinned consumers,
        # so it requires a new version instead.
        if not existing or existing[0] != entry:
            raise PackToolError(
                f"different pack already published at {entry['id']} {entry['version']}; bump version"
            )
        return existing[0]
    if existing:
        raise PackToolError(
            f"index lists {entry['id']} {entry['version']} but its pack directory is missing"
        )
    _copy_pack(pack_dir, target)
    doc["packs"].append(entry)
    doc["packs"].sort(key=lambda e: (e["id"], _version_key(e["version"])))
    save_registry(registry_dir, doc)
    return entry


def _version_key(version: str) -> tuple[int, ...]:
    return tuple(int(p) if p.isdigit() else 0 for p in re.split(r"[.\-+_]", version))


def find_entry(doc: dict, lang: str) -> dict:
    matches = [e for e in doc["packs"] if e.get("id") == lang or lang in e.get("aliases", [])]
    if not matches:
        raise PackToolError(f"unknown language or alias {lang!r} in registry")
    ids = {e["id"] for e in matches}
    if len(ids) > 1:
        # JUDGMENT CALL: an alias collision is ambiguous even if versions differ;
        # guessing would violate the "never silently guess" rule in project.md §3.1.
        raise PackToolError(f"ambiguous alias {lang!r}: {', '.join(sorted(ids))}")
    return max(matches, key=lambda e: _version_key(e["version"]))


def install_pack(lang: str, registry_dir: Path, into_dir: Path,
                 key_path: Path | None = None) -> Path:
    """Copy a verified pack into ``<into_dir>/<id>``; never execute pack code."""
    doc = load_registry(registry_dir)
    entry = find_entry(doc, lang)
    registry_dir = Path(registry_dir)
    src = (registry_dir / entry["path"]).resolve()
    if not src.is_relative_to(registry_dir.resolve()):
        raise PackToolError(f"unsafe pack path in registry index: {entry['path']}")
    verify_pack(src, key_path)
    # The index itself is untrusted data too: pin it to the staged pack so a
    # hand-edited index cannot swap in different bytes.
    if _sha256_file(src / CHECKSUM_FILE) != entry.get("checksums_sha256"):
        raise PackToolError("registry index checksum does not match the staged pack")
    staged_sig = _read_signature(src)
    index_sig = entry.get("signature")
    if index_sig is None:
        if staged_sig is not None:
            raise PackToolError("registry index signature does not match the staged pack")
    elif (staged_sig is None or index_sig.get("scheme") != "hmac-sha256"
          or index_sig.get("key_id") != staged_sig[0]
          or index_sig.get("digest") != staged_sig[1]):
        raise PackToolError("registry index signature does not match the staged pack")
    target = Path(into_dir) / entry["id"]
    if target.exists():
        # JUDGMENT CALL: no silent overwrite of an installed pack; the caller
        # removes it explicitly (install is data-only, not a package manager).
        raise PackToolError(f"install target already exists: {target}")
    _copy_pack(src, target)
    verify_pack(target, key_path)  # verify the bytes as installed, not just the source
    return target


# --- CLI ------------------------------------------------------------------

def _print_verification(v: Verification) -> None:
    print(f"pack: {v.pack_dir}")
    print(f"id: {v.pack_id}  version: {v.version}  strict: {str(v.strict).lower()}")
    print(f"checksums: {v.file_count} file(s) verified")
    if v.signature == "absent":
        print("signature: absent")
    elif v.signature == "present-unverified":
        print(f"signature: present (key-id {v.key_id}) but not verified; pass --key for MAC check")
    else:
        print(f"signature: verified (key-id {v.key_id})")
    print("OK")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    subs = parser.add_subparsers(dest="command", required=True)

    p_verify = subs.add_parser("verify", help="verify checksums and (optionally) signature")
    p_verify.add_argument("pack_dir")
    p_verify.add_argument("--key", help="local HMAC key file (enables MAC verification)")

    p_sign = subs.add_parser("sign", help="write signature.hmac over checksums.sha256")
    p_sign.add_argument("pack_dir")
    p_sign.add_argument("--key", required=True, help="local HMAC key file")

    p_publish = subs.add_parser("publish", help="verify and copy a pack into a registry")
    p_publish.add_argument("pack_dir")
    p_publish.add_argument("--registry", required=True)
    p_publish.add_argument("--key", help="local HMAC key file (optional MAC check)")

    p_list = subs.add_parser("list", help="list published packs")
    p_list.add_argument("--registry", required=True)
    p_list.add_argument("--json", action="store_true", help="emit the index as JSON")

    p_install = subs.add_parser("install", help="copy a verified pack out of a registry")
    p_install.add_argument("lang", help="pack id or alias")
    p_install.add_argument("--registry", required=True)
    p_install.add_argument("--into", required=True)
    p_install.add_argument("--key", help="local HMAC key file (optional MAC check)")

    args = parser.parse_args(argv)
    try:
        if args.command == "verify":
            _print_verification(verify_pack(args.pack_dir, args.key))
        elif args.command == "sign":
            key_path = Path(args.key)
            if key_path.stat().st_mode & 0o077:
                print("packtool: warning: key file is group/world readable", file=sys.stderr)
            path = sign_pack(args.pack_dir, key_path)
            print(f"signed: {path}")
        elif args.command == "publish":
            entry = publish_pack(args.pack_dir, args.registry, args.key)
            print(f"published: {entry['id']} {entry['version']} -> {args.registry}/{entry['path']}")
        elif args.command == "list":
            doc = load_registry(args.registry)
            if args.json:
                print(json.dumps(doc, indent=2, sort_keys=True))
            elif not doc["packs"]:
                print("(no packs published)")
            else:
                print(f"registry: {args.registry}")
                for e in sorted(doc["packs"], key=lambda x: (x["id"], _version_key(x["version"]))):
                    sig = "signed" if e.get("signature") else "unsigned"
                    print(f"{e['id']} {e['version']} tier={e['tier']} level={e['level']} "
                          f"strict={str(e.get('strict', False)).lower()} {sig} path={e['path']}")
        elif args.command == "install":
            target = install_pack(args.lang, args.registry, args.into, args.key)
            print(f"installed: {target}")
    except (PackToolError, OSError) as exc:
        print(f"packtool: error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
