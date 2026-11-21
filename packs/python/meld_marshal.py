"""Meld Value Model wire codec for Python (spec/value-model.md).

JUDGMENT CALL: a plain Python ``dict`` encodes as ``$map`` (string keys only)
so every pack emits the same shape for dictionaries. Records, variants,
options, results, handles and callable handles have explicit wrapper classes
so the wire round-trip is exact for the full type set.

Integers use the canonical tagged form ``{"$int": "123"}`` so a decimal-looking
string can never be confused with an integer. The legacy bare decimal string
reader is kept for old fixtures; pass ``strict=True`` to reject that ambiguous
form. ``$``-prefixed keys are reserved by the wire format.
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


WIRE_TAGS = frozenset(
    {"$bytes", "$int", "$map", "$variant", "$some", "$ok", "$err", "$handle", "$fn"}
)


def _is_decimal(text: str) -> bool:
    s = text[1:] if text.startswith("-") else text
    return bool(s) and all("0" <= c <= "9" for c in s)


def _single_tag(wire: dict):
    """Return the one recognised tag key in a wire object, or None."""
    tags = [key for key in wire if isinstance(key, str) and key.startswith("$")]
    if not tags:
        return None
    if len(tags) != 1:
        raise ValueError("multiple wire tags in one object")
    tag = tags[0]
    if tag not in WIRE_TAGS:
        raise ValueError(f"unknown wire tag {tag}")
    if len(wire) != 1:
        raise ValueError(f"tag {tag} must be the only key in its object")
    return tag


def _reject_reserved_keys(obj: dict, context: str) -> None:
    for key in obj:
        if isinstance(key, str) and key.startswith("$"):
            raise ValueError(f"reserved key {key} is not valid in {context}")


def encode(value):
    """Native Python value -> Meld wire JSON value."""
    if value is None:
        return None
    if isinstance(value, bool):  # bool is a subclass of int: check first
        return value
    if isinstance(value, int):
        return {"$int": str(value)}
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
        _reject_reserved_keys(value, "dict keys")
        return {"$map": {str(k): encode(v) for k, v in value.items()}}
    raise TypeError(f"cannot encode {type(value).__name__} as a Meld value")


def decode(wire, strict: bool = False):
    """Meld wire JSON value -> native Python value.

    ``strict=True`` rejects the legacy untagged decimal-string integer form.
    """
    if wire is None:
        return None
    if isinstance(wire, bool):
        return wire
    if isinstance(wire, (int, float)):
        # JSON numbers are Meld floats; Meld ints use the $int tag.
        return float(wire)
    if isinstance(wire, str):
        if wire == "NaN":
            return float("nan")
        if wire == "Inf":
            return float("inf")
        if wire == "-Inf":
            return float("-inf")
        if _is_decimal(wire):
            if strict:
                raise ValueError('untagged integer string; use {"$int": "..."}')
            return int(wire)
        return wire
    if isinstance(wire, list):
        return [decode(x, strict) for x in wire]
    if isinstance(wire, dict):
        tag = _single_tag(wire)
        if tag == "$int":
            text = wire["$int"]
            if not isinstance(text, str) or not _is_decimal(text):
                raise ValueError(f"invalid integer tag payload: {text!r}")
            return int(text)
        if tag == "$bytes":
            return base64.b64decode(wire["$bytes"], validate=True)
        if tag == "$map":
            payload = wire["$map"]
            _reject_reserved_keys(payload, "$map")
            return {k: decode(v, strict) for k, v in payload.items()}
        if tag == "$variant":
            payload = wire["$variant"]
            _reject_reserved_keys(payload, "$variant")
            return MeldVariant(payload["case"], decode(payload["value"], strict))
        if tag == "$some":
            return MeldOption(decode(wire["$some"], strict))
        if tag == "$ok":
            return MeldOk(decode(wire["$ok"], strict))
        if tag == "$err":
            return MeldErr(decode(wire["$err"], strict))
        if tag == "$handle":
            payload = wire["$handle"]
            _reject_reserved_keys(payload, "$handle")
            return MeldHandle(
                payload.get("id", ""),
                payload.get("resource", ""),
                bool(payload.get("borrow", False)),
            )
        if tag == "$fn":
            return MeldFn(wire["$fn"])
        if tag is None:
            # A bare object arriving from another pack is treated as a map.
            _reject_reserved_keys(wire, "record")
            return {k: decode(v, strict) for k, v in wire.items()}
        raise AssertionError("unreachable tag")
    raise TypeError(f"cannot decode Meld wire value {wire!r}")
