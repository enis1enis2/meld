#!/usr/bin/env python3
"""Replay tests/conformance.json against worker.sh over the T2 wire protocol.

JUDGMENT CALL: this is a pack-owned test harness, not registry tooling.
`packtool` never executes it (or any pack payload); running it is an explicit
developer action, as with any other test suite.
"""
from __future__ import annotations

import json
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACK = HERE.parent


def main() -> int:
    cases = json.loads((HERE / "conformance.json").read_text(encoding="utf-8"))
    assert cases["protocol"] == "meld-t2/1", cases
    proc = subprocess.Popen(
        ["sh", str(PACK / "worker.sh")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, bufsize=1, cwd=PACK,
    )
    next_id = 1

    def call(method: str, params: dict) -> dict:
        nonlocal next_id
        rid = next_id
        next_id += 1
        proc.stdin.write(json.dumps({"id": rid, "method": method, "params": params}) + "\n")
        proc.stdin.flush()
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError(f"worker closed stdout: {proc.stderr.read()}")
        msg = json.loads(line)
        assert msg["id"] == rid, msg
        return msg

    failures = 0
    try:
        hello = call("initialize", {"protocol": "meld-t2/1", "lang": "bash"})
        assert hello["result"]["level"] == "L1", hello
        print("initialize: ok")
        for case in cases["cases"]:
            reply = call("execute", {
                "region_id": case["name"], "code": case["code"], "env": {},
            })
            expect = case["expect"]
            if "stdout" in expect:
                got = reply.get("result", {}).get("stdout")
                ok = got == expect["stdout"]
            else:
                err = reply.get("error", {})
                ok = (err.get("code") == expect["error_code"]
                      and expect["error_contains"] in err.get("message", ""))
            print(f"{case['name']}: {'ok' if ok else 'FAIL'} ({reply})")
            failures += 0 if ok else 1
        call("shutdown", {})
    finally:
        proc.wait(timeout=5)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
