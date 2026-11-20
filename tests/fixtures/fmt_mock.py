#!/usr/bin/env python3
"""Deterministic stand-in for a language formatter: collapse whitespace."""
import sys

with open(sys.argv[1], encoding="utf-8") as src:
    text = src.read()
formatted = "\n".join(" ".join(line.split()) for line in text.splitlines())
with open(sys.argv[2], "w", encoding="utf-8") as out:
    out.write(formatted + "\n")
