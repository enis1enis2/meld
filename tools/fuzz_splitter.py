#!/usr/bin/env python3
"""Bounded Python reference/differential harness for the Meld splitter.

This is a fast design check, not a substitute for the Rust libFuzzer target in
`fuzz/fuzz_targets/splitter.rs` or for `cargo test`. It reimplements the
split/render algorithm in Python, including data-only scanner profiles, so a
fixed set of 1000 deterministic seeds can be checked without invoking the Rust
toolchain: every seed must render byte-for-byte unchanged and re-split with
identical region count, language, kind and body.

The cargo-fuzz targets remain the authoritative panic/sanitizer fuzzing for the
production splitter; this harness complements them with a bounded smoke and
reference-model check.
"""
from __future__ import annotations

import random
import sys

PARTS = [
    "#meld python\n",
    "@python { s = 'a } b'; print(s) }\n",
    '@python#\" body } inside raw \"#\n',
    '@javascript as j { const x = \"} not a close\"; console.log(x); }\n',
    "⟦python: 1 + 2⟧\n",
    "```python\nvalue = '``` inside a string'\n```\n",
    "plain text with an email a@b.com\n",
    "print('default tail')\n",
    # Phase 6 deterministic scanner cases: a plain brace scan gets these wrong.
    "@ruby { html = <<~SQL\n  SELECT } FROM t\nSQL\nputs html }\n",
    "@python { s = \"\"\"a \" } b\"\"\"; print(s) }\n",
    "@javascript { const t = `a ${ `inner }` } b`; }\n",
    "@lua { local s = [==[ } ]==]; print(s) }\n",
]


def take_id(s: str, i: int) -> tuple[str, int]:
    start = i
    while i < len(s) and (s[i].isalnum() or s[i] in "_-"):
        i += 1
    return s[start:i], i


def skip_spaces(s: str, i: int) -> int:
    while i < len(s) and s[i] in " \t":
        i += 1
    return i


def block_head(s: str, at: int):
    if at >= len(s) or s[at] != "@":
        return None
    lang, i = take_id(s, at + 1)
    if not lang:
        return None
    ident = None
    after = skip_spaces(s, i)
    if s.startswith("as ", after):
        name, nxt = take_id(s, after + 3)
        if not name:
            return None
        ident = name
        i = skip_spaces(s, nxt)
    else:
        i = after
    if i >= len(s):
        return None
    if s[i] == "#":
        start = i
        while i < len(s) and s[i] == "#":
            i += 1
        if i >= len(s) or s[i] != '"':
            return None
        hashes = i - start
        return dict(lang=lang, ident=ident, body=i + 1, close='"' + "#" * hashes)
    if s[i] == "{":
        return dict(lang=lang, ident=ident, body=i + 1, close="}")
    return None


def find_brace_end(s: str, start: int) -> int | None:
    depth = 1
    quote = None
    line_comment = False
    block_comment = False
    i = start
    while i < len(s):
        c = s[i]
        if line_comment:
            if c == "\n":
                line_comment = False
            i += 1
            continue
        if block_comment:
            if c == "*" and i + 1 < len(s) and s[i + 1] == "/":
                block_comment = False
                i += 2
            else:
                i += 1
            continue
        if quote:
            if c == "\\":
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in "\"'`":
            quote = c
        elif c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            line_comment = True
            i += 2
            continue
        elif c == "/" and i + 1 < len(s) and s[i + 1] == "*":
            block_comment = True
            i += 2
            continue
        elif c == "#" and (i == 0 or s[i - 1].isspace()):
            line_comment = True
            i += 1
            continue
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


# ---------------------------------------------------------------------------
# Phase 6: deterministic, data-only scanner profiles. These mirror the Rust
# built-ins selected by region_end.toml; none of them executes pack code.
# ---------------------------------------------------------------------------
def _heredoc_opener(s: str, i: int):
    if not s.startswith("<<", i):
        return None
    if i > 0 and (s[i - 1].isalnum() or s[i - 1] in "_."):
        return None
    j = i + 2
    indent_ok = False
    if j < len(s) and s[j] in "-~":
        indent_ok = True
        j += 1
    quote = None
    if j < len(s) and s[j] in "\"'":
        quote = s[j]
        j += 1
    start = j
    while j < len(s) and (s[j].isalnum() or s[j] == "_"):
        j += 1
    if j == start:
        return None
    term = s[start:j]
    if quote:
        if j >= len(s) or s[j] != quote:
            return None
        j += 1
    return (term, indent_ok, j)


def heredoc_end(s: str, start: int) -> int | None:
    i = start
    depth = 1
    quote = None
    line_comment = False
    block_comment = False
    pending: list[tuple[str, bool, int]] = []
    active: list[tuple[str, bool]] = []
    while i < len(s):
        if active:
            term, indent_ok = active[0]
            end = s.find("\n", i)
            if end < 0:
                end = len(s)
            candidate = s[i:end].rstrip(" \t\r")
            if indent_ok:
                candidate = candidate.lstrip(" \t")
            if candidate == term:
                active.pop(0)
            i = end + 1 if end < len(s) else end
            continue
        if quote is None and not line_comment and not block_comment:
            spec = _heredoc_opener(s, i)
            if spec:
                pending.append(spec)
                i = spec[2]
                continue
            if s[i] == "\n" and pending:
                active = [(t, ok) for t, ok, _ in pending]
                pending = []
                i += 1
                continue
        c = s[i]
        if line_comment:
            if c == "\n":
                line_comment = False
            i += 1
            continue
        if block_comment:
            if c == "*" and i + 1 < len(s) and s[i + 1] == "/":
                block_comment = False
                i += 2
            else:
                i += 1
            continue
        if quote:
            if c == "\\":
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in "\"'`":
            quote = c
        elif c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            line_comment = True
            i += 2
            continue
        elif c == "/" and i + 1 < len(s) and s[i + 1] == "*":
            block_comment = True
            i += 2
            continue
        elif c == "#" and (i == 0 or s[i - 1].isspace()):
            line_comment = True
            i += 1
            continue
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def triple_end(s: str, start: int) -> int | None:
    """Python profile: skip triple-quoted strings and dedented comments."""
    i = start
    depth = 1
    quote = None
    triple = None
    line_comment = False
    while i < len(s):
        c = s[i]
        if line_comment:
            if c == "\n":
                line_comment = False
            i += 1
            continue
        if triple:
            if s.startswith(triple * 3, i):
                triple = None
                i += 3
            else:
                i += 2 if c == "\\" else 1
            continue
        if quote:
            if c == "\\":
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in "\"'" and s.startswith(c * 3, i):
            triple = c
            i += 3
            continue
        if c in "\"'":
            quote = c
        elif c == "#" and (i == 0 or s[i - 1].isspace()):
            line_comment = True
            i += 1
            continue
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def _skip_interpolation(s: str, i: int) -> int:
    depth = 1
    while i < len(s):
        c = s[i]
        if c in "\"'":
            q = c
            i += 1
            while i < len(s):
                if s[i] == "\\":
                    i += 2
                elif s[i] == q:
                    i += 1
                    break
                else:
                    i += 1
            continue
        if c == "`":
            i = _skip_template(s, i)
            continue
        if c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        elif c == "{":
            depth += 1
        i += 1
    return len(s)


def _skip_template(s: str, i: int) -> int:
    i += 1
    while i < len(s):
        if s[i] == "\\":
            i += 2
        elif s[i] == "`":
            return i + 1
        elif s.startswith("${", i):
            i = _skip_interpolation(s, i + 2)
        else:
            i += 1
    return len(s)


def template_end(s: str, start: int) -> int | None:
    i = start
    depth = 1
    while i < len(s):
        if s[i] == "`":
            i = _skip_template(s, i)
            continue
        c = s[i]
        if c in "\"'":
            quote = c
            i += 1
            while i < len(s):
                if s[i] == "\\":
                    i += 2
                elif s[i] == quote:
                    quote = None
                    i += 1
                    break
                else:
                    i += 1
            continue
        if c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            end = s.find("\n", i)
            i = len(s) if end < 0 else end + 1
            continue
        if c == "/" and i + 1 < len(s) and s[i + 1] == "*":
            end = s.find("*/", i + 2)
            i = len(s) if end < 0 else end + 2
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def lua_long(s: str, i: int) -> int | None:
    if s[i] != "[":
        return None
    j = i + 1
    while j < len(s) and s[j] == "=":
        j += 1
    if j >= len(s) or s[j] != "[":
        return None
    close = "]" + s[i + 1:j] + "]"
    end = s.find(close, j + 1)
    return None if end < 0 else end + len(close)


def lua_end(s: str, start: int) -> int | None:
    i = start
    depth = 1
    quote = None
    while i < len(s):
        if quote:
            if s[i] == "\\":
                i += 2
                continue
            if s[i] == quote:
                quote = None
            i += 1
            continue
        end = lua_long(s, i)
        if end is not None:
            i = end
            continue
        c = s[i]
        if c in "\"'":
            quote = c
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


SCANNERS = {
    "ruby": heredoc_end,
    "python": triple_end,
    "javascript": template_end,
    "typescript": template_end,
    "tsx": template_end,
    "lua": lua_end,
}


def next_marker(s: str, start: int) -> int | None:
    i = start
    while i < len(s):
        if s[i] == "@" and block_head(s, i):
            return i
        if s[i] in "⟦⟧":
            return i
        if s[i] == "`" and (i == 0 or s[i - 1] == "\n") and s.startswith("```", i):
            return i
        i += 1
    return None


def split(s: str):
    regions = []
    default = ""
    i = 0
    counter = 0
    if s.startswith("#meld "):
        end = s.find("\n")
        if end == -1:
            end = len(s)
        default = s[6:end].strip()
        close = "\n" if end < len(s) else ""
        full_end = end + 1 if end < len(s) else end
        regions.append(dict(id="header", lang=default, kind="header", body=s[6:end],
                            open="#meld ", close=close, full_start=0, full_end=full_end,
                            confidence=1.0))
        i = full_end
    while i < len(s):
        line_start = i == 0 or s[i - 1] == "\n"
        if s[i] == "`" and line_start and s.startswith("```", i):
            k = i + 3
            lang, k = take_id(s, k)
            while k < len(s) and s[k] in " \t":
                k += 1
            if k >= len(s) or s[k] != "\n":
                raise ValueError(f"malformed fence at {i}")
            body_start = k + 1
            rel = s.find("\n```", body_start)
            if rel < 0:
                raise ValueError(f"unterminated fence at {i}")
            body_end = rel
            full_end = body_end + 4
            if full_end < len(s) and s[full_end] == "\n":
                full_end += 1
            regions.append(dict(id=f"f{counter}", lang=lang or default, kind="fence",
                                body=s[body_start:body_end], open=s[i:body_start],
                                close=s[body_end:full_end], full_start=i,
                                full_end=full_end, confidence=1.0))
            counter += 1
            i = full_end
            continue
        head = block_head(s, i)
        if head:
            if head["close"].startswith('"'):
                rel = s.find(head["close"], head["body"])
                if rel < 0:
                    raise ValueError(f"unterminated raw block at {i}")
                body_end = rel
                full_end = body_end + len(head["close"])
            else:
                finder = SCANNERS.get(head["lang"], find_brace_end)
                body_end = finder(s, head["body"])
                if body_end is None:
                    raise ValueError(f"unterminated block at {i}")
                full_end = body_end + 1
            regions.append(dict(id=head["ident"] or f"r{counter}", lang=head["lang"],
                                kind="block", body=s[head["body"]:body_end],
                                open=s[i:head["body"]], close=s[body_end:full_end],
                                full_start=i, full_end=full_end, confidence=1.0))
            counter += 1
            i = full_end
            continue
        if s[i] == "⟦":
            colon = s.find(":", i + 1)
            if colon < 0:
                raise ValueError(f"inline missing ':' at {i}")
            lang = s[i + 1:colon].strip()
            body_start = colon + 1
            rel = s.find("⟧", body_start)
            if rel < 0:
                raise ValueError(f"unterminated inline at {i}")
            body_end = rel
            full_end = body_end + 1
            regions.append(dict(id=f"i{counter}", lang=lang or default, kind="inline",
                                body=s[body_start:body_end], open=s[i:body_start],
                                close="⟧", full_start=i, full_end=full_end,
                                confidence=1.0))
            counter += 1
            i = full_end
            continue
        end = next_marker(s, i) or len(s)
        body = s[i:end]
        if default:
            lang, conf = default, 1.0
        elif "print(" in body or "def " in body or "import " in body:
            lang, conf = "python", 0.82
        else:
            lang, conf = "text", 0.60
        regions.append(dict(id=f"u{counter}", lang=lang, kind="untagged", body=body,
                            open="", close="", full_start=i, full_end=end,
                            confidence=conf))
        counter += 1
        i = end
    return dict(default=default, regions=regions)


def render(doc):
    return "".join(r["open"] + r["body"] + r["close"] for r in doc["regions"])


def generate(seed: int) -> str:
    rng = random.Random(seed)
    return "".join(rng.choice(PARTS) for _ in range(rng.randint(4, 9)))


def check_scanner_cases() -> None:
    cases = [
        ("@ruby { html = <<~SQL\n  SELECT } FROM t\nSQL\nputs html }\n", "ruby", "SELECT } FROM t"),
        ("@ruby { a = <<A + <<B\n  } first\nA\n  } second\nB\nputs a }\n", "ruby", "} second"),
        ("@python { s = \"\"\"a \" } b\"\"\"; print(s) }\n", "python", "a \" } b"),
        ("@javascript { const t = `a ${ `inner }` } b`; }\n", "javascript", "inner }"),
        ("@lua { local s = [==[ } ]==]; print(s) }\n", "lua", "[==[ } ]==]"),
    ]
    for src, lang, needle in cases:
        start = src.find("{") + 1
        intended = src.rfind("}")
        got = SCANNERS[lang](src, start)
        if got != intended:
            print(f"FAIL scanner {lang}: {got} != {intended}: {src!r}")
            raise SystemExit(1)
        if find_brace_end(src, start) == intended:
            print(f"FAIL negative control for {lang}: generic scanner passed: {src!r}")
            raise SystemExit(1)
        doc = split(src)
        if render(doc) != src:
            print(f"FAIL scanner render {lang}")
            raise SystemExit(1)
        block = next(r for r in doc["regions"] if r["kind"] == "block")
        if needle not in block["body"] or block["body"] != src[start:intended]:
            print(f"FAIL scanner span {lang}: {block['body']!r}")
            raise SystemExit(1)

    # Raw hash delimiters remain the normative escape hatch and bypass all
    # language scanners, including when the registry is effectively empty.
    raw = '@ruby#" } literal "#\n'
    doc = split(raw)
    block = next(r for r in doc["regions"] if r["kind"] == "block")
    if "} literal" not in block["body"] or render(doc) != raw:
        print("FAIL raw hash escape hatch")
        raise SystemExit(1)


def main() -> int:
    check_scanner_cases()
    count = 1000
    for seed in range(count):
        src = generate(seed)
        doc = split(src)
        rendered = render(doc)
        if rendered != src:
            print(f"FAIL seed {seed}: render changed source")
            return 1
        again = split(rendered)
        if len(again["regions"]) != len(doc["regions"]):
            print(f"FAIL seed {seed}: region count drift")
            return 1
        for a, b in zip(doc["regions"], again["regions"]):
            if (a["lang"], a["kind"], a["body"]) != (b["lang"], b["kind"], b["body"]):
                print(f"FAIL seed {seed}: region drift {a} != {b}")
                return 1
    print(f"splitter reference fuzz: PASS ({count} cases)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
