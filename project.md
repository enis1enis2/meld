# Meld — a language that switches to any language mid-file

Status: authoritative design plan. Historical note: the original draft said
"PLAN ONLY" and treated "Meld" as a working name with the judgment call "any
name works; rename freely". That framing was superseded on 2026-11-20 after
Phases 0–7 were implemented. Implementation evidence lives in `status/`; this
document remains the source of truth for design decisions. The release package
name is `meld-lang`; binaries are `meld`, `meld-lsp` and `meld-pack`.

## 1. Goal (and the honest version of it)

Build a language/toolchain where a single source file can switch language mid-file (even mid-line), and where each region is parsed, understood, and executed in its own language, with values passing between regions.

Reality check (judgment call, documented): "any and every language" literally cannot be built as a closed list. Languages are unbounded, change versions, and many have no embeddable runtime. The workable interpretation:

Meld is open-ended: a small fixed core plus a plugin contract (Lang Pack). Any language becomes supported by writing/generating a Lang Pack. Coverage grows by adding packs, not by rewriting the core.

Success = adding a new language costs a manifest + grammar + runner, not a core change.

Non-goals: one unified semantics for all languages; zero-cost cross-language calls; running untrusted code without a sandbox.

## 2. What already exists (research findings)

Meld should assemble proven parts, not invent them.

| Idea | What it proves | Source |
| --- | --- | --- |
| Tree-sitter language injection | A parent syntax tree can hold injected child trees (HTML→JS/CSS, JS→regex, Markdown code fences, Vue/Svelte). Injections can be static or dynamically named, nested recursively, and incrementally reparsed. | tree-sitter multi-language docs (https://tree-sitter.github.io/tree-sitter/using-parsers#multi-language-documents); ast-grep (https://ast-grep.github.io/advanced/language-injection.html); gotreesitter (https://pkg.go.dev/github.com/odvcencio/gotreesitter) lists 206 grammars |
| GraalVM Truffle polyglot | Many languages in one runtime, calling each other in shared memory via a standard interop protocol: a set of messages every language implements for foreign values, so languages need not know about each other. | https://docs.oracle.com/en/graalvm/jdk/17/docs/reference-manual/polyglot-programming/ |
| WebAssembly Component Model + WIT | A language-agnostic IDL for typed interfaces (strings, lists, records, variants, resources). Components don't share memory; data is copied via the Canonical ABI. | https://component-model.bytecodealliance.org/design/wit.html ; https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md |
| Racket #lang | A file declares its language at the top; a language = reader + expander, packaged under a name. Proves "language chosen per module" is a clean model. | https://docs.racket-lang.org/guide/languages.html |
| Raku slangs / "braid" | A parser that holds a set of mutually-embedded sublanguages and switches the active grammar mid-parse (strings, regex, pod, main language). Closest existing model to "switch mid-file". | https://gist.github.com/raiph/849a4a9d8875542fb86df2b2eda89296 ; https://mouq.github.io/slangs.html |
| SDF3 / SGLR (Spoofax) | Scannerless generalized-LR parsing supports the full class of context-free grammars, which is closed under composition, so separately defined grammars can be combined, even with different lexical syntax. LR variants can't do this. | https://link.springer.com/doi/10.1007/978-3-030-58768-0_1 ; https://repository.tudelft.nl/file/File_0d239cac-92d0-4eca-979f-51c0eed00bff |

Takeaway: the pieces exist separately. Tree-sitter gives breadth of grammars, Truffle gives the interop-protocol idea, WIT gives the cross-language type contract, Racket gives the per-module language declaration, Raku gives mid-parse grammar switching, SGLR gives the principled route to true grammar composition. Nobody ships all of it as an open plugin system across arbitrary languages. That's the gap.

## 3. Architecture

```text
 source file (.meld)
        │
 ┌──────▼───────┐
 │ 1. Region    │  splits file into language-tagged regions (islands)
 │    Splitter  │  using switch markers + per-language end scanners
 └──────┬───────┘
        │ regions[]
 ┌──────▼───────┐
 │ 2. Syntax    │  tree-sitter per region (injection layers), keeps
 │    Layer     │  byte ranges + source maps; highlighting, structure
 └──────┬───────┘
        │ typed region trees
 ┌──────▼───────┐
 │ 3. Binding   │  finds import/export/shared names between regions,
 │    Resolver  │  builds the cross-region dependency graph
 └──────┬───────┘
        │ execution plan (DAG of region jobs)
 ┌──────▼───────┐
 │ 4. Runner    │  dispatches each region to a backend tier (§5)
 │    Scheduler │
 └──────┬───────┘
        │ Meld Values over the bridge (§6)
 ┌──────▼───────┐
 │ 5. Backends  │  in-proc embed │ worker process │ Wasm │ Truffle │ toolchain
 └──────────────┘
```

### 3.1 Switching mechanisms (the user-visible syntax)

Proposed (design, not final). Use several; they all lower to the same region model.

File-level declaration (Racket-style): #meld python as the default language.
Block switch: @rust { ... } / @sql ``` ... ```  for multi-line regions.
Inline switch: ⟦js: items.map(x => x*2)⟧ for expression islands inside another language's code.
Fence switch for literate files: Markdown-style ```lang.
Auto-detect fallback: classify an untagged region by shebang, file extension, and token/grammar heuristics. Always report confidence; never silently guess on low confidence (flag it).
### 3.2 The hard parsing problem: where does a region end?

The nested language has no idea about Meld's closing marker, and Meld doesn't know the nested language's string/comment rules (a } inside a Python string).

Plan: each Lang Pack ships an end scanner, a small function find_region_end(text, start, marker) -> offset that understands that language's strings, comments, brace/indent rules. Fallbacks, in order:

Pack-provided scanner (preferred).
Tree-sitter-guided: parse greedily, take the balanced node containing the start.
Unambiguous raw delimiters (e.g. @rust#"... "# with a user-chosen hash count, like Rust raw strings).

Judgment call: ship delimiter-based switching first (Phase 1), grammar-level composition later. Delimiters make the splitter deterministic and cheap. True mid-parse composition (Raku/SGLR style) is Phase 6 research.

### 3.3 Two parsing strategies (decide by measurement)
A. Island parsing with tree-sitter (default, Phases 1–5). Outer splitter + per-region tree-sitter parse via ranges/injections. Wide language coverage immediately (hundreds of grammars exist), incremental, editor-friendly. Weakness: composition is shallow; no cross-language grammar rules.
B. Composed grammars via SGLR/SDF3 (research track, Phase 6). Gives real grammar composition and ambiguity handling. Weakness: you must express each language in SDF3 (few exist); slower; smaller ecosystem.

Decision rule: stay on A unless a concrete user need (e.g. a language embedded syntactically inside another's expression grammar) fails with islands.

## 4. Lang Pack contract (how "any language" scales)

A Lang Pack is a directory:

```text
packs/<lang-id>/
  pack.toml          # id, aliases, extensions, version range, tier, capabilities
  grammar/           # tree-sitter grammar (or reference to published one)
  region_end.*       # end scanner (WASM-compiled or built-in)
  runner.toml        # how to execute (tier, command, protocol, sandbox profile)
  marshal.*          # Meld Value <-> native value converters
  toolchain.toml     # detect/install hints (e.g. needs rustc >= x)
  tests/             # conformance cases (§8)
```

Rules:

Packs are data + small plugins, ideally sandboxed (Wasm) so a third-party pack can't run arbitrary code at load time.
A pack generator bootstraps a minimal pack (syntax-only) automatically from any published tree-sitter grammar. Syntax-only packs give highlighting/structure; execution tiers get added per language. This is the cheap route to a long tail.
Packs declare a tier (§5) and a conformance level: L0 syntax-only, L1 executes statements, L2 values in/out, L3 full bridging (handles/callbacks).
## 5. Execution tiers

Not every language can be embedded. Use tiers; each pack picks the best available.

| Tier | Mechanism | Fits | Cost |
| --- | --- | --- | --- |
| T1 | In-process embed via the language's embedding API (e.g. Python, Lua, a JS engine) | scripting languages with embedding APIs | fast bridge, fragile ABI, crash risk |
| T2 | Worker process speaking a Meld protocol (JSON-RPC or binary over stdio/socket), like notebook kernels | anything with a REPL or script runner | safest, slowest, universal |
| T3 | Compile to Wasm component with WIT bindings | Rust, C/C++, Go-ish, Zig, etc. | strong isolation, typed contract, copying at boundary |
| T4 | GraalVM/Truffle backend | languages implemented on Truffle | shared-memory interop, heavy dependency |
| T5 | Toolchain shell-out (compile region to a shared lib/executable, run, capture) | compiled languages without Wasm story | slow cold start |

Judgment call: T2 is the default universal tier. It is the only one that works for "any language" with a runnable CLI. T1/T3/T4 are optimizations per language. Build T2 first.

Honesty note: a language with no way to run it (no interpreter/compiler, proprietary, GUI-only) can only reach L0 (syntax-only). State this plainly in docs.
## 6. Meld Value Model (cross-language data)

A small universal type system, modeled on WIT's high-level types and Truffle's idea of a standardized interop message set.

Types: null, bool, int (arbitrary precision, with declared width on narrowing), float (f64), string (UTF-8), bytes, list<T>, map<string,V>, record{...}, variant, option<T>, result<T,E>, handle<R> (opaque resource), fn (callable handle).

Rules:

Copy by value across boundaries by default (matches Component Model: separate memories, copy at the interface). Large data: pass bytes/shared-buffer handle.
Handles for non-copyable things (DB connections, objects). Owned/borrowed semantics borrowed from WIT resources; drop on region exit.
Errors cross the bridge as result / structured exception, never as raw host exceptions.
Numeric edge cases documented, not hidden: int overflow, NaN, float→int, big ints into 32-bit languages raise a typed error.
Interface description: a .wit-like IDL (reuse WIT directly if possible, judgment call: reuse over inventing).

Scoping: each region has a private namespace. Sharing is explicit: export x / import y from <region>. A shared "bridge namespace" holds exported values. No implicit global leakage.

## 7. Tooling
Language server: one Meld LSP that creates a virtual document per region and forwards to that language's own LSP, mapping positions back via source maps. (Virtual-document forwarding is the standard approach for embedded-language editing; verify per editor during Phase 5.)
Highlighting: tree-sitter injection queries per pack.
Formatter: run each region through its language's formatter; Meld only formats the glue.
Error mapping: every runtime/parse error carries a source-map entry back to the .meld line/col and region language.
CLI: meld run file.meld, meld check, meld fmt, meld pack new <lang>, meld doctor (detect toolchains).
REPL: language-switching shell (Truffle's polyglot shell has Alt+L to switch language, a good UX precedent).
## 8. Testing strategy
Pack conformance suite: same behavioral cases run against every pack at its declared level (hello, arithmetic, strings, list round-trip, error propagation, handle lifetime).
Round-trip matrix: for N languages, test A→B value passing for all pairs at L2 (generated, not hand-written).
Splitter fuzzing: adversarial inputs where the closing marker appears inside strings/comments/heredocs of each language.
Golden source-map tests for error positions.
Cold-start/latency benchmarks per tier so regressions show.
## 9. Security model

Running every language means running arbitrary code from arbitrary toolchains.

Default: no network, no filesystem outside a project dir, per-region capability grants in pack.toml/file header.
T3 (Wasm) gives real sandboxing; T1/T2/T5 need OS-level confinement (container/seccomp/job objects), call it out per tier.
The `meld` binary never auto-installs toolchains; `meld doctor` reports, doesn't install. The standalone installers install missing runtimes only when the user runs them, with `--skip-dependencies`/`-SkipDependencies` to opt out.
Pack supply chain: signed packs, pinned versions, reproducible grammar builds.
## 10. Implementation language and stack (judgment call)

Rust for the core: tree-sitter has a first-class Rust binding, wasmtime provides the Component Model runtime, memory safety suits a tool that hosts untrusted code. Alternatives considered: Go (pure-Go tree-sitter port exists, simpler concurrency, weaker Wasm component story), Zig (small, but thinner ecosystem). Revisit only if a blocker appears.

Key dependencies: tree-sitter, wasmtime (+ component model), a JSON-RPC lib, tower-lsp (or equivalent) for the language server.

## 11. Phased roadmap

| Phase | Deliverable | Done when |
| --- | --- | --- |
| 0. Spec | Syntax spec (§3.1), Value Model spec (§6), Lang Pack spec (§4), protocol spec for T2 | Specs reviewed; 3 example .meld files hand-written |
| 1. Front end | Region splitter + tree-sitter syntax layer; meld check; highlighting for ≥10 languages via auto-generated syntax-only packs | Splitter fuzz suite passes; highlights mixed file correctly |
| 2. T2 runner | Worker protocol + Python and JavaScript packs (L1) | meld run executes a Python+JS file, stdout merged in order |
| 3. Bridge | Value Model, import/export, errors; L2 for Python, JS, Lua | Round-trip matrix green for those 3 |
| 4. Compiled tier | T3 (Rust/C → Wasm component with WIT) and T5 fallback | Rust function called from Python with typed args |
| 5. Tooling | LSP with virtual documents, formatter, REPL language switch | Editor completion works inside a non-default-language region |
| 6. Composition research | Prototype B (SGLR/SDF3 or Raku-slang-style grammar switch) on one real embedding case that islands can't handle | Written go/no-go decision with benchmarks |
| 7. Ecosystem | Pack registry, signing, pack generator polish, docs | A third party adds a language without touching core |

## 12. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| "Any language" is unbounded | Open pack contract; syntax-only tier L0 for the long tail (§4) |
| Region-end ambiguity | Per-language end scanners + raw delimiters (§3.2) |
| Semantic mismatch (GC, threads, async, exceptions, numerics) | Copy-by-value default, typed errors, documented numeric rules, no shared mutable state by default |
| Cross-boundary performance | Tiering; batch calls; shared-buffer handles for bulk data |
| Debugging multi-language stacks | Source maps first; unified debugger only after Phase 5 |
| Toolchain/version sprawl | meld doctor, per-pack version ranges, containerized runners |
| Security of running arbitrary code | §9 sandboxing, no implicit capabilities |
| Scope creep into "new general-purpose language" | Meld is glue + orchestration; no new semantics for the languages it hosts |

## 13. Open questions (decided by default, revisable)
Is Meld a language (own syntax and semantics for glue) or a host/format (just region switching)? Default: thin language, glue limited to import/export/control-of-order. Revisit after Phase 3.
Reuse WIT as the IDL vs own format? Default: reuse WIT.
Is in-order sequential execution enough, or regions run concurrently (DAG)? Default: sequential by file order, with opt-in async regions after Phase 3.
File extension and branding. Default .meld.

Blockers: none identified at plan stage.

## 14. Sources
Tree-sitter multi-language documents: https://tree-sitter.github.io/tree-sitter/using-parsers#multi-language-documents
ast-grep language injection: https://ast-grep.github.io/advanced/language-injection.html
gotreesitter (injection parsing, 206 grammars): https://pkg.go.dev/github.com/odvcencio/gotreesitter
GraalVM polyglot programming: https://docs.oracle.com/en/graalvm/jdk/17/docs/reference-manual/polyglot-programming/
Java on Truffle interop: https://docs.oracle.com/en/graalvm/enterprise/21/docs/reference-manual/java-on-truffle/interoperability
WebAssembly Component Model / WIT: https://component-model.bytecodealliance.org/design/wit.html
WIT spec: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
Bytecode Alliance roadmap: https://bytecodealliance.org/articles/webassembly-the-updated-roadmap-for-developers
Racket creating languages: https://docs.racket-lang.org/guide/languages.html
Raku slangs/braid: https://gist.github.com/raiph/849a4a9d8875542fb86df2b2eda89296
Raku slangs how-to: https://mouq.github.io/slangs.html
SDF3 overview: https://link.springer.com/doi/10.1007/978-3-030-58768-0_1
SGLR parsing thesis (Spoofax/JSGLR2): https://repository.tudelft.nl/file/File_0d239cac-92d0-4eca-979f-51c0eed00bff
Embedded-language IDE editing with SGLR: https://researchr.org/publication/KatsJNV09

Claims in §2 come directly from these pages. Everything in §3–§11 is design proposal, not sourced fact. Re-verify library capabilities and versions before Phase 0 ends.
