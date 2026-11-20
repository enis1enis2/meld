#!/bin/sh
# Produce a sorted, GNU-coreutils-compatible SHA256SUMS for release artifacts.
# The release manifest must be generated first and is included. Signatures,
# bundles, attestations and SHA256SUMS itself are excluded.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/generate_checksums.sh --dir DIR --out SHA256SUMS
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIR=
OUT=

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR=${2:?--dir needs a value}; shift 2 ;;
        --out) OUT=${2:?--out needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'generate_checksums.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$DIR" ] && [ -n "$OUT" ] || { usage >&2; exit 2; }
[ -d "$DIR" ] || { printf 'generate_checksums.sh: missing directory %s\n' "$DIR" >&2; exit 1; }

mkdir -p "$(dirname "$OUT")"
python3 - "$DIR" "$OUT" <<'PY'
import hashlib, pathlib, sys
root, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
excluded_suffixes = ('.sig', '.bundle', '.intoto.jsonl', '.sha256')
excluded_names = {'SHA256SUMS', 'SHA256SUMS.sig', 'SHA256SUMS.bundle'}

manifests = sorted(p for p in root.glob('*release-manifest*.json') if p.is_file())
if len(manifests) != 1:
    raise SystemExit('generate_checksums.sh: generate exactly one release manifest before SHA256SUMS')

lines = []
for path in sorted((p for p in root.rglob('*') if p.is_file()), key=lambda p: p.relative_to(root).as_posix()):
    name = path.relative_to(root).as_posix()
    if name in excluded_names or name.endswith(excluded_suffixes):
        continue
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    lines.append(f'{digest}  {name}')
if not lines:
    raise SystemExit('generate_checksums.sh: no artifacts to checksum')
if manifests[0].relative_to(root).as_posix() not in {line.split('  ', 1)[1] for line in lines}:
    raise SystemExit('generate_checksums.sh: release manifest was excluded from SHA256SUMS')
out.write_text('\n'.join(lines) + '\n', encoding='utf-8')
print(f'generate_checksums.sh: wrote {len(lines)} checksum(s) to {out}')
PY
