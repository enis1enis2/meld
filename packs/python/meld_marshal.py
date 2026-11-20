"""Meld Value Model wire codec for Python (spec/value-model.md).

JUDGMENT CALL: a plain Python ``dict`` encodes as ``$map`` (string keys only)
so every pack emits the same shape for dictionaries. Records, variants,
options, results, handles and callable handles have explicit wrapper classes
so the wire round-trip is exact for the full type set.

KNOWN LIMITATION (documented in spec/value-model.md): a native string made only
of decimal digits decodes back as Meld ``int`` because the wire form for both
is a JSON string. Test vectors avoid that ambiguous case.
"""
from __future__ import annotations

import base64
import math


class MeldVariant:
    def __init__(self, case: str, value):
        self.case = case
        self.value = value

    def __eq__(self, other):
        return isinstance(other, MeldVariant) and (self.case, self.value) == (other.case, other.value)

    def __repr__(self):
        return f"MeldVariant({self.case!r}, {self.value!r})"


class MeldOption:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return isinstance(other, MeldOption) and self.value == other.value

    def __repr__(self):
        return f"MeldOption({self.value!r})"


class MeldOk:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return isinstance(other, MeldOk) and self.value == other.value

    def __repr__(self):
        return f"MeldOk({self.value!r})"


class MeldErr:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return isinstance(other, MeldErr) and self.value == other.value

    def __repr__(self):
        return f"MeldErr({self.value!r})"


class MeldHandle:
    def __init__(self, id: str, resource: str, borrow: bool = False):
        self.id = id
        self.resource = resource
        self.borrow = borrow

    def __eq__(self, other):
        return isinstance(other, MeldHandle) and (
            self.id, self.resource, self.borrow
        ) == (other.id, other.resource, other.borrow)

    def __repr__(self):
        return f"MeldHandle({self.id!r}, {self.resource!r}, {self.borrow!r})"


class MeldFn:
    def __init__(self, target: str):
        self.target = target

    def __eq__(self, other):
        return isinstance(other, MeldFn) and self.target == other.target

    def __repr__(self):
        return f"MeldFn({self.target!r})"


def _is_decimal(text: str) -> bool:
    s = text[1:] if text.startswith("-") else text
    return bool(s) and all("0" <= c <= "9" for c in s)


def encode(value):
    """Native Python value -> Meld wire JSON value."""
    if value is None:
        return None
    if isinstance(value, bool):  # bool is a subclass of int: check first
        return value
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        if math.isnan(value):
            return "NaN"
        if math.isinf(value):
            return "Inf" if value > 0 else "-Inf"
        return value
    if isinstance(value, str):
        return value
    if isinstance(value, (bytes, bytearray)):
        return {"$bytes": base64.b64encode(bytes(value)).decode("ascii")}
    if isinstance(value, (list, tuple)):
        return [encode(x) for x in value]
    if isinstance(value, MeldVariant):
        return {"$variant": {"case": value.case, "value": encode(value.value)}}
    if isinstance(value, MeldOption):
        return {"$some": encode(value.value)}
    if isinstance(value, MeldOk):
        return {"$ok": encode(value.value)}
    if isinstance(value, MeldErr):
        return {"$err": encode(value.value)}
    if isinstance(value, MeldHandle):
        return {"$handle": {"id": value.id, "resource": value.resource, "borrow": value.borrow}}
    if isinstance(value, MeldFn):
        return {"$fn": value.target}
    if isinstance(value, dict):
        return {"$map": {str(k): encode(v) for k, v in value.items()}}
    raise TypeError(f"cannot encode {type(value).__name__} as a Meld value")


def decode(wire):
    """Meld wire JSON value -> native Python value."""
    if wire is None:
        return None
    if isinstance(wire, bool):
        return wire
    if isinstance(wire, (int, float)):
        # JSON numbers are Meld floats; Meld ints travel as decimal strings.
        return float(wire)
    if isinstance(wire, str):
        if wire == "NaN":
            return float("nan")
        if wire == "Inf":
            return float("inf")
        if wire == "-Inf":
            return float("-inf")
        if _is_decimal(wire):
            return int(wire)
        return wire
    if isinstance(wire, list):
        return [decode(x) for x in wire]
    if isinstance(wire, dict):
        if "$bytes" in wire:
            return base64.b64decode(wire["$bytes"])
        if "$map" in wire:
            return {k: decode(v) for k, v in wire["$map"].items()}
        if "$variant" in wire:
            v = wire["$variant"]
            return MeldVariant(v["case"], decode(v["value"]))
        if "$some" in wire:
            return MeldOption(decode(wire["$some"]))
        if "$ok" in wire:
            return MeldOk(decode(wire["$ok"]))
        if "$err" in wire:
            return MeldErr(decode(wire["$err"]))
        if "$handle" in wire:
            h = wire["$handle"]
            return MeldHandle(h.get("id", ""), h.get("resource", ""), bool(h.get("borrow", False)))
        if "$fn" in wire:
            return MeldFn(wire["$fn"])
        # A bare object arriving from another pack is treated as a map.
        return {k: decode(v) for k, v in wire.items()}
    raise TypeError(f"cannot decode Meld wire value {wire!r}")
