#!/usr/bin/env python3
"""Generate the deterministic, signed release manifest for a candidate build.

The manifest is derived only from files that exist on disk. It fails if any
checksum in SHA256SUMS is missing or wrong, if the SBOM is not JSON, or if no
platform archive is present. No signature is ever synthesized here.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

SCHEMA = "meld-release-manifest/1"
SEMVER_SPEC = "2.0.0"
ARCHIVE_RE = re.compile(r"^meld-lang-[0-9A-Za-z.+-]+-[A-Za-z0-9_-]+\.(?:tar\.gz|zip)$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_checksums(path: Path, root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise SystemExit(f"{path}:{lineno}: malformed SHA256SUMS line")
        digest, name = match.groups()
        if name in result:
            raise SystemExit(f"{path}:{lineno}: duplicate entry {name}")
        artifact = root / name
        if not artifact.is_file():
            raise SystemExit(f"{path}:{lineno}: missing artifact {name}")
        actual = sha256(artifact)
        if actual != digest:
            raise SystemExit(f"{path}:{lineno}: checksum mismatch for {name}: {actual} != {digest}")
        result[name] = digest
    if not result:
        raise SystemExit(f"{path}: empty checksum file")
    return result


def kind_of(name: str) -> str:
    if name.endswith(".tar.gz") or name.endswith(".zip"):
        return "platform-archive"
    if name.endswith(".cdx.json") or "sbom" in name:
        return "sbom"
    if name == "SHA256SUMS":
        return "checksums"
    return "metadata"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--checksums", type=Path, required=True)
    parser.add_argument("--sbom", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--source-date-epoch", type=int, required=True)
    parser.add_argument("--repository", default="enis1enis2/meld")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    root = args.artifacts_dir.resolve()
    if not root.is_dir():
        raise SystemExit(f"artifacts directory does not exist: {root}")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", args.version):
        raise SystemExit(f"version is not SemVer 2.0.0: {args.version}")
    if args.source_date_epoch <= 0:
        raise SystemExit("source-date-epoch must be positive")
    if not args.checksums.is_file():
        raise SystemExit(f"missing checksums: {args.checksums}")
    if not args.sbom.is_file():
        raise SystemExit(f"missing SBOM: {args.sbom}")
    try:
        sbom = json.loads(args.sbom.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise SystemExit(f"SBOM is not valid JSON: {exc}") from exc
    if sbom.get("bomFormat") != "CycloneDX":
        raise SystemExit("SBOM is not a CycloneDX document")

    checksums = read_checksums(args.checksums, root)
    archives = [name for name in checksums if ARCHIVE_RE.match(name)]
    if not archives:
        raise SystemExit("no platform archives listed in SHA256SUMS")
    sbom_name = args.sbom.resolve().relative_to(root).as_posix() if args.sbom.is_relative_to(root) else args.sbom.name
    if sbom_name not in checksums:
        raise SystemExit(f"SBOM {sbom_name} is not covered by SHA256SUMS")

    artifacts = []
    for name in sorted(checksums):
        path = root / name
        artifacts.append({
            "name": name,
            "sha256": checksums[name],
            "size": path.stat().st_size,
            "kind": kind_of(name),
        })
    signatures = sorted(p.relative_to(root).as_posix() for p in root.rglob("*")
                        if p.is_file() and p.name.endswith((".sig", ".bundle", ".intoto.jsonl")))

    manifest = {
        "schema": SCHEMA,
        "product": "meld-lang",
        "binaries": ["meld", "meld-lsp", "meld-pack"],
        "version": args.version,
        "semver_spec": SEMVER_SPEC,
        "git_commit": args.commit,
        "source_date_epoch": args.source_date_epoch,
        "repository": args.repository,
        "build": {
            "cargo_flags": ["build", "--release", "--locked"],
            "reproducible": True,
            "note": "Byte-identical archives require the same toolchain, target and SOURCE_DATE_EPOCH; CI enforces a two-run comparison.",
        },
        "sbom": {
            "format": sbom.get("specVersion"),
            "serial_number": sbom.get("serialNumber"),
            "file": sbom_name,
        },
        "artifacts": artifacts,
        "signatures": signatures,
        "verification": {
            "checksums": "sha256sum -c --strict SHA256SUMS",
            "signatures": "scripts/verify_artifacts.sh --dir <release-dir> --identity-regexp <regexp> --issuer <issuer>",
            "install": "scripts/verify_install.sh --root <install-prefix>",
        },
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"generate_release_manifest.py: wrote {args.out} ({len(artifacts)} artifacts)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
