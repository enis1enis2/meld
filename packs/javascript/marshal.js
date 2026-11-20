'use strict';
// Meld Value Model wire codec for JavaScript (spec/value-model.md).
//
// JUDGMENT CALL: a plain JS object encodes as `$map` so all packs emit the same
// shape for dictionaries. Wrapper classes give exact round-trips for the tagged
// types. A decimal-digit string decodes as Meld `int` (wire ambiguity is
// documented in spec/value-model.md).

class MeldVariant {
  constructor(caseName, value) { this.case = caseName; this.value = value; }
}
class MeldOption {
  constructor(value) { this.value = value; }
}
class MeldOk {
  constructor(value) { this.value = value; }
}
class MeldErr {
  constructor(value) { this.value = value; }
}
class MeldHandle {
  constructor(id, resource, borrow = false) {
    this.id = id; this.resource = resource; this.borrow = borrow;
  }
}
class MeldFn {
  constructor(target) { this.target = target; }
}

function isDecimal(text) {
  if (typeof text !== 'string') return false;
  const s = text.startsWith('-') ? text.slice(1) : text;
  return s.length > 0 && /^[0-9]+$/.test(s);
}

function encode(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === 'boolean') return value;
  if (typeof value === 'bigint') return value.toString(10);
  if (typeof value === 'number') {
    if (Number.isNaN(value)) return 'NaN';
    if (value === Infinity) return 'Inf';
    if (value === -Infinity) return '-Inf';
    // JUDGMENT CALL: safe integers are Meld ints; other numbers are floats.
    if (Number.isSafeInteger(value)) return String(value);
    return value;
  }
  if (typeof value === 'string') return value;
  if (value instanceof Uint8Array || Buffer.isBuffer(value)) {
    return { $bytes: Buffer.from(value).toString('base64') };
  }
  if (Array.isArray(value)) return value.map(encode);
  if (value instanceof MeldVariant) {
    return { $variant: { case: value.case, value: encode(value.value) } };
  }
  if (value instanceof MeldOption) return { $some: encode(value.value) };
  if (value instanceof MeldOk) return { $ok: encode(value.value) };
  if (value instanceof MeldErr) return { $err: encode(value.value) };
  if (value instanceof MeldHandle) {
    return { $handle: { id: value.id, resource: value.resource, borrow: value.borrow } };
  }
  if (value instanceof MeldFn) return { $fn: value.target };
  if (typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[String(k)] = encode(v);
    return { $map: out };
  }
  throw new TypeError(`cannot encode ${typeof value} as a Meld value`);
}

function decode(wire) {
  if (wire === null || wire === undefined) return null;
  if (typeof wire === 'boolean') return wire;
  if (typeof wire === 'number') return wire; // JSON number -> Meld float
  if (typeof wire === 'string') {
    if (wire === 'NaN') return NaN;
    if (wire === 'Inf') return Infinity;
    if (wire === '-Inf') return -Infinity;
    if (isDecimal(wire)) {
      const n = Number(wire);
      return Number.isSafeInteger(n) ? n : BigInt(wire);
    }
    return wire;
  }
  if (Array.isArray(wire)) return wire.map(decode);
  if (typeof wire === 'object') {
    if ('$bytes' in wire) return Buffer.from(wire.$bytes, 'base64');
    if ('$map' in wire) {
      const out = {};
      for (const [k, v] of Object.entries(wire.$map)) out[k] = decode(v);
      return out;
    }
    if ('$variant' in wire) return new MeldVariant(wire.$variant.case, decode(wire.$variant.value));
    if ('$some' in wire) return new MeldOption(decode(wire.$some));
    if ('$ok' in wire) return new MeldOk(decode(wire.$ok));
    if ('$err' in wire) return new MeldErr(decode(wire.$err));
    if ('$handle' in wire) {
      const h = wire.$handle;
      return new MeldHandle(h.id, h.resource, Boolean(h.borrow));
    }
    if ('$fn' in wire) return new MeldFn(wire.$fn);
    const out = {};
    for (const [k, v] of Object.entries(wire)) out[k] = decode(v);
    return out;
  }
  throw new TypeError(`cannot decode Meld wire value ${JSON.stringify(wire)}`);
}

module.exports = {
  MeldVariant, MeldOption, MeldOk, MeldErr, MeldHandle, MeldFn, encode, decode,
};
