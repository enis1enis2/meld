//! Phase 4 compiled tiers: T3 Wasm components and the T5 toolchain fallback.
//!
//! JUDGMENT CALL: T3 is demonstrated with a real Rust function compiled to a
//! Component Model component and invoked through Wasmtime's typed component
//! API. The WIT file (`wit/meld.wit`) is the IDL; the host never guesses a
//! signature from raw core-Wasm exports. T5 is deliberately generic: it writes
//! a region to a temp file, runs a pack-declared build command, then runs the
//! produced artifact. It never downloads or installs a toolchain.
use meld_core::error::{MeldError, Result};
use meld_sandbox::{
    Access, Capability, CommandSpec, SafeRoot, SandboxBackend, SandboxError, SandboxPolicy,
    SpawnedProcess,
};
use meld_value::Value;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::env;
use std::fs;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wasmtime::component::{Component, Linker};
use wasmtime::{
    Config, Engine, PoolingAllocationConfig, Store, StoreLimits, StoreLimitsBuilder, Trap,
};

/// Deterministic instruction budget per T3 call. Exhaustion is a sandbox
/// denial, not a generic Wasmtime error.
const T3_FUEL_LIMIT: u64 = 2_000_000;
/// Coarse wall-clock backstop. A watchdog increments the engine epoch once
/// after this duration; stores use a one-tick deadline, so the guest is trapped
/// on its next epoch check.
const T3_EPOCH_TIMEOUT: Duration = Duration::from_millis(500);
/// 8 MiB is ample for the committed demo components and small enough to keep a
/// hostile guest from pinning host memory.
const T3_MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
/// Host-side allocation budget for one guest-to-host component call.
const T3_HOSTCALL_FUEL: usize = 64 * 1024;
/// Cap on simultaneously live component handles (resources are a cheap DoS).
const T3_MAX_COMPONENT_HANDLES: usize = 256;
/// Refuse oversized binaries before JIT compilation, which is itself costly.
const T3_MAX_COMPONENT_BYTES: usize = 16 * 1024 * 1024;
/// SHA-256 of the committed `component/demo.component.wasm`. The release gate
/// is enforced by [`T3Backend::demo`], not merely documented for CI.
pub const DEMO_COMPONENT_SHA256: &str =
    "1f4ad3f28b77075cded9a3c0dcf3bca5cd91a7595a57da3079bfd4104e7f30e1";

/// Build the single hardened Wasmtime configuration used by every T3 call.
///
/// JUDGMENT CALL: the pooling allocator plus store limits give deterministic
/// caps on instances, memories and tables; fuel and epochs bound execution even
/// when a guest never returns. Debug info, backtraces and concurrency-heavy
/// proposals are off so a hostile component cannot turn diagnostics or threads
/// into an escape or resource-amplification channel.
pub fn strict_config() -> Config {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config.debug_info(false);
    config.wasm_backtrace(false);
    config.wasm_threads(false);
    config.wasm_multi_memory(false);
    config.wasm_tail_call(false);
    config.wasm_gc(false);
    config.max_wasm_stack(1024 * 1024);

    let mut pooling = PoolingAllocationConfig::new();
    pooling.total_component_instances(1);
    pooling.total_core_instances(4);
    pooling.total_memories(4);
    pooling.total_tables(4);
    pooling.table_elements(1024);
    pooling.max_memory_size(T3_MAX_MEMORY_BYTES);
    pooling.max_component_instance_size(1024 * 1024);
    config.allocation_strategy(pooling);
    config
}

/// Engine built from [`strict_config`]. Exposed for hosts that need to create
/// related Wasmtime values against exactly the same hardening posture.
pub fn strict_engine() -> Engine {
    Engine::new(&strict_config()).expect("strict T3 config must be valid")
}

/// Hex-encode a digest without pulling a formatting helper into the hot path.
fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Verify the committed demo bytes before they reach Wasmtime. A mismatch is a
/// sandbox denial: this is a release/provenance gate, not a diagnostic.
pub fn verify_demo_component_bytes(wasm: &[u8]) -> Result<()> {
    let actual = hex_digest(&Sha256::digest(wasm));
    if actual != DEMO_COMPONENT_SHA256 {
        return Err(MeldError::SandboxDenied(format!(
            "committed T3 component digest mismatch: expected {DEMO_COMPONENT_SHA256}, got {actual}"
        )));
    }
    Ok(())
}

/// Classify an unstructured Wasmtime error by a small allowlist of stable
/// keywords. Only the fixed label is returned, never any part of the diagnostic
/// text (which may carry host paths or register dumps).
fn wasm_error_category(error: &wasmtime::Error) -> &'static str {
    let text = format!("{error:#}").to_ascii_lowercase();
    if text.contains("handle") || text.contains("resource") {
        "resource handle failure"
    } else if text.contains("fuel") {
        "fuel budget exhausted"
    } else if text.contains("epoch") || text.contains("interrupt") {
        "wall-clock deadline exceeded"
    } else if text.contains("stack") {
        "wasm stack limit exceeded"
    } else if text.contains("memory") || text.contains("bounds") {
        "wasm memory bounds violation"
    } else if text.contains("type") {
        "wasm type mismatch"
    } else {
        "wasm runtime failure"
    }
}

/// Convert Wasmtime failures into Meld errors without leaking a raw backtrace.
/// Resource-limit traps are sandbox denials; epoch interruption is a worker
/// timeout; ordinary traps are reported by code only. The raw `Display` text is
/// deliberately discarded: Wasmtime diagnostics can contain host paths, register
/// dumps and source snippets, none of which belong in a Meld error (T3 detail
/// normalization).
fn map_wasm_error(context: &str, error: wasmtime::Error) -> MeldError {
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => {
                MeldError::SandboxDenied(format!("{context}: fuel budget exhausted"))
            }
            Trap::Interrupt => {
                MeldError::WorkerCrashed(format!("{context}: wall-clock deadline exceeded"))
            }
            Trap::StackOverflow => {
                MeldError::SandboxDenied(format!("{context}: wasm stack limit exceeded"))
            }
            other => MeldError::Wasm(format!("{context}: wasm trap {other:?}")),
        };
    }
    // Stable, sanitized category only. Never interpolate `error` here.
    MeldError::Wasm(format!("{context}: {}", wasm_error_category(&error)))
}

/// The demo component's canonical export, derived from `wit/meld.wit`.
pub const ADD_EXPORT: &str = "meld:demo/ops@0.1.0#add";
pub const MUL_EXPORT: &str = "meld:demo/ops@0.1.0#mul";

/// T3 backend holding the compiled component. `Engine` and `Component` are
/// Send+Sync, so a single instance can be shared by T2 host callbacks.
pub struct T3Backend {
    engine: Engine,
    component: Component,
    /// One engine epoch is shared by every store built from this engine. Without
    /// serialization, a watchdog armed for call A can interrupt call B. The
    /// lock spans the watchdog so no two epoch deadlines can overlap.
    call_lock: Mutex<()>,
}

impl T3Backend {
    pub fn from_bytes(wasm: &[u8]) -> Result<Self> {
        if wasm.is_empty() || wasm.len() > T3_MAX_COMPONENT_BYTES {
            return Err(MeldError::SandboxDenied(format!(
                "T3 component size {} outside 1..={T3_MAX_COMPONENT_BYTES} bytes",
                wasm.len()
            )));
        }
        let engine = strict_engine();
        let component = Component::new(&engine, wasm)
            .map_err(|_| MeldError::Wasm("T3 component decode failed".to_string()))?;
        // No host functions are registered anywhere in T3. A component that
        // imports anything is therefore rejected up front rather than allowed
        // to fail late with an ambiguous linker error.
        if component.component_type().imports(&engine).next().is_some() {
            return Err(MeldError::SandboxDenied(
                "T3 component declares imports; host imports are disabled".to_string(),
            ));
        }
        Ok(Self {
            engine,
            component,
            call_lock: Mutex::new(()),
        })
    }

    /// Load the committed Rust demo component (see tools/build_t3_guest.sh).
    ///
    /// PROVENANCE GATE: `component/demo.component.wasm` is a committed binary
    /// artifact. The recorded digest is checked here on every load, so a swapped
    /// artifact fails closed before JIT compilation even if build tooling forgets
    /// its provenance check.
    pub fn demo() -> Result<Self> {
        let wasm = include_bytes!("../component/demo.component.wasm");
        verify_demo_component_bytes(wasm)?;
        Self::from_bytes(wasm)
    }

    /// Start a per-call watchdog. The channel lets a fast call cancel the
    /// thread immediately; only a genuine timeout increments the engine epoch.
    fn start_watchdog(&self) -> Result<(mpsc::Sender<()>, thread::JoinHandle<()>)> {
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let engine = self.engine.clone();
        let handle = thread::Builder::new()
            .name("meld-t3-epoch".to_string())
            .spawn(move || {
                if cancel_rx.recv_timeout(T3_EPOCH_TIMEOUT).is_err() {
                    engine.increment_epoch();
                }
            })
            .map_err(|e| MeldError::WorkerCrashed(format!("T3 watchdog spawn: {e}")))?;
        Ok((cancel_tx, handle))
    }

    /// Call a WIT `func(a: s64, b: s64) -> s64` export with typed arguments.
    /// Every call gets a fresh bounded store; no state leaks between calls.
    pub fn call_i64_pair(&self, export: &str, a: i64, b: i64) -> Result<i64> {
        // Serialize the whole call, including watchdog cancellation. A poisoned
        // lock means a previous call panicked while owning the engine; fail
        // closed rather than sharing a possibly corrupted epoch state.
        let _call = self.call_lock.lock().map_err(|_| {
            MeldError::WorkerCrashed("T3 call lock poisoned by an earlier call".to_string())
        })?;
        // JUDGMENT CALL: componentizing a WIT interface nests functions under
        // an interface instance. Resolve the two-level export index instead of
        // assuming a flat core-Wasm export name.
        let (iface_name, func_name) = export
            .split_once('#')
            .unwrap_or(("meld:demo/ops@0.1.0", export));
        let iface_index = self
            .component
            .get_export_index(None, iface_name)
            .ok_or_else(|| MeldError::Wasm(format!("no interface export {iface_name}")))?;
        let func_index = self
            .component
            .get_export_index(Some(&iface_index), func_name)
            .ok_or_else(|| MeldError::Wasm(format!("no function export {export}")))?;

        let (cancel, watchdog) = self.start_watchdog()?;
        let result = (|| -> wasmtime::Result<i64> {
            let limits = StoreLimitsBuilder::new()
                .memory_size(T3_MAX_MEMORY_BYTES)
                .table_elements(1024)
                .instances(1)
                .tables(4)
                .memories(4)
                .trap_on_grow_failure(true)
                .build();
            let mut store: Store<StoreLimits> = Store::new(&self.engine, limits);
            store.limiter(|data| data);
            store.set_fuel(T3_FUEL_LIMIT)?;
            store.set_epoch_deadline(1);
            store.epoch_deadline_trap();
            store.set_hostcall_fuel(T3_HOSTCALL_FUEL);
            store.set_max_component_handles(T3_MAX_COMPONENT_HANDLES);

            let linker = Linker::new(&self.engine);
            let instance = linker.instantiate(&mut store, &self.component)?;
            // JUDGMENT CALL: Wasmtime models component results as a named list;
            // a single s64 result is therefore the unary tuple `(i64,)`, not `i64`.
            let func = instance.get_typed_func::<(i64, i64), (i64,)>(&mut store, &func_index)?;
            let (result,) = func.call(&mut store, (a, b))?;
            Ok(result)
        })();
        let _ = cancel.send(());
        let _ = watchdog.join();
        result.map_err(|e| map_wasm_error(&format!("T3 {export}"), e))
    }

    /// Host-callback entry point used by the T2 bridge. Resolves reserved
    /// `meld:t3/*` names to the demo WIT exports and converts Meld Values at
    /// the boundary. Narrowing to s64 is checked, never silent.
    pub fn call_value(&self, name: &str, args: &[Value]) -> Result<Value> {
        let export = match name {
            "meld:t3/add" => ADD_EXPORT,
            "meld:t3/mul" => MUL_EXPORT,
            other => {
                return Err(MeldError::TypeMismatch(format!(
                    "unknown T3 export {other:?}"
                )))
            }
        };
        if args.len() != 2 {
            return Err(MeldError::TypeMismatch(format!(
                "{name} expects 2 s64 args, got {}",
                args.len()
            )));
        }
        let a = args[0]
            .as_i64_checked()
            .map_err(|e| MeldError::TypeMismatch(format!("arg 0: {e}")))?;
        let b = args[1]
            .as_i64_checked()
            .map_err(|e| MeldError::TypeMismatch(format!("arg 1: {e}")))?;
        Ok(Value::int(self.call_i64_pair(export, a, b)? as i128))
    }
}

/// T5 toolchain fallback. Commands are argv vectors (no shell interpolation
/// by Meld) with `{src}`, `{out}` and `{dir}` placeholders substituted.
///
/// JUDGMENT CALL 56: a T5 backend without an attached
/// [`SandboxBackend`] is not a "best effort"
/// fallback. It fails closed. The host must supply both the enforcement backend
/// and the effective policy through [`T5Backend::with_sandbox`].
pub struct T5Backend {
    pub lang: String,
    pub build: Vec<String>,
    pub run: Vec<String>,
    pub cwd: PathBuf,
    sandbox: Option<Arc<dyn SandboxBackend>>,
    policy: Option<SandboxPolicy>,
}

static T5_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Variables a pack-declared toolchain needs in order to find its compiler and
/// runtime. `CommandSpec` clears the environment, so these are copied
/// explicitly and then filtered again through the stage policy's allowlist.
const T5_TOOLCHAIN_ENV_KEYS: &[&str] = &["PATH", "SystemRoot", "TEMP", "TMP", "PATHEXT", "COMSPEC"];

/// Exclusive, unpredictable scratch directory with RAII cleanup. The random
/// component comes from `RandomState`'s OS seed; `create` (never
/// `create_dir_all`) makes collision attempts fail rather than reuse another
/// process's directory.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn create(lang: &str, seq: u64) -> Result<Self> {
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let label: String = lang
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .take(24)
            .collect();
        let label = if label.is_empty() { "lang" } else { &label };
        for attempt in 0..8u64 {
            let entropy = RandomState::new().hash_one((pid, seq, nanos, attempt));
            let path =
                env::temp_dir().join(format!("meld-t5-{label}-{pid}-{seq:x}-{entropy:016x}"));
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(MeldError::Io(error)),
            }
        }
        Err(MeldError::SandboxDenied(
            "could not create an exclusive T5 scratch directory".to_string(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn sandbox_error(stage: &str, error: SandboxError) -> MeldError {
    MeldError::SandboxDenied(format!("T5 {stage}: {}", error.denial_message()))
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

/// Resolve a program to an absolute path so the sandbox policy can grant exec
/// on the real file rather than on a PATH lookup that the backend cannot see.
fn resolve_program(program: &str, cwd: &Path) -> PathBuf {
    let raw = Path::new(program);
    if raw.components().count() > 1 || raw.is_absolute() {
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            cwd.join(raw)
        };
        return canonical_or_original(candidate);
    }
    if let Some(paths) = env::var_os("PATH") {
        #[allow(unused_mut)] // Windows appends PATHEXT candidates.
        let mut names = vec![raw.to_path_buf()];
        #[cfg(windows)]
        {
            let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string());
            for extension in extensions.split(';').filter(|ext| !ext.is_empty()) {
                names.push(PathBuf::from(format!("{program}{extension}")));
            }
        }
        for directory in env::split_paths(&paths) {
            for name in &names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return canonical_or_original(candidate);
                }
            }
        }
    }
    raw.to_path_buf()
}

impl T5Backend {
    pub fn new(
        lang: impl Into<String>,
        build: Vec<String>,
        run: Vec<String>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            lang: lang.into(),
            build,
            run,
            cwd,
            sandbox: None,
            policy: None,
        }
    }

    /// Attach the enforcement backend and effective policy. Every build/run
    /// argv is then handed to `SandboxBackend::spawn` as a `CommandSpec`;
    /// without this call [`compile_and_run`](Self::compile_and_run) denies.
    pub fn with_sandbox<B>(mut self, backend: B, policy: SandboxPolicy) -> Self
    where
        B: SandboxBackend + 'static,
    {
        self.sandbox = Some(Arc::new(backend));
        self.policy = Some(policy);
        self
    }

    /// Variant for callers that need to share one backend instance.
    pub fn with_shared_sandbox(
        mut self,
        backend: Arc<dyn SandboxBackend>,
        policy: SandboxPolicy,
    ) -> Self {
        self.sandbox = Some(backend);
        self.policy = Some(policy);
        self
    }

    fn fill(template: &str, dir: &Path) -> String {
        // JUDGMENT CALL 55: substituted paths use forward slashes. Windows
        // accepts them in CreateProcess paths, while Git Bash and other POSIX
        // toolchains would otherwise consume `C:\...` backslashes as shell
        // escapes. The filesystem paths themselves are unchanged.
        let portable = |path: PathBuf| path.to_string_lossy().replace('\\', "/");
        let src = portable(dir.join("region.src"));
        let out = portable(dir.join("region.bin"));
        let root = portable(dir.to_path_buf());
        template
            .replace("{src}", &src)
            .replace("{out}", &out)
            .replace("{dir}", &root)
    }

    /// Derive the stage policy from the caller's effective policy.
    ///
    /// JUDGMENT CALL: the scratch directory is host-managed, not a region file
    /// grant. A random path cannot be named in a static manifest, so T5 grants
    /// read/write/exec only on that fresh directory and on the exact toolchain
    /// executables. Limits, network and environment allowlists are inherited
    /// unchanged; no other filesystem authority is added.
    fn stage_policy(
        base: &SandboxPolicy,
        scratch: &Path,
        programs: &[PathBuf],
        seq: u64,
    ) -> Result<SandboxPolicy> {
        let mut policy = SandboxPolicy::deny_all(format!("{}:t5-{seq}", base.name));
        policy.limits = base.limits;
        policy.network = base.network.clone();
        policy.env = base.env.clone();
        for key in T5_TOOLCHAIN_ENV_KEYS {
            policy.env.insert((*key).to_string());
        }
        policy.capabilities.allow(Capability::FsRead);
        policy.capabilities.allow(Capability::FsWrite);
        policy.capabilities.allow(Capability::FsExec);

        let scratch_root = format!("t5-scratch-{seq}");
        policy
            .fs
            .add_root(
                SafeRoot::new(scratch_root.clone(), scratch)
                    .map_err(|e| sandbox_error("policy", e))?,
            )
            .map_err(|e| sandbox_error("policy", e))?;
        policy
            .fs
            .grant(&scratch_root, "/**", Access::Read)
            .map_err(|e| sandbox_error("policy", e))?;
        policy
            .fs
            .grant(&scratch_root, "/**", Access::Write)
            .map_err(|e| sandbox_error("policy", e))?;
        policy
            .fs
            .grant(&scratch_root, "/**", Access::Exec)
            .map_err(|e| sandbox_error("policy", e))?;

        let mut parents = std::collections::BTreeSet::new();
        for (index, program) in programs.iter().enumerate() {
            if program.starts_with(scratch) {
                continue;
            }
            let parent = program.parent().unwrap_or_else(|| Path::new("/"));
            if !parents.insert(parent.to_path_buf()) {
                continue;
            }
            let name = format!("t5-tool-{seq}-{index}");
            policy
                .fs
                .add_root(
                    SafeRoot::new(name.clone(), parent).map_err(|e| sandbox_error("policy", e))?,
                )
                .map_err(|e| sandbox_error("policy", e))?;
            let file = program
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    MeldError::SandboxDenied(
                        "T5 toolchain path has no representable file name".to_string(),
                    )
                })?;
            policy
                .fs
                .grant(&name, file, Access::Exec)
                .map_err(|e| sandbox_error("policy", e))?;
        }
        policy.validate().map_err(|e| sandbox_error("policy", e))?;
        Ok(policy)
    }

    fn spawn_stage(
        backend: &dyn SandboxBackend,
        policy: &SandboxPolicy,
        cwd: &Path,
        argv: &[String],
        stage: &str,
    ) -> Result<SpawnedProcess> {
        let program = argv
            .first()
            .ok_or_else(|| MeldError::SandboxDenied(format!("T5 {stage} command is empty")))?;
        let mut spec = CommandSpec::new(program, argv[1..].iter().cloned()).with_cwd(cwd);
        for key in T5_TOOLCHAIN_ENV_KEYS {
            if let Some(value) = env::var_os(key) {
                spec = spec.with_env(*key, value.to_string_lossy().into_owned());
            }
        }
        backend
            .spawn(&spec, policy)
            .map_err(|e| sandbox_error(stage, e))
    }

    /// Run one stage with bounded line reads and a bounded wait. The backend's
    /// `SpawnedProcess` kills the whole process tree on any violation.
    fn collect_stage(
        backend: &dyn SandboxBackend,
        policy: &SandboxPolicy,
        cwd: &Path,
        argv: &[String],
        stage: &str,
    ) -> Result<Vec<String>> {
        let mut child = Self::spawn_stage(backend, policy, cwd, argv, stage)?;
        child.close_stdin();
        let mut lines = Vec::new();
        loop {
            match child.read_line() {
                Ok(Some(line)) => lines.push(line),
                Ok(None) => break,
                Err(error) => {
                    let stderr = child.drain_stderr();
                    return Err(MeldError::SandboxDenied(format!(
                        "T5 {stage}: {}; stderr: {}",
                        error.denial_message(),
                        stderr.trim()
                    )));
                }
            }
        }
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                let stderr = child.drain_stderr();
                return Err(MeldError::SandboxDenied(format!(
                    "T5 {stage}: {}; stderr: {}",
                    error.denial_message(),
                    stderr.trim()
                )));
            }
        };
        if !status.success() {
            let stderr = child.drain_stderr();
            return Err(MeldError::WorkerCrashed(format!(
                "T5 {stage} failed ({status}): {}",
                stderr.trim()
            )));
        }
        Ok(lines)
    }

    /// Compile and execute a region. A final stdout line beginning `@meld `
    /// is parsed as a JSON Meld Value; everything else is ordinary stdout.
    /// **JUDGMENT CALL:** this keeps T5 output compatible with T2's ordered
    /// stdout contract while giving compiled regions a typed return path.
    pub fn compile_and_run(
        &self,
        code: &str,
        args: &[Value],
    ) -> Result<(Vec<String>, Option<Value>)> {
        let backend = self.sandbox.as_deref().ok_or_else(|| {
            MeldError::SandboxDenied(
                "T5 requires a sandbox backend; attach one with with_sandbox".to_string(),
            )
        })?;
        let attached_policy = self.policy.as_ref().ok_or_else(|| {
            MeldError::SandboxDenied("T5 requires an effective sandbox policy".to_string())
        })?;

        let seq = T5_COUNTER.fetch_add(1, Ordering::Relaxed);
        let scratch = ScratchDir::create(&self.lang, seq)?;
        fs::write(scratch.path().join("region.src"), code)?;

        let mut build_args: Vec<String> = self
            .build
            .iter()
            .map(|a| Self::fill(a, scratch.path()))
            .collect();
        let build_program = {
            let program = build_args
                .first()
                .ok_or_else(|| MeldError::SandboxDenied("T5 build command is empty".to_string()))?;
            resolve_program(program, &self.cwd)
        };
        build_args[0] = build_program.to_string_lossy().into_owned();

        let mut run_args: Vec<String> = self
            .run
            .iter()
            .map(|a| Self::fill(a, scratch.path()))
            .collect();
        let run_program = {
            let program = run_args
                .first()
                .ok_or_else(|| MeldError::SandboxDenied("T5 run command is empty".to_string()))?;
            resolve_program(program, &self.cwd)
        };
        run_args[0] = run_program.to_string_lossy().into_owned();
        // JUDGMENT CALL: JSON arguments are passed as one trailing argv value,
        // not shell text, so quotes and spaces in strings cannot be reinterpreted.
        let wire: Vec<Json> = args.iter().map(Value::to_json).collect();
        run_args.push(serde_json::to_string(&wire).map_err(|e| MeldError::Wasm(e.to_string()))?);

        let policy = Self::stage_policy(
            attached_policy,
            scratch.path(),
            &[build_program, run_program],
            seq,
        )?;
        Self::collect_stage(backend, &policy, &self.cwd, &build_args, "build")?;
        let stdout = Self::collect_stage(backend, &policy, &self.cwd, &run_args, "run")?;

        let mut value = None;
        let mut lines = stdout;
        if let Some(last) = lines.last() {
            if let Some(json) = last.strip_prefix("@meld ") {
                value = Some(
                    Value::from_json(
                        &serde_json::from_str(json)
                            .map_err(|e| MeldError::Wasm(format!("T5 result JSON: {e}")))?,
                    )
                    .map_err(|e| MeldError::TypeMismatch(e.to_string()))?,
                );
                lines.pop();
            }
        }
        Ok((lines, value))
    }
}
