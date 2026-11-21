#!/bin/sh
# Assemble and archive one signed-release candidate payload.
#
# Archives contain the three binaries, all bundled Lang Pack data, licenses and
# docs. File order, ownership and timestamps are normalized before archiving,
# so two runs with the same SOURCE_DATE_EPOCH produce identical bytes.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/package_release.sh --version VERSION --target TRIPLE [--build-dir DIR] [--out DIR]

Options:
  --version VERSION     release version, e.g. 0.2.0-rc.2
  --target TRIPLE       target triple already built by build_matrix.sh
  --build-dir DIR       build root (default: dist/build)
  --out DIR             archive output directory (default: dist/packages)
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=
TARGET=
BUILD_DIR=$ROOT/dist/build
OUT=$ROOT/dist/packages

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION=${2:?--version needs a value}; shift 2 ;;
        --target) TARGET=${2:?--target needs a value}; shift 2 ;;
        --build-dir) BUILD_DIR=${2:?--build-dir needs a value}; shift 2 ;;
        --out) OUT=${2:?--out needs a value}; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'package_release.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$VERSION" ] && [ -n "$TARGET" ] || { usage >&2; exit 2; }
printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || {
    printf 'package_release.sh: version is not SemVer 2.0.0: %s\n' "$VERSION" >&2; exit 2;
}

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    SOURCE_DATE_EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf '0')
fi
case "$SOURCE_DATE_EPOCH" in
    ''|*[!0-9]*) printf 'package_release.sh: SOURCE_DATE_EPOCH must be an integer\n' >&2; exit 2 ;;
esac
[ "$SOURCE_DATE_EPOCH" -gt 0 ] || { printf 'package_release.sh: SOURCE_DATE_EPOCH must be positive\n' >&2; exit 2; }
export SOURCE_DATE_EPOCH

BUILT="$BUILD_DIR/$TARGET"
[ -d "$BUILT/bin" ] || { printf 'package_release.sh: missing build directory %s\n' "$BUILT/bin" >&2; exit 1; }
case "$TARGET" in *windows*) ext=.exe ;; *) ext= ;; esac
for bin in meld meld-lsp meld-pack; do
    [ -f "$BUILT/bin/$bin$ext" ] || { printf 'package_release.sh: missing binary %s\n' "$bin$ext" >&2; exit 1; }
done

for required in README.md CHANGELOG.md RELEASE.md SBOM.md SECURITY.md \
    THIRD-PARTY-NOTICES.md LICENSE-APACHE LICENSE-MIT NOTICE docs/schema-stability.md \
    docs/release-artifacts.md packs/registry/index.json; do
    [ -f "$ROOT/$required" ] || { printf 'package_release.sh: missing required payload %s\n' "$required" >&2; exit 1; }
done

mkdir -p "$OUT"
PACKAGE="meld-lang-$VERSION-$TARGET"
STAGE="$OUT/.stage-$PACKAGE-$$"
trap 'rm -rf "$STAGE"' EXIT HUP INT TERM
mkdir -p "$STAGE/$PACKAGE/bin" \
    "$STAGE/$PACKAGE/packs" \
    "$STAGE/$PACKAGE/share/doc/meld-lang" \
    "$STAGE/$PACKAGE/share/meld-lang/examples"

for bin in meld meld-lsp meld-pack; do
    cp "$BUILT/bin/$bin$ext" "$STAGE/$PACKAGE/bin/$bin$ext"
done
cp -R "$ROOT/packs/." "$STAGE/$PACKAGE/packs/"
cp -R "$ROOT/examples/." "$STAGE/$PACKAGE/share/meld-lang/examples/"
for doc in README.md CHANGELOG.md RELEASE.md SBOM.md SECURITY.md THIRD-PARTY-NOTICES.md \
    LICENSE-APACHE LICENSE-MIT NOTICE docs/schema-stability.md docs/release-artifacts.md; do
    mkdir -p "$STAGE/$PACKAGE/share/doc/meld-lang/$(dirname "$doc")"
    cp "$ROOT/$doc" "$STAGE/$PACKAGE/share/doc/meld-lang/$doc"
done
cp "$BUILT/BUILD_INFO" "$STAGE/$PACKAGE/share/doc/meld-lang/BUILD_INFO.txt"

# Portable metadata normalization: fixed mtimes for every file and directory.
# Windows does not implement utime(follow_symlinks=False), so use it only where
# the platform supports it; the staged tree contains no symlinks to preserve.
python3 - "$STAGE" "$SOURCE_DATE_EPOCH" <<'PY'
import os, pathlib, sys
root = pathlib.Path(sys.argv[1])
epoch = int(sys.argv[2])
follow = {}
if os.utime in os.supports_follow_symlinks:
    follow["follow_symlinks"] = False
for path in sorted(root.rglob('*'), key=lambda p: str(p).encode()):
    os.utime(path, (epoch, epoch), **follow)
os.utime(root, (epoch, epoch))
PY

case "$TARGET" in
    *windows*) archive="$OUT/$PACKAGE.zip"; format=zip ;;
    *) archive="$OUT/$PACKAGE.tar.gz"; format=tar.gz ;;
esac

# Architecture-independent, byte-stable archive writer: sorted entries, fixed
# uid/gid/uname/gname, gzip header without a timestamp or source filename.
python3 - "$STAGE" "$PACKAGE" "$archive" "$format" "$SOURCE_DATE_EPOCH" <<'PY'
import gzip, io, pathlib, sys, tarfile, zipfile
stage, package, archive, fmt, epoch = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
base = stage / package
entries = sorted((p for p in base.rglob('*')), key=lambda p: str(p.relative_to(base)).encode())
if fmt == 'tar.gz':
    with open(archive, 'wb') as raw:
        with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as gz:
            with tarfile.open(fileobj=gz, mode='w', format=tarfile.GNU_FORMAT) as tf:
                for path in entries:
                    ti = tf.gettarinfo(str(path), arcname=str(path.relative_to(base)))
                    ti.uid = ti.gid = 0
                    ti.uname = ti.gname = ''
                    ti.mtime = epoch
                    if path.is_symlink():
                        ti.type = tarfile.SYMTYPE
                        ti.linkname = path.readlink().as_posix()
                        tf.addfile(ti)
                    elif path.is_dir():
                        ti.type = tarfile.DIRTYPE
                        ti.size = 0
                        tf.addfile(ti)
                    elif path.is_file():
                        with path.open('rb') as fh:
                            tf.addfile(ti, fh)
                    else:
                        raise SystemExit(f'unsupported file type: {path}')
else:
    # ZIP stores DOS time; clamp to the ZIP epoch and use UTC from SOURCE_DATE_EPOCH.
    import datetime
    dt = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc)
    date_time = (
        max(dt.year, 1980), dt.month, dt.day, dt.hour, dt.minute, dt.second
    )
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for path in entries:
            if not path.is_file():
                continue
            name = f'{package}/{path.relative_to(base).as_posix()}'
            zi = zipfile.ZipInfo(name, date_time=date_time)
            zi.create_system = 0
            zi.external_attr = 0o644 << 16
            zi.compress_type = zipfile.ZIP_DEFLATED
            with path.open('rb') as fh:
                zf.writestr(zi, fh.read())
PY

printf 'package_release.sh: wrote %s\n' "$archive"
