#!/usr/bin/env python3
"""Phase 6 benchmark: generic brace scanner vs composed grammar switching.

Run:
    python3 research/composition/benchmark.py
    python3 research/composition/benchmark.py --reps 500

WHAT IS MEASURED
----------------
For every generated case we know, by construction, the byte offset of the real
closing `}` of the `@ruby { ... }` block. We ask two parsers for that offset:

  1. `reference_generic_end` — a literal Python transliteration of
     `crates/meld-core/src/splitter.rs::BalancedBraceScanner::find_end`
     (lines 55-130, Phase 1 snapshot). This is the Phase 1 default fallback.
  2. `composed_end` — Prototype B from prototype.py, a deterministic
     scannerless-style state machine that switches between a Ruby host "slang"
     and a heredoc "slang".

We measure correctness (returned offset == intended offset) and wall-clock
time over the whole corpus, repeated `--reps` times.

JUDGMENT CALL: correctness is judged only on region-end discovery, not on
producing a full syntax tree. That is the exact responsibility split in
project.md §3.2: the splitter finds the region, tree-sitter parses inside it.
A composed grammar would additionally change what happens *inside* the region;
we do not model that here, and README.md says so.

JUDGMENT CALL: the corpus is generated deterministically (fixed seeds, fixed
matrices) so the JSON result is reproducible. No network and no third-party
packages are used.
"""

from __future__ import annotations

import argparse
import json
import platform
import random
import sys
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

from prototype import composed_end, generic_brace_end

HERE = Path(__file__).resolve().parent
RESULTS_PATH = HERE / "benchmark-results.json"

NL = 0x0A
BS = 0x5C


# ---------------------------------------------------------------------------
# 1. Independent copy of the Rust generic scanner (drift-checked in main)
# ---------------------------------------------------------------------------
def reference_generic_end(source: bytes, body_start: int):
    """Verbatim transliteration of splitter.rs lines 55-130 (Phase 1 snapshot).

    JUDGMENT CALL: this copy lives in benchmark.py rather than being imported
    from prototype.py so that the benchmark file is self-contained and so that
    an accidental "improvement" to prototype.py cannot silently change what
    "Phase 1 behaviour" means. `main()` asserts both copies agree on the whole
    corpus before any numbers are published.
    """
    b = source
    i = body_start
    depth = 1
    quote = None
    line_comment = False
    block_comment = False
    while i < len(b):
        c = b[i]
        if line_comment:
            if c == NL:
                line_comment = False
            i += 1
            continue
        if block_comment:
            if c == ord("*") and i + 1 < len(b) and b[i + 1] == ord("/"):
                block_comment = False
                i += 2
            else:
                i += 1
            continue
        if quote is not None:
            if c == BS:
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in (ord('"'), ord("'"), ord("`")):
            quote = c
        elif c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("/"):
            line_comment = True
            i += 2
            continue
        elif c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("*"):
            block_comment = True
            i += 2
            continue
        elif c == ord("#"):
            if i == 0 or b[i - 1] in (0x20, 0x09, 0x0A, 0x0C, 0x0D):
                line_comment = True
            i += 1
            continue
        elif c == ord("{"):
            depth += 1
        elif c == ord("}"):
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


# ---------------------------------------------------------------------------
# 2. Case model
# ---------------------------------------------------------------------------
@dataclass
class Case:
    name: str
    category: str
    source: bytes
    body_start: int
    intended_end: int
    note: str
    expect_generic_ok: bool


def mk(
    name: str,
    category: str,
    body: str,
    note: str,
    expect_generic_ok: bool,
    prefix: str = "",
    lang: str = "ruby",
    suffix: str = "\n",
) -> Case:
    """Build a full source with a known closing-brace offset."""
    open_marker = f"@{lang} {{".encode("ascii")
    pre = prefix.encode("ascii")
    body_b = body.encode("ascii")
    source = pre + open_marker + body_b + b"}" + suffix.encode("ascii")
    body_start = len(pre) + len(open_marker)
    intended = body_start + len(body_b)
    assert source[intended : intended + 1] == b"}", (name, intended)
    return Case(name, category, source, body_start, intended, note, expect_generic_ok)


def heredoc_opener(prefix: str, term: str) -> str:
    """JUDGMENT CALL: quoted heredoc delimiters repeat the quote on both sides."""
    if prefix in ('<<"', "<<'"):
        return prefix + term + prefix[2]
    return prefix + term


# ---------------------------------------------------------------------------
# 3. Corpus generators
# ---------------------------------------------------------------------------
def gen_benign_simple(n: int) -> list[Case]:
    """Strings, comments and balanced hashes — what islands handle well."""
    patterns = [
        ' x = "brace } in string"; y = { a: 1 }; ',
        " x = 'brace } in string' # trailing } comment\n y = 2 ",
        " /* comment with } and { */ x = 1; ",
        ' h = {a: "}", b: \'{\'}; z = 3 ',
        " x = `shell } cmd`; y = 4 ",
        ' a = {n: {"m": "}"}}; b = 5 ',
        " # } comment at line start\n z = {q: 1} ",
        ' s = "escaped \\" } quote"; t = {\'k\': \'}\'} ',
    ]
    out = []
    for k in range(n):
        p = patterns[k % len(patterns)]
        body = f" i{k} = {k}; {p}" if k % 3 == 0 else p
        out.append(
            mk(
                f"benign-simple-{k:03d}",
                "benign_simple",
                body,
                "balanced braces hidden in strings/comments; Phase 1 handles this",
                expect_generic_ok=True,
            )
        )
    return out


def gen_adversarial_heredoc(n: int) -> list[Case]:
    """The core failure: a `}` inside heredoc data."""
    # (opener prefix, terminator may be indented)
    openers = [
        ("<<~", True),
        ("<<-", True),
        ("<<", False),
        ('<<"', False),
        ("<<'", False),
    ]
    terms = ["SQL", "HTML", "EOS", "TEXT", "DATA", "JSON"]
    out = []
    for k in range(n):
        prefix, indent_ok = openers[k % len(openers)]
        term = terms[(k // len(openers)) % len(terms)]
        op = heredoc_opener(prefix, term)
        nbraces = 1 + (k % 3)
        content = "  SELECT " + "} " * nbraces + "FROM t"
        term_line = ("  " + term) if indent_ok else term
        body = f" html = {op}\n{content}\n{term_line}\nputs html "
        out.append(
            mk(
                f"adv-heredoc-{k:03d}",
                "adversarial_heredoc",
                body,
                f"opener={op}, {nbraces} brace(s) in heredoc data",
                expect_generic_ok=False,
            )
        )
    return out


def gen_adversarial_lookalike(n: int) -> list[Case]:
    """Near-miss terminators must not end the heredoc early."""
    openers = ["<<SQL", '<<"SQL"', "<<'SQL'", "<<-SQL", "<<~SQL"]
    lookalikes = ["SQLx", "sql", "SQL;", "SQL_", "XSQL", "SQL1"]
    out = []
    for k in range(n):
        op = openers[k % len(openers)]
        # JUDGMENT CALL: choose a lookalike that is safe for both column-zero
        # and indented-terminator openers. ` SQL ` is deliberately excluded:
        # it WOULD correctly terminate a `<<~SQL` heredoc.
        look = lookalikes[k % len(lookalikes)]
        body = (
            f" html = {op}\n"
            "  SELECT } FROM t\n"
            f"{look}\n"
            f"{'  SQL' if op in ('<<-SQL', '<<~SQL') else 'SQL'}\n"
            "puts html "
        )
        out.append(
            mk(
                f"adv-lookalike-{k:03d}",
                "adversarial_lookalike",
                body,
                f"near-miss line {look!r} before real terminator",
                expect_generic_ok=False,
            )
        )
    return out


def gen_benign_heredoc_no_brace(n: int) -> list[Case]:
    """Heredoc with no stray brace: generic scanner gets lucky."""
    openers = [("<<~", True), ("<<-", True), ("<<", False), ('<<"', False)]
    out = []
    for k in range(n):
        prefix, indent_ok = openers[k % len(openers)]
        op = heredoc_opener(prefix, "SQL")
        term_line = "  SQL" if indent_ok else "SQL"
        body = f" html = {op}\n  SELECT 1 FROM t\n{term_line}\nputs html "
        out.append(
            mk(
                f"benign-heredoc-{k:03d}",
                "benign_heredoc_no_brace",
                body,
                "heredoc present but no '}' in its data; generic scanner happens to work",
                expect_generic_ok=True,
            )
        )
    return out


def gen_adversarial_multiple(n: int) -> list[Case]:
    """Two heredocs on one block test the pending-heredoc queue."""
    out = []
    for k in range(n):
        a, bb = ("A", "B") if k % 2 == 0 else ("HTML", "XML")
        body = (
            f" html = <<~{a}\n"
            "  <div> } </div>\n"
            f"{a}\n"
            f" xml = <<~{bb}\n"
            "  <x> } </x>\n"
            f"{bb}\n"
            "puts html, xml "
        )
        out.append(
            mk(
                f"adv-multi-{k:03d}",
                "adversarial_multiple_heredocs",
                body,
                f"sequenced heredocs {a} then {bb}",
                expect_generic_ok=False,
            )
        )
    return out


def gen_fuzz_benign(n: int) -> list[Case]:
    """Deterministic random polyglot noise, no heredocs."""
    rng = random.Random(0xC0FFEE)
    frags = [
        ' x = "a}b" ',
        " y = 'c{d' ",
        " # comment } here\n",
        " /* c } */ ",
        " h = {a: 1} ",
        " z = 2 ",
        " `echo }` ",
        ' q = {"}": "{"} ',
    ]
    out = []
    for k in range(n):
        body = "".join(rng.choice(frags) for _ in range(rng.randint(3, 6)))
        out.append(
            mk(
                f"fuzz-benign-{k:03d}",
                "fuzz_benign",
                body,
                "seeded random strings/comments/braces, no heredoc",
                expect_generic_ok=True,
            )
        )
    return out


def gen_nested_blocks(n: int) -> list[Case]:
    """Ruby blocks and hashes with braces hidden in literals."""
    out = []
    for k in range(n):
        body = (
            f" v{k} = [1, 2].map {{ |x| x * {k + 1} }}; # }} comment\n"
            ' h = { k: "}", j: \'{\' }; '
        )
        out.append(
            mk(
                f"nested-blocks-{k:03d}",
                "nested_blocks",
                body,
                "nested braces in blocks/hashes, all balanced outside literals",
                expect_generic_ok=True,
            )
        )
    return out


def build_corpus() -> list[Case]:
    corpus = []
    corpus += gen_benign_simple(40)
    corpus += gen_adversarial_heredoc(60)
    corpus += gen_adversarial_lookalike(30)
    corpus += gen_benign_heredoc_no_brace(30)
    corpus += gen_adversarial_multiple(20)
    corpus += gen_fuzz_benign(40)
    corpus += gen_nested_blocks(20)
    assert len(corpus) >= 200, len(corpus)
    names = [c.name for c in corpus]
    assert len(names) == len(set(names)), "duplicate case names"
    return corpus


# ---------------------------------------------------------------------------
# 4. Measurement
# ---------------------------------------------------------------------------
def score(corpus: list[Case]) -> list[dict]:
    rows = []
    for c in corpus:
        g = reference_generic_end(c.source, c.body_start)
        p = composed_end(c.source, c.body_start)
        rows.append(
            {
                "name": c.name,
                "category": c.category,
                "intended_end": c.intended_end,
                "generic_end": g,
                "composed_end": p,
                "generic_ok": g == c.intended_end,
                "composed_ok": p == c.intended_end,
                "expect_generic_ok": c.expect_generic_ok,
                "note": c.note,
            }
        )
    return rows


def time_all(fn, corpus: list[Case], reps: int) -> dict:
    """Time `fn` over the whole corpus, `reps` times."""
    t0 = time.perf_counter_ns()
    calls = 0
    for _ in range(reps):
        for c in corpus:
            fn(c.source, c.body_start)
            calls += 1
    ns = time.perf_counter_ns() - t0
    return {
        "calls": calls,
        "total_ms": round(ns / 1_000_000, 3),
        "us_per_call": round(ns / calls / 1_000, 4),
        "ns_per_call": round(ns / calls, 1),
    }


def aggregate(rows: list[dict]) -> dict:
    def block(rowset):
        return {
            "cases": len(rowset),
            "generic_correct": sum(r["generic_ok"] for r in rowset),
            "composed_correct": sum(r["composed_ok"] for r in rowset),
            "generic_accuracy_pct": round(
                100 * sum(r["generic_ok"] for r in rowset) / len(rowset), 2
            )
            if rowset
            else 0.0,
            "composed_accuracy_pct": round(
                100 * sum(r["composed_ok"] for r in rowset) / len(rowset), 2
            )
            if rowset
            else 0.0,
        }

    by_cat = {}
    for cat in sorted({r["category"] for r in rows}):
        by_cat[cat] = block([r for r in rows if r["category"] == cat])

    mismatches = [
        {
            "name": r["name"],
            "category": r["category"],
            "expected_generic_ok": r["expect_generic_ok"],
            "actual_generic_ok": r["generic_ok"],
        }
        for r in rows
        if r["generic_ok"] != r["expect_generic_ok"]
    ]

    return {"overall": block(rows), "by_category": by_cat, "expectation_mismatches": mismatches}


def print_report(results: dict) -> None:
    meta = results["meta"]
    agg = results["correctness"]
    tm = results["timing"]
    o = agg["overall"]

    print("=" * 78)
    print("PHASE 6 — COMPOSITION BENCHMARK")
    print("=" * 78)
    print(f"cases={meta['corpus_size']}  reps={meta['reps']}  "
          f"parses_per_parser={tm['generic']['calls']}")
    print(f"python={meta['python']}")
    print()
    print("CORRECTNESS (returned end offset == intended end offset)")
    print("-" * 78)
    print(f"{'category':34s} {'n':>4s} {'generic':>16s} {'composed':>16s}")
    for cat, d in agg["by_category"].items():
        print(
            f"{cat:34s} {d['cases']:4d} "
            f"{d['generic_correct']:5d}/{d['cases']:<4d} "
            f"({d['generic_accuracy_pct']:6.2f}%) "
            f"{d['composed_correct']:5d}/{d['cases']:<4d} "
            f"({d['composed_accuracy_pct']:6.2f}%)"
        )
    print("-" * 78)
    print(
        f"{'OVERALL':34s} {o['cases']:4d} "
        f"{o['generic_correct']:5d}/{o['cases']:<4d} "
        f"({o['generic_accuracy_pct']:6.2f}%) "
        f"{o['composed_correct']:5d}/{o['cases']:<4d} "
        f"({o['composed_accuracy_pct']:6.2f}%)"
    )
    print()
    print("TIMING (all cases, all reps)")
    print("-" * 78)
    print(f"generic scanner : {tm['generic']['total_ms']:10.3f} ms total, "
          f"{tm['generic']['us_per_call']:8.4f} us/call")
    print(f"composed parser : {tm['composed']['total_ms']:10.3f} ms total, "
          f"{tm['composed']['us_per_call']:8.4f} us/call")
    print(f"slowdown factor : {tm['slowdown']:10.2f}x  "
          f"(composed / generic, us/call)")
    print()
    print(f"expectation mismatches: {len(agg['expectation_mismatches'])} "
          "(cases where generic did better/worse than its category label)")
    for m in agg["expectation_mismatches"][:10]:
        print(f"  {m['name']}: expected_generic_ok={m['expected_generic_ok']} "
              f"actual={m['actual_generic_ok']}")
    print(f"results written to: {meta['results_path']}")
    print("=" * 78)


# ---------------------------------------------------------------------------
# 5. Main
# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--reps", type=int, default=200,
                    help="number of full-corpus timing repetitions (default: 200)")
    args = ap.parse_args()
    if args.reps < 1:
        ap.error("--reps must be >= 1")

    corpus = build_corpus()

    # JUDGMENT CALL: publish no numbers until the two generic-scanner copies
    # are proven identical on every generated input. This guards the baseline.
    drift = [
        c.name
        for c in corpus
        if generic_brace_end(c.source, c.body_start)
        != reference_generic_end(c.source, c.body_start)
    ]
    if drift:
        print(f"FATAL: generic scanner copies disagree on {len(drift)} cases: {drift[:5]}",
              file=sys.stderr)
        return 2

    correctness = score(corpus)

    # Warm up both parsers so import/bytecode effects are not charged.
    for c in corpus[:16]:
        reference_generic_end(c.source, c.body_start)
        composed_end(c.source, c.body_start)

    t_generic = time_all(reference_generic_end, corpus, args.reps)
    t_composed = time_all(composed_end, corpus, args.reps)
    slowdown = round(
        t_composed["us_per_call"] / t_generic["us_per_call"], 3
    ) if t_generic["us_per_call"] else None

    results = {
        "meta": {
            "generated_at_utc": datetime.now(timezone.utc).isoformat(),
            "python": platform.python_version(),
            "corpus_size": len(corpus),
            "reps": args.reps,
            "results_path": str(RESULTS_PATH),
            "scanner_source": "crates/meld-core/src/splitter.rs:55-130 "
                              "(BalancedBraceScanner::find_end)",
            "composed_source": "research/composition/prototype.py::composed_end",
            "drift_check": "both generic scanner copies agree on all cases",
        },
        "correctness": aggregate(correctness),
        "timing": {
            "generic": t_generic,
            "composed": t_composed,
            "slowdown": slowdown,
        },
        "cases": correctness,
    }

    RESULTS_PATH.write_text(json.dumps(results, indent=2, sort_keys=True) + "\n",
                            encoding="utf-8")
    print_report(results)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
