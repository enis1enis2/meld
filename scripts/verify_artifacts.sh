#!/bin/sh
# Verify a downloaded release: checksums, manifest consistency, archive payload
# and (unless --offline) Sigstore cosign signatures.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/verify_artifacts.sh --dir DIR [--identity-regexp REGEXP] [--issuer URL] [--offline]

--offline checks integrity only; it never claims signature verification.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIR=
IDENTITY='https://github.com/enis1enis2/meld/.github/workflows/release.yml@refs/tags/.*'
ISSUER='https://token.actions.githubusercontent.com'
OFFLINE=

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR=${2:?--dir needs a value}; shift 2 ;;
        --identity-regexp) IDENTITY=${2:?--identity-regexp needs a value}; shift 2 ;;
        --issuer) ISSUER=${2:?--issuer needs a value}; shift 2 ;;
        --offline) OFFLINE=yes; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'verify_artifacts.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$DIR" ] || { usage >&2; exit 2; }
[ -d "$DIR" ] || { printf 'verify_artifacts.sh: missing directory %s\n' "$DIR" >&2; exit 1; }

python3 - "$DIR" <<'PY'
import json, pathlib, sys, tarfile, zipfile
root = pathlib.Path(sys.argv[1])
sums = root / 'SHA256SUMS'
if not sums.is_file():
    raise SystemExit('verify_artifacts.sh: SHA256SUMS is missing')
entries = {}
for line in sums.read_text(encoding='utf-8').splitlines():
    if not line.strip():
        continue
    digest, name = line.split('  ', 1)
    path = root / name
    if not path.is_file():
        raise SystemExit(f'verify_artifacts.sh: missing artifact {name}')
    import hashlib
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != digest:
        raise SystemExit(f'verify_artifacts.sh: checksum mismatch for {name}')
    entries[name] = digest
print(f'verify_artifacts.sh: PASS {len(entries)} checksum(s)')

manifests = sorted(root.glob('*release-manifest*.json'))
if len(manifests) != 1:
    raise SystemExit(f'verify_artifacts.sh: expected one release manifest, found {len(manifests)}')
manifest = json.loads(manifests[0].read_text(encoding='utf-8'))
if manifest.get('schema') != 'meld-release-manifest/1' or manifest.get('semver_spec') != '2.0.0':
    raise SystemExit('verify_artifacts.sh: unsupported manifest schema or SemVer policy')
for artifact in manifest['artifacts']:
    if entries.get(artifact['name']) != artifact['sha256']:
        raise SystemExit(f'verify_artifacts.sh: manifest/checksum mismatch for {artifact["name"]}')
print('verify_artifacts.sh: PASS manifest schema, SemVer 2.0.0 and artifact digests')

archives = [root / name for name in entries if name.endswith(('.tar.gz', '.zip'))]
if not archives:
    raise SystemExit('verify_artifacts.sh: no platform archives in SHA256SUMS')
common = ('packs/python/pack.toml', 'packs/bash/pack.toml',
          'share/doc/meld-lang/LICENSE-APACHE',
          'share/doc/meld-lang/LICENSE-MIT',
          'share/doc/meld-lang/THIRD-PARTY-NOTICES.md',
          'share/doc/meld-lang/CHANGELOG.md')
for archive in archives:
    ext = '.exe' if archive.name.endswith('.zip') else ''
    required = tuple(f'bin/{name}{ext}' for name in ('meld', 'meld-lsp', 'meld-pack')) + common
    names = set()
    if archive.name.endswith('.tar.gz'):
        with tarfile.open(archive, 'r:gz') as tf:
            names = {n.split('/', 1)[1] for n in tf.getnames() if '/' in n}
    else:
        with zipfile.ZipFile(archive) as zf:
            names = {n.split('/', 1)[1] for n in zf.namelist() if '/' in n}
    missing = [item for item in required if item not in names]
    if missing:
        raise SystemExit(f'verify_artifacts.sh: {archive.name} is missing {missing}')
print(f'verify_artifacts.sh: PASS archive payload ({len(archives)} archive(s))')
PY

if [ -n "$OFFLINE" ]; then
    printf 'verify_artifacts.sh: OFFLINE - signatures were NOT verified\n'
    exit 0
fi
command -v cosign >/dev/null 2>&1 || {
    printf 'verify_artifacts.sh: cosign not found; use --offline for integrity-only checks\n' >&2
    exit 1
}
count=0
find "$DIR" -maxdepth 1 -type f -name '*.bundle' -print | LC_ALL=C sort | while IFS= read -r bundle; do
    subject=${bundle%.bundle}
    [ -f "$subject" ] || { printf 'verify_artifacts.sh: missing subject for %s\n' "$bundle" >&2; exit 1; }
    printf 'verify_artifacts.sh: verifying signature for %s\n' "${subject##*/}"
    cosign verify-blob \
        --bundle "$bundle" \
        --certificate-identity-regexp "$IDENTITY" \
        --certificate-oidc-issuer "$ISSUER" \
        "$subject"
done
count=$(find "$DIR" -maxdepth 1 -type f -name '*.bundle' | wc -l | tr -d ' ')
[ "$count" -gt 0 ] || { printf 'verify_artifacts.sh: no signature bundles found\n' >&2; exit 1; }
printf 'verify_artifacts.sh: PASS %s Sigstore signature(s)\n' "$count"
