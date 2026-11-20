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

const RESOURCE_IMPORT_COMPONENT: &str = r#"
(component
  (import "host" (instance
    (export "r" (type (sub resource)))
  ))
)
"#;

const RESOURCE_HANDLE_TRAP_COMPONENT: &str = r#"
(component
  (type $r (resource (rep i32)))
  (core func $drop (canon resource.drop $r))
  (core module $m
    (import "" "drop" (func $drop (param i32)))
    (func (export "trap") (param i64 i64) (result i64)
      (call $drop (i32.const 0))
      (i64.const 0))
  )
  (core instance $i (instantiate $m
    (with "" (instance (export "drop" (func $drop))))
  ))
  (func $add (param "a" s64) (param "b" s64) (result s64)
    (canon lift (core func $i "trap")))
  (instance $ops
    (export "add" (func $add)))
  (export "meld:demo/ops@0.1.0" (instance $ops))
)
"#;

const RESOURCE_ALLOCATION_COMPONENT: &str = r#"
(component
  (type $r (resource (rep i32)))
  (core func $new (canon resource.new $r))
  (core module $m
    (import "" "new" (func $new (param i32) (result i32)))
    (func (export "allocate") (param i64 i64) (result i64)
      (local $i i32)
      (block $done
        (loop $loop
          (br_if $done (i32.ge_u (local.get $i) (i32.const 300)))
          (drop (call $new (local.get $i)))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $loop)))
      (i64.const 0))
  )
  (core instance $i (instantiate $m
    (with "" (instance (export "new" (func $new))))
  ))
  (func $add (param "a" s64) (param "b" s64) (result s64)
    (canon lift (core func $i "allocate")))
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

#[test]
fn resource_type_imports_are_rejected_before_instantiation() {
    let Err(error) = T3Backend::from_bytes(RESOURCE_IMPORT_COMPONENT.as_bytes()) else {
        panic!("component importing a resource type was accepted");
    };
    assert!(
        matches!(error, MeldError::SandboxDenied(_)),
        "resource import must fail closed: {error}"
    );
}

#[test]
fn invalid_resource_handle_traps_are_mapped_without_backtraces() {
    let backend = T3Backend::from_bytes(RESOURCE_HANDLE_TRAP_COMPONENT.as_bytes()).unwrap();
    let error = backend.call_i64_pair(ADD_EXPORT, 1, 2).unwrap_err();
    let message = error.to_string();
    assert!(
        matches!(error, MeldError::Wasm(_)),
        "resource trap must stay in the Wasm error channel: {message}"
    );
    assert!(
        message.contains("handle"),
        "resource handle diagnostic should be preserved: {message}"
    );
    assert!(
        !message.contains("backtrace"),
        "raw Wasmtime backtrace must not escape: {message}"
    );
}

#[test]
fn resource_handle_allocation_is_bounded() {
    let backend = T3Backend::from_bytes(RESOURCE_ALLOCATION_COMPONENT.as_bytes()).unwrap();
    let error = backend.call_i64_pair(ADD_EXPORT, 1, 2).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("handle") || message.contains("resource"),
        "excessive resource allocation must be rejected: {message}"
    );
}

#[test]
fn canonical_abi_scalar_binding_rejects_bad_shape_and_range() {
    let backend = T3Backend::demo().expect("committed demo component");

    let error = backend
        .call_value("meld:t3/add", &[meld_value::Value::int(1)])
        .unwrap_err();
    assert!(
        matches!(error, MeldError::TypeMismatch(_)),
        "arity must be checked at the canonical boundary: {error}"
    );

    let too_large = meld_value::Value::int(i128::from(i64::MAX) + 1);
    let error = backend
        .call_value("meld:t3/add", &[too_large, meld_value::Value::int(1)])
        .unwrap_err();
    assert!(
        matches!(error, MeldError::TypeMismatch(_)),
        "s64 narrowing must be checked, never silent: {error}"
    );

    let error = backend
        .call_value(
            "meld:t3/nope",
            &[meld_value::Value::int(1), meld_value::Value::int(2)],
        )
        .unwrap_err();
    assert!(
        matches!(error, MeldError::TypeMismatch(_)),
        "unknown reserved name must fail: {error}"
    );
}
