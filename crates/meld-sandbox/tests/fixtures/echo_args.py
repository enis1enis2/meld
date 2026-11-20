#!/usr/bin/env python3
"""Tiny argv-only echo worker used by backend tests.

It has no shell interpolation: each argument is printed as a separate line.
Not used with a sandbox backend in tests that require unavailable mechanisms.
"""
import sys

for argument in sys.argv[1:]:
    print(argument)
