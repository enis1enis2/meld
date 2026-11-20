# Production security model and threat model

Status: normative for production hardening. Authority: `../project.md` §9
(security model), §5 (execution tiers), §10 (Rust core), and §13 (reuse WIT).
This document records the security invariants that production code must
enforce. It is a judgment-call record as well as a threat model: every
**JUDGMENT CALL** below is binding until a later spec changes it. Runtime
evidence belongs in `../status/production-readiness.md`; this file does not
claim that any control is complete.

## 1. Assets

1. **Host authority** — filesystem, network, environment, process tree,
   devices, and credentials available to the user running `meld`.
2. **User source and workspaces** — `.meld` files, imported libraries, editor
   buffers, build outputs, and local caches.
3. **Supply-chain roots** — Ed25519 signing keys, trust roots, revocations,
   registry attestations, release signing identities, and checksum manifests.
4. **Availability** — CPU, memory, file descriptors, process slots, and wall
   clock time of the host and CI runners.
5. **Integrity of results** — correct region boundaries, source maps, value
   decoding, diagnostics, and reproducible artifacts.

## 2. Trust boundaries

| Boundary | Trust given | Rationale |
|---|---|---|
| Rust core (`meld-core`, `meld-value`, scheduler, CLI, LSP host) | Trusted code, untrusted input | Memory-safe core; all inputs still validated and bounded. |
| `.meld` source, including `grant(...)` and region bodies | Untrusted | Author-controlled text may be malicious or malformed. Grants may only narrow authority. |
| Lang Pack data (`pack.toml`, `runner.toml`, queries, LSP/formatter metadata, `region_end.toml`) | Authenticated but not safe | A valid signature proves origin, not benign behavior. Pack content is data except for the explicitly declared runtime command. |
| T2/T5 worker process | Untrusted | It executes pack/runtime code. Protocol cooperation is not a security boundary. |
| T3 component | Untrusted | Wasm is isolated by the engine but may be hostile and resource-exhausting. |
| Registry, mirrors, index, and pack archives | Untrusted transport | Integrity comes from Ed25519 signatures, trust roots, revocations, and a signed attestation. |
| External LSP/formatters and toolchains | Untrusted subprocesses | Must be invoked with argv vectors and placed under the same OS policy as T2/T5. |
| Release/installer channels | Untrusted until verified | Artifacts are trusted only after signature, checksum, and manifest verification. |

**JUDGMENT CALL:** a signed pack is authenticated data, never automatically a
safe program. Signing protects the supply chain; OS enforcement protects the
host. Neither substitutes for the other.

## 3. Adversaries and capabilities

- **Malicious `.meld` author:** controls source text, delimiters, region
  bodies, imports, and grant expressions; may attempt parser confusion,
  delimiter injection, path traversal, or capability escalation.
- **Malicious pack publisher:** controls pack files and declared commands; may
  attempt to smuggle executable code in scanner/LSP metadata, over-declare
  capabilities, point queries at host files, or ship malicious grammar blobs.
- **Compromised runtime or third-party library:** controls a T2/T5 worker or
  external tool; may ignore the protocol, flood stdout/stderr, hang, spawn
  children, open network sockets, or read environment secrets.
- **Malicious Wasm guest:** attempts import smuggling, unbounded loops,
  allocator pressure, handle exhaustion, or hostcall abuse.
- **Network/registry attacker:** controls mirrors or transport and may tamper
  with archives, checksums, metadata, versions, or revocation data.
- **Key/repository attacker:** attempts to introduce a signing key, revoke a
  legitimate key, downgrade a package, or commit private key material.
- **Local co-tenant:** on shared CI hosts, attempts to observe temp files,
  caches, or process output.

## 4. Security invariants

### 4.1 Deny-by-default authority

1. A region starts with no filesystem and no network access unless a policy
   explicitly grants them. Missing `[sandbox]` means `network=none` and `fs=[]`.
2. `grant(...)` expressions may only narrow authority. They cannot add a
   capability absent from the pack maximum, runner constraints, or file default
   grant.
3. The effective policy is the intersection:

   ```text
   pack maximum ∩ runner constraints ∩ file default grant ∩ region grant
   ```

4. Unknown capabilities, unreadable policy files, `..` path traversal,
   symlink escapes, overlapping unsafe roots, and inconsistent manifests are
   hard errors.
5. If a declared policy cannot be enforced by an available OS backend, the
   operation fails closed. The host must not silently run with fewer
   restrictions or more authority.

**JUDGMENT CALL:** sandbox authority is an intersection, not a union, and
enforcement failure is an execution error. This prevents a region grant from
turning a pack or runner limitation into host authority.

### 4.2 Tier enforcement

- **T1/T4:** declarative native code; no implicit capabilities. Any command or
  external tool runs through the same OS confinement path as T2/T5.
- **T2:** the JSON line protocol is a compatibility contract only. It does not
  enforce capabilities. Worker launch must apply OS-enforced confinement plus
  bounded I/O, timeouts, process-tree termination, and a hostcall allowlist.
- **T3:** Wasmtime isolation is enforced independently with a strict config:
  no WASI, no unexpected imports, fresh store per call, fuel and epoch
  deadlines, stack/memory/table/instance/handle limits, and trap-to-denied
  mapping. T3 must not depend on the OS sandbox backend being available.
- **T5:** argv vectors only; no shell strings. Placeholder substitution must
  stay inside a per-call temp directory. Toolchain execution uses the same
  deny-by-default OS policy.

**JUDGMENT CALL:** T2 protocol declarations are requests, not enforcement;
T3 limits are strict even when OS confinement is present. Multiple layers are
intentional because the tiers have different failure modes.

### 4.3 Parsing and scanner safety

1. End-of-region detection consumes source as data. Pack-provided scanner
   code, including `region_end.py/js/lua/sh/wat`, is never executed.
2. `region_end.toml` may select a built-in, data-only algorithm by name and
   options. Unknown schema, unknown kind, `kind=wasm`, or malformed options
   warn and fall back to the conservative brace scanner.
3. Raw hash delimiters remain a normative escape hatch when a scanner cannot
   describe a language's lexical state.
4. Scanners must preserve byte offsets and `render(parse(src)) == src`.
   Adversarial cases include heredocs with braces, multiple(FIFO) heredocs,
   triple-quoted strings, raw strings, nested comments, template interpolation,
   and SQL dollar quotes.
5. Source maps and diagnostics must not trust scanner output blindly; offsets
   are validated against the original source buffer.

**JUDGMENT CALL:** the composition research rejected SGLR/SDF3/Raku-slang
composition. Production uses pack-owned declarative scanner profiles implemented
in Rust and keeps raw delimiters as the fallback, avoiding arbitrary code
execution at parse time.

### 4.4 Supply chain and signing

1. Pack archives are signed with Ed25519. The signed message is versioned and
   binds the pack id, version, and the raw bytes of `checksums.sha256`.
2. Trust is anchored in `packs/registry/trust-roots.toml`. Key ids are derived
   from the public key. Revoked key ids in `packs/registry/revocations.json`
   are rejected even if the key remains in the trust roots.
3. HMAC is removed. A symmetric development key cannot be a public trust root
   and is never accepted as one.
4. Registry metadata, trust roots, revocations, and mirrors are bound by a
   signed attestation. Mirrors are untrusted transport; signatures and
   checksums are verified after download.
5. Private keys live outside the repository, CI secrets, and artifacts. Test
   keys are generated per test run. Public material only is committed.
6. Release artifacts are reproducible where the platform permits, accompanied
   by checksums, SBOM, provenance, and a signed release manifest. The system
   package is named `meld-lang`; binaries remain `meld`, `meld-lsp`, and
   `meld-pack`.

**JUDGMENT CALL:** the hard cutover to Ed25519 replaces HMAC entirely; rotation
is adding a trust root and revocation is an explicit signed registry update.
This is preferable to a compatibility mode that would leave symmetric
verification as a downgrade path.

### 4.5 Availability and resource control

- All protocol reads, stderr pumps, and line lengths are bounded.
- Worker sessions have idle and overall timeouts and terminate the entire
  process tree on failure.
- Wasm execution has fuel, epoch interruption, memory/table/handle limits, and
  a watchdog. Exhaustion maps to a structured sandbox denial, not a host panic.
- Fuzz targets cover every parser/decoder boundary: splitter/scanners, Meld
  value decoding, T2 envelopes, pack manifests, trust roots, revocations, and
  installer inputs.
- Pack extraction rejects traversal, absolute paths, symlinks that leave the
  destination, oversized entries, and undeclared files.

## 5. Attack scenarios and required mitigations

| Scenario | Required mitigation |
|---|---|
| Ruby heredoc contains `}`; generic brace scanner ends region early | Heredoc/lexer scanner selected by pack metadata; render/idempotence tests; raw hash escape hatch. |
| A pack adds `region_end.py` that runs shell commands | Only `region_end.toml` is parsed; executable extensions are never loaded; unknown kinds fail closed/fallback. |
| Region declares `grant(net=all)` for a network-denied pack | Intersection rejects or drops the granted capability; effective policy remains network-denied. |
| Worker writes unbounded stdout or never responds | Bounded reads, stderr draining, idle/overall timeouts, process-tree kill, structured error. |
| Worker emits an unexpected `__host_call` name | Hostcall allowlist rejects it and returns a structured error. |
| Wasm guest loops forever or allocates enormous memory | Fuel/epoch deadlines and store/resource limits trap as `SandboxDenied`. |
| Mirror serves a tampered pack | Checksum and Ed25519 verification against trust roots; revoked keys rejected; attestation checked. |
| Attacker adds a new public key to trust roots | Trust-root changes are a release/registry control point; attestation binds canonical roots; CI and release review must detect unauthorized changes. |
| Private key accidentally committed | Secret scanning in CI; key generation outside repos; test keys ephemeral; release keys in CI secrets only. |
| Installer follows a symlink outside the install root | Canonical path checks, reject symlinks/traversal, install into a controlled root, verify before extraction. |
| External LSP server reads arbitrary host files | Same deny-by-default OS policy as T2/T5; grants can only narrow; fail closed if enforcement is unavailable. |

## 6. Residual risks

1. This Linux host cannot runtime-prove macOS Seatbelt or Windows Job
   Objects/AppContainer behavior. Native CI on those OSes is required.
2. Landlock may be unavailable (`ENOSYS`) on older kernels. Code may include a
   best-effort Landlock layer, but the primary Linux backend must still fail
   closed using namespaces/seccomp/cgroups or refuse execution.
3. OCI/runc fallback depends on a correctly configured host runtime. It is an
   optional backend and must be probed and attested before use.
4. Signed packs are authenticated, not reviewed. A malicious trusted key holder
   remains a supply-chain risk mitigated by key rotation, revocation, release
   review, and reproducibility.
5. Source-level scanners improve correctness but cannot be complete for every
   language. Raw delimiters remain the correctness escape hatch.
6. Performance counters and timing side channels are out of scope unless a
   deployment explicitly requires multi-tenant isolation.

## 7. Verification obligation

No control in this document is production-complete until its implementation is
executed and evidence is recorded in
`../status/production-readiness.md`, including:

- workspace fmt, clippy, tests, docs, Python tests, fuzz smoke, and examples;
- `cargo audit` and `cargo deny`;
- Ed25519 pack/registry verification, tampering, unknown key, rotation, and
  revocation tests;
- scanner adversarial and idempotence tests;
- policy intersection, escape-attempt, and fail-closed backend tests;
- native Linux/macOS/Windows CI for OS enforcement;
- reproducibility, artifact signature, installer, and fresh-install smoke
  checks.

## 8. Judgment call index

1. Authenticated pack data is not automatically safe code (§2).
2. Sandbox authority is an intersection and enforcement failure fails closed (§4.1).
3. T2 protocol declarations do not enforce; T3 limits are independent (§4.2).
4. Pack scanner code is never executed; data-only profiles plus raw delimiters (§4.3).
5. HMAC is removed in favor of Ed25519 trust roots, rotation, revocation, and attestation (§4.4).
6. Wasmtime is pinned to a version compatible with the workspace MSRV rather than
   raising the MSRV for production hardening (recorded during implementation).
7. Release artifacts are signed and reproducible where possible, with honest
   platform limitations recorded rather than claimed (§4.4, §6).
