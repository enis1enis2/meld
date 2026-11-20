# Phase 6 status — composition research

## Done
- Prototype B and its benchmark live entirely under `research/composition/`:
  - `prototype.py` — scannerless grammar-stack parser and failure demo;
  - `benchmark.py` — 240 deterministic cases, correctness and timing, drift
    check, JSON output;
  - `benchmark-results.json` — generated raw results;
  - `README.md` — the written go/no-go decision.
- Researched embedding case: a Ruby heredoc whose data contains `}` breaks the
  generic balanced-brace scanner but is handled correctly by a grammar-stack
  scanner that switches into the heredoc slang at the opener and back at the
  terminator.

## Measured results
- Generic scanner: **130/240 (54.17%)** correct; it failed **110/110** cases
  where a heredoc hid a brace and passed 130/130 benign cases.
- Composed scanner: **240/240 (100%)** correct.
- Timing over 48,000 parses: 720.010 ms vs 1465.100 ms, a **2.04x** slowdown.
  Both implementations are Python, so this is algorithmic overhead, not an
  SGLR benchmark. The corpus deliberately over-weights heredocs; 54.17% is not
  a production failure-rate estimate.
- Expectation mismatches: 0. Two independent copies of the generic scanner are
  asserted identical before any number is published.

## Decision
**NO-GO** on adopting SGLR/SDF3 or a full Raku-slang composition stack now.
**GO** on targeted, pack-owned end scanners and on raw hash delimiters as the
collision-free escape hatch.

The plan's decision rule (`project.md` §3.3) is "stay on islands unless a
concrete user need fails with islands". The measured failure is in one default
scanner, not in cross-language grammar composition: `EndScanner::find_end` is
the documented hook, raw delimiters already avoid the case, and tree-sitter
parses the correctly split body normally. No case was found where the region
boundary itself requires two grammars to be simultaneously active.

## Judgment calls
- The prototype holds a stack of active grammars but is deterministic and
  hand-written; it is not GLR and does not preserve ambiguity.
- The benchmark compares region-end discovery only; neither parser builds a
  syntax tree.
- SDF3's ecosystem tax conflicts with "adding a language costs a pack": few
  languages have SDF3 grammars, while tree-sitter already has broad coverage.
- Reopen the NO-GO only for a real cross-grammar boundary case, a scanner
  maintenance bottleneck, raw delimiters proving unacceptable, or a maintained
  SDF3 grammar set appearing.

## Blockers
None. The research did not modify the Rust core or install packages.

## Done-when assessment
"Written go/no-go decision with benchmarks": satisfied by
`research/composition/README.md` and the reproducible benchmark data.
