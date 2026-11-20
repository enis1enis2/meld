//! Real Phase 4 verification: the committed Rust component is instantiated by
//! Wasmtime and called with typed s64 args; T5 is exercised through a real
//! shell-out build/run cycle.
//!
//! PROVENANCE NOTE: `component/demo.component.wasm` is committed as a binary.
//! CI/release must verify its provenance by rebuilding from `guest/` and
//! `wit/meld.wit` and comparing a recorded digest; these tests do not fabricate
//! checksum evidence.
use meld_core::error::Result;
use meld_t3::{T3Backend, T5Backend};
use meld_value::Value;
use std::path::PathBuf;

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
fn t5_toolchain_fallback_compiles_and_runs_a_region() -> Result<()> {
    // JUDGMENT CALL 55: use Python for both stages instead of POSIX shell.
    // Windows has no `/bin/sh`, cannot execute a shebang script directly, and
    // Git Bash mangles backslash paths (then crashes on the run stage). Python
    // is already installed by every CI job, so this remains a real external
    // build/run round trip without host-specific shell semantics.
    #[cfg(windows)]
    let python = "python";
    #[cfg(not(windows))]
    let python = "python3";
    let backend = T5Backend::new(
        python,
        vec![
            python.to_string(),
            "-c".to_string(),
            "import shutil,sys; shutil.copyfile(sys.argv[1], sys.argv[2])".to_string(),
            "{src}".to_string(),
            "{out}".to_string(),
        ],
        vec![python.to_string(), "{out}".to_string()],
        PathBuf::from("."),
    );
    let code = "print('t5-ok')\nprint('@meld \"7\"')\n";
    let (stdout, value) = backend.compile_and_run(code, &[])?;
    assert_eq!(stdout, vec!["t5-ok".to_string()]);
    assert_eq!(value, Some(Value::int(7)));
    Ok(())
}
