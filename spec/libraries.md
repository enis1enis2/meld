# Meld Lang Library Specification

Status: normative. Derived from `project.md` §4 (Lang Pack contract), §7
(tooling), §9 (security/trust) and §11 (ecosystem). Implemented by
`crates/meld-lib`, `crates/meld-installer` (binary `meld-pack`) and the library
discovery path in `crates/meld-cli`.

A **Lang Library** is a Lang Pack directory plus a `library.toml` manifest. It
is an independently installable and importable unit of language data, not a
dynamically linked artifact. This document defines the manifest, the installed
on-disk index, the project import lock, and the terminal installer. Integrity
rules for the underlying pack are defined in `registry.md` §1 and reused
verbatim.

## 1. Why a library is data

```text
packs/<lang-id>/
  pack.toml          # Lang Pack contract (lang-pack.md §2)
  library.toml       # install/import manifest (this document)
  ...                # grammar reference, queries, runner, worker, tests
  checksums.sha256   # registry.md §1.1
  signature.ed25519  # registry.md §1.2, required when pack.toml has strict = true
```

**JUDGMENT CALL:** "library" uses the package-manager sense from `project.md`
§4/§11: a reviewable directory that can be copied, pinned and removed. A
tree-sitter `cdylib` would require compiling arbitrary downloaded code at
discovery time, which is outside the trust model of `project.md` §9. The
installer never imports, evaluates or executes pack content; it only clones a
git repository, verifies bytes, and copies files.

`library.toml` is required for a pack to be installable by `meld-pack`. Every
checked-in pack and every pack created by `tools/gen_pack.py` ships one. A bare
pack directory can still be used directly through `MELD_PACKS`; the manifest is
what makes it a library with an identity, a version and a declared data surface.

## 2. `library.toml` fields (v1)

```toml
api = "meld-lang-library/1"      # required, exact
kind = "lang"                    # required, exact
id = "bash"                      # required, unique library id
name = "Bash"                    # required, non-empty display name
version = "0.1.0"                # required, exact installed version
aliases = ["sh", "shell"]        # optional, must equal pack.toml aliases
exports = [                      # optional, stable relative data surface
  "pack.toml",
  "runner.toml",
  "worker.sh",
]
```

Validation performed by `meld_lib::validate_library_dir`:

1. `api` is exactly `meld-lang-library/1`; `kind` is exactly `lang`. Unknown
   values are rejected instead of guessed.
2. `id`, `version`, and every alias match `^[A-Za-z0-9][A-Za-z0-9._-]*$`.
   `name` must be a non-empty string.
3. `id`, `version`, and `aliases` agree with `pack.toml`. `aliases` must be an
   exact set match, not a subset, so the runtime and installer resolve the same
   names. **JUDGMENT CALL:** legacy packs without a `pack.toml` version are read
   as `0.1.0` for library compatibility; newly generated packs always write the
   field explicitly.
4. Every `exports` entry is a relative POSIX-style path: no absolute path, no
   `..` component, and no backslash. Every named path must exist in the pack.
   **JUDGMENT CALL:** `exports` records the stable data surface an importer may
   rely on; the installer copies the whole verified pack, but a consumer is only
   promised the listed paths. Missing or traversal paths fail validation before
   any copy is made (`project.md` §9).
5. The pack passes `registry.md` §1 verification: canonical checksums, no
   unexpected/missing files, no symlinks, and a signature when `strict = true`.
   With `--trust-roots`, the Ed25519 signature is authenticated against a
   public key and a revoked key-id is rejected; without them it is only
   reported as present. Remote installs set `require_trusted`, so an absent,
   unknown or unverified signature is a hard failure there.

`tools/gen_pack.py` emits this manifest for every generated pack.
`tools/make_libraries.py` derives it from existing `pack.toml` files, refreshes
`checksums.sha256`, and deliberately leaves signing as a separate explicit step
so the private key never enters the repository.

## 3. Installed state

### 3.1 Library root

The default root is resolved in this order:

1. `--root <dir>` on `meld-pack`.
2. `MELD_LIB_ROOT` (non-empty).
3. `XDG_DATA_HOME/meld/libraries`.
4. `HOME/.local/share/meld/libraries`.
5. `.meld-libraries` as a last-resort relative path.

Installed layout:

```text
<root>/
  index.json              # canonical JSON, sorted keys, trailing LF, no timestamps
  <id>/<version>/         # complete copied Lang Library, including integrity files
```

`index.json` schema `meld-lang-library-index/1`:

```json
{
  "libraries": {
    "bash": {
      "aliases": ["sh", "shell", "posix-sh"],
      "id": "bash",
      "level": "L1",
      "path": "bash/0.1.0",
      "source": "github:enis1enis2/meld@main",
      "tier": "T2",
      "version": "0.1.0"
    }
  },
  "schema": "meld-lang-library-index/1"
}
```

Rules:

- The outer map key equals `entry.id`; both and `entry.version` are validated
  names. `path` is relative to the root, must not be absolute, and cannot
  contain `..`.
- One active version is recorded per library id. Installing another version of
  the same id replaces the index entry; the old directory is left on disk and
  can be removed explicitly.
- `source` is a stable label such as `github:owner/repo@ref` or
  `local:/abs/path@main`; it is provenance metadata, not a fetched URL used at
  runtime.

### 3.2 Project import lock

A project records the libraries it actually uses in `meld.lock` at the project
root:

```json
{
  "libraries": {
    "bash": "0.1.0"
  },
  "schema": "meld-project-libraries/1"
}
```

Schema `meld-project-libraries/1`. Keys are canonical library ids (not
aliases), values are exact versions. The file is canonical JSON with sorted
keys, a trailing LF and no timestamps, so it is reviewable and reproducible.

Project root resolution for `meld-pack` is `--project`, then `MELD_ROOT`, then
the nearest current-directory ancestor containing `project.md` or
`Cargo.toml`, then the current directory. `meld` uses `MELD_ROOT` or the same
ancestor search.

**JUDGMENT CALL:** installation and import are separate operations. `install`
writes bytes to the root and, by default, imports the id/version into
`meld.lock`; `--no-import` installs without changing the project. `import` and
`unimport` edit only the lock. This keeps shared installs distinct from
per-project choices and makes a lock diff the reviewable record of a language
dependency.

## 4. Terminal installer (`meld-pack`)

```text
meld-pack remote-list [--source <repo>] [--ref <ref>]
meld-pack install <lang>... [--source <repo>] [--ref <ref>]
                            [--trust-roots <file>] [--revocations <file>]
                            [--root <dir>] [--project <dir>] [--force] [--no-import]
meld-pack remove <lang>... [--root <dir>] [--project <dir>] [--keep-import]
meld-pack import <lang>... [--root <dir>] [--project <dir>]
meld-pack unimport <lang>... [--root <dir>] [--project <dir>]
meld-pack list [--root <dir>] [--json]
meld-pack verify <dir> [--trust-roots <file>] [--revocations <file>]
meld-pack self-install [--bin-dir <dir>]
meld-pack self-uninstall [--bin-dir <dir>]
```

Environment fallbacks: `MELD_PACK_SOURCE`, `MELD_PACK_REF`,
`MELD_PACK_TRUST_ROOTS`, `MELD_PACK_REVOCATIONS`, `MELD_LIB_ROOT`, `MELD_ROOT`,
and `TMPDIR`/`MELD_PACK_TMP` for temporary git checkouts. `--key` was removed
with HMAC and exits with a migration error. Exit codes are `0` success,
`1` validation/integrity failure, and `2` usage error.

### 4.1 Source selection

`--source` accepts:

- `owner/repo` (default `enis1enis2/meld`);
- `github:owner/repo`;
- a `https://`, `http://`, `git@`, `file://`, or `.git`-suffixed URL;
- a local git repository path, canonicalized before use.

A reference can be given as `--ref`, `MELD_PACK_REF`, or inline `source@ref`
(the inline form is ignored for `git@` SSH syntax). A bare name defaults to
`main`. The installer runs `git init`, `git remote add origin`, `git fetch
--depth 1 origin <ref>`, and `git checkout FETCH_HEAD` in a temporary directory
that is removed on exit.

**JUDGMENT CALL:** the installer shells out to `git` instead of linking an
HTTP client. Git is already a development dependency, understands GitHub URLs
and credentials, and makes the trust boundary explicit: the source is a git
tree, not a mutable working directory, and no pack code is imported. Local
paths use the same clone path as GitHub so tests and offline mirrors cannot
silently take a different code path.

### 4.2 Resolution and lifecycle

- `remote-list` scans the fetched tree recursively for `library.toml`, skips
  `.git`/`target`/`__pycache__`, and prints id, version, tier, level and name.
  Invalid manifests abort the command with their paths listed.
- Names resolve by exact id first, then by unique alias. An alias matching more
  than one id is an error (`bash` aliases include `sh`; the resolver never
  guesses).
- `install` validates and verifies every selected library in the checkout,
  copies it to `<root>/<id>/<version>`, and verifies the copy again. The second
  check catches copy-time corruption and makes installed bytes authoritative.
  An identical existing install is a no-op; different bytes at the same
  coordinates require `--force`. **JUDGMENT CALL:** remote installs require a
  verified Ed25519 signature by default. Trust roots and revocations are read
  from the fetched tree (`packs/registry/trust-roots.toml` and
  `packs/registry/revocations.json`) unless `--trust-roots`/`--revocations`
  pin an out-of-band copy. Missing files are a hard error, not a silent
  presence-only check.
- After a successful copy, the index is upserted and, unless `--no-import`, the
  exact version is written to `meld.lock`. Imports are saved after each library
  so a multi-name install leaves a consistent prefix on failure.
- `remove` resolves id/alias from the index, deletes the installed directory,
  updates the index, and removes the corresponding lock entry (including an
  alias-named stale entry) unless `--keep-import`. Empty `<root>/<id>` parents
  are pruned.
- `verify` prints the manifest identity, tier/level, strictness, verified file
  count, and whether a signature is absent, present-unverified, or verified
  with the supplied trust roots.
- `self-install` copies the running `meld-pack` binary to
  `--bin-dir`, `XDG_BIN_HOME`, or `~/.local/bin` and sets mode `0755` on Unix.
  `self-uninstall` uses the same resolution, including `--bin-dir`. Both are
  explicit convenience commands; they never touch the library root.

## 5. Runtime handoff

`meld` reads `meld.lock` through `meld_lib::discover_imported`. It searches the
default library root plus any `MELD_PACKS` roots and returns the first existing
`<root>/<id>/<version>` directory for each locked id.

**JUDGMENT CALL:** a lock entry whose directory is missing is skipped rather
than failing `check`/`run`; `meld-pack list` and `meld doctor` report the drift,
and reinstalling repairs it. A malformed lock is a warning, not a startup
failure. Neither case can execute code.

Imported libraries are inserted **after** built-in and `MELD_PACKS` packs in
discovery. Because later inserts win, a project lock with the same id as a
built-in pack takes precedence; `meld-cli` has a regression test asserting this.
Runner/formatter argument rebasing prefers a path inside the discovered pack,
then the pack directory by basename, and only then the project root, so an
imported worker is not shadowed by an identically named built-in worker.

`meld lib` and `meld doctor` are read-only views of installed libraries, active
imports, and their `ok`/`MISSING` status. All mutation lives in `meld-pack`.

## 6. Security and failure modes

Hard rules inherited from `project.md` §9 and `registry.md` §4:

- No pack code is executed by the installer; no toolchain is installed;
  `toolchain.toml` remains data and `meld doctor` only reports.
- Symlinks are rejected while hashing and copying, so a pack cannot reach
  outside its own directory.
- All manifest and index paths are relative and traversal-checked.
- Canonical JSON/checksum encoding has no timestamps, making repeated output
  byte-for-byte reproducible.
- Public trust roots are distributed with the registry; private keys stay
  outside the repository and never enter a pack. Ed25519 lets verifiers check
  provenance without gaining the ability to forge. Revoked key ids are rejected
  before signature verification.
- Local `verify` may report a well-formed signature as `present-unverified`,
  but strict packs always need a signature file and **remote installs** require
  a verified one. Pass `--trust-roots`/`MELD_PACK_TRUST_ROOTS` (and optionally
  revocations) for local authenticity checks.

| Condition | Result |
| --- | --- |
| Missing/wrong `api` or `kind` | unsupported API or library kind |
| Missing/empty `name`, invalid id/version/alias | manifest validation error |
| `library.toml` disagrees with `pack.toml` id/version/aliases | consistency error |
| Unsafe or missing `exports` path | manifest validation error |
| Checksum mismatch, added/removed file, symlink | pack verification error |
| `strict = true` without signature | `strict pack requires signature.ed25519` |
| Unknown key-id | `is not in the trust roots` |
| Revoked key-id | `is revoked` |
| Modified checksums, id or version | `signature mismatch` |
| Missing trust roots on a remote install | `no trust roots; pass --trust-roots` |
| Alias used by two installed/remote libraries | ambiguous alias error |
| Different bytes at installed coordinates | bump version or pass `--force` |
| Missing imported directory at runtime | entry skipped; reported as `MISSING` |
| Temporary clone not visible after git checkout | actionable error: set `TMPDIR` or `MELD_PACK_TMP` to a shared directory |
| Unsupported `index.json`/`meld.lock` schema | hard error in `meld-pack`, warning in `meld` |

## 7. Implementation note

`meld_lib::verify_pack` mirrors `tools/packtool.py`: canonical checksums are
hashed with `sha2 0.11.1`, and the signature is checked with
`ed25519-dalek = "=3.0.0"` using `verify_strict`. `ed25519-dalek 3.0.0` declares
`rust-version 1.85`, which fits the workspace's `rust-version 1.90`. A unit test
pins the exact signed byte sequence
(`meld-pack/ed25519/v1\n<id>\n<version>\n<checksums bytes>`), so a dependency
upgrade cannot silently change the integrity format. HMAC code and tests were
removed; only a migration note remains.

## 8. Judgment call index

1. A Lang Library is a Lang Pack plus `library.toml`, data rather than a
   `cdylib` (§1).
2. `exports` is a stable relative data surface; traversal and missing paths fail
   validation, but the whole verified pack is copied (§2).
3. Shared installed root with canonical `index.json` and per-project
   `meld.lock`; install and import are separate commands (§3).
4. `git` CLI with shallow fetch of a git tree instead of an HTTP client or
   direct working-tree reads (§4.1).
5. Verify in checkout and destination; idempotent identical installs, `--force`
   for changed bytes (§4.2).
6. Missing imports are skipped, and imported libraries are discovered last so
   the project lock wins over built-ins (§5).
7. Ed25519 replaced HMAC so public trust roots can authenticate a pack without
   making every verifier a signer. Strict packs require a signature file;
   remote installs require it to verify, and revoked key ids fail first (§6).
8. Temp clones honor `TMPDIR`/`MELD_PACK_TMP` and are checked for visibility
   after checkout, so a chrooted git cannot silently produce an empty catalog
   (§4.1/§6).
