# Third-party demo pack: `shell` (T2, L1)

This pack adds POSIX shell to Meld **without changing the Rust core** or any
existing language pack. It is the worked example for `spec/registry.md` §6 and
the Phase 7 done-when criterion in `project.md` §11.

## Contents

| File | Purpose |
| --- | --- |
| `pack.toml` | Manifest: id, aliases, version, tier, level, capabilities, pinned grammar rev, `strict = true` |
| `runner.toml` | T2 launch profile: `sh worker.sh`, JSON-line protocol, project-relative path |
| `toolchain.toml` | Detect hints only; `install = "never"` (project.md §9) |
| `worker.sh` | Self-contained newline-delimited JSON worker; `initialize`, `execute`, `shutdown` |
| `region_end.sh` | Scanner stub; signals "use raw hash delimiters" |
| `grammar/REFERENCE.md`, `queries/highlights.scm` | Pinned grammar reference and L0 highlight query |
| `tests/conformance.json` | Data-driven behavioral cases (no code in the manifest) |
| `tests/feed.meld` | Mixed-language feeder fixture |
| `tests/run_conformance.py` | Explicit developer harness that replays the JSON cases |
| `checksums.sha256`, `signature.ed25519` | Integrity metadata for registry tooling |

## Why `shell`, and why L1

`shell` is not among the built-in packs (`python`, `javascript`, `lua`, plus
syntax-only packs). It demonstrates that a new language is just a directory of
data + a small plugin. The worker implements statements and ordered stdout but
no value bridge, so the pack honestly declares `level = "L1"`. A language with
no runnable CLI would be L0 instead (project.md §5 honesty note).

## Judgment calls

- **JSON codec:** `worker.sh` uses the Python 3 standard library only to encode
  and decode protocol lines. Python 3 is already present and nothing is
  installed; region code itself is executed by `/bin/sh`. A portable-sh JSON
  string decoder would be fragile, so the codec is isolated in small inline
  helpers (see comments in `worker.sh`).
- **Stateless regions:** each `execute` runs in a fresh `sh -c`, with stdin
  redirected to `/dev/null` so a region cannot consume protocol input. Shell
  variables do not persist between regions; L1 does not require persistence.
- **Sandbox:** the worker does not implement OS confinement. `runner.toml`
  declares the profile; the launcher is responsible for enforcing it
  (`spec/t2-protocol.md` §6).
- **Runner paths:** `args[0]` is project-root-relative to match existing packs.
  `packtool install` is a pure data copy and does not rewrite it; the future
  `runner.toml` discovery code must rebase it when the pack is installed
  elsewhere.
- **Signature key:** `signature.ed25519` is produced with a private key that is
  intentionally **not distributed** with the fixture. Only its public key is in
  `packs/registry/trust-roots.toml`. To re-sign after editing files, run:
  ```
  python3 tools/packtool.py sign tests/fixtures/pack-third-party \
    --private-key /path/to/ed25519-private.pem
  ```
  and regenerate `checksums.sha256` first if payloads changed. Verification
  without `--trust-roots` checks signature presence only; passing the public
  trust roots authenticates the exact signed message (see `spec/registry.md`).
  HMAC `--key` was removed.

## Use

From the project root:

```
python3 tools/packtool.py verify tests/fixtures/pack-third-party \
  --trust-roots packs/registry/trust-roots.toml \
  --revocations packs/registry/revocations.json
python3 tests/fixtures/pack-third-party/tests/run_conformance.py
python3 tools/packtool.py verify-registry packs/registry --verify-packs
python3 tools/packtool.py install shell --registry packs/registry \
  --into /tmp/meld-packs --trust-roots packs/registry/trust-roots.toml \
  --revocations packs/registry/revocations.json --require-trusted
```
