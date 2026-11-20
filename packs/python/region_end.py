"""End-scanner reference for plain `@python { ... }` blocks.

JUDGMENT CALL: the Rust host currently wires the generic C-like balanced-brace
scanner for every pack. This file is the pack-owned contract stub; Wasm-loaded
scanners land with the compiled tier (Phase 4). Python regions containing
triple-quoted strings are safest written with raw hash delimiters until then.
"""


def find_region_end(text, start, marker="}"):
    """Return the byte offset of the closing marker, or -1 if unbalanced."""
    depth = 1
    quote = None
    triple = False
    i = start
    while i < len(text):
        c = text[i]
        if triple:
            if text[i:i + 3] == quote * 3:
                triple = False
                i += 3
                continue
        elif quote:
            if c == "\\":
                i += 2
                continue
            if c == quote:
                quote = None
        elif c in "\"'":
            if text[i:i + 3] == c * 3:
                triple = True
                quote = c
                i += 3
                continue
            quote = c
        elif c == "#" and (i == 0 or text[i - 1].isspace()):
            while i < len(text) and text[i] != "\n":
                i += 1
            continue
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1
