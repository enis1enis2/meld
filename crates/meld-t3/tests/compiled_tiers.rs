//! Real Phase 4 verification: the committed Rust component is instantiated by
//! Wasmtime and called with typed s64 args; T5 is exercised through a real
//! sandboxed build/run cycle.
//!
//! The committed `component/demo.component.wasm` digest is now checked by
//! `meld_t3::demo()` itself, not left to release tooling.
use meld_core::error::{MeldError, Result};
use meld_sandbox::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, CommandSpec, IoPolicy,
    SandboxBackend, SandboxPolicy, SpawnedProcess,
};
use meld_t3::{verify_demo_component_bytes, T3Backend, T5Backend, ADD_EXPORT};
use meld_value::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Test-only backend. It performs the real spawn but claims the enforcement
/// mechanisms a production backend would attest; it is never selected by
/// `platform_backend()` and cannot be reached from a Meld program.
struct TestBackend;

impl SandboxBackend for TestBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::UnconfinedTestOnly
    }

    fn probe(&self) -> BackendProbe {
        BackendProbe {
            kind: self.kind(),
            available: true,
            features: BackendFeatures {
                root_confinement: true,
                seccomp: true,
                rlimits: true,
                process_group_kill: true,
                ..BackendFeatures::default()
            },
            notes: vec!["test fixture backend".to_string()],
        }
    }

    fn spawn(
        &self,
        spec: &CommandSpec,
        policy: &SandboxPolicy,
    ) -> meld_sandbox::Result<SpawnedProcess> {
        require_enforcement(self, policy)?;
        let mut command = spec.to_command_for(policy);
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| meld_sandbox::SandboxError::WorkerIo("no stdout pipe".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| meld_sandbox::SandboxError::WorkerIo("no stderr pipe".to_string()))?;
        SpawnedProcess::new(child, stdout, stderr, IoPolicy::from_limits(&policy.limits))
    }
}

#[cfg(windows)]
const PYTHON: &str = "python";
#[cfg(not(windows))]
const PYTHON: &str = "python3";

fn copy_stage() -> Vec<String> {
    vec![
        PYTHON.to_string(),
        "-c".to_string(),
        "import shutil,sys; shutil.copyfile(sys.argv[1], sys.argv[2])".to_string(),
        "{src}".to_string(),
        "{out}".to_string(),
    ]
}

fn run_stage() -> Vec<String> {
    vec![PYTHON.to_string(), "{out}".to_string()]
}

fn t5_with_policy(label: &str, policy: SandboxPolicy) -> T5Backend {
    T5Backend::new(label, copy_stage(), run_stage(), PathBuf::from("."))
        .with_sandbox(TestBackend, policy)
}

fn scratch_entries(label: &str) -> BTreeSet<String> {
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(&format!("meld-t5-{label}-"))
                .then_some(name)
        })
        .collect()
}

#[test]
fn rust_component_function_is_callable_with_typed_args() -> Result<()> {
    let backend = T3Backend::demo()?;
    assert_eq!(
        backend.call_i64_pair(meld_t3::ADD_EXPORT, 20, 22)?,
        42,
        "WIT add(20, 22)"
    );
    assert_eq!(
        backend.call_i64_pair(meld_t3::MUL_EXPORT, 6, 7)?,
        42,
        "WIT mul(6, 7)"
    );
    assert_eq!(
        backend.call_value("meld:t3/add", &[Value::int(1), Value::int(2)])?,
        Value::int(3)
    );
    Ok(())
}

#[test]
fn canonical_abi_s64_limits_and_wrapping_are_preserved() -> Result<()> {
    let backend = T3Backend::demo()?;
    // The guest implements wrapping add/mul. These inputs cross the canonical
    // ABI at the s64 boundaries where a lowering or sign-extension bug would
    // otherwise be visible.
    for (a, b) in [
        (0, 0),
        (i64::MIN, 0),
        (i64::MAX, 0),
        (i64::MIN, -1),
        (i64::MAX, 1),
        (i64::MIN, i64::MIN),
        (i64::MAX, i64::MAX),
    ] {
        assert_eq!(
            backend.call_i64_pair(meld_t3::ADD_EXPORT, a, b)?,
            a.wrapping_add(b),
            "add({a}, {b})"
        );
        assert_eq!(
            backend.call_i64_pair(meld_t3::MUL_EXPORT, a, b)?,
            a.wrapping_mul(b),
            "mul({a}, {b})"
        );
    }
    Ok(())
}

#[test]
fn demo_component_digest_gate_rejects_tampered_bytes() -> Result<()> {
    let committed = include_bytes!("../component/demo.component.wasm");
    verify_demo_component_bytes(committed)?;
    let mut tampered = committed.to_vec();
    tampered[0] ^= 0xff;
    assert!(matches!(
        verify_demo_component_bytes(&tampered),
        Err(MeldError::SandboxDenied(_))
    ));
    Ok(())
}

#[test]
fn concurrent_t3_calls_do_not_share_watchdog_epoch() -> Result<()> {
    // A single engine epoch is shared by all stores. The call lock must keep
    // one call's watchdog from incrementing another call's epoch.
    let backend = Arc::new(T3Backend::demo()?);
    let mut workers = Vec::new();
    for offset in 0..4i64 {
        let backend = Arc::clone(&backend);
        workers.push(thread::spawn(move || -> Result<()> {
            for value in 0..25i64 {
                assert_eq!(
                    backend.call_i64_pair(ADD_EXPORT, value, offset)?,
                    value + offset
                );
            }
            Ok(())
        }));
    }
    for worker in workers {
        worker.join().expect("T3 worker panicked")?;
    }
    Ok(())
}

#[test]
fn t5_requires_a_sandbox_backend() {
    let backend = T5Backend::new("nosandbox", copy_stage(), run_stage(), PathBuf::from("."));
    let error = backend.compile_and_run("print('never')", &[]).unwrap_err();
    assert!(
        matches!(error, MeldError::SandboxDenied(_)),
        "missing backend must fail closed: {error}"
    );
}

#[test]
fn t5_sandbox_stage_runs_and_cleans_up_scratch() -> Result<()> {
    let before = scratch_entries("sandboxed");
    let backend = t5_with_policy("sandboxed", SandboxPolicy::deny_all("t5-test"));
    let code = "print('t5-ok')\nprint('@meld \"7\"')\n";
    let (stdout, value) = backend.compile_and_run(code, &[])?;
    assert_eq!(stdout, vec!["t5-ok".to_string()]);
    assert_eq!(value, Some(Value::int(7)));
    assert_eq!(before, scratch_entries("sandboxed"), "scratch dir leaked");
    Ok(())
}

#[test]
fn t5_bounded_wait_kills_a_sleeping_toolchain() {
    let mut policy = SandboxPolicy::deny_all("t5-timeout");
    policy.limits.idle_timeout_ms = Some(100);
    policy.limits.overall_timeout_ms = Some(500);
    let backend = T5Backend::new(
        "sleepy",
        vec![
            PYTHON.to_string(),
            "-c".to_string(),
            "import time; time.sleep(5)".to_string(),
        ],
        run_stage(),
        PathBuf::from("."),
    )
    .with_sandbox(TestBackend, policy);
    let started = Instant::now();
    let error = backend
        .compile_and_run("print('never')", &[])
        .expect_err("sleeping toolchain must be killed");
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "bounded wait took {:?}",
        started.elapsed()
    );
}
