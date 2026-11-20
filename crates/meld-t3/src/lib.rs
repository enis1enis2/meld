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
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

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
        let engine = Engine::default();
        let component = Component::new(&engine, wasm)
            .map_err(|e| MeldError::Wasm(format!("component decode: {e}")))?;
        Ok(Self { engine, component })
    }

    /// Load the committed Rust demo component (see tools/build_t3_guest.sh).
    pub fn demo() -> Result<Self> {
        Self::from_bytes(include_bytes!("../component/demo.component.wasm"))
    }

    /// Call a WIT `func(a: s64, b: s64) -> s64` export with typed arguments.
    pub fn call_i64_pair(&self, export: &str, a: i64, b: i64) -> Result<i64> {
        let mut store = Store::new(&self.engine, ());
        let linker = Linker::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &self.component)
            .map_err(|e| MeldError::Wasm(format!("instantiate: {e}")))?;
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
        // JUDGMENT CALL: Wasmtime models component results as a named list;
        // a single s64 result is therefore the unary tuple `(i64,)`, not `i64`.
        let func = instance
            .get_typed_func::<(i64, i64), (i64,)>(&mut store, &func_index)
            .map_err(|e| MeldError::Wasm(format!("typed export {export}: {e}")))?;
        func.call(&mut store, (a, b))
            .map(|(result,)| result)
            .map_err(|e| MeldError::Wasm(format!("call {export}: {e}")))
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
