//! Phase 4 compiled tiers: T3 Wasm components and the T5 toolchain fallback.
//!
//! JUDGMENT CALL: T3 is demonstrated with a real Rust function compiled to a
//! Component Model component and invoked through Wasmtime's typed component
//! API. The WIT file (`wit/meld.wit`) is the IDL; the host never guesses a
//! signature from raw core-Wasm exports. T5 is deliberately generic: it writes
//! a region to a temp file, runs a pack-declared build command, then runs the
//! produced artifact. It never downloads or installs a toolchain.
use meld_core::error::{MeldError, Result};
use meld_value::Value;
use serde_json::Value as Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
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
    config.max_wasm_stack(1 * 1024 * 1024);

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

/// Convert Wasmtime failures into Meld errors without leaking a raw backtrace.
/// Resource-limit traps are sandbox denials; epoch interruption is a worker
/// timeout; ordinary traps are reported by code only.
fn map_wasm_error(context: &str, error: wasmtime::Error) -> MeldError {
    if let Some(trap) = error.downcast_ref::<Trap>() {
        return match trap {
            Trap::OutOfFuel => MeldError::SandboxDenied(format!("{context}: fuel budget exhausted")),
            Trap::Interrupt => {
                MeldError::WorkerCrashed(format!("{context}: wall-clock deadline exceeded"))
            }
            Trap::StackOverflow => {
                MeldError::SandboxDenied(format!("{context}: wasm stack limit exceeded"))
            }
            other => MeldError::Wasm(format!("{context}: wasm trap {other:?}")),
        };
    }
    MeldError::Wasm(format!("{context}: {error:#}"))
}

/// The demo component's canonical export, derived from `wit/meld.wit`.
pub const ADD_EXPORT: &str = "meld:demo/ops@0.1.0#add";
pub const MUL_EXPORT: &str = "meld:demo/ops@0.1.0#mul";

/// T3 backend holding the compiled component. `Engine` and `Component` are
/// Send+Sync, so a single instance can be shared by T2 host callbacks.
pub struct T3Backend {
    engine: Engine,
    component: Component,
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
            .map_err(|e| MeldError::Wasm(format!("component decode: {e}")))?;
        // No host functions are registered anywhere in T3. A component that
        // imports anything is therefore rejected up front rather than allowed
        // to fail late with an ambiguous linker error.
        if component.component_type().imports(&engine).next().is_some() {
            return Err(MeldError::SandboxDenied(
                "T3 component declares imports; host imports are disabled".to_string(),
            ));
        }
        Ok(Self { engine, component })
    }

    /// Load the committed Rust demo component (see tools/build_t3_guest.sh).
    pub fn demo() -> Result<Self> {
        Self::from_bytes(include_bytes!("../component/demo.component.wasm"))
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
pub struct T5Backend {
    pub lang: String,
    pub build: Vec<String>,
    pub run: Vec<String>,
    pub cwd: PathBuf,
}

static T5_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        }
    }

    fn fill(template: &str, dir: &Path) -> String {
        let src = dir.join("region.src");
        let out = dir.join("region.bin");
        template
            .replace("{src}", &src.to_string_lossy())
            .replace("{out}", &out.to_string_lossy())
            .replace("{dir}", &dir.to_string_lossy())
    }

    fn run_stage(program: &str, args: &[String], cwd: &Path, stage: &str) -> Result<Vec<String>> {
        let output = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| MeldError::WorkerCrashed(format!("T5 {stage} spawn: {e}")))?;
        if !output.status.success() {
            return Err(MeldError::WorkerCrashed(format!(
                "T5 {stage} failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect())
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
        let seq = T5_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("meld-t5-{}-{seq}", self.lang));
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("region.src"), code)?;

        let build_args: Vec<String> = self.build.iter().map(|a| Self::fill(a, &dir)).collect();
        let build_prog = build_args
            .first()
            .cloned()
            .ok_or_else(|| MeldError::Wasm("T5 build command is empty".to_string()))?;
        Self::run_stage(&build_prog, &build_args[1..], &self.cwd, "build")?;

        let mut run_args: Vec<String> = self.run.iter().map(|a| Self::fill(a, &dir)).collect();
        // JUDGMENT CALL: JSON arguments are passed as one trailing argv value,
        // not shell text, so quotes and spaces in strings cannot be reinterpreted.
        let wire: Vec<Json> = args.iter().map(Value::to_json).collect();
        run_args.push(serde_json::to_string(&wire).map_err(|e| MeldError::Wasm(e.to_string()))?);
        let run_prog = run_args
            .first()
            .cloned()
            .ok_or_else(|| MeldError::Wasm("T5 run command is empty".to_string()))?;
        let stdout = Self::run_stage(&run_prog, &run_args[1..], &self.cwd, "run")?;

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
