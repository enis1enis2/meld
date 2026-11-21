# Lang Pack Specification (Phase 0)

Status: normative. Derived from `project.md` §4, §5, §9.

## 1. Layout

```text
packs/<lang-id>/
  pack.toml          # required manifest
  library.toml       # install/import manifest (spec/libraries.md)
  grammar/           # tree-sitter grammar source or pinned reference
  queries/           # copied highlight/injection queries (tools/sync_grammars.py)
  region_end.toml    # data-only end-scanner profile (built-in kind + options)
  runner.toml        # tier, command, protocol, sandbox profile (T2 runtime)
  marshal.*          # Meld Value <-> native converters
  toolchain.toml     # detect hints; NEVER install hints executed automatically
  inline.toml        # source-literal style for values rendered into this pack
  lsp.toml           # editor metadata: language id, extension, LSP command
  formatter.toml     # optional pack-owned formatter command
  checksums.sha256   # Phase 7 registry integrity
  signature.ed25519  # Phase 7 registry integrity (required when strict)
  tests/             # conformance cases
```

Only `pack.toml` is mandatory for a pack used directly from a directory.
`library.toml` is mandatory for an installable Lang Library
(`spec/libraries.md`); every checked-in and generated pack ships one. The
remaining files are required by the features a pack declares: `runner.toml` for
auto-launched T2, `inline.toml` when the pack can contain inline islands,
`lsp.toml`/`formatter.toml` for editor tooling, and the integrity files for
strict publication (`spec/registry.md` §1).

## 2. `pack.toml` fields (v1)

```toml
id = "python"                  # required, unique
version = "0.1.0"              # required for Lang Libraries; exact version
aliases = ["py", "python3"]    # optional
extensions = ["py"]            # optional, used by auto-detect
tree_sitter = { crate = "tree-sitter-python", rev = "pinned" }
tier = "T2"                    # T1..T5, or "L0" shorthand for syntax-only
level = "L1"                   # L0 syntax, L1 statements, L2 values, L3 handles
capabilities = ["stdout", "env:PATH"]
sandbox = { network = false, fs = ["project"] }
```

Validation: `meld check` validates a `.meld` file's regions, source maps and
low-confidence detection; it does not validate pack manifests. `packtool verify`
validates `pack.toml` tiers/levels, integrity files, and (for strict packs) the
grammar pin. Launching a T2 pack at `level >= L1` additionally requires a usable
`runner.toml` (§4.1), which is a runtime discovery rule rather than a checksum
rule. `meld_lib::validate_library_dir` additionally checks that a `library.toml`
agrees with `pack.toml` id, version and aliases, and that every declared export
exists (`spec/libraries.md` §2). **JUDGMENT CALL:** a grammar rev is pinned when
it is either an opaque `pinned:<ref>` or an exact crate version (`0.25.0` /
`=0.25.0`). The checked-in packs use exact versions so `tools/sync_grammars.py`
can consume the same field; floating refs such as `main` are not pins
(project.md §9).

## 3. Conformance levels

- **L0** syntax-only: highlighting/structure; no runner invoked.
- **L1** statements: code executes, stdout merged, no bridge values.
- **L2** values: import/export of the full value model round-trips.
- **L3** full: handles and callbacks (`$fn`, `$handle`) work with lifetimes.

A language with no runnable CLI/interpreter is L0 only; this is stated plainly
in the pack and in `meld doctor` (project.md §5 honesty note).

## 4. Tier selection

Packs declare one primary tier. T2 is the default universal tier (project.md
§5 judgment call). T1/T3/T4 are optimizations and must declare a T2 fallback so
`meld run` works when the optimization is unavailable.

## 4.1 `runner.toml` (T2 runtime discovery)

```toml
tier = "T2"
protocol = "meld-t2/1"
command = "python3"
args = ["packs/python/worker.py"]
cwd = "project"
stdout = "protocol"
stderr = "forward"

[limits]
max_line_bytes = 1048576
idle_timeout_ms = 30000
```

`meld run` discovers this file at runtime; no language is hardcoded in the CLI.
Command and argument paths are rebased pack-directory first, then
pack-directory by basename, and only then project-root, so an imported library
cannot be shadowed by a same-named built-in worker. Set `MELD_ROOT` to override
the project root, `MELD_PACKS` to add pack roots, and `MELD_LIB_ROOT` to move the
installed library root (`spec/libraries.md` §3–§5).
**JUDGMENT CALL:** only `tier = "T2"` with `protocol = "meld-t2/1"` is
auto-launched. Other tiers use explicit host APIs and are never guessed from a
manifest.

## 4.2 `inline.toml` (inline value rendering)

```toml
[inline]
style = "python"        # python | javascript | json | lua
```

The inline island's language evaluates `⟦lang: expr⟧` with the T2 `eval` method
(`spec/t2-protocol.md` §3). The *containing* pack then renders the returned
Meld Value back into source using this style. **JUDGMENT CALL:** semantics stay
with the inline language, syntax stays with the host language. Workers receive
only the trimmed expression: delimiter padding is Meld syntax, not source code.

## 4.3 `lsp.toml` (editor tooling)

```toml
[lsp]
language_id = "python"         # passed to the external server / virtual doc
extension = "py"               # virtual document URI suffix
command = "python3"            # optional external stdio LSP
args = ["vendor/server.py"]
keywords = ["print", "len"]    # deterministic fallback completions
```

See `spec/lsp.md` for virtual-document and source-map rules. **JUDGMENT CALL:**
`command` is optional. A pack with only `keywords` still gets completions; a
pack with `command` gets real server forwarding. The core links no language
server and keeps no language-id table.

## 4.4 `formatter.toml` (region formatting)

```toml
[formatter]
command = "python3"
args = ["vendor/fmt.py", "{src}", "{out}"]
```

`meld fmt` formats glue itself (preserving region byte layout) and writes each
region body to `{src}` for the pack to rewrite into `{out}`. **JUDGMENT CALL:**
the core never parses a language to format it; pack commands are argv vectors
with placeholders, never shell strings, and paths follow the `runner.toml`
rebasing rule.

## 4.5 `region_end.toml` (region boundary profile)

```toml
schema = "meld-region-end/1"
kind = "heredoc"        # built-in Rust algorithm; "wasm" is reserved
fallback = "brace"      # used when metadata is unknown or invalid
hint = "raw hash delimiters remain available"
[options]
dialect = "ruby"
```

`meld check` and `meld-lsp` build a `ScannerRegistry` deterministically: core
built-ins first, then discovered packs sorted by id with sorted aliases. The
file names a built-in algorithm (`brace`, `heredoc`, `indent`, `delimited`,
`counted`, `template`) and its options; the returned `RegionEnd` carries
`body_end`, `full_end`, and `close_marker` so glue and diagnostics keep exact
offsets. The raw hash delimiter form never consults this registry.

**JUDGMENT CALL:** pack end-scanner metadata is data-only. No `region_end.py`,
`region_end.js`, `region_end.lua`, `region_end.sh`, or `region_end.wat` path is
loaded, spawned, or compiled. `kind = "wasm"`, an unknown kind/schema, or invalid
TOML prints `meld: warning: ... using brace fallback`. Adding this payload file
to an already signed strict pack changes its checksum set; the registry owner
re-sums and re-signs it, and this implementation never edits signatures or
`checksums.sha256`.

## 5. Generator contract (Phase 1)

`tools/gen_pack.py <lang-id> --grammar <pinned-ref>` creates a minimal L0 pack:
manifest, `library.toml`, grammar reference, a `region_end.toml` selecting the
`brace` profile, highlight query, and a feeder test. This is the "cheap route to a long tail" from
project.md §4, and generated packs are libraries from birth
(`spec/libraries.md` §2). The generator never downloads or executes grammar
code; it writes references only.

**JUDGMENT CALL:** `--strict` adds an exact crate pin, aliases/extensions,
version, LSP fallback keywords, and a raw-delimiter feeder fixture so a
reviewed pack can be signed immediately by `packtool`. The 15 packs from
`status/language-expansion.md` were created this way; the default mode still
emits `generated-placeholder` and `strict = false`.

## 6. Security

- Packs are data + small sandboxed plugins. `region_end.toml` is data-only in
  Phase 6 (§4.5); `kind = "wasm"` is reserved and warns instead of executing a
  module. Wasm remains the intended model for `marshal` (project.md §4).
- No implicit network/filesystem. `sandbox` in manifest is deny-by-default.
- Signing and registry are Phase 7 (`spec/registry.md`): `checksums.sha256`
  covers every payload file and `signature.ed25519` authenticates it against a
  public trust root. A strict pack refuses to publish or install without both
  files; the private key stays outside the repository.
- Installation and import are the Lang Library layer (`spec/libraries.md`):
  `meld-pack` verifies data in the git checkout and at the destination, copies
  no symlinks, executes no pack code, and records exact versions in
  `meld.lock`.
- `meld doctor` reports toolchain state and library drift; it never installs
  (project.md §9).

## 7. Example minimal pack

See `packs/python/` and `packs/javascript/` for L1/L2 examples,
`packs/bash/` for an executable L1 Lang Library, and any generated pack for the
`library.toml` shape.
