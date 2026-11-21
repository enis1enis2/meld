#!/bin/sh
# Verify a downloaded release: strict checksums, manifest consistency, expected
# targets, archive payload, one cosign signature+bundle per signed artifact with
# matching signature bytes, and (optionally or mandatorily) the GitHub
# build-provenance attestation. --offline is integrity-only and never claims
# signature or attestation trust.
set -eu

usage() {
    cat <<'EOF'
usage: scripts/verify_artifacts.sh --dir DIR [--tag TAG] [--repo OWNER/REPO]
                                     [--identity-regexp REGEXP] [--issuer URL]
                                     [--attestation FILE] [--require-attestation]
                                     [--expected-target TARGET]... [--offline]

Online verification requires --tag; the default identity is anchored to the
exact release workflow tag. --identity-regexp overrides it but must be anchored
with ^ and $. --require-attestation resolves the single --attestation file or
the sole *.intoto.jsonl in --dir and fails when none is present.

--offline checks integrity, signature-file shape and byte equality only; it
never claims cosign or attestation trust and rejects --require-attestation.
EOF
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DIR=
TAG=
REPO='enis1enis2/meld'
IDENTITY=
ISSUER='https://token.actions.githubusercontent.com'
ATTESTATION=
OFFLINE=
REQUIRE_ATTESTATION=
EXPECTED_TARGETS=

escape_regex() {
    printf '%s' "$1" | sed -e 's/[][\.^$*+?(){}|]/\\&/g'
}

identity_from_tag() {
    printf '^https://github\\.com/%s/\\.github/workflows/release\\.yml@refs/tags/%s$' \
        "$(escape_regex "$REPO")" "$(escape_regex "$TAG")"
}

fail() {
    printf 'verify_artifacts.sh: %s\n' "$1" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --dir) DIR=${2:?--dir needs a value}; shift 2 ;;
        --tag) TAG=${2:?--tag needs a value}; shift 2 ;;
        --repo) REPO=${2:?--repo needs a value}; shift 2 ;;
        --identity-regexp) IDENTITY=${2:?--identity-regexp needs a value}; shift 2 ;;
        --issuer) ISSUER=${2:?--issuer needs a value}; shift 2 ;;
        --attestation) ATTESTATION=${2:?--attestation needs a value}; shift 2 ;;
        --require-attestation) REQUIRE_ATTESTATION=yes; shift ;;
        --expected-target)
            target=${2:?--expected-target needs a value}
            if [ -n "$EXPECTED_TARGETS" ]; then
                EXPECTED_TARGETS="$EXPECTED_TARGETS
$target"
            else
                EXPECTED_TARGETS=$target
            fi
            shift 2
            ;;
        --offline) OFFLINE=yes; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'verify_artifacts.sh: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ -n "$DIR" ] || { usage >&2; exit 2; }
[ -d "$DIR" ] || fail "missing directory $DIR"

if [ -n "$OFFLINE" ]; then
    [ -z "$REQUIRE_ATTESTATION" ] || fail '--require-attestation cannot be combined with --offline'
else
    [ -n "$TAG" ] || fail 'online verification requires --tag (use --offline for integrity-only checks)'
    if [ -z "$IDENTITY" ]; then
        IDENTITY=$(identity_from_tag)
    fi
    printf '%s\n' "$IDENTITY" | grep -Eq '^\^.*\$$' || \
        fail 'identity regexp must be anchored with ^ and $'
    if [ -n "$REQUIRE_ATTESTATION" ] && [ -z "$ATTESTATION" ]; then
        attestation_count=$(find "$DIR" -maxdepth 1 -type f -name '*.intoto.jsonl' | wc -l | tr -d ' ')
        [ "$attestation_count" -eq 1 ] || \
            fail "--require-attestation needs --attestation or exactly one *.intoto.jsonl in $DIR (found $attestation_count)"
        ATTESTATION=$(find "$DIR" -maxdepth 1 -type f -name '*.intoto.jsonl')
    fi
fi

REPORT=$(mktemp "${TMPDIR:-/tmp}/meld-verify-report.XXXXXX")
SUBJECTS=$(mktemp "${TMPDIR:-/tmp}/meld-verify-subjects.XXXXXX")
trap 'rm -f "$REPORT" "$SUBJECTS"' EXIT HUP INT TERM

python3 - "$DIR" "$TAG" "$ATTESTATION" "$EXPECTED_TARGETS" > "$REPORT" <<'PY'
import base64, binascii, fnmatch, hashlib, json, pathlib, re, sys, tarfile, zipfile

root = pathlib.Path(sys.argv[1])
tag = sys.argv[2]
attestation_arg = sys.argv[3]
expected_arg = sys.argv[4]
SCHEMA = 'meld-release-manifest/1'
TARGET_RE = re.compile(r'[A-Za-z0-9_-]+')
EXCLUDED_SUFFIXES = ('.sig', '.bundle', '.intoto.jsonl', '.sha256')
EXCLUDED_NAMES = {'SHA256SUMS', 'SHA256SUMS.sig', 'SHA256SUMS.bundle'}
expected_targets = sorted({line for line in expected_arg.splitlines() if line})


def archive_target(name, version):
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


def fail(message):
    raise SystemExit(f'verify_artifacts.sh: {message}')


def info(message):
    print(f'INFO\t{message}')


def subject(name):
    print(f'SUBJECT\t{name}')


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def excluded(name):
    return name in EXCLUDED_NAMES or name.endswith(EXCLUDED_SUFFIXES)


def signed_kind(name):
    if name == 'SHA256SUMS':
        return 'checksums'
    if fnmatch.fnmatch(name, '*release-manifest*.json'):
        return 'manifest'
    if fnmatch.fnmatch(name, '*.cdx.json'):
        return 'sbom'
    if name.endswith(('.tar.gz', '.zip')):
        return 'archive'
    return None


def validate_entry_name(name, where):
    if not name or '\\' in name or name.startswith('/'):
        fail(f'{where}: unsafe archive entry {name!r}')
    parts = pathlib.PurePosixPath(name).parts
    if any(part in ('', '.', '..') for part in parts):
        fail(f'{where}: unsafe archive entry {name!r}')


def signature_bytes(path, where):
    """Read a cosign .sig file as either raw bytes or its base64 text form."""
    raw = path.read_bytes()
    try:
        decoded = base64.b64decode(raw.strip(), validate=True)
    except (binascii.Error, ValueError):
        decoded = None
    if decoded:
        return decoded
    return raw


def bundle_signature(path, where):
    try:
        bundle = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f'{where}: unreadable signature bundle: {exc}')
    sigs = []
    envelope = bundle.get('dsseEnvelope') if isinstance(bundle, dict) else None
    if isinstance(envelope, dict):
        sigs = [item.get('sig') for item in envelope.get('signatures', []) if isinstance(item, dict)]
    if not sigs:
        envelopes = bundle.get('dsseEnvelopes') if isinstance(bundle, dict) else None
        if isinstance(envelopes, list) and envelopes and isinstance(envelopes[0], dict):
            sigs = [item.get('sig') for item in envelopes[0].get('signatures', []) if isinstance(item, dict)]
    if not sigs:
        message = bundle.get('messageSignature') if isinstance(bundle, dict) else None
        if isinstance(message, dict) and message.get('signature'):
            sigs = [message['signature']]
    if not sigs:
        # cosign sign-blob also emits the legacy bundle shape used by Keyless
        # signing: {"base64Signature": ..., "cert": ..., "rekorBundle": ...}.
        legacy = bundle.get('base64Signature') if isinstance(bundle, dict) else None
        if isinstance(legacy, str) and legacy:
            sigs = [legacy]
    if len(sigs) != 1 or not isinstance(sigs[0], str):
        fail(f'{where}: expected exactly one signature in bundle')
    try:
        return base64.b64decode(sigs[0], validate=True)
    except (binascii.Error, ValueError) as exc:
        fail(f'{where}: invalid base64 signature in bundle: {exc}')


sums = root / 'SHA256SUMS'
if not sums.is_file():
    fail('SHA256SUMS is missing')

entries = {}
for lineno, line in enumerate(sums.read_text(encoding='utf-8').splitlines(), 1):
    if not line.strip():
        fail(f'SHA256SUMS:{lineno}: blank lines are not allowed')
    match = re.fullmatch(r'([0-9a-f]{64})  (.+)', line)
    if not match:
        fail(f'SHA256SUMS:{lineno}: malformed line')
    digest, name = match.groups()
    if name in entries:
        fail(f'SHA256SUMS:{lineno}: duplicate entry {name}')
    parts = pathlib.PurePosixPath(name).parts
    if name.startswith('/') or any(part in ('', '.', '..') for part in parts):
        fail(f'SHA256SUMS:{lineno}: unsafe path {name}')
    path = root.joinpath(*parts)
    if path.is_symlink() or not path.is_file():
        fail(f'SHA256SUMS:{lineno}: missing or symlinked artifact {name}')
    actual = sha256(path)
    if actual != digest:
        fail(f'checksum mismatch for {name}: {actual} != {digest}')
    entries[name] = digest
info(f'PASS {len(entries)} checksum(s)')

actual_files = []
for path in root.rglob('*'):
    if not path.is_file():
        continue
    name = path.relative_to(root).as_posix()
    if not excluded(name):
        actual_files.append(name)
unlisted = sorted(set(actual_files) - set(entries))
missing = sorted(set(entries) - set(actual_files))
if unlisted or missing:
    fail(f'strict checksum coverage failed: unlisted={unlisted} missing={missing}')
info('PASS strict SHA256SUMS coverage')

# SHA256SUMS cannot list itself, so add it explicitly to the signed set.
signed = {name: signed_kind(name) for name in entries if signed_kind(name)}
signed['SHA256SUMS'] = 'checksums'
manifests = sorted(name for name, kind in signed.items() if kind == 'manifest')
sboms = sorted(name for name, kind in signed.items() if kind == 'sbom')
archives = sorted(name for name, kind in signed.items() if kind == 'archive')
if len(manifests) != 1:
    fail(f'expected one release manifest in SHA256SUMS, found {len(manifests)}')
if len(sboms) != 1:
    fail(f'expected one SBOM in SHA256SUMS, found {len(sboms)}')
if not archives:
    fail('no platform archives in SHA256SUMS')
for name in sorted(signed):
    sig = root / f'{name}.sig'
    bundle = root / f'{name}.bundle'
    if not sig.is_file() or sig.stat().st_size == 0:
        fail(f'missing or empty signature file {sig.name}')
    if not bundle.is_file() or bundle.stat().st_size == 0:
        fail(f'missing or empty signature bundle {bundle.name}')
    if signature_bytes(sig, sig.name) != bundle_signature(bundle, bundle.name):
        fail(f'signature bytes in {sig.name} do not match {bundle.name}')
for path in root.rglob('*'):
    if not path.is_file():
        continue
    name = path.relative_to(root).as_posix()
    for suffix in ('.sig', '.bundle'):
        if name.endswith(suffix) and name[:-len(suffix)] not in signed:
            fail(f'unexpected signature file {name}')
info(f'PASS one .sig and .bundle with equal signature bytes for {len(signed)} signed artifacts')

attestations = sorted(p.name for p in root.glob('*.intoto.jsonl') if p.is_file())
if len(attestations) > 1:
    fail(f'expected at most one attestation file, found {attestations}')
if attestation_arg:
    attestation_path = pathlib.Path(attestation_arg).resolve()
    if not attestation_path.is_file():
        fail(f'attestation file is missing: {attestation_arg}')
    if attestations and attestation_path != (root / attestations[0]).resolve():
        fail('multiple attestation candidates: explicit file and directory scan disagree')

manifest_name = manifests[0]
if manifest_name not in entries:
    fail(f'{manifest_name} is not covered by SHA256SUMS')
info(f'PASS independent manifest digest {entries[manifest_name]} from SHA256SUMS')

try:
    manifest = json.loads((root / manifest_name).read_text(encoding='utf-8'))
except json.JSONDecodeError as exc:
    fail(f'{manifest_name} is not valid JSON: {exc}')
if manifest.get('schema') != SCHEMA or manifest.get('semver_spec') != '2.0.0':
    fail('unsupported manifest schema or SemVer policy')
if 'signatures' in manifest:
    fail('stale manifest contains a signatures field; regenerate it')
if tag.startswith('v') and manifest.get('version') != tag[1:]:
    fail(f'manifest version {manifest.get("version")} does not match tag {tag}')
checksums_meta = manifest.get('checksums', {})
if checksums_meta.get('file') != 'SHA256SUMS' or checksums_meta.get('generated_after_manifest') is not True:
    fail('manifest does not declare the post-manifest SHA256SUMS contract')
if checksums_meta.get('includes_manifest') is not True:
    fail('manifest must declare that SHA256SUMS includes the manifest digest')
signature_policy = manifest.get('signature_policy', {})
if signature_policy.get('embedded_signatures') is not False:
    fail('manifest must not embed signature data')
if signature_policy.get('expected_files_per_artifact') != ['.sig', '.bundle']:
    fail('manifest signature policy must require .sig and .bundle files')

expected_artifacts = sorted(set(entries) - {'SHA256SUMS', manifest_name})
listed = manifest.get('artifacts')
if not isinstance(listed, list):
    fail('manifest artifacts is not a list')
by_name = {}
for item in listed:
    if not isinstance(item, dict) or not isinstance(item.get('name'), str):
        fail('malformed manifest artifact entry')
    name = item['name']
    if name in by_name:
        fail(f'duplicate manifest artifact {name}')
    by_name[name] = item
if set(by_name) != set(expected_artifacts):
    fail(f'manifest artifact set mismatch: {sorted(by_name)} != {expected_artifacts}')
for name in expected_artifacts:
    item = by_name[name]
    if item.get('sha256') != entries[name]:
        fail(f'manifest/checksum mismatch for {name}')
    if item.get('size') != (root / name).stat().st_size:
        fail(f'manifest size mismatch for {name}')
info('PASS manifest schema, SemVer 2.0.0, artifact digests and sizes')

manifest_targets = manifest.get('expected_targets')
if not isinstance(manifest_targets, list) or not manifest_targets:
    fail('manifest does not declare expected_targets')
if any(not isinstance(target, str) or not TARGET_RE.fullmatch(target) for target in manifest_targets):
    fail('manifest expected_targets contains invalid target triples')
manifest_targets = sorted(set(manifest_targets))
if manifest_targets != sorted(manifest.get('expected_targets')):
    fail('manifest expected_targets must be unique and sorted')
if expected_targets and expected_targets != manifest_targets:
    fail(f'caller expected targets {expected_targets} != manifest {manifest_targets}')

common = ('packs/python/pack.toml', 'packs/bash/pack.toml',
          'share/doc/meld-lang/LICENSE-APACHE',
          'share/doc/meld-lang/LICENSE-MIT',
          'share/doc/meld-lang/THIRD-PARTY-NOTICES.md',
          'share/doc/meld-lang/CHANGELOG.md')
targets = []
archive_paths = []
for name in archives:
    target = archive_target(name, manifest.get('version', ''))
    if target is None:
        fail(f'unexpected archive name {name}')
    targets.append(target)
    archive_paths.append(root / name)
if len(targets) != len(set(targets)):
    fail(f'duplicate archive targets: {targets}')
actual_targets = sorted(set(targets))
if actual_targets != manifest_targets:
    fail(f'archive targets {actual_targets} != manifest expected_targets {manifest_targets}')
for path in archive_paths:
    ext = '.exe' if path.name.endswith('.zip') else ''
    required = tuple(f'bin/{name}{ext}' for name in ('meld', 'meld-lsp', 'meld-pack')) + common
    names = set()
    if path.name.endswith('.tar.gz'):
        with tarfile.open(path, 'r:gz') as tf:
            members = tf.getmembers()
            for member in members:
                validate_entry_name(member.name, path.name)
                if member.issym() or member.islnk():
                    validate_entry_name(member.linkname, path.name)
            names = {n.split('/', 1)[1] for n in tf.getnames() if '/' in n}
    else:
        with zipfile.ZipFile(path) as zf:
            for name in zf.namelist():
                validate_entry_name(name, path.name)
                if (name.endswith('/') is False and
                        ((zf.getinfo(name).external_attr >> 16) & 0o170000) == 0o120000):
                    fail(f'{path.name}: symlink entries are not allowed ({name})')
            names = {n.split('/', 1)[1] for n in zf.namelist() if '/' in n}
    absent = [item for item in required if item not in names]
    if absent:
        fail(f'{path.name} is missing {absent}')
info(f'PASS archive payload and entry names ({len(archives)} archive(s), {len(actual_targets)} target(s))')
info(f'PASS expected target set {actual_targets}')

for name in sorted(signed):
    subject(name)
PY

awk -F '\t' '$1 == "INFO" { print $2 }' "$REPORT"
awk -F '\t' '$1 == "SUBJECT" { print $2 }' "$REPORT" > "$SUBJECTS"
[ -s "$SUBJECTS" ] || fail 'no subjects were produced for signature verification'

if [ -n "$OFFLINE" ]; then
    printf 'verify_artifacts.sh: OFFLINE - cosign signatures and attestations were NOT verified for trust\n'
    exit 0
fi

command -v cosign >/dev/null 2>&1 || fail 'cosign not found; use --offline for integrity-only checks'
while IFS= read -r name; do
    [ -n "$name" ] || continue
    subject_file="$DIR/$name"
    printf 'verify_artifacts.sh: verifying signature for %s\n' "$name"
    cosign verify-blob \
        --bundle "$subject_file.bundle" \
        --certificate-identity-regexp "$IDENTITY" \
        --certificate-oidc-issuer "$ISSUER" \
        "$subject_file" >/dev/null
done < "$SUBJECTS"
subject_count=$(wc -l < "$SUBJECTS" | tr -d ' ')
printf 'verify_artifacts.sh: PASS %s Sigstore signature(s)\n' "$subject_count"

if [ -z "$ATTESTATION" ]; then
    printf 'verify_artifacts.sh: NOTICE GitHub attestation not verified; pass --attestation FILE or --require-attestation\n'
else
    command -v gh >/dev/null 2>&1 || fail 'gh is required for --attestation verification'
    gh attestation verify --help 2>&1 | grep -q -- '--bundle' || \
        fail 'installed gh does not support `gh attestation verify --bundle`'
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        gh attestation verify "$DIR/$name" \
            --repo "$REPO" \
            --bundle "$ATTESTATION" \
            --cert-identity-regex "$IDENTITY" \
            --cert-oidc-issuer "$ISSUER" \
            --predicate-type 'https://slsa.dev/provenance/v1' >/dev/null
    done < "$SUBJECTS"
    printf 'verify_artifacts.sh: PASS GitHub build-provenance attestations\n'
fi
