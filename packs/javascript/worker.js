#!/usr/bin/env node
'use strict';
// Meld T2 worker for JavaScript, conformance level L2.
// Protocol: spec/t2-protocol.md. Wire values: spec/value-model.md.
//
// JUDGMENT CALL: `vm.runInContext` does not expose top-level `const`/`let`
// bindings after a script finishes. Regions that export a value must therefore
// assign it to `globalThis.<name>`. L1 regions that only print need no convention.

const readline = require('readline');
const vm = require('vm');
const util = require('util');
const { MeldHandle, decode, encode } = require('./marshal.js');

const PROTOCOL = 'meld-t2/1';
const LEVEL = 'L2';
const CAPS = ['values', 'stdout', 'eval'];

let stdoutLines = [];
const namespace = {
  console: {
    log: (...args) => { stdoutLines.push(util.format(...args)); },
    info: (...args) => { stdoutLines.push(util.format(...args)); },
    warn: (...args) => { process.stderr.write(`[javascript:stderr] ${util.format(...args)}\n`); },
    error: (...args) => { process.stderr.write(`[javascript:stderr] ${util.format(...args)}\n`); },
  },
};
const context = vm.createContext(namespace);
const handles = new Map();

class WorkerError extends Error {
  constructor(code, message, source = null) {
    super(message);
    this.code = code;
    this.source = source;
  }
  obj() { return { code: this.code, message: this.message, source: this.source }; }
}

function initialize(params) {
  if (params.protocol !== PROTOCOL) {
    throw new WorkerError('E_PROTOCOL', `expected ${PROTOCOL}, got ${String(params.protocol)}`);
  }
  return { protocol: PROTOCOL, level: LEVEL, caps: CAPS };
}

function execute(params) {
  stdoutLines = [];
  const regionId = params.region_id || '';
  try {
    vm.runInContext(String(params.code || ''), context, { filename: `<meld:${regionId}>` });
  } catch (exc) {
    throw new WorkerError('RuntimeError', `${exc.name}: ${exc.message}`,
      { region_id: regionId, lang: 'javascript' });
  }
  const lines = stdoutLines;
  stdoutLines = [];
  return { stdout: lines, exports: [] };
}

// JUDGMENT CALL: an inline island is an expression, so it is parenthesised
// before vm.runInContext. Statement regions keep using execute(); this keeps
// the two protocol methods distinct and makes syntax errors precise.
function evaluate(params) {
  stdoutLines = [];
  const regionId = params.region_id || '';
  // Delimiter padding is Meld syntax; workers accept expression text only.
  const code = String(params.code || '').trim();
  let value;
  try {
    value = vm.runInContext(`(${code})`, context,
      { filename: `<meld-inline:${regionId}>` });
  } catch (exc) {
    throw new WorkerError('RuntimeError', `${exc.name}: ${exc.message}`,
      { region_id: regionId, lang: 'javascript' });
  }
  const lines = stdoutLines;
  stdoutLines = [];
  return { value: encode(value), stdout: lines };
}

function fetch(params) {
  const name = params.name;
  if (!(name in namespace)) throw new WorkerError('UnknownBinding', `no binding named ${String(name)}`);
  return { value: encode(namespace[name]) };
}

function importValue(params) {
  const value = decode(params.value);
  namespace[params.name] = value;
  if (value instanceof MeldHandle && !value.borrow) handles.set(value.id, value);
  return { ok: true };
}

function call(params) {
  const target = params.fn;
  const args = (params.args || []).map(decode);
  const name = (target && typeof target === 'object' && '$fn' in target) ? target.$fn : target;
  const fn = typeof name === 'string' ? namespace[name] : null;
  if (typeof fn !== 'function') {
    throw new WorkerError('TypeMismatch', `${String(name)} is not a callable binding`);
  }
  return { value: encode(fn(...args)) };
}

function drop(params) {
  handles.delete(params.handle_id);
  return { ok: true };
}

function shutdown() { return { ok: true }; }

const DISPATCH = {
  initialize, execute, eval: evaluate, fetch, import: importValue, call, drop, shutdown,
};

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  const text = line.trim();
  if (!text) return;
  let req;
  try {
    req = JSON.parse(text);
  } catch (exc) {
    emit({ id: 0, error: { code: 'E_PROTOCOL', message: exc.message, source: null } });
    return;
  }
  const id = req.id === undefined ? 0 : req.id;
  const method = req.method || '';
  const params = req.params || {};
  try {
    const handler = DISPATCH[method];
    if (!handler) throw new WorkerError('E_PROTOCOL', `unknown method ${method}`);
    emit({ id, result: handler(params) });
  } catch (exc) {
    if (exc instanceof WorkerError) emit({ id, error: exc.obj() });
    else emit({ id, error: { code: 'InternalError', message: `${exc.name}: ${exc.message}`, source: null } });
  }
  if (method === 'shutdown') rl.close();
});
rl.on('close', () => process.exit(0));
