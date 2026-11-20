#!/usr/bin/env python3
"""Generate the deterministic release manifest for a candidate build.

The manifest is generated BEFORE SHA256SUMS so that the manifest itself is
covered by the checksum file. It is derived only from files that exist on disk,
fails on unexpected files or target drift, validates the CycloneDX SBOM, and
never synthesizes signature data.
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
TARGET_RE = re.compile(r"[A-Za-z0-9_-]+")
EXCLUDED_SUFFIXES = ('.sig', '.bundle', '.intoto.jsonl', '.sha256')
EXCLUDED_NAMES = {'SHA256SUMS', 'SHA256SUMS.sig', 'SHA256SUMS.bundle'}


def archive_target(name: str, version: str) -> str | None:
    """Extract a target triple using the known version as an unambiguous prefix."""
    prefix = f'meld-lang-{version}-'
    if not name.startswith(prefix):
        return None
    stem = name[len(prefix):]
    if stem.endswith('.tar.gz'):
        target = stem[:-len('.tar.gz')]
    elif stem.endswith('.zip'):
        target = stem[:-len('.zip')]
    else:
        return None
    return target if TARGET_RE.fullmatch(target) else None


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def is_excluded(name: str) -> bool:
    return name in EXCLUDED_NAMES or name.endswith(EXCLUDED_SUFFIXES)


def kind_of(name: str) -> str:
    if name.endswith('.tar.gz') or name.endswith('.zip'):
        return 'platform-archive'
    if name.endswith('.cdx.json') or 'sbom' in name:
        return 'sbom'
    return 'metadata'


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--artifacts-dir', type=Path, required=True)
    parser.add_argument('--sbom', type=Path, required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--source-date-epoch', type=int, required=True)
    parser.add_argument('--expected-target', action='append', default=[], dest='expected_targets')
    parser.add_argument('--repository', default='enis1enis2/meld')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()

    root = args.artifacts_dir.resolve()
    out = args.out.resolve()
    if not root.is_dir():
        raise SystemExit(f'artifacts directory does not exist: {root}')
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", args.version):
        raise SystemExit(f'version is not SemVer 2.0.0: {args.version}')
    if args.source_date_epoch <= 0:
        raise SystemExit('source-date-epoch must be positive')
    if not args.expected_targets:
        raise SystemExit('at least one --expected-target is required')
    if len(set(args.expected_targets)) != len(args.expected_targets):
        raise SystemExit('duplicate --expected-target values')
    if not args.sbom.is_file():
        raise SystemExit(f'missing SBOM: {args.sbom}')
    if not args.sbom.resolve().is_relative_to(root):
        raise SystemExit('SBOM must be inside --artifacts-dir')
    try:
        sbom = json.loads(args.sbom.read_text(encoding='utf-8'))
    except json.JSONDecodeError as exc:
        raise SystemExit(f'SBOM is not valid JSON: {exc}') from exc
    if sbom.get('bomFormat') != 'CycloneDX' or not str(sbom.get('specVersion', '')).startswith('1.'):
        raise SystemExit('SBOM is not a CycloneDX 1.x document')

    sbom_name = args.sbom.resolve().relative_to(root).as_posix()
    candidates = []
    for path in sorted((p for p in root.rglob('*') if p.is_file()), key=lambda p: p.relative_to(root).as_posix()):
        name = path.relative_to(root).as_posix()
        if path.resolve() == out or is_excluded(name):
            continue
        candidates.append(name)

    archive_targets: list[str] = []
    archive_names: set[str] = set()
    for name in candidates:
        target = archive_target(name, args.version)
        if target is None:
            if name.endswith(('.tar.gz', '.zip')) and name.startswith('meld-lang-'):
                raise SystemExit(f'archive version does not match manifest version: {name}')
            continue
        archive_names.add(name)
        archive_targets.append(target)
    if not archive_names:
        raise SystemExit('no platform archives found in --artifacts-dir')
    if len(archive_targets) != len(set(archive_targets)):
        raise SystemExit('duplicate platform archive targets')
    if set(archive_targets) != set(args.expected_targets):
        raise SystemExit(
            'archive target set mismatch: expected '
            f'{sorted(args.expected_targets)}, got {sorted(set(archive_targets))}'
        )
    if sbom_name not in candidates:
        raise SystemExit(f'SBOM {sbom_name} is not in --artifacts-dir')

    unexpected = sorted(set(candidates) - archive_names - {sbom_name})
    if unexpected:
        raise SystemExit(f'unexpected release inputs: {unexpected}')

    artifacts = []
    for name in sorted(archive_names | {sbom_name}):
        path = root / name
        artifacts.append({
            'name': name,
            'sha256': sha256(path),
            'size': path.stat().st_size,
            'kind': kind_of(name),
        })

    manifest = {
        'schema': SCHEMA,
        'product': 'meld-lang',
        'binaries': ['meld', 'meld-lsp', 'meld-pack'],
        'version': args.version,
        'semver_spec': SEMVER_SPEC,
        'git_commit': args.commit,
        'source_date_epoch': args.source_date_epoch,
        'repository': args.repository,
        'build': {
            'cargo_flags': ['build', '--release', '--locked'],
            'reproducible': True,
            'note': 'Byte-identical archives require the same toolchain, target and SOURCE_DATE_EPOCH; CI enforces a two-run comparison.',
        },
        'sbom': {
            'format': sbom.get('specVersion'),
            'serial_number': sbom.get('serialNumber'),
            'file': sbom_name,
        },
        'checksums': {
            'file': 'SHA256SUMS',
            'generated_after_manifest': True,
            'includes_manifest': True,
        },
        'signature_policy': {
            'scheme': 'cosign-keyless-plus-github-build-provenance',
            'signed_files': 'SHA256SUMS, this manifest, the SBOM and every platform archive',
            'expected_files_per_artifact': ['.sig', '.bundle'],
            'embedded_signatures': False,
            'note': 'Signatures are verified from files on disk; no signature list is embedded to avoid staleness.',
        },
        'artifacts': artifacts,
        'verification': {
            'checksums': 'sha256sum -c --strict SHA256SUMS',
            'signatures': 'scripts/verify_artifacts.sh --dir <release-dir> --tag <tag> --issuer <issuer> [--attestation <bundle>]',
            'install': 'scripts/verify_install.sh --root <install-prefix>',
        },
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + '\n', encoding='utf-8')
    print(f'generate_release_manifest.py: wrote {out} ({len(artifacts)} artifacts)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
