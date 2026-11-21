#!/bin/sh
# Generate a CycloneDX SBOM with the real cargo-sbom tool and normalize its
# volatile metadata (UUID/timestamp/tool list) against SOURCE_DATE_EPOCH.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/generate_sbom.sh [--version VERSION] [--out FILE] [--format FORMAT]

FORMAT is a cargo-sbom output format (default: cyclone_dx_json_1_6).
Requires `cargo sbom` (cargo-sbom 0.10.0) on PATH.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=${MELD_VERSION:-}
OUT=$ROOT/dist/release/sbom.cdx.json
FORMAT=cyclone_dx_json_1_6

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --out) OUT=${2:?--out needs a value}; shift 2 ;;
        --format) FORMAT=${2:?--format needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'generate_sbom.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$VERSION" ] || { printf 'generate_sbom.sh: set --version or MELD_VERSION\n' >&2; exit 2; }

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf '0')
fi
case "$SOURCE_DATE_EPOCH" in
    ''|*[!0-9]*) printf 'generate_sbom.sh: SOURCE_DATE_EPOCH must be an integer\n' >&2; exit 2 ;;
esac
[ "$SOURCE_DATE_EPOCH" -gt 0 ] || { printf 'generate_sbom.sh: SOURCE_DATE_EPOCH must be positive\n' >&2; exit 2; }
export SOURCE_DATE_EPOCH

command -v cargo >/dev/null 2>&1 || { printf 'generate_sbom.sh: cargo not found\n' >&2; exit 1; }
cargo sbom --help >/dev/null 2>&1 || {
    printf 'generate_sbom.sh: cargo-sbom is not installed (cargo install cargo-sbom --version 0.10.0 --locked)\n' >&2
    exit 1
}

COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')
RAW=$(mktemp "${TMPDIR:-/tmp}/meld-sbom.XXXXXX")
trap 'rm -f "$RAW"' EXIT HUP INT TERM
# cargo-sbom 0.10.0 has no --locked flag; CI pins resolution with
# `cargo fetch --locked` and the committed Cargo.lock before calling this script.
cargo sbom --project-directory "$ROOT" --output-format "$FORMAT" > "$RAW"

mkdir -p "$(dirname "$OUT")"
python3 - "$RAW" "$OUT" "$SOURCE_DATE_EPOCH" "$VERSION" "$COMMIT" <<'PY'
import datetime, json, pathlib, sys, uuid
raw, out, epoch, version, commit = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]
doc = json.loads(raw.read_text(encoding='utf-8'))
if doc.get('bomFormat') != 'CycloneDX' or not str(doc.get('specVersion', '')).startswith('1.'):
    raise SystemExit('generate_sbom.sh: cargo-sbom did not produce a CycloneDX document')
stamp = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
namespace = uuid.UUID('6ba7b810-9dad-11d1-80b4-00c04fd430c8')
doc['serialNumber'] = 'urn:uuid:' + str(uuid.uuid5(namespace, f'meld-lang/{version}/{commit}/{epoch}'))
meta = doc.setdefault('metadata', {})
meta['timestamp'] = stamp
meta['tools'] = [{'vendor': 'Meld', 'name': 'cargo-sbom', 'version': '0.10.0'}]
props = meta.setdefault('properties', [])
props[:] = [p for p in props if not str(p.get('name', '')).startswith('meld:')]
props.extend([
    {'name': 'meld:release_version', 'value': version},
    {'name': 'meld:source_date_epoch', 'value': str(epoch)},
    {'name': 'meld:commit', 'value': commit},
])
props.sort(key=lambda p: p['name'])
out.write_text(json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=False) + '\n', encoding='utf-8')
print(f'generate_sbom.sh: wrote {out} ({len(doc.get("components", []))} component(s))')
PY
