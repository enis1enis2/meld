# Meld Pack Registry and Signing Specification (Phase 7)

Status: normative for Phase 7. Derived from `project.md` §4 (Lang Pack
contract), §7 (tooling), §9 (security), §11 (ecosystem). Implemented by
`tools/packtool.py`; every decision marked **JUDGMENT CALL** is binding until a
later phase explicitly changes it.

The goal is narrow: a third party can publish, discover and install a Lang Pack
as **data**, with integrity checks, and without touching `crates/` or any
existing language pack. The registry has no server, no network and no package
manager semantics; it is a checked-in directory that can be mirrored like any
other source tree.

## 1. Integrity files in a pack

Each pack carries two extra files next to `pack.toml`:

```
checksums.sha256      # SHA-256 of every payload file
signature.hmac        # optional HMAC-SHA256 over the checksums file
```

### 1.1 `checksums.sha256`

One line per payload file:

```
<64 lowercase hex chars>  <posix-relative path>
```

Exactly two spaces separate the digest from the path; paths are relative to the
pack root, use `/`, and contain no `..` or absolute prefix. Lines are sorted by
path and terminated with LF. `packtool.verify` rejects a file that is not in
this canonical form.

Excluded from checksumming (they cannot checksum themselves and are control
metadata): `checksums.sha256`, `signature.hmac`, caches (`__pycache__`,
`.mypy_cache`, `.pytest_cache`, `.git`), and `*.pyc`/`*.pyo`.
**JUDGMENT CALL:** excluding caches makes a pack's digest reproducible from its
source files. Symlinks are rejected rather than followed: following one could
make a published pack read outside its own directory (project.md §9).

`pack.toml` may set `strict = true`. Then both files are mandatory for
`verify` and `publish`; a missing one is an error, not a warning. Non-strict
packs still require `checksums.sha256` (there is nothing to verify without it)
but may omit `signature.hmac`. **JUDGMENT CALL:** strict packs additionally
require an immutable `[tree_sitter].rev`: either an opaque `pinned:<ref>` or an
exact crate version such as `0.25.0` (optionally `=0.25.0`). Both spellings are
honest pins; a floating ref like `main` or the word `pinned` is rejected. The
checked-in grammar packs use the exact-version form because
`tools/sync_grammars.py` reads that field as a crate version, matching
`lang-pack.md` §2/§6.

### 1.2 `signature.hmac`

One line:

```
hmac-sha256 <key-id> <64 lowercase hex chars>
```

- `digest = HMAC-SHA256(key_bytes, raw_bytes_of_checksums.sha256)`.
- `key-id = first 16 hex chars of SHA-256(key_bytes)`; it identifies a key
  without revealing it and lets a verifier reject the wrong key early.
- The MAC covers the checksums file, which in turn commits to every payload
  file. It does not cover itself.

**JUDGMENT CALL (major):** HMAC-SHA256 with a local key file is used instead of
public-key signatures. Rationale: HMAC is in the Python standard library, so the
tool has no third-party dependency, and it provides local integrity and
authenticity between parties that already share a key. Full public-key signing
(e.g. Ed25519/minisign) and key distribution are future work. Consequences,
which callers must understand:

- HMAC is symmetric: anyone who can verify can also forge. Treat the key as a
  secret held by the publisher and by verifiers that need authenticity.
- Keys must be at least 16 bytes and must not live inside the pack (otherwise
  the key would be published with the signature). A key file must be
  distributed out of band.
- `verify`/`publish`/`install` without `--key` check only that a well-formed
  signature **exists**; they cannot prove authenticity. With `--key`, the MAC
  is checked with `hmac.compare_digest` and a mismatch is a hard failure.

## 2. Registry format

A registry directory:

```
<registry>/
  index.json              # canonical JSON, sorted keys, no timestamps
  packs/<id>/<version>/    # complete pack copies, including integrity files
```

`index.json` schema `meld-pack-registry/1`:

```json
{
  "schema": "meld-pack-registry/1",
  "packs": [
    {
      "id": "shell",
      "version": "0.1.0",
      "aliases": ["sh", "posix-sh"],
      "tier": "T2",
      "level": "L1",
      "strict": true,
      "path": "packs/shell/0.1.0",
      "checksums_sha256": "<sha256 of that pack's checksums.sha256>",
      "signature": {"scheme": "hmac-sha256", "key_id": "<16 hex>", "digest": "<64 hex>"}
    }
  ]
}
```

Rules:

- `id` and `version` match `^[A-Za-z0-9][A-Za-z0-9._-]*$`; `version` defaults to
  `0.0.0` for pre-registry packs. `path` is relative to the registry root and
  must stay inside it. **JUDGMENT CALL:** relative paths allow a registry to be
  moved or mirrored without rewriting the index.
- `signature` is `null` for unsigned (necessarily non-strict) packs.
- The index is written with sorted keys and **no timestamps**, so repeated
  publishes of identical content are byte-for-byte reproducible (project.md
  §9). Entries are sorted by `(id, version)`.
- The checked-in `packs/registry/index.json` is intentionally empty: only
  `packs/registry/index.json` is shipped, not staged pack copies. Publish a
  pack into a registry directory to create its entry and staged copy; this
  keeps the repo free of duplicate pack data and keeps `install` honest (an
  entry always has bytes to verify).
- **JUDGMENT CALL:** publishing an existing `id`/`version`
  with identical integrity data is idempotent; publishing *different* content
  at the same coordinates is rejected. Bump the version instead, because
  consumers pin versions.

## 3. CLI

```
python3 tools/packtool.py verify <pack-dir> [--key <path>]
python3 tools/packtool.py sign <pack-dir> --key <path>
python3 tools/packtool.py publish <pack-dir> --registry <dir> [--key <path>]
python3 tools/packtool.py list --registry <dir> [--json]
python3 tools/packtool.py install <lang> --registry <dir> --into <dir> [--key <path>]
```

- `verify` checks payloads against `checksums.sha256`, then signature presence
  (and MAC if `--key` is given). It prints a short report ending in `OK`.
- `sign` writes `signature.hmac`; it refuses a stale/malformed checksums file
  and a key located inside the pack.
- `publish` verifies first, copies the pack to `packs/<id>/<version>/`, then
  updates `index.json`.
- `list` prints one row per pack; `--json` emits the raw index for tooling.
- `install` resolves an id or alias, verifies the staged pack, checks that the
  index's checksum/signature metadata matches the staged files, copies it to
  `<into>/<id>`, and verifies the copy. If several versions match, the highest
  numeric dotted version wins.
- Exit codes: `0` success, `1` integrity/validation failure, `2` argparse usage
  error. All expected failures are printed as `packtool: error: ...` on stderr.

**JUDGMENT CALL:** `install` refuses to overwrite an existing target. It is a
data copy, not a package manager; no uninstall/upgrade machinery is implied.
Alias collisions between two different ids are an error rather than a guess.

## 4. Trust model

| Actor | Power | Defense |
|---|---|---|
| Pack author | supplies pack data and plugins | checksums detect changes after publication; signature authenticates the checksums file to key holders |
| Registry maintainer | edits `index.json` and staged files | index checksum/signature fields are compared to the staged pack on install; canonical sorted JSON is reviewable in version control |
| Installer | chooses registry, target dir, optional key | install only copies data; refuses strict packs without signature, refuses mismatches, never overwrites |
| Worker/runtime (future) | executes pack code | out of scope for packtool; OS sandbox and pack `sandbox` rules come from project.md §9 / `lang-pack.md` §6 |

Hard rules for registry tooling:

- **No network access** and no downloads.
- **No toolchain installation.** `toolchain.toml` is copied as data;
  `meld doctor` reports, never installs (project.md §9).
- **No pack code execution.** `packtool` never imports a worker, `marshal`
  module or end scanner, and never spawns a process from pack content.
- **No implicit trust in the index.** Index metadata is validated against the
  staged pack before a copy is made.
- Unauthenticated packs (no key supplied) may still be installed for local
  development, but `list` marks them `unsigned` and strict packs are always
  required to carry a signature. Verifiers that need provenance must supply
  the key.

## 5. Failure modes

| Condition | Result |
|---|---|
| `checksums.sha256` missing | error for verify/publish/install (explicitly required for strict packs) |
| Checksums file malformed, unsorted, duplicate path, unsafe path, non-LF | `malformed checksums.sha256` / `not canonical` |
| Payload edited after checksumming | `checksum mismatch: <path>` |
| File added or removed | `unexpected file(s): ...` / `missing file(s): ...` |
| Symlink inside pack | `symlink in pack is not allowed` |
| `strict = true` without `signature.hmac` | `strict pack requires signature.hmac` (verify and publish both refuse) |
| Malformed signature line | `malformed signature.hmac` |
| Wrong or tampered key / modified checksums | `signature key-id does not match...` or `signature mismatch...` |
| Key inside pack / shorter than 16 bytes | `key file must not be inside the pack directory` / key length error |
| Unpinned grammar rev on a strict pack | `strict pack requires a pinned [tree_sitter].rev` |
| Invalid `pack.toml` / unknown tier or level | descriptive `invalid pack.toml` / `unknown tier`/`level` |
| Unknown id/alias, or ambiguous alias | `unknown language or alias` / `ambiguous alias` |
| Registry index missing / wrong schema / corrupt JSON | `registry index not found` / `unsupported registry schema` / `invalid registry index` |
| Index checksum/signature disagrees with staged pack | `registry index ... does not match the staged pack` |
| Install target exists | `install target already exists` |
| Different bytes at an existing id/version | `different pack already published ...; bump version` |

## 6. Adding a language without touching the Rust core

This is the Phase 7 done-when criterion (project.md §11). No file under
`crates/` and no existing `packs/<language>/` directory is modified.

1. Write a pack directory containing `pack.toml`, `runner.toml`,
   `toolchain.toml`, a worker (if `level >= L1`), `region_end.*`, and `tests/`
   as specified by `lang-pack.md`. A language with no runnable CLI stays L0;
   say so in its README (project.md §5 honesty note).
2. Run `packtool sign <pack-dir> --key <private-key>` (after generating
   `checksums.sha256`; `packtool` also exposes this step to tooling). Set
   `strict = true` when both integrity files are present.
3. Run `packtool publish <pack-dir> --registry <registry>`. This copies data
   only; the pack is not executed.
4. Consumers run `packtool list --registry <registry>` and
   `packtool install <lang> --registry <registry> --into <dir> [--key ...]`.
5. Runtime discovery of `runner.toml` is the integration point for the Rust
   runner; it reads the manifest and launches the declared worker. That work
   is intentionally outside this phase's tooling scope.
6. `tests/fixtures/pack-third-party/` is a complete worked example for a
   `shell` pack (a language absent from the existing packs). It is data-driven:
   its conformance cases live in `tests/conformance.json`, not in code.

**JUDGMENT CALL:** the registry stores only pack data and metadata; generator
polish and automatic grammar downloads remain out of scope. The registry
tooling is deliberately usable before any Rust changes exist, so third-party
packs can be reviewed as plain files.

## 7. Judgment call index

1. HMAC-SHA256 with a local key file, not public-key signatures; MAC covers the
   checksums file; key holders must share the secret; keyed verification is
   optional on verify/publish/install (§1.2).
2. Canonical checksums (sorted, LF, two spaces); caches and control files
   excluded; symlinks rejected (§1.1).
3. Checksums always required; signature required only for `strict` packs;
   strict also requires an immutable grammar rev (`pinned:<ref>` or exact
   version) (§1).
4. Relative registry paths, sorted canonical index with no timestamps;
   idempotent republish, version bump for changed bytes (§2).
5. Highest numeric dotted version on alias lookup; ambiguous aliases are errors
   (§3).
6. Install is a non-overwriting data copy: no network, no toolchain install,
   no pack code execution, no implicit trust in the index (§3–§4).
7. A pre-version pack publishes as `0.0.0`; `tomllib` means Python 3.11+ is
   required (§2 and implementation header).
