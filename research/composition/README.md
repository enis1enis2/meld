# Phase 6 — Composition research: go/no-go decision

> **FROZEN RESEARCH ARTIFACT — DECISION RECORD, NOT PRODUCTION SOURCE.**
> This directory records the Phase 6 experiment and its outcome. `prototype.py`
> behaviour and `benchmark-results.json` are frozen. Production evolution belongs
> in `crates/`, `packs/*/region_end.toml`, and the fuzz suites; see §10 for the
> shipped design. Do not edit files here to change runtime behaviour.

**Decision: NO-GO on adopting SGLR/SDF3 (or a full Raku "slang"-style composed
grammar stack) for Meld at this time. GO on targeted scanner upgrades inside
the existing island strategy.**

The researched embedding case is a real, reproducible failure of the Phase 1
generic scanner. It is **not** a failure of the island *architecture*: the
remedy already exists in the plan (pack-provided end scanners, §3.2 fallback 1)
and in the syntax (raw hash delimiters, §3.2 fallback 3 / spec §2.1), and is
currently just unwired. Under the decision rule in project.md §3.3 — "stay on A
unless a concrete user need ... fails with islands" — the triggering condition
was not met, because the failure is in one scanner, not in cross-language
grammar composition.

---

## 1. The embedding case

```meld
@ruby { html = <<~SQL
  SELECT } FROM t
SQL
puts html }
```

`SELECT } FROM t` is heredoc **data**. The `}` on that line is not a brace. The
heredoc's end is grammar-dependent: it is the line matching the identifier
named by the `<<~SQL` opener, with indentation rules that differ between
`<<ID`, `<<-ID`, `<<~ID`, `<<"ID"` and `<<'ID'`. The Ruby heredoc grammar knows
this. The host Meld splitter does not, and `BalancedBraceScanner`
(`crates/meld-core/src/splitter.rs:55-130`, **Phase 1 snapshot**) has no heredoc
concept at all.

> **Current-code pointer (review hardening, not Phase 1 evidence):** the shipped
> `EndScanner` contract is `crates/meld-core/src/splitter.rs:63-72`, and the
> registry lookup is `crates/meld-core/src/splitter.rs:404-413`. Pack profiles
> are parsed and resolved in `crates/meld-core/src/region_end_spec.rs:77-128`
> (`RegionEndSpec::parse`), `:142-183` (`scanner`) and `:186-192` (`register`).
> The deterministic registries are built in `crates/meld-cli/src/main.rs:287-308`
> (`scanner_registry`) and `crates/meld-lsp/src/packs.rs:63-80`
> (`scanner_registry`). All other bare `file:line` references in this record
> identify the Phase 1 tree and are retained for historical traceability only.

Measured on the demo in `prototype.py`:

```
body_start          = 7
intended region end = 54  (byte offset of the final '}')
generic brace end   = 31  -> WRONG
composed parser end = 54  -> CORRECT
max grammar stack   = 2
grammar switches:
  queue  SQL   @ byte 15
  enter  SQL   @ byte 22
  exit   SQL   @ byte 43
```

Prototype B is a **deterministic scannerless-style state machine**. It holds a
stack of active grammars, queues the heredoc "slang" at the `<<~SQL` opener,
**switches mid-parse** to that heredoc "slang" at the following newline, ignores
braces while inside it, pops back to the host "slang" on the `SQL` terminator
line, and then finds the real closing `}`. It is "scannerless" only in the
narrow sense that there is no token stream: rules consume bytes directly. It is
hand-written and deterministic; it is not GLR, not SGLR, not SDF3, and does not
preserve ambiguity. Raku "slang" is an analogy for the mid-parse state switch,
not an implementation dependency.

---

## 2. Benchmark method and measured results

Reproduce with:

```sh
cd /mnt/data/supervisor/share/project
python3 research/composition/benchmark.py
```

> **Stale reproduction path (review hardening note):** the absolute `cd` above
> is the pre-relocation Phase 1 worktree. In the current worktree
> (`/mnt/data/supervisor/media/backup_hdd/meld-production-relocation-20261120/share/project`),
> run from the repository root instead. `benchmark-results.json::meta.results_path`
> deliberately retains the stale path because the raw results file is frozen.
> Re-running `benchmark.py` would rewrite that JSON, so it was not run during
> review.

Command actually run for this report: `python3 research/composition/benchmark.py`
(Python 3.12.15, 240 cases, `--reps 200`, 48,000 parses per parser). Raw output
is in `benchmark-results.json`.

### Correctness (returned end offset == intended end offset)

| Category | n | generic | composed |
| --- | ---: | ---: | ---: |
| `adversarial_heredoc` | 60 | 0/60 (0.00%) | 60/60 (100%) |
| `adversarial_lookalike` | 30 | 0/30 (0.00%) | 30/30 (100%) |
| `adversarial_multiple_heredocs` | 20 | 0/20 (0.00%) | 20/20 (100%) |
| `benign_heredoc_no_brace` | 30 | 30/30 (100%) | 30/30 (100%) |
| `benign_simple` | 40 | 40/40 (100%) | 40/40 (100%) |
| `fuzz_benign` | 40 | 40/40 (100%) | 40/40 (100%) |
| `nested_blocks` | 20 | 20/20 (100%) | 20/20 (100%) |
| **OVERALL** | **240** | **130/240 (54.17%)** | **240/240 (100%)** |

Expectation mismatches: **0** — every case behaved exactly as its label
predicted. The generic scanner failed **110/110 (100%)** of the adversarial
heredoc cases and passed **130/130 (100%)** of the benign cases.

### Timing (all 240 cases, 200 repetitions)

| Parser | Total | Per call |
| --- | ---: | ---: |
| Generic scanner (transliteration of `splitter.rs`) | 720.010 ms | 15.0002 µs |
| Composed parser (Prototype B) | 1465.100 ms | 30.5229 µs |
| **Slowdown** | | **2.04x** |

> **2.03 vs 2.04 rounding note:** `benchmark-results.json` stores
> `slowdown = 2.035`, derived from `1465.1 / 720.01 = 2.03479...`. Rounded
> directly to two decimals, the exact ratio is **2.03x**; the historical
> **2.04x** shown in this table and in §5 is a double-rounding artifact from
> rounding the already-rounded `2.035` upward. Both labels describe the same
> frozen measurement. The table and JSON were intentionally left unchanged.

**Honest reading of these numbers:**

- The 54.17% is **not** an estimate of production failure rate. The corpus was
  deliberately built to over-weight heredoc cases so the failure mode could be
  measured. It says "the generic scanner is 0-for-110 when a heredoc hides a
  brace", not "54% of real files are broken".
- Both parsers are pure Python, so the 2.04x reflects **algorithmic** overhead
  (grammar-object dispatch, per-line heredoc scanning), not Rust-vs-Python.
  It is a **lower bound on composition cost, not a forecast of SGLR cost**.
  Published SGLR/SDF3 implementations are typically far slower than a
  hand-written scanner; no SGLR implementation was available here and none was
  installed. Do not quote "2x" as an SGLR benchmark.
- Neither parser builds a syntax tree. The comparison is strictly about region
  end discovery, which is the splitter's contract in §3.2.
- The benchmark's two independent copies of the generic scanner are asserted
  identical on all 240 cases before any number is published, so the baseline
  cannot drift with edits to the prototype.

---

## 3. What islands (Strategy A) handle well

- Every benign category: braces inside single/double/backtick strings, `//`,
  `/* */` and `#` comments, nested hashes and blocks, and seeded random
  polyglot noise — 130/130 correct.
- Breadth of grammars: hundreds of tree-sitter grammars exist; no per-language
  SDF3 grammar has to be authored. This is the ecosystem advantage §3.3
  identifies and it is decisive for a long tail of languages.
- Incremental reparse and editor tooling (tree-sitter included ranges, virtual
  documents for the LSP) already work in Phases 1 and 5.
- Determinism: with raw hash delimiters (`@ruby#"..."#`) the region end needs
  no language knowledge at all. This escape hatch is already in the spec and
  was never exercised in the benchmark — it would make all 110 adversarial
  cases trivially correct for the *existing* splitter.

## 4. What composition buys

- Correct region ends when a boundary is defined by the embedded language's
  own lexical grammar (heredocs, and by extension indent-sensitive blocks,
  quoted delimiters, Python triple-quoted strings with embedded markers).
  Measured: 110/110 failures recovered.
- A principled place to express **cross-language grammar rules** — e.g. a
  region whose extent is an SQL expression inside a Ruby interpolation. That is
  the case §3.3 explicitly names as the trigger to revisit. **It was not
  demonstrated here.** The heredoc end is knowable from Ruby alone.
- Grammar composition is closed under SDF3, so combined grammars are
  well-defined rather than ad-hoc; ambiguity is preserved instead of being
  silently resolved by scanner heuristics. Real value, but not needed for the
  measured failure.

**Terminology note — Rascal is not SDF3.** Rascal has its own syntax notation
and parser generation; SDF/SDF3 specifications can be converted for use in
Rascal, but that does not make Rascal syntax definitions SDF3 files or make a
Rascal parser an SGLR runtime. No Rascal runtime or Rascal grammar was used in
this experiment. Likewise, the Raku "slang" language in this record is an
analogy for mid-parse grammar switching, not a claim that a Rascal or SDF3
"slang" was adopted. See the
[Rascal/ASF+SDF comparison](https://www.rascal-mpl.org/docs/WhyRascal/CompareWithOtherParadigms/ASFPlusSDF).

## 5. Cost and limitations

- **2.04x on this microbenchmark** (~15 µs extra per region). In absolute terms
  that is negligible next to T2 worker startup or LSP round trips, but the
  number says nothing about a real SGLR runtime.
- **Prototype scope is one embedding case.** It handles Ruby heredoc openers
  (`<<`, `<<-`, `<<~`, `<<"ID"`, `<<'ID'`), terminators with allowed indentation,
  lookalike lines, multiple queued heredocs, and the host's strings/comments.
  It is not a Ruby grammar.
- **Documented blind spots** (all marked `JUDGMENT CALL` inline in
  `prototype.py`): interpolation `#{...}` inside heredocs is opaque; a heredoc
  opener inside a host `#{...}` interpolation is missed; `a <<b` can be a false
  positive (Ruby has the same ambiguity); multibyte grammar tokens need
  explicit char-boundary handling; trailing whitespace on a terminator is
  accepted leniently.
- **The SDF3 ecosystem tax remains:** few languages have SDF3 grammars. Every
  new language would need one before composition could even be used, which
  conflicts with the Lang Pack goal ("adding a language costs a manifest +
  grammar + runner, not a core change") unless "grammar" is redefined to mean
  SDF3 grammar.
- **The actual root cause was wiring in the Phase 1 snapshot.**
  `crates/meld-cli/src/main.rs:30-36` (**Phase 1 snapshot**; current registry:
  `crates/meld-cli/src/main.rs:287-308`) registered `BalancedBraceScanner` for
  every language, and `packs/python/region_end.py` (**Phase 1 snapshot**) stated
  that the pack scanner was only a contract stub. The `EndScanner` trait and the
  lookup in `splitter.rs:371-375` (**Phase 1 snapshot**; current contract
  `splitter.rs:63-72`, lookup `splitter.rs:404-413`, profiles
  `region_end_spec.rs:77-192`) already supported a language-specific scanner; it
  was simply never supplied. `ruby` was not even in the Phase 1 CLI scanner map.

## 6. Why the decision rule is not triggered

project.md §3.3: *"stay on A unless a concrete user need (e.g. a language
embedded syntactically inside another's expression grammar) fails with
islands."*

The heredoc failure is a failure of **one default scanner**, not of islands:

1. `EndScanner::find_end` is the documented hook for exactly this knowledge
   (§3.2 fallback 1). A heredoc-aware Ruby scanner is ~60 lines — Prototype B's
   `HeredocGrammar` is already that logic, just expressed as a grammar object.
2. Raw hash delimiters (§3.2 fallback 3) avoid the problem without any scanner.
3. Once the region is split correctly, tree-sitter parses the Ruby body,
   including the heredoc, normally. No cross-language grammar rule is needed.

No case was found where the *region boundary itself* requires two grammars to
be simultaneously active in a way no per-language `find_end` can express.
Therefore Strategy B is not justified now.

## 7. Recommended actions (ordered, all inside Strategy A)

1. **Wire pack scanners into the Rust host** — replace the loop in
   `meld-cli/src/main.rs:30-36` (**Phase 1 snapshot**; shipped as the
   deterministic registry in `meld-cli/src/main.rs:287-308` and
   `meld-lsp/src/packs.rs:63-80`) with real registered `EndScanner`
   implementations (or the Wasm-loaded scanners planned for Phase 4), keeping
   `BalancedBraceScanner` strictly as the fallback. See §10 for what actually
   landed.
2. **Add a Ruby pack with a heredoc-aware `find_region_end`** ported from
   `prototype.py` (`_match_heredoc_opener` + `HeredocGrammar`). Add `ruby` to the
   scanner map.
3. **Extend `tools/fuzz_splitter.py` and `crates/meld-core/tests/splitter_fuzz.rs`**
   with the 110 adversarial heredoc shapes from `benchmark.py`. The current
   1000-case suite passes because it does not generate heredocs.
4. **Surface raw delimiters in diagnostics** — when a block body starts with a
   `<<`-style opener in a language without a registered scanner, have
   `meld check` suggest `@ruby#"..."#`.
5. **Keep this prototype as the reference implementation** for action 2; it is
   the executable specification of the scanner logic.

## 8. Revisit triggers (when to reopen the NO-GO)

Re-run this research and consider SGLR/SDF3 only if at least one of these is
observed:

- A real user case where a region's extent is determined by a rule spanning two
  grammars (not expressible as any single-language `find_end`).
- End scanners become a maintenance bottleneck: more than ~20% of packs need
  bespoke scanner code and the scanners disagree with tree-sitter on the same
  text.
- Raw delimiters prove unacceptable UX for the majority of mixed-language
  files (measurement needed, not an assumption).
- A maintained SDF3 grammar set appears for Meld's top languages, removing the
  ecosystem tax.

## 9. Files

| File | Purpose |
| --- | --- |
| `research/composition/prototype.py` | Prototype B: deterministic scannerless-style state-machine grammar stack, the failure demo, and the transliterated Phase 1 scanner. Run directly for the demo. |
| `research/composition/benchmark.py` | 240-case deterministic corpus, correctness + timing, drift check, JSON writer. |
| `research/composition/benchmark-results.json` | Frozen generated raw results; `meta.results_path` is a stale Phase 1 path (see §2). |
| `research/composition/README.md` | This decision and review-hardening notes. |

All bare `file:line` references in this directory, including
`benchmark-results.json::meta.scanner_source`, are **Phase 1 snapshots**; current
production locations are listed in §1 and §10. At the time of the NO-GO research,
no files outside `research/composition/` were modified and no packages were
installed. The later Phase 6 implementation is recorded in §10.

## 10. Phase 6 resolution (post-decision addendum)

**Current production map (review hardening, not frozen research):** further work
belongs in `crates/meld-core/src/region_end_spec.rs`
(`RegionEndSpec::parse`/`scanner`/`register`), `crates/meld-core/src/splitter.rs`
(`EndScanner` and registry lookup), the CLI/LSP `scanner_registry` builders,
`packs/*/region_end.toml`, and the fuzz suites. The line references below remain
historical Phase 1 snapshots.

The GO decision was implemented without Strategy B. Actions 1–3 of §7 landed as:

- `RegionEnd { body_end, full_end, close_marker }`, `find_region_end`, the
  compatibility `find_end` shim, and `ScannerRegistry` in `meld-core`.
- Six built-in Rust scanners: `brace`, `heredoc`, `indent`, `delimited`,
  `counted`, `template`. The heredoc rules are ported from `prototype.py`,
  including the FIFO queue for multiple openers and opaque heredoc bodies.
- CLI and LSP build registries from core built-ins plus pack `region_end.toml`
  files in deterministic order; 15 production packs and a heredoc fixture pack
  declare profiles.
- Rust and Python fuzz suites now include the adversarial heredoc shapes and
  negative controls; `python3 tools/fuzz_splitter.py` passes 1000 cases.

**JUDGMENT CALL:** §7 anticipated "Wasm-loaded scanners", but the shipped Phase 6
design is data-only: pack metadata names a built-in Rust algorithm. `kind =
"wasm"`, unknown kinds/schemas, and invalid TOML warn and fall back to `brace`;
no pack script or module is loaded, spawned, or compiled. This keeps the L0
promise and preserves `render(doc) == source` with exact byte offsets.

**JUDGMENT CALL:** raw hash delimiters remain the normative escape hatch. Their
branch is parsed before the registry and bypasses every scanner, even when a
pack declares one. Adding `region_end.toml` makes strict packs carry one extra
payload file; the registry owner re-sums and re-signs it, so this change does
not edit `checksums.sha256` or any signature file.

**JUDGMENT CALL:** actions 4 and 5 of §7 stay deferred. The Rust heredoc scanner
is now the executable specification; `prototype.py` is retained as the research
artifact that defined its rules.
