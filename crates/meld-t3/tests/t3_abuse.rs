//! Abuse tests for the T3 hardening posture. These components are small but
//! hostile: they request host imports, spin forever, trap explicitly or arrive
//! oversized. All of them must be rejected or bounded by the engine/store.

use meld_core::error::MeldError;
use meld_t3::{T3Backend, ADD_EXPORT};

const IMPORTING_COMPONENT: &str = r#"
(component
  (import "host" (func))
)
"#;

const INFINITE_LOOP_COMPONENT: &str = r#"
(component
  (core module $m
    (func (export "loop") (param i64 i64) (result i64)
      (loop $forever
        br $forever)
      (i64.const 0)))
  (core instance $i (instantiate $m))
  (func $add (param "a" s64) (param "b" s64) (result s64)
    (canon lift (core func $i "loop")))
  (instance $ops
    (export "add" (func $add)))
  (export "meld:demo/ops@0.1.0" (instance $ops))
)
"#;

const TRAPPING_COMPONENT: &str = r#"
(component
  (core module $m
    (func (export "trap") (param i64 i64) (result i64)
      unreachable))
  (core instance $i (instantiate $m))
  (func $add (param "a" s64) (param "b" s64) (result s64)
    (canon lift (core func $i "trap")))
  (instance $ops
    (export "add" (func $add)))
  (export "meld:demo/ops@0.1.0" (instance $ops))
)
"#;

#[test]
fn host_imports_are_rejected_before_instantiation() {
    let Err(error) = T3Backend::from_bytes(IMPORTING_COMPONENT.as_bytes()) else {
        panic!("importing component was accepted");
    };
    assert!(
        matches!(error, MeldError::SandboxDenied(_)),
        "importing component must fail closed: {error}"
    );
}

#[test]
fn oversized_components_are_rejected_before_compilation() {
    let oversized = vec![0u8; 16 * 1024 * 1024 + 1];
    let Err(error) = T3Backend::from_bytes(&oversized) else {
        panic!("oversized component was accepted");
    };
    assert!(
        matches!(error, MeldError::SandboxDenied(_)),
        "oversized component must fail closed: {error}"
    );
}

#[test]
fn fuel_exhaustion_is_a_sandbox_denial() {
    let backend = match T3Backend::from_bytes(INFINITE_LOOP_COMPONENT.as_bytes()) {
        Ok(backend) => backend,
        Err(error) => panic!("malformed abuse component: {error:?}"),
    };
    let error = backend.call_i64_pair(ADD_EXPORT, 1, 2).unwrap_err();
    assert!(
        matches!(error, MeldError::SandboxDenied(ref reason) if reason.contains("fuel")),
        "spin loop must exhaust its fuel budget: {error}"
    );
}

#[test]
fn explicit_traps_are_mapped_without_leaking_backtraces() {
    let backend = T3Backend::from_bytes(TRAPPING_COMPONENT.as_bytes()).unwrap();
    let error = backend.call_i64_pair(ADD_EXPORT, 1, 2).unwrap_err();
    assert!(
        matches!(error, MeldError::Wasm(ref reason) if reason.contains("UnreachableCodeReached")),
        "trap code should be reported: {error}"
    );
    assert!(
        !format!("{error}").contains("backtrace"),
        "raw Wasmtime backtrace must not escape: {error}"
    );
}
