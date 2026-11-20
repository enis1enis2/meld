#!/usr/bin/env python3
"""Local pack registry and Ed25519 signing CLI for Meld Lang Packs (Phase 7).

Authority: project.md §4 (Lang Pack contract), §7 (tooling), §9 (security),
§11 (Phase 7: "a third party adds a language without touching core"). The
registry is a directory of *data*; this tool never downloads, installs a
toolchain, imports pack code, or spawns a pack worker.

JUDGMENT CALL (signing): HMAC-SHA256 was cut over to public-key Ed25519. A
symmetric key cannot be a public trust root: everyone who can verify can also
forge. Ed25519 gives verifiers an immutable public key while the private key
stays outside the repository. The development/release private key is generated
and stored outside every worktree; rotation is expressed by adding a key to
``trust-roots.toml`` and revocation by adding its key-id to
``revocations.json``. A signed registry attestation binds the canonical index,
trust roots, revocations and mirrors so metadata cannot be swapped unnoticed.

JUDGMENT CALL (Python version): ``tomllib`` requires Python 3.11+. The project
environment has Python 3.12; no vendored TOML parser is shipped. Cryptography
50.0.2 supplies Ed25519 primitives; in this sandbox its wheels are available
via ``PYTHONPATH=/mnt/data/opencode/tmp/opencode/pydeps``.

JUDGMENT CALL (copies): install/publish are plain filesystem copies of pack
data. They never rewrite runner.toml (including its project-relative worker
path) and never act on toolchain.toml; toolchain detection remains `meld
doctor`'s job (project.md §9).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import tomllib
from dataclasses import dataclass
from datetime import date
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

SCHEMA = "meld-pack-registry/1"
CHECKSUM_FILE = "checksums.sha256"
SIGNATURE_FILE = "signature.ed25519"
SIGNATURE_SCHEME = "meld-pack/ed25519/v1"
KEY_ID_CHARS = 16
TRUST_ROOTS_SCHEMA = "meld-trust-roots/1"
REVOCATIONS_SCHEMA = "meld-revocations/1"
MIRRORS_SCHEMA = "meld-registry-mirrors/1"
ATTESTATION_SCHEME = "meld-registry/attestation/ed25519/v1"
ATTESTATION_FILE = "attestation.ed25519"
TRUST_ROOTS_FILE = "trust-roots.toml"
REVOCATIONS_FILE = "revocations.json"
MIRRORS_FILE = "mirrors.json"
ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
HEX64_RE = re.compile(r"^[0-9a-f]{64}$")
HEX128_RE = re.compile(r"^[0-9a-f]{128}$")
HEX16_RE = re.compile(r"^[0-9a-f]{16}$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
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

    JUDGMENT CALL: checksums.sha256 and signature.ed25519 are control files and
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


# --- Ed25519 primitives ---------------------------------------------------

def _load_private_key(path: Path) -> Ed25519PrivateKey:
    path = Path(path)
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise PackToolError(f"cannot read private key {path}: {exc}") from exc
    try:
        key = serialization.load_pem_private_key(data, password=None)
    except (TypeError, ValueError) as exc:
        raise PackToolError(f"invalid Ed25519 private-key PEM in {path}: {exc}") from exc
    if not isinstance(key, Ed25519PrivateKey):
        raise PackToolError(f"private key in {path} is not Ed25519")
    return key


def _public_bytes(key: Ed25519PrivateKey) -> bytes:
    return key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )


def _key_id(public_bytes: bytes) -> str:
    if len(public_bytes) != 32:
        raise PackToolError("an Ed25519 public key must be 32 bytes")
    return hashlib.sha256(public_bytes).hexdigest()[:KEY_ID_CHARS]


def _read_public_bytes(value: object) -> bytes:
    """Accept a 64-hex raw public key, a PEM file, or a 32-byte raw-key file."""
    text = str(value)
    if HEX64_RE.fullmatch(text):
        return bytes.fromhex(text)
    path = Path(text)
    if not path.is_file():
        raise PackToolError("--public-key must be 64 lowercase hex chars or an existing PEM file")
    data = path.read_bytes()
    try:
        key = serialization.load_pem_public_key(data)
    except (TypeError, ValueError):
        key = None
    if isinstance(key, Ed25519PublicKey):
        return key.public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw,
        )
    if len(data) == 32:
        return data
    raise PackToolError(f"--public-key file {path} is not an Ed25519 public key")


def _sign(key: Ed25519PrivateKey, message: bytes) -> str:
    return key.sign(message).hex()


def _verify(public_bytes: bytes, message: bytes, signature_hex: str) -> bool:
    try:
        key = Ed25519PublicKey.from_public_bytes(public_bytes)
        key.verify(bytes.fromhex(signature_hex), message)
    except (ValueError, TypeError, InvalidSignature):
        return False
    return True


def _pack_message(pack_id: str, version: str, checksums_bytes: bytes) -> bytes:
    # The signed message is exactly this byte sequence; no normalization, no
    # JSON wrapper, and no trailing newline beyond the checksum file's own LF.
    return (
        SIGNATURE_SCHEME.encode("utf-8")
        + b"\n"
        + pack_id.encode("utf-8")
        + b"\n"
        + version.encode("utf-8")
        + b"\n"
        + checksums_bytes
    )


def _signature_text(key: Ed25519PrivateKey, message: bytes) -> str:
    return f"{SIGNATURE_SCHEME} {_key_id(_public_bytes(key))} {_sign(key, message)}\n"


def parse_signature(text: str, *, scheme: str = SIGNATURE_SCHEME) -> tuple[str, str]:
    if not text.endswith("\n") or text.count("\n") != 1:
        raise PackToolError(f"malformed {scheme} signature (expected exactly one LF-terminated line)")
    parts = text.strip().split()
    if (len(parts) != 3 or parts[0] != scheme
            or not HEX16_RE.fullmatch(parts[1]) or not HEX128_RE.fullmatch(parts[2])):
        raise PackToolError(
            f"malformed {SIGNATURE_FILE} (expected '{scheme} <key-id> <128 hex chars>')"
        )
    return parts[1], parts[2]


def _read_signature(pack_dir: Path) -> tuple[str, str] | None:
    path = Path(pack_dir) / SIGNATURE_FILE
    if not path.is_file():
        return None
    return parse_signature(path.read_text(encoding="utf-8"))


def sign_pack(pack_dir: Path, private_key_path: Path) -> Path:
    pack_dir = Path(pack_dir)
    key_path = Path(private_key_path)
    # JUDGMENT CALL: a private key inside the pack would be published with the
    # pack and would let anyone forge its signature, so refuse that combination.
    if key_path.resolve().is_relative_to(pack_dir.resolve()):
        raise PackToolError("private key file must not be inside the pack directory")
    key = _load_private_key(key_path)
    # JUDGMENT CALL 54: signing a canonical but stale manifest would produce a
    # pack that fails verification on every host. Recompute and compare the
    # payload digests before signing; the signed message still uses the exact
    # checksum bytes on disk, including their platform line endings.
    _expected, _actual, problems = _checksum_problems(pack_dir)
    if problems:
        raise PackToolError("refusing to sign stale checksums: " + "; ".join(problems))
    manifest = load_manifest(pack_dir)
    message = _pack_message(
        str(manifest["id"]), _version(manifest), (pack_dir / CHECKSUM_FILE).read_bytes()
    )
    target = pack_dir / SIGNATURE_FILE
    target.write_text(_signature_text(key, message), encoding="utf-8")
    return target


# --- trust roots, revocations, mirrors ------------------------------------

def canonical_json_bytes(doc: object) -> bytes:
    return (json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


def _canonical_compact_json(doc: object) -> bytes:
    return json.dumps(doc, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def canonical_trust_roots_bytes(doc: dict) -> bytes:
    lines = [f'schema = "{TRUST_ROOTS_SCHEMA}"', ""]
    for entry in doc["keys"]:
        lines.extend([
            "[[keys]]",
            f'key_id = "{entry["key_id"]}"',
            f'public_key = "{entry["public_key"]}"',
            f"label = {json.dumps(entry['label'], ensure_ascii=False)}",
            f'created = "{entry["created"]}"',
            "",
        ])
    return "\n".join(lines).encode("utf-8")


def _new_root(public_key: object, label: str, created: str, *, today: str | None = None) -> dict:
    created = created or today or date.today().isoformat()
    if not DATE_RE.fullmatch(created):
        raise PackToolError("--created must be YYYY-MM-DD")
    if not label.strip():
        raise PackToolError("--label must not be empty")
    raw = _read_public_bytes(public_key)
    return {
        "key_id": _key_id(raw),
        "public_key": raw.hex(),
        "label": label,
        "created": created,
    }


def load_trust_roots(path: Path) -> dict:
    path = Path(path)
    if not path.is_file():
        raise PackToolError(f"trust roots not found: {path}")
    try:
        doc = tomllib.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise PackToolError(f"invalid trust roots {path}: {exc}") from exc
    if not isinstance(doc, dict) or doc.get("schema") != TRUST_ROOTS_SCHEMA:
        raise PackToolError(
            f"unsupported trust-root schema in {path} (expected {TRUST_ROOTS_SCHEMA})"
        )
    if set(doc) != {"schema", "keys"} or not isinstance(doc["keys"], list):
        raise PackToolError(f"invalid trust roots {path}: expected only schema and keys")
    seen: set[str] = set()
    for index, entry in enumerate(doc["keys"]):
        where = f"{path}: keys[{index}]"
        if not isinstance(entry, dict) or set(entry) != {"key_id", "public_key", "label", "created"}:
            raise PackToolError(
                f"invalid {where}: key_id, public_key, label and created are required"
            )
        if not HEX16_RE.fullmatch(entry["key_id"]):
            raise PackToolError(f"invalid key-id in {where}")
        if not HEX64_RE.fullmatch(entry["public_key"]):
            raise PackToolError(f"invalid public key in {where} (expected 64 lowercase hex chars)")
        if not isinstance(entry["label"], str) or not entry["label"].strip():
            raise PackToolError(f"invalid label in {where}")
        if not isinstance(entry["created"], str) or not DATE_RE.fullmatch(entry["created"]):
            raise PackToolError(f"invalid created date in {where} (expected YYYY-MM-DD)")
        expected = _key_id(bytes.fromhex(entry["public_key"]))
        if entry["key_id"] != expected:
            raise PackToolError(f"key-id does not match public key in {where} (expected {expected})")
        if entry["key_id"] in seen:
            raise PackToolError(f"duplicate trust root {entry['key_id']} in {path}")
        seen.add(entry["key_id"])
    if path.read_bytes() != canonical_trust_roots_bytes(doc):
        raise PackToolError(f"{path} is not canonical (sorted key ids, LF, fixed fields)")
    return doc


def write_trust_roots(path: Path, doc: dict) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(canonical_trust_roots_bytes(load_trust_roots_doc(doc)))
    return path


def load_trust_roots_doc(doc: dict) -> dict:
    """Validate an in-memory trust-root document with the same rules as a file."""
    if not isinstance(doc, dict) or doc.get("schema") != TRUST_ROOTS_SCHEMA:
        raise PackToolError(f"unsupported trust-root schema (expected {TRUST_ROOTS_SCHEMA})")
    if set(doc) != {"schema", "keys"} or not isinstance(doc["keys"], list):
        raise PackToolError("invalid trust roots: expected only schema and keys")
    seen: set[str] = set()
    for entry in doc["keys"]:
        if not isinstance(entry, dict) or set(entry) != {"key_id", "public_key", "label", "created"}:
            raise PackToolError("invalid trust root entry")
        if not HEX16_RE.fullmatch(entry["key_id"]) or not HEX64_RE.fullmatch(entry["public_key"]):
            raise PackToolError("invalid trust root key-id or public key")
        if _key_id(bytes.fromhex(entry["public_key"])) != entry["key_id"]:
            raise PackToolError("trust root key-id does not match its public key")
        if entry["key_id"] in seen:
            raise PackToolError(f"duplicate trust root {entry['key_id']}")
        seen.add(entry["key_id"])
    return doc


def load_revocations(path: Path) -> dict:
    path = Path(path)
    if not path.is_file():
        raise PackToolError(f"revocations not found: {path}")
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PackToolError(f"invalid revocations {path}: {exc}") from exc
    if not isinstance(doc, dict) or doc.get("schema") != REVOCATIONS_SCHEMA:
        raise PackToolError(
            f"unsupported revocations schema in {path} (expected {REVOCATIONS_SCHEMA})"
        )
    if set(doc) != {"schema", "revoked"} or not isinstance(doc["revoked"], list):
        raise PackToolError(f"invalid revocations {path}: expected only schema and revoked")
    seen: set[str] = set()
    for index, entry in enumerate(doc["revoked"]):
        where = f"{path}: revoked[{index}]"
        if not isinstance(entry, dict) or set(entry) != {"key_id", "reason", "revoked"}:
            raise PackToolError(f"invalid {where}: key_id, reason and revoked are required")
        if not HEX16_RE.fullmatch(entry["key_id"]):
            raise PackToolError(f"invalid key-id in {where}")
        if not isinstance(entry["reason"], str):
            raise PackToolError(f"invalid reason in {where}")
        if not isinstance(entry["revoked"], str) or not DATE_RE.fullmatch(entry["revoked"]):
            raise PackToolError(f"invalid revocation date in {where} (expected YYYY-MM-DD)")
        if entry["key_id"] in seen:
            raise PackToolError(f"duplicate revocation {entry['key_id']} in {path}")
        seen.add(entry["key_id"])
    if path.read_bytes() != canonical_json_bytes(doc):
        raise PackToolError(f"{path} is not canonical (sorted keys, LF, two-space indent)")
    return doc


def write_revocations(path: Path, doc: dict) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    loaded = load_revocations_from_doc(doc)
    path.write_bytes(canonical_json_bytes(loaded))
    return path


def load_revocations_from_doc(doc: object) -> dict:
    if not isinstance(doc, dict) or doc.get("schema") != REVOCATIONS_SCHEMA:
        raise PackToolError(f"unsupported revocations schema (expected {REVOCATIONS_SCHEMA})")
    if set(doc) != {"schema", "revoked"} or not isinstance(doc["revoked"], list):
        raise PackToolError("invalid revocations: expected only schema and revoked")
    seen: set[str] = set()
    for entry in doc["revoked"]:
        if not isinstance(entry, dict) or set(entry) != {"key_id", "reason", "revoked"}:
            raise PackToolError("invalid revocation entry")
        if not HEX16_RE.fullmatch(entry["key_id"]) or not DATE_RE.fullmatch(entry["revoked"]):
            raise PackToolError("invalid revocation key-id or date")
        if entry["key_id"] in seen:
            raise PackToolError(f"duplicate revocation {entry['key_id']}")
        seen.add(entry["key_id"])
    return doc


def load_mirrors(path: Path) -> dict:
    path = Path(path)
    if not path.is_file():
        raise PackToolError(f"mirrors not found: {path}")
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PackToolError(f"invalid mirrors {path}: {exc}") from exc
    if not isinstance(doc, dict) or doc.get("schema") != MIRRORS_SCHEMA:
        raise PackToolError(f"unsupported mirrors schema in {path} (expected {MIRRORS_SCHEMA})")
    if set(doc) != {"schema", "mirrors"} or not isinstance(doc["mirrors"], list):
        raise PackToolError(f"invalid mirrors {path}: expected only schema and mirrors")
    if not all(isinstance(m, str) and m.strip() for m in doc["mirrors"]):
        raise PackToolError(f"invalid mirror entry in {path}")
    if path.read_bytes() != canonical_json_bytes(doc):
        raise PackToolError(f"{path} is not canonical (sorted keys, LF, two-space indent)")
    return doc


def write_mirrors(path: Path, mirrors: list[str]) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    doc = {"schema": MIRRORS_SCHEMA, "mirrors": sorted(set(mirrors))}
    path.write_bytes(canonical_json_bytes(doc))
    return path


def _registry_files(registry_dir: Path, trust_roots: Path | None,
                    revocations: Path | None, mirrors: Path | None) -> dict[str, Path]:
    registry_dir = Path(registry_dir)
    return {
        "index.json": registry_dir / "index.json",
        TRUST_ROOTS_FILE: Path(trust_roots) if trust_roots else registry_dir / TRUST_ROOTS_FILE,
        REVOCATIONS_FILE: Path(revocations) if revocations else registry_dir / REVOCATIONS_FILE,
        MIRRORS_FILE: Path(mirrors) if mirrors else registry_dir / MIRRORS_FILE,
    }


def _attestation_message(files: dict[str, bytes]) -> bytes:
    payload = {
        "schema": ATTESTATION_SCHEME,
        "files": {
            name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            for name, data in sorted(files.items())
        },
    }
    return ATTESTATION_SCHEME.encode("utf-8") + b"\n" + _canonical_compact_json(payload)


def _read_attestation(registry_dir: Path) -> tuple[str, str]:
    path = Path(registry_dir) / ATTESTATION_FILE
    if not path.is_file():
        raise PackToolError(f"registry attestation not found: {path}")
    return parse_signature(path.read_text(encoding="utf-8"), scheme=ATTESTATION_SCHEME)


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


def _checksum_problems(pack_dir: Path) -> tuple[dict[str, str], dict[str, str], list[str]]:
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
    return expected, actual, problems


def verify_pack(pack_dir: Path, trust_roots_path: Path | None = None,
                revocations_path: Path | None = None, require_trusted: bool = False) -> Verification:
    """Verify payloads against checksums, then (if present) the Ed25519 signature.

    Checksums are always required. A signature is required only for
    ``strict = true`` packs, matching spec/lang-pack.md §6. Trust roots turn a
    signature from a well-formed blob into a provenance claim; without them the
    report stays "present-unverified". ``require_trusted`` turns that into a hard
    failure, which is the default for remote installs.
    """
    pack_dir = Path(pack_dir)
    manifest = load_manifest(pack_dir)
    strict = bool(manifest.get("strict", False))
    _check_pinned_rev(manifest, pack_dir)
    if strict and not (pack_dir / CHECKSUM_FILE).is_file():
        raise PackToolError(f"strict pack requires {CHECKSUM_FILE} (missing in {pack_dir})")

    _expected, actual, problems = _checksum_problems(pack_dir)
    if problems:
        raise PackToolError("; ".join(problems))

    signature = _read_signature(pack_dir)
    if strict and signature is None:
        raise PackToolError(f"strict pack requires {SIGNATURE_FILE} (missing in {pack_dir})")
    state, key_id = ("absent", None) if signature is None else ("present-unverified", signature[0])
    if signature is not None and trust_roots_path is not None:
        roots = load_trust_roots(Path(trust_roots_path))
        trusted = {entry["key_id"]: entry["public_key"] for entry in roots["keys"]}
        if revocations_path is not None:
            revoked = {entry["key_id"] for entry in load_revocations(Path(revocations_path))["revoked"]}
        else:
            revoked = set()
        if signature[0] in revoked:
            raise PackToolError(f"signature key-id {signature[0]} is revoked")
        public_hex = trusted.get(signature[0])
        if public_hex is None:
            raise PackToolError(
                f"signature key-id {signature[0]} is not in the trust roots; rotate trust roots "
                "for a new key instead of accepting an unknown signer"
            )
        message = _pack_message(
            str(manifest["id"]), _version(manifest), (pack_dir / CHECKSUM_FILE).read_bytes()
        )
        if not _verify(bytes.fromhex(public_hex), message, signature[1]):
            raise PackToolError(
                "signature mismatch: checksums, pack id, version or signer were modified"
            )
        state = "verified"
    if require_trusted and state != "verified":
        if state == "absent":
            raise PackToolError("trusted Ed25519 signature required but none is present")
        raise PackToolError("trusted Ed25519 signature required; pass --trust-roots")
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
    for index, entry in enumerate(doc["packs"]):
        if not isinstance(entry, dict):
            raise PackToolError(f"invalid registry index {path}: packs[{index}] is not an object")
        sig = entry.get("signature")
        if entry.get("strict") and not isinstance(sig, dict):
            raise PackToolError(f"strict pack {entry.get('id')} has no signature in {path}")
        if sig is not None:
            if (not isinstance(sig, dict) or sig.get("scheme") != SIGNATURE_SCHEME
                    or not HEX16_RE.fullmatch(str(sig.get("key_id", "")))
                    or not HEX128_RE.fullmatch(str(sig.get("signature", "")))):
                raise PackToolError(f"invalid Ed25519 signature metadata in {path}: packs[{index}]")
        if not HEX64_RE.fullmatch(str(entry.get("checksums_sha256", ""))):
            raise PackToolError(f"invalid checksum metadata in {path}: packs[{index}]")
    if path.read_bytes() != canonical_json_bytes(doc):
        raise PackToolError(f"{path} is not canonical (sorted keys, LF, two-space indent)")
    return doc


def save_registry(registry_dir: Path, doc: dict) -> Path:
    path = Path(registry_dir) / "index.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    # JUDGMENT CALL: no timestamps in the index. Canonical, sorted JSON makes
    # repeated publishes byte-for-byte reproducible (project.md §9).
    path.write_bytes(canonical_json_bytes(load_registry_from_doc(doc)))
    return path


def load_registry_from_doc(doc: dict) -> dict:
    if not isinstance(doc, dict) or doc.get("schema") != SCHEMA:
        raise PackToolError(f"unsupported registry schema (expected {SCHEMA})")
    if not isinstance(doc.get("packs"), list):
        raise PackToolError("invalid registry index: 'packs' must be a list")
    return doc


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
            "scheme": SIGNATURE_SCHEME, "key_id": signature[0], "signature": signature[1],
        }),
    }


def _copy_pack(src: Path, dst: Path) -> None:
    shutil.copytree(src, dst, symlinks=False,
                    ignore=shutil.ignore_patterns(*SKIP_DIRS, "*.pyc", "*.pyo"))


def publish_pack(pack_dir: Path, registry_dir: Path, trust_roots_path: Path | None = None,
                 revocations_path: Path | None = None) -> dict:
    pack_dir, registry_dir = Path(pack_dir), Path(registry_dir)
    verify_pack(pack_dir, trust_roots_path, revocations_path)
    entry = _entry(pack_dir, verify_pack(pack_dir))
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
    # JUDGMENT CALL: an exact id always wins over aliases. The checked-in
    # registry ships both `bash` (alias `shell`) and the third-party `shell`
    # pack, so treating both as candidates would make the package's own id
    # unaddressable. Alias-only collisions remain errors: guessing between two
    # different ids would violate the "never silently guess" rule in project.md
    # §3.1, and this matches `meld-pack`'s "exact id first, then unique alias".
    exact = [e for e in doc["packs"] if e.get("id") == lang]
    if exact:
        return max(exact, key=lambda e: _version_key(e["version"]))
    matches = [e for e in doc["packs"] if lang in e.get("aliases", [])]
    if not matches:
        raise PackToolError(f"unknown language or alias {lang!r} in registry")
    ids = {e["id"] for e in matches}
    if len(ids) > 1:
        raise PackToolError(f"ambiguous alias {lang!r}: {', '.join(sorted(ids))}")
    return max(matches, key=lambda e: _version_key(e["version"]))


def resolve_registry_pack(registry_dir: Path, rel: str, attested: bool) -> Path:
    registry_dir = Path(registry_dir).resolve()
    rel_path = Path(rel)
    if rel_path.is_absolute() or "\\" in rel:
        raise PackToolError(f"unsafe pack path in registry index: {rel}")
    candidate = (registry_dir / rel_path).resolve()
    # An attested built-in index may reference sibling pack directories (and
    # the checked-in fixture) with `../` paths. Without a valid attestation the
    # index is data only and must stay inside the registry directory.
    allowed_root = registry_dir.parent.parent if attested else registry_dir
    if not candidate.is_relative_to(allowed_root.resolve()):
        raise PackToolError(f"unsafe pack path in registry index: {rel}")
    return candidate


def attest_registry(registry_dir: Path, private_key_path: Path | None,
                    trust_roots_path: Path | None, revocations_path: Path | None,
                    mirrors_path: Path | None, output: Path | None = None) -> Path:
    registry_dir = Path(registry_dir)
    paths = _registry_files(registry_dir, trust_roots_path, revocations_path, mirrors_path)
    doc = load_registry(registry_dir)
    trust_doc = load_trust_roots(paths[TRUST_ROOTS_FILE])
    revocations_doc = load_revocations(paths[REVOCATIONS_FILE])
    mirrors_doc = load_mirrors(paths[MIRRORS_FILE])
    files = {
        "index.json": canonical_json_bytes(doc),
        TRUST_ROOTS_FILE: canonical_trust_roots_bytes(trust_doc),
        REVOCATIONS_FILE: canonical_json_bytes(revocations_doc),
        MIRRORS_FILE: canonical_json_bytes(mirrors_doc),
    }
    key = _load_private_key(Path(private_key_path) if private_key_path else Path("."))
    key_id = _key_id(_public_bytes(key))
    trusted = {entry["key_id"] for entry in trust_doc["keys"]}
    revoked = {entry["key_id"] for entry in revocations_doc["revoked"]}
    if key_id in revoked:
        raise PackToolError(f"attestation signer {key_id} is revoked")
    if key_id not in trusted:
        raise PackToolError(
            f"attestation signer {key_id} is not a trust root; add its public key first"
        )
    signature = _sign(key, _attestation_message(files))
    target = Path(output) if output else registry_dir / ATTESTATION_FILE
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(f"{ATTESTATION_SCHEME} {key_id} {signature}\n", encoding="utf-8")
    return target


def verify_registry(registry_dir: Path, trust_roots_path: Path | None = None,
                    revocations_path: Path | None = None,
                    mirrors_path: Path | None = None, verify_packs: bool = False) -> dict:
    registry_dir = Path(registry_dir)
    paths = _registry_files(registry_dir, trust_roots_path, revocations_path, mirrors_path)
    doc = load_registry(registry_dir)
    trust_doc = load_trust_roots(paths[TRUST_ROOTS_FILE])
    revocations_doc = load_revocations(paths[REVOCATIONS_FILE])
    mirrors_doc = load_mirrors(paths[MIRRORS_FILE])
    files = {
        "index.json": canonical_json_bytes(doc),
        TRUST_ROOTS_FILE: canonical_trust_roots_bytes(trust_doc),
        REVOCATIONS_FILE: canonical_json_bytes(revocations_doc),
        MIRRORS_FILE: canonical_json_bytes(mirrors_doc),
    }
    key_id, signature = _read_attestation(registry_dir)
    revoked = {entry["key_id"] for entry in revocations_doc["revoked"]}
    if key_id in revoked:
        raise PackToolError(f"attestation signer {key_id} is revoked")
    trusted = {entry["key_id"]: entry["public_key"] for entry in trust_doc["keys"]}
    public_hex = trusted.get(key_id)
    if public_hex is None:
        raise PackToolError(f"attestation key-id {key_id} is not in the trust roots")
    if not _verify(bytes.fromhex(public_hex), _attestation_message(files), signature):
        raise PackToolError(
            "registry attestation mismatch: index, trust roots, revocations or mirrors were modified"
        )
    if verify_packs:
        for entry in doc["packs"]:
            src = resolve_registry_pack(registry_dir, entry["path"], attested=True)
            verify_pack(
                src, paths[TRUST_ROOTS_FILE], paths[REVOCATIONS_FILE],
                require_trusted=entry.get("signature") is not None,
            )
    return {
        "registry": registry_dir,
        "packs": len(doc["packs"]),
        "trust_roots": len(trust_doc["keys"]),
        "revoked": len(revocations_doc["revoked"]),
        "mirrors": len(mirrors_doc["mirrors"]),
        "key_id": key_id,
        "verified_packs": len(doc["packs"]) if verify_packs else 0,
    }


def install_pack(lang: str, registry_dir: Path, into_dir: Path,
                 trust_roots_path: Path | None = None,
                 revocations_path: Path | None = None,
                 require_trusted: bool = False) -> Path:
    """Copy a verified pack into ``<into_dir>/<id>``; never execute pack code."""
    doc = load_registry(registry_dir)
    entry = find_entry(doc, lang)
    registry_dir = Path(registry_dir)
    attested = (registry_dir / ATTESTATION_FILE).is_file()
    if attested:
        verify_registry(registry_dir, trust_roots_path, revocations_path)
    src = resolve_registry_pack(registry_dir, entry["path"], attested)
    verify_pack(src, trust_roots_path, revocations_path, require_trusted)
    # The index itself is untrusted data too: pin it to the staged pack so a
    # hand-edited index cannot swap in different bytes.
    if _sha256_file(src / CHECKSUM_FILE) != entry.get("checksums_sha256"):
        raise PackToolError("registry index checksum does not match the staged pack")
    staged_sig = _read_signature(src)
    index_sig = entry.get("signature")
    if index_sig is None:
        if staged_sig is not None:
            raise PackToolError("registry index signature does not match the staged pack")
    elif (staged_sig is None or index_sig.get("scheme") != SIGNATURE_SCHEME
          or index_sig.get("key_id") != staged_sig[0]
          or index_sig.get("signature") != staged_sig[1]):
        raise PackToolError("registry index signature does not match the staged pack")
    target = Path(into_dir) / entry["id"]
    if target.exists():
        # JUDGMENT CALL: no silent overwrite of an installed pack; the caller
        # removes it explicitly (install is data-only, not a package manager).
        raise PackToolError(f"install target already exists: {target}")
    _copy_pack(src, target)
    verify_pack(target, trust_roots_path, revocations_path, require_trusted)
    return target


# --- CLI ------------------------------------------------------------------

def _print_verification(v: Verification) -> None:
    print(f"pack: {v.pack_dir}")
    print(f"id: {v.pack_id}  version: {v.version}  strict: {str(v.strict).lower()}")
    print(f"checksums: {v.file_count} file(s) verified")
    if v.signature == "absent":
        print("signature: absent")
    elif v.signature == "present-unverified":
        print(f"signature: present (key-id {v.key_id}) but not verified; pass --trust-roots")
    else:
        print(f"signature: verified (key-id {v.key_id})")
    print("OK")


def _add_removed_key(parser: argparse.ArgumentParser) -> None:
    # Keep a visible, actionable error instead of argparse's generic complaint.
    parser.add_argument("--key", dest="removed_hmac_key", nargs="?", const="",
                        help=argparse.SUPPRESS)


def _cmd_keygen(args: argparse.Namespace) -> None:
    private = Path(args.private_key)
    public = Path(args.public_key)
    for path in (private, public):
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() and not args.force:
            raise PackToolError(f"refusing to overwrite {path}; pass --force")
    key = Ed25519PrivateKey.generate()
    private.write_bytes(key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.PKCS8,
        encryption_algorithm=serialization.NoEncryption(),
    ))
    private.chmod(0o600)
    public.write_bytes(key.public_key().public_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PublicFormat.SubjectPublicKeyInfo,
    ))
    print(f"key-id: {_key_id(_public_bytes(key))}")
    print(f"private-key: {private} (mode 0600, keep outside all repositories)")
    print(f"public-key: {public}")


def _cmd_trust_roots(args: argparse.Namespace) -> None:
    target = Path(args.trust_roots)
    if args.trust_command == "init":
        if target.exists() and not args.force:
            raise PackToolError(f"refusing to overwrite {target}; pass --force")
        doc = {"schema": TRUST_ROOTS_SCHEMA, "keys": []}
    else:
        doc = load_trust_roots(target)
    entry = _new_root(args.public_key, args.label, args.created)
    if any(existing["key_id"] == entry["key_id"] for existing in doc["keys"]):
        raise PackToolError(f"trust root {entry['key_id']} is already present")
    doc["keys"].append(entry)
    doc["keys"].sort(key=lambda e: e["key_id"])
    write_trust_roots(target, doc)
    print(f"trust root {entry['key_id']} -> {target}")


def _cmd_revocations(args: argparse.Namespace) -> None:
    target = Path(args.revocations)
    if args.revocation_command == "init":
        if target.exists() and not args.force:
            raise PackToolError(f"refusing to overwrite {target}; pass --force")
        write_revocations(target, {"schema": REVOCATIONS_SCHEMA, "revoked": []})
        print(f"revocations: {target}")
        return
    doc = load_revocations(target)
    key_id = args.key_id.lower()
    if not HEX16_RE.fullmatch(key_id):
        raise PackToolError("--key-id must be 16 lowercase hex chars")
    if any(entry["key_id"] == key_id for entry in doc["revoked"]):
        raise PackToolError(f"key-id {key_id} is already revoked")
    revoked_on = args.revoked or date.today().isoformat()
    if not DATE_RE.fullmatch(revoked_on):
        raise PackToolError("--revoked must be YYYY-MM-DD")
    doc["revoked"].append({
        "key_id": key_id,
        "reason": args.reason or "compromised or retired",
        "revoked": revoked_on,
    })
    doc["revoked"].sort(key=lambda e: e["key_id"])
    write_revocations(target, doc)
    print(f"revoked {key_id} in {target}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    subs = parser.add_subparsers(dest="command", required=True)

    p_verify = subs.add_parser("verify", help="verify checksums and (optionally) Ed25519 signature")
    p_verify.add_argument("pack_dir")
    p_verify.add_argument("--trust-roots", help="TOML trust roots (enables signature verification)")
    p_verify.add_argument("--revocations", help="JSON revocations (rejects revoked key ids)")
    p_verify.add_argument("--require-trusted", action="store_true",
                          help="fail unless the signature verifies against a trust root")
    _add_removed_key(p_verify)

    p_sign = subs.add_parser("sign", help="write signature.ed25519 over the signed pack message")
    p_sign.add_argument("pack_dir")
    p_sign.add_argument("--private-key", help="Ed25519 private-key PEM (outside the pack)")
    _add_removed_key(p_sign)

    p_publish = subs.add_parser("publish", help="verify and copy a pack into a registry")
    p_publish.add_argument("pack_dir")
    p_publish.add_argument("--registry", required=True)
    p_publish.add_argument("--trust-roots", help="verify signatures against these trust roots")
    p_publish.add_argument("--revocations", help="reject revoked key ids")
    _add_removed_key(p_publish)

    p_list = subs.add_parser("list", help="list published packs")
    p_list.add_argument("--registry", required=True)
    p_list.add_argument("--json", action="store_true", help="emit the index as JSON")

    p_install = subs.add_parser("install", help="copy a verified pack out of a registry")
    p_install.add_argument("lang", help="pack id or alias")
    p_install.add_argument("--registry", required=True)
    p_install.add_argument("--into", required=True)
    p_install.add_argument("--trust-roots", help="verify signatures against these trust roots")
    p_install.add_argument("--revocations", help="reject revoked key ids")
    p_install.add_argument("--require-trusted", action="store_true",
                           help="fail unless the signature verifies against a trust root")
    _add_removed_key(p_install)

    p_keygen = subs.add_parser("keygen", help="generate an Ed25519 key pair outside the repo")
    p_keygen.add_argument("--private-key", required=True)
    p_keygen.add_argument("--public-key", required=True)
    p_keygen.add_argument("--force", action="store_true", help="overwrite existing key files")

    p_roots = subs.add_parser("trust-roots", help="manage public trust roots")
    roots_subs = p_roots.add_subparsers(dest="trust_command", required=True)
    for name in ("init", "add"):
        p_root = roots_subs.add_parser(name)
        p_root.add_argument("--trust-roots", required=True)
        p_root.add_argument("--public-key", required=True,
                            help="Ed25519 public-key PEM, 32-byte raw file, or 64 hex chars")
        p_root.add_argument("--label", default="Meld Ed25519 release key")
        p_root.add_argument("--created", help="YYYY-MM-DD (default: today)")
        if name == "init":
            p_root.add_argument("--force", action="store_true")

    p_rev = subs.add_parser("revocations", help="manage revoked key ids")
    rev_subs = p_rev.add_subparsers(dest="revocation_command", required=True)
    p_rev_init = rev_subs.add_parser("init")
    p_rev_init.add_argument("--revocations", required=True)
    p_rev_init.add_argument("--force", action="store_true")
    p_rev_add = rev_subs.add_parser("revoke")
    p_rev_add.add_argument("--revocations", required=True)
    p_rev_add.add_argument("--key-id", required=True)
    p_rev_add.add_argument("--reason")
    p_rev_add.add_argument("--revoked", help="YYYY-MM-DD (default: today)")

    p_mirrors = subs.add_parser("mirrors", help="initialize canonical registry mirrors metadata")
    p_mirrors.add_argument("--registry", required=True)
    p_mirrors.add_argument("--mirror", action="append", default=[])
    p_mirrors.add_argument("--force", action="store_true")

    p_attest = subs.add_parser("attest", help="sign canonical registry metadata")
    p_attest.add_argument("--registry", required=True)
    p_attest.add_argument("--private-key", required=True)
    p_attest.add_argument("--trust-roots")
    p_attest.add_argument("--revocations")
    p_attest.add_argument("--mirrors")
    p_attest.add_argument("--output")

    p_verify_registry = subs.add_parser("verify-registry", help="verify a registry attestation")
    p_verify_registry.add_argument("registry_dir")
    p_verify_registry.add_argument("--trust-roots")
    p_verify_registry.add_argument("--revocations")
    p_verify_registry.add_argument("--mirrors")
    p_verify_registry.add_argument("--verify-packs", action="store_true",
                                   help="also verify every pack listed in the index")

    args = parser.parse_args(argv)
    try:
        if getattr(args, "removed_hmac_key", False):
            raise PackToolError(
                "HMAC --key was removed; use --private-key for signing or --trust-roots for verification"
            )
        if args.command == "verify":
            _print_verification(verify_pack(
                args.pack_dir, args.trust_roots, args.revocations, args.require_trusted,
            ))
        elif args.command == "sign" and not args.private_key:
            raise PackToolError("--private-key is required for Ed25519 signing")
        elif args.command == "sign":
            key_path = Path(args.private_key)
            if key_path.stat().st_mode & 0o077:
                print("packtool: warning: private key file is group/world readable", file=sys.stderr)
            path = sign_pack(args.pack_dir, key_path)
            print(f"signed: {path}")
        elif args.command == "publish":
            entry = publish_pack(args.pack_dir, args.registry, args.trust_roots, args.revocations)
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
            target = install_pack(args.lang, args.registry, args.into, args.trust_roots,
                                  args.revocations, args.require_trusted)
            print(f"installed: {target}")
        elif args.command == "keygen":
            _cmd_keygen(args)
        elif args.command == "trust-roots":
            _cmd_trust_roots(args)
        elif args.command == "revocations":
            _cmd_revocations(args)
        elif args.command == "mirrors":
            target = Path(args.registry) / MIRRORS_FILE
            if target.exists() and not args.force:
                raise PackToolError(f"refusing to overwrite {target}; pass --force")
            write_mirrors(target, args.mirror)
            print(f"mirrors: {target}")
        elif args.command == "attest":
            path = attest_registry(
                args.registry, Path(args.private_key), args.trust_roots,
                args.revocations, args.mirrors, args.output,
            )
            print(f"attested: {path}")
        elif args.command == "verify-registry":
            report = verify_registry(
                args.registry_dir, args.trust_roots, args.revocations,
                args.mirrors, args.verify_packs,
            )
            print(f"registry: {report['registry']}")
            print(f"packs: {report['packs']}  trust roots: {report['trust_roots']}  "
                  f"revoked: {report['revoked']}  mirrors: {report['mirrors']}")
            if report["verified_packs"]:
                print(f"packs verified: {report['verified_packs']}")
            print(f"attestation: verified (key-id {report['key_id']})")
            print("OK")
    except (PackToolError, OSError, ValueError) as exc:
        print(f"packtool: error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
