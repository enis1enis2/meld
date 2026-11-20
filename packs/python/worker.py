#!/usr/bin/env python3
"""Meld T2 worker for Python, conformance level L2.

Protocol: spec/t2-protocol.md. Wire values: spec/value-model.md. stdout is
reserved for the protocol; region ``print`` output is captured and returned as
an ordered array in the ``execute`` response.
"""
from __future__ import annotations

import contextlib
import io
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# JUDGMENT CALL: the module is named `meld_marshal`, not `marshal`, because
# Python ships a built-in `marshal` module that shadows a same-named file.
from meld_marshal import MeldHandle, decode, encode  # noqa: E402

PROTOCOL = "meld-t2/1"
LEVEL = "L2"
CAPS = ["values", "stdout", "eval"]

# JUDGMENT CALL: pin the protocol streams before any region code runs. Region
# `print()` is redirected through sys.stdout during execute(), so protocol I/O
# must not go through that mutable name. Worker-initiated host calls happen
# *inside* execute(), and must still be visible to the host process.
PROTOCOL_STDIN = sys.stdin
PROTOCOL_STDOUT = sys.stdout

# One worker process owns one persistent namespace; regions of the same
# language share it, so imports from earlier regions are visible.
NAMESPACE: dict = {}
HANDLES: dict = {}


class WorkerError(Exception):
    """Structured protocol error; the worker stays alive."""

    def __init__(self, code: str, message: str, source=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.source = source

    def obj(self):
        return {"code": self.code, "message": self.message, "source": self.source}


# JUDGMENT CALL: `meld_call` is injected into the region namespace rather than
# implemented as a new statement in Meld syntax. A region can therefore call a
# host-resolved T3/WIT function with ordinary Python arguments and receive a
# decoded Meld Value, while the worker process stays unaware of Wasmtime.
CALLBACK_ID = 0


def host_call(name, *args):
    global CALLBACK_ID
    CALLBACK_ID -= 1
    emit({
        "id": CALLBACK_ID,
        "method": "__host_call",
        "params": {"fn": name, "args": [encode(a) for a in args]},
    })
    line = PROTOCOL_STDIN.readline()
    if not line:
        raise WorkerError("HostCallError", "host closed the callback channel")
    try:
        response = json.loads(line)
    except Exception as exc:  # noqa: BLE001
        raise WorkerError("E_PROTOCOL", f"bad host callback response: {exc}") from exc
    if "error" in response:
        raise WorkerError(
            response["error"].get("code", "HostCallError"),
            response["error"].get("message", "host call failed"),
        )
    return decode(response.get("result", {}).get("value"))


NAMESPACE["meld_call"] = host_call


def initialize(params):
    if params.get("protocol") != PROTOCOL:
        raise WorkerError("E_PROTOCOL", f"expected {PROTOCOL}, got {params.get('protocol')!r}")
    return {"protocol": PROTOCOL, "level": LEVEL, "caps": CAPS}


def execute(params):
    code = params.get("code", "")
    region_id = params.get("region_id", "")
    buf = io.StringIO()
    try:
        # JUDGMENT CALL: a worker exception is a response-level error so the
        # host can map it to MeldError and keep this worker alive. Meld
        # ``result<T,E>`` values are ordinary encoded values, not exceptions.
        with contextlib.redirect_stdout(buf):
            exec(compile(code, f"<meld:{region_id}>", "exec"), NAMESPACE)
    except Exception as exc:  # noqa: BLE001 - the protocol requires a typed error
        raise WorkerError(
            "RuntimeError",
            f"{type(exc).__name__}: {exc}",
            {"region_id": region_id, "lang": "python"},
        ) from exc
    return {"stdout": buf.getvalue().splitlines(), "exports": []}


def evaluate(params):
    """Evaluate one inline expression island (spec/t2-protocol.md §3)."""
    # JUDGMENT CALL: delimiter padding is Meld syntax; workers only accept
    # expression text, so trim defensively even though compose.rs already does.
    code = str(params.get("code", "")).strip()
    region_id = params.get("region_id", "")
    buf = io.StringIO()
    try:
        with contextlib.redirect_stdout(buf):
            value = eval(compile(code, f"<meld-inline:{region_id}>", "eval"), NAMESPACE)
    except Exception as exc:  # noqa: BLE001 - protocol requires a typed error
        raise WorkerError(
            "RuntimeError",
            f"{type(exc).__name__}: {exc}",
            {"region_id": region_id, "lang": "python"},
        ) from exc
    return {"value": encode(value), "stdout": buf.getvalue().splitlines()}


def fetch(params):
    name = params.get("name", "")
    if name not in NAMESPACE:
        raise WorkerError("UnknownBinding", f"no native binding named {name!r}")
    return {"value": encode(NAMESPACE[name])}


def import_value(params):
    value = decode(params.get("value"))
    NAMESPACE[params["name"]] = value
    if isinstance(value, MeldHandle) and not value.borrow:
        HANDLES[value.id] = value
    return {"ok": True}


def call(params):
    target = params.get("fn")
    args = [decode(a) for a in params.get("args", [])]
    if isinstance(target, dict) and "$fn" in target:
        target = target["$fn"]
    fn = NAMESPACE.get(target) if isinstance(target, str) else None
    if not callable(fn):
        # L3 cross-region callable resolution is a later-phase feature; L2
        # only needs local callables to be invocable.
        raise WorkerError("TypeMismatch", f"{target!r} is not a callable binding")
    return {"value": encode(fn(*args))}


def drop(params):
    HANDLES.pop(params.get("handle_id"), None)
    return {"ok": True}


def shutdown(_params):
    return {"ok": True}


DISPATCH = {
    "initialize": initialize,
    "execute": execute,
    "eval": evaluate,
    "fetch": fetch,
    "import": import_value,
    "call": call,
    "drop": drop,
    "shutdown": shutdown,
}


def emit(obj):
    PROTOCOL_STDOUT.write(json.dumps(obj, ensure_ascii=False, separators=(",", ":")) + "\n")
    PROTOCOL_STDOUT.flush()


def main():
    for line in PROTOCOL_STDIN:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as exc:  # noqa: BLE001
            emit({"id": 0, "error": {"code": "E_PROTOCOL", "message": str(exc), "source": None}})
            continue
        rid = req.get("id", 0)
        method = req.get("method", "")
        params = req.get("params") or {}
        try:
            handler = DISPATCH.get(method)
            if handler is None:
                raise WorkerError("E_PROTOCOL", f"unknown method {method!r}")
            emit({"id": rid, "result": handler(params)})
        except WorkerError as exc:
            emit({"id": rid, "error": exc.obj()})
        except Exception as exc:  # noqa: BLE001 - never crash the protocol loop
            emit({
                "id": rid,
                "error": {
                    "code": "InternalError",
                    "message": f"{type(exc).__name__}: {exc}",
                    "source": None,
                },
            })
        if method == "shutdown":
            break


if __name__ == "__main__":
    main()
