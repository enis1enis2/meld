#!/usr/bin/env python3
"""Prototype B — deterministic scannerless-style state machine for Meld.

WHAT THIS IS
------------
A deliberately small, working demonstration of "Strategy B" from project.md
§3.3: instead of asking a *single* generic brace scanner to find the end of a
`@ruby { ... }` region, we run a deterministic state machine that holds a
*stack of active grammars* and switches the active grammar mid-parse,
analogous to a Raku "slang"/braid.

The one real embedding case researched here is a Ruby heredoc inside a Meld
host block:

    @ruby { html = <<~SQL
      SELECT } FROM t
    SQL
    puts html }

The `}` on the `SELECT } FROM t` line is *literal heredoc data*, not a brace.
The Phase 1 generic scanner (crates/meld-core/src/splitter.rs:55-130,
**Phase 1 snapshot**; current contract `splitter.rs:63-72` and registry lookup
`splitter.rs:404-413`) has no idea what `<<~SQL` means, so it closes the Meld
region at that inner `}`. The heredoc terminator is *grammar-dependent*: it is
the identifier named by the `<<` opener, and its indentation rules depend on
the modifier (`<<ID`, `<<-ID`, `<<~ID`, `<<"ID"`, `<<'ID'`). Only a parser that
is inside Ruby's lexical grammar at that point can know where the heredoc ends.

HONESTY ABOUT THE NAME "SCANNERLESS"
------------------------------------
In the SGLR sense, "scannerless" means there is no separate token stream:
grammar rules are applied directly to the character stream. This prototype is
scannerless-style in *that* narrow sense (we consume UTF-8 bytes directly, no
lexer and no token list). It is NOT SGLR: it is deterministic, hand-written, has
no GLR ambiguity handling, and is not expressed in SDF3. "Slang" is a Raku
analogy, not an SDF3 or Rascal claim: Rascal has its own syntax notation and
parser generation, while SDF/SDF3 specs are a separate formalism that can be
converted for Rascal use. See README.md for why this distinction matters to the
go/no-go decision.

JUDGMENT CALL: implement the prototype in Python, not Rust or SDF3. This is a
research spike whose job is to measure a failure mode and the shape of a fix,
not to ship production code. Python keeps the state machine legible and lets
benchmark.py import it directly. No system packages are used.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

# ---------------------------------------------------------------------------
# 0. Shared byte helpers
# ---------------------------------------------------------------------------
# JUDGMENT CALL: operate on UTF-8 *bytes*, not Python str characters. The spec
# (spec/syntax.md §1) defines byte offsets for all IR positions, and the Rust
# splitter uses `text.as_bytes()`. Using bytes here makes our offsets directly
# comparable to the host implementation. The corpus is ASCII; a production
# version must handle multibyte grammar tokens explicitly (the Rust splitter
# already does this in `next_marker`, which walks char boundaries).

NL = 0x0A
BS = 0x5C  # backslash
QUOTE_BYTES = (0x22, 0x27, 0x60)  # " ' `  (backtick = Ruby shell command)


def _is_ascii_ws(c: int) -> bool:
    """Rust `u8::is_ascii_whitespace`: space, tab, LF, FF, CR."""
    return c in (0x20, 0x09, 0x0A, 0x0C, 0x0D)


def _is_id_byte(c: int) -> bool:
    return (
        ord("0") <= c <= ord("9")
        or ord("a") <= c <= ord("z")
        or ord("A") <= c <= ord("Z")
        or c == ord("_")
    )


def _skip_line(b: bytes, i: int) -> int:
    """JUDGMENT CALL: a comment ends at the newline; consume the newline too."""
    j = b.find(b"\n", i)
    return len(b) if j < 0 else j + 1


def _skip_block_comment(b: bytes, i: int) -> int:
    j = b.find(b"*/", i)
    return len(b) if j < 0 else j + 2


def _skip_quoted(b: bytes, i: int, q: int) -> int:
    """Consume a quoted run starting at the opening quote byte.

    JUDGMENT CALL: backslash escapes are honored for all three quote kinds,
    matching the generic scanner. Ruby interpolation `#{...}` inside a
    double-quoted string is treated as OPAQUE here. This is a known limitation
    (documented in README.md): a heredoc opener inside a `#{}` interpolation
    would be missed. It does not affect the researched embedding case.
    """
    j = i + 1
    n = len(b)
    while j < n:
        if b[j] == BS:
            j += 2
            continue
        if b[j] == q:
            return j + 1
        j += 1
    return n


# ---------------------------------------------------------------------------
# 1. The Phase 1 generic scanner, transliterated verbatim from splitter.rs
# ---------------------------------------------------------------------------
def generic_brace_end(source: bytes, body_start: int) -> Optional[int]:
    """Faithful Python transliteration of `BalancedBraceScanner::find_end`.

    Source: crates/meld-core/src/splitter.rs lines 55-130 (Phase 1 snapshot).
    Kept here so the prototype demo can show the failure side-by-side.
    benchmark.py carries its own copy and cross-checks the two, so this is not
    a hidden dependency.
    """
    b = source
    i = body_start
    depth = 1
    quote: Optional[int] = None
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
        # JUDGMENT CALL: quote detection precedes comment detection. A `//`
        # or `#` inside a quoted string must never start a comment.
        if c in QUOTE_BYTES:
            quote = c
        elif c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("/"):
            line_comment = True
            i += 2
            continue
        elif c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("*"):
            block_comment = True
            i += 2
            continue
        elif c == ord("#") and (i == 0 or _is_ascii_ws(b[i - 1])):
            # JUDGMENT CALL: `#` is a Ruby/Python comment only at line start or
            # after whitespace; otherwise it is an operator (e.g. C preprocessor
            # token in a polyglot file). This heuristic is copied from Rust.
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
# 2. Grammar state
# ---------------------------------------------------------------------------
@dataclass(frozen=True)
class HeredocSpec:
    """The grammar-dependent facts carried by one `<<ID` opener."""

    terminator: bytes
    indent_ok: bool  # `<<-`, `<<~`: terminator may be indented
    squiggly: bool  # `<<~`: content is de-indented (cosmetic for us)
    interpolating: bool  # `<<'ID'` disables #{...} interpolation
    opener_start: int
    opener_end: int


@dataclass
class State:
    source: bytes
    i: int
    depth: int = 1
    grammars: list = field(default_factory=list)
    pending: list = field(default_factory=list)
    end: Optional[int] = None
    switches: list = field(default_factory=list)
    max_stack: int = 1


class Grammar:
    """A "slang" state: one active grammar rule set in the Raku sense.

    JUDGMENT CALL: grammars are objects with a single `consume(state)` method
    rather than a lexical token type. This mirrors Raku's "slang" contract —
    each "slang" owns how to advance over its own syntax and may push/pop other
    "slangs" — while staying small enough to audit in one file. This is an
    analogy for a state-machine transition, not SDF3 or Rascal syntax.
    """

    name = "?"

    def consume(self, st: State) -> None:
        raise NotImplementedError


# ---------------------------------------------------------------------------
# 3. Heredoc opener recognition (the rule the generic scanner lacks)
# ---------------------------------------------------------------------------
def _match_heredoc_opener(b: bytes, i: int) -> Optional[HeredocSpec]:
    """Recognise Ruby's `<<ID`, `<<-ID`, `<<~ID`, `<<"ID"`, `<<'ID'` openers.

    JUDGMENT CALL: require a non-identifier byte immediately before `<<`.
    Without this guard, an ordinary shift expression such as `flags<<mask`
    would be misread as a heredoc opener. With it, `x = <<SQL`, `puts(<<SQL)`
    and `return <<~SQL` are recognised. This heuristic is deliberately narrow:
    it can still produce a false positive on `a <<b` (shift with no space),
    which Ruby itself would also lex as a heredoc opener. Documented in README.
    """
    if b[i : i + 2] != b"<<":
        return None
    if i > 0 and (_is_id_byte(b[i - 1]) or b[i - 1] == ord(".")):
        return None

    j = i + 2
    indent_ok = False
    squiggly = False
    if j < len(b) and b[j] in (ord("-"), ord("~")):
        indent_ok = True
        squiggly = b[j] == ord("~")
        j += 1

    # JUDGMENT CALL: a quoted delimiter still names a plain identifier as the
    # terminator (`<<"SQL"` terminates on a line containing `SQL`). The quote
    # only controls interpolation, not the terminator spelling.
    quote: Optional[int] = None
    if j < len(b) and b[j] in (ord('"'), ord("'")):
        quote = b[j]
        j += 1

    start = j
    while j < len(b) and _is_id_byte(b[j]):
        j += 1
    if j == start:
        return None
    terminator = b[start:j]

    if quote is not None:
        if j >= len(b) or b[j] != quote:
            return None
        j += 1
    # JUDGMENT CALL: reject `<<SQLish` only when a quote was expected? No — we
    # greedily consume all identifier bytes, so `<<SQLish` names terminator
    # `SQLish`. That is exactly Ruby's behaviour; no extra check needed.

    return HeredocSpec(
        terminator=terminator,
        indent_ok=indent_ok,
        squiggly=squiggly,
        interpolating=(quote != ord("'")),
        opener_start=i,
        opener_end=j,
    )


# ---------------------------------------------------------------------------
# 4. The two "slangs"
# ---------------------------------------------------------------------------
class HostGrammar(Grammar):
    """Ruby-like host grammar. Balances braces and dispatches heredocs."""

    name = "ruby-host"

    def consume(self, st: State) -> None:
        b = st.source
        i = st.i
        c = b[i]

        # --- strings are opaque, exactly as in the Phase 1 scanner ---
        if c in QUOTE_BYTES:
            st.i = _skip_quoted(b, i, c)
            return

        # --- comments are opaque, exactly as in the Phase 1 scanner ---
        if c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("/"):
            st.i = _skip_line(b, i + 2)
            return
        if c == ord("/") and i + 1 < len(b) and b[i + 1] == ord("*"):
            st.i = _skip_block_comment(b, i + 2)
            return
        if c == ord("#") and (i == 0 or _is_ascii_ws(b[i - 1])):
            st.i = _skip_line(b, i + 1)
            return

        # --- THE COMPOSITION POINT: heredoc opener switches grammar later ---
        spec = _match_heredoc_opener(b, i)
        if spec is not None:
            # JUDGMENT CALL: in Ruby the opener line continues as host code and
            # the heredoc body begins on the NEXT line. We therefore queue the
            # spec and activate it at the following newline rather than
            # switching immediately. Multiple openers on one line queue in
            # order, matching Ruby's `x = <<A + <<B` semantics.
            st.pending.append(spec)
            st.switches.append(("queue", spec.terminator.decode("ascii"), i))
            st.i = spec.opener_end
            return

        # --- newline is the activation boundary for queued heredocs ---
        if c == NL:
            st.i = i + 1
            if st.pending:
                hd = st.pending.pop(0)
                st.grammars.append(HeredocGrammar(hd))
                st.switches.append(
                    ("enter", hd.terminator.decode("ascii"), i + 1)
                )
                st.max_stack = max(st.max_stack, len(st.grammars))
            return

        # --- brace balance, relative to the Meld block body ---
        if c == ord("{"):
            st.depth += 1
            st.i = i + 1
            return
        if c == ord("}"):
            st.depth -= 1
            st.i = i + 1
            if st.depth == 0:
                # JUDGMENT CALL: record the brace's own offset as the region
                # end. The Rust splitter's `find_end` returns exactly this
                # offset (the close marker is `source[end..end+1]`).
                st.end = i
            return

        st.i = i + 1


class HeredocGrammar(Grammar):
    """The embedded grammar. Inside it, braces and quotes are plain data.

    This is the whole point of composition: the host grammar cannot know these
    rules, and the heredoc grammar cannot know about Meld's `}`. The parser
    keeps both alive and switches between them at the right byte offsets.
    """

    name = "ruby-heredoc"

    def __init__(self, spec: HeredocSpec):
        self.spec = spec

    def consume(self, st: State) -> None:
        b = st.source
        line_end = b.find(b"\n", st.i)
        if line_end < 0:
            line_end = len(b)
        line = b[st.i : line_end]

        # JUDGMENT CALL: trailing horizontal whitespace is ignored when
        # matching a terminator. Ruby tolerates it in practice and being lenient
        # here avoids a silent under-run at the cost of a rare false positive.
        candidate = line.rstrip(b" \t\r")
        if self.spec.indent_ok:
            # JUDGMENT CALL: `<<-` and `<<~` permit an indented terminator.
            # `<<ID` and quoted `<<"ID"`/`<<'ID'` do NOT — for those the
            # terminator must start at column zero. This is a real Ruby rule,
            # not a convenience, and the benchmark generates both shapes.
            candidate = candidate.lstrip(b" \t")

        if candidate == self.spec.terminator:
            # JUDGMENT CALL: consume the terminator line *including* its
            # newline, then pop back to the host "slang". The newline itself is
            # not a host activation point (no pending heredoc is queued on a
            # terminator line), so this is safe.
            st.i = line_end + (1 if line_end < len(b) else 0)
            st.grammars.pop()
            st.switches.append(
                ("exit", self.spec.terminator.decode("ascii"), line_end)
            )
            return

        # JUDGMENT CALL: the entire content line is consumed atomically. We do
        # not interpret `{`, `}`, `"`, `#` or `//` here. For interpolating
        # heredocs a `#{...}` could contain arbitrary Ruby (even another
        # heredoc); we deliberately treat it as opaque because terminator
        # detection only inspects line starts. Documented limitation.
        st.i = line_end + (1 if line_end < len(b) else 0)


# ---------------------------------------------------------------------------
# 5. Driver
# ---------------------------------------------------------------------------
def parse_composed(source: bytes, body_start: int) -> tuple[Optional[int], State]:
    """Run the composed parser; return (region_end, final_state)."""
    st = State(source=source, i=body_start, grammars=[HostGrammar()])

    # JUDGMENT CALL: a defensive progress counter. Every grammar rule must
    # advance `i` by at least one byte; if one does not, fail loudly instead
    # of hanging. Bound is generous relative to input length.
    max_steps = 4 * (len(source) - body_start + 1) + 16
    steps = 0
    while st.i < len(source) and st.end is None:
        before = st.i
        st.grammars[-1].consume(st)
        if st.i <= before and st.end is None:
            raise RuntimeError(
                f"grammar {st.grammars[-1].name!r} made no progress at {before}"
            )
        steps += 1
        if steps > max_steps:
            raise RuntimeError("composed parser exceeded step budget")
    return st.end, st


def composed_end(source: bytes, body_start: int) -> Optional[int]:
    """Convenience wrapper used by benchmark.py (timed hot path)."""
    return parse_composed(source, body_start)[0]


# ---------------------------------------------------------------------------
# 6. Demonstration of the researched failure
# ---------------------------------------------------------------------------
DEMO_SOURCE = (
    b"@ruby { html = <<~SQL\n"
    b"  SELECT } FROM t\n"
    b"SQL\n"
    b"puts html }\n"
)


def _demo() -> None:
    body_start = DEMO_SOURCE.index(b"{") + 1
    intended = DEMO_SOURCE.rindex(b"}")

    g = generic_brace_end(DEMO_SOURCE, body_start)
    c, st = parse_composed(DEMO_SOURCE, body_start)

    print("Prototype B — one real embedding case islands get wrong")
    print("=" * 68)
    print(DEMO_SOURCE.decode("ascii"), end="")
    print("-" * 68)
    print(f"body_start          = {body_start}")
    print(f"intended region end = {intended}  (byte offset of the final '}}')")
    print(f"generic brace end   = {g}  -> {'CORRECT' if g == intended else 'WRONG'}")
    if g is not None:
        print(f"  generic stopped at: {DEMO_SOURCE[g - 12 : g + 13]!r}")
    print(f"composed parser end = {c}  -> {'CORRECT' if c == intended else 'WRONG'}")
    print(f"max grammar stack   = {st.max_stack}")
    print("grammar switches:")
    for kind, term, off in st.switches:
        print(f"  {kind:6s} {term:5s} @ byte {off}")
    print("-" * 68)
    print("JUDGMENT CALL: the generic scanner stopped at the '}' inside the")
    print("heredoc because `<<~SQL` is invisible to it. The composed parser")
    print("entered the heredoc slang at the newline, ignored that '}' as data,")
    print("exited on the `SQL` terminator line, and resumed the host slang.")


if __name__ == "__main__":
    _demo()
