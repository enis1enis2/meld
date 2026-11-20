#!/usr/bin/env python3
"""Trusted reference harness for the Meld splitter.

JUDGMENT CALL (spec/README.md B1): Rust/cargo are absent in this environment,
so this harness executes the same algorithm in Python to validate the splitter
design. It is a proxy, not a substitute for `cargo test`.
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
                body_end = find_brace_end(s, head["body"])
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


def main() -> int:
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
