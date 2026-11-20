#!/usr/bin/env python3
"""T2 worker conformance + round-trip matrix driver.

Exercises the real worker processes over the newline-delimited JSON protocol
(spec/t2-protocol.md). Used for Phase 2 stdout-ordering and Phase 3 value
round-trips. No toolchain is mocked: python3, node and lua are spawned directly.
"""
from __future__ import annotations

import base64
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_COMMANDS = {"python": "python3", "javascript": "node", "lua": "lua"}
WORKERS = {
    "python": ["python3", "-u", str(ROOT / "packs/python/worker.py")],
    "javascript": ["node", str(ROOT / "packs/javascript/worker.js")],
    "lua": ["lua", str(ROOT / "packs/lua/worker.lua")],
}
PRINT_CODE = {
    "python": "print('{lang}-1')\nprint('{lang}-2')",
    "javascript": "console.log('{lang}-1'); console.log('{lang}-2');",
    "lua": "print('{lang}-1')\nprint('{lang}-2')",
}

# Canonical Meld wire values (spec/value-model.md §1). Numeric strings are
# ints; the ambiguous decimal-string case is deliberately excluded.
VECTORS = {
    "null": None,
    "bool": True,
    "int": "42",
    "bigint": "123456789012345678901234567890",
    "float": 1.5,
    "string": "hello",
    "bytes": {"$bytes": base64.b64encode(b"\x00\x01\xfe").decode("ascii")},
    "list": ["1", "2", "3"],
    "map": {"$map": {"a": "7", "b": "hello"}},
    "variant": {"$variant": {"case": "some", "value": "1"}},
    "option": {"$some": "5"},
    "ok": {"$ok": "6"},
    "err": {"$err": "bad"},
    "handle": {"$handle": {"id": "h1", "resource": "Db", "borrow": True}},
    "fn": {"$fn": "r0:produce"},
}


class WorkerError(Exception):
    def __init__(self, obj):
        self.message = obj.get("message", str(obj))
        super().__init__(self.message)
        self.code = obj.get("code", "Unknown")
        self.obj = obj


class Worker:
    def __init__(self, lang: str):
        self.lang = lang
        self.proc = subprocess.Popen(
            WORKERS[lang],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            cwd=ROOT,
        )
        self.next_id = 1

    def call(self, method: str, params: dict):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write(json.dumps({"id": rid, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            stderr = self.proc.stderr.read()
            raise WorkerError({"code": "WorkerCrashed", "message": f"worker closed stdout: {stderr}"})
        msg = json.loads(line)
        if "error" in msg:
            raise WorkerError(msg["error"])
        return msg.get("result", {})

    def close(self):
        try:
            self.call("shutdown", {})
        except Exception:
            pass
        self.proc.wait(timeout=5)


def check_initialize(lang):
    w = Worker(lang)
    try:
        res = w.call("initialize", {
            "protocol": "meld-t2/1", "lang": lang, "caps": ["values", "stdout"],
        })
        assert res["protocol"] == "meld-t2/1", res
        assert res["level"] == "L2", res
        print(f"{lang}: initialize ok (level={res['level']})")
    finally:
        w.close()


def check_stdout(lang):
    w = Worker(lang)
    try:
        w.call("initialize", {"protocol": "meld-t2/1", "lang": lang})
        code = PRINT_CODE[lang].format(lang=lang)
        res = w.call("execute", {"region_id": "r0", "code": code, "env": {}})
        expected = [f"{lang}-1", f"{lang}-2"]
        assert res["stdout"] == expected, (lang, res)
        print(f"{lang}: execute stdout ok ({res['stdout']})")
    finally:
        w.close()


def check_error(lang):
    w = Worker(lang)
    try:
        w.call("initialize", {"protocol": "meld-t2/1", "lang": lang})
        code = {
            "python": "raise ValueError('boom')",
            "javascript": "throw new Error('boom');",
            "lua": "error('boom')",
        }[lang]
        try:
            w.call("execute", {"region_id": "r0", "code": code, "env": {}})
        except WorkerError as exc:
            assert "boom" in exc.message, exc.message
            print(f"{lang}: error propagation ok ({exc.code})")
            return
        raise AssertionError(f"{lang}: expected a structured error")
    finally:
        w.close()


def check_roundtrip(src: str, dst: str):
    a = Worker(src)
    b = Worker(dst)
    try:
        for w, lang in ((a, src), (b, dst)):
            w.call("initialize", {"protocol": "meld-t2/1", "lang": lang})
        failures = []
        for name, wire in VECTORS.items():
            binding = f"v_{name}"
            b.call("import", {"name": binding, "value": wire})
            out = b.call("fetch", {"name": binding})["value"]
            if out != wire:
                failures.append(f"{name}: expected {wire!r}, got {out!r}")
        if failures:
            print(f"{src}->{dst}: FAIL " + "; ".join(failures))
            return False
        print(f"{src}->{dst}: round-trip ok ({len(VECTORS)} values)")
        return True
    finally:
        a.close()
        b.close()


def main() -> int:
    available = []
    missing_runtimes = {
        lang: command
        for lang, command in RUNTIME_COMMANDS.items()
        if shutil.which(command) is None
    }
    for lang, command in sorted(missing_runtimes.items()):
        print(f"SKIP {lang}: runtime {command!r} is not on PATH")
    for lang in WORKERS:
        if lang in missing_runtimes:
            continue
        try:
            check_initialize(lang)
            available.append(lang)
        except (FileNotFoundError, WorkerError) as exc:
            print(f"SKIP {lang}: worker unavailable ({exc})")
    if not available:
        print("t2 reference harness: SKIPPED (no T2 runtimes available)")
        return 0
    problems = 0

    print("\n-- Phase 2: T2 stdout --")
    for lang in available:
        try:
            check_stdout(lang)
        except Exception as exc:  # noqa: BLE001
            print(f"{lang}: stdout FAIL ({exc})")
            problems += 1
    if "python" in available and "javascript" in available:
        merged = []
        for lang in ("python", "javascript"):
            w = Worker(lang)
            try:
                w.call("initialize", {"protocol": "meld-t2/1", "lang": lang})
                merged.extend(w.call("execute", {
                    "region_id": lang, "code": PRINT_CODE[lang].format(lang=lang), "env": {},
                })["stdout"])
            finally:
                w.close()
        expected = ["python-1", "python-2", "javascript-1", "javascript-2"]
        assert merged == expected, merged
        print(f"merged stdout in order: {merged}")

    print("\n-- Phase 3: error propagation --")
    for lang in available:
        try:
            check_error(lang)
        except Exception as exc:  # noqa: BLE001
            print(f"{lang}: error FAIL ({exc})")
            problems += 1

    print("\n-- Phase 3: value round-trip matrix --")
    for src in available:
        for dst in available:
            if not check_roundtrip(src, dst):
                problems += 1

    print(f"\nt2 reference harness: {problems} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
