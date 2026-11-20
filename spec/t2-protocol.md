# T2 Worker Protocol Specification (Phase 0)

Status: normative for Phase 2/3. T2 = worker process speaking a Meld protocol
over stdio, the universal default tier (project.md §5).

## 1. Framing

One JSON object per line on stdin/stdout. UTF-8, no BOM. No multi-line messages.
A worker must not write non-protocol text to stdout; diagnostic/log text goes to
stderr and is drained by a bounded host-side buffer, then attached to crash
reports with a `[<lang>:stderr]` prefix.

**JUDGMENT CALL:** the host caps every protocol line at 1 MiB and bounds its
request/stdout queues. Oversized, non-UTF-8, malformed, or unsolicited stdout is
a hard `SandboxDenied`, not an ignored line. Every wait has an absolute 10 s
deadline; a worker that cannot answer in time is killed and reaped. These
limits protect the host from a hostile worker; they do not replace OS
confinement (§6).

## 2. Handshake

Host → worker:

```json
{"id":0,"method":"initialize","params":{"protocol":"meld-t2/1","lang":"python","caps":["values","stdout"]}}
```

Worker → host:

```json
{"id":0,"result":{"protocol":"meld-t2/1","level":"L2","caps":["values","stdout"]}}
```

Protocol mismatch is `E_PROTOCOL` and the worker is terminated. The host also
validates `level` (when present) and `caps`: only `values`, `stdout`, and `eval`
are known capabilities, `values` and `stdout` are required, and any unknown
capability is a hard `SandboxDenied`. `eval` stays optional so workers that only
support statements remain usable for ordinary regions.

## 3. Methods

| method | params | result |
|---|---|---|
| `execute` | `{region_id, code, env}` | `{stdout:[...], exports:[{name,value}]}` |
| `eval` | `{region_id, code}` | `{value}` — evaluate one expression and return a Meld Value |
| `fetch` | `{name}` | `{value}` — read a native binding produced by `execute` |
| `export` | `{name, value}` | `{ok:true}` — host pushes a value into the worker |
| `import` | `{name, value}` | `{ok:true}` |
| `call` | `{fn,args}` | `{value}` or `{error}` |
| `drop` | `{handle_id}` | `{ok:true}` |
| `shutdown` | `{}` | `{ok:true}` |

`execute` runs the region body. `export`/`import` are invoked by the host after
the binding resolver computes the DAG; glue lines are stripped from `code`.
`eval` exists for inline islands `⟦lang: expr⟧`: the host evaluates the expression
in its own language's worker, receives a Meld Value, then renders that value as
a source literal using the *containing* pack's `inline.toml` style. **JUDGMENT
CALL:** evaluation and rendering are split this way so the inline language owns
semantics while the containing language owns syntax. A worker advertises `eval`
in its handshake `caps`; packs that do not support it must remain usable for
ordinary regions.

**JUDGMENT CALL:** glue-declared exports (`export x as y`) are authoritative and
are read with `fetch({name:"x"})` after `execute`; the worker publishes the
native binding `x` in its namespace. The `exports` array in the `execute`
response is an optional shortcut for workers that push values proactively, and
is overridden by any glue-declared export of the same bridge name.

## 3.1 Worker-initiated host calls (Phase 4)

A worker may ask the host to resolve a callable while an `execute` request is
still in flight. This is how a Python region calls a Rust/WIT function without
linking Wasmtime into the worker.

Worker → host:

```json
{"id":-1,"method":"__host_call","params":{"fn":"meld:t3/add","args":["20","22"]}}
```

Host → worker:

```json
{"id":-1,"result":{"value":"42"}}
```

**JUDGMENT CALL:** callback ids are negative, unique per request, and cannot
collide with the host's positive request ids. The host handles the callback on
a bounded helper thread and then continues waiting for the original response;
no second stdout reader is needed. Callbacks are optional and must return
structured errors, never crash the worker. `meld_call(name, *args)` is the
Python pack's convenience binding.

**JUDGMENT CALL:** a worker cannot reach an arbitrary host callback. With no
callback registered, all `__host_call` requests are denied. Registering one
(`T2Backend::set_host_call`) allowlists only the reserved `meld:t3/*` namespace;
other `meld:*` names need an explicit `allow_host_call` entry. Unknown names,
non-negative or duplicate callback ids, malformed params, and more than 64
callbacks for one request are hard denials. A callback that does not answer in
2 s causes the host to stop waiting, kill the worker, and report a timeout; the
callback thread itself cannot be cancelled safely, so callers must keep
callbacks short-lived.

## 4. Values

Values use the Meld Value Model wire encoding (`spec/value-model.md`). Errors
are `{"error":{"code":"...","message":"...","source":{...}}}` and are values,
not process failures. A process crash is reported by the host as
`WorkerCrashed` with captured stderr.

## 5. Ordering and stdout

The scheduler executes regions in file order and merges stdout in that order.
Each `execute` response includes an ordered array of stdout lines so interleaving
is deterministic. **JUDGMENT CALL:** async regions are reserved but not active
through Phase 3 (project.md §13 default).

## 6. Sandbox

Default worker launch: no network, cwd = project dir, filesystem access limited
to project dir via the launcher profile. **OS confinement is mandatory for T2**
(project.md §9). The launcher must resolve the effective policy as the
**intersection** `pack maximum ∩ runner constraints ∩ file default grant ∩
region grant`; grants only narrow, and a missing `[sandbox]` declaration means
network denied and an empty filesystem grant set. The `meld-sandbox` core
provides the policy model and fail-closed platform backends. If a declared
policy cannot be enforced on the current host, launch fails with
`SandboxDenied`; there is no silent unconfined fallback. `runner.toml` declares
the profile, `meld doctor` reports capability, and `meld-sandbox` is the
authority at spawn time. The stdio hardening in §1–§3.1 is defence in depth, not
a substitute for the OS sandbox.

## 7. Example session (Python worker)

```
→ {"id":1,"method":"initialize",...}
← {"id":1,"result":{"protocol":"meld-t2/1","level":"L2",...}}
→ {"id":2,"method":"execute","params":{"region_id":"r0","code":"print('hi')","env":{}}}
← {"id":2,"result":{"stdout":["hi"],"exports":[]}}
→ {"id":3,"method":"shutdown","params":{}}
← {"id":3,"result":{"ok":true}}
```
