//! Real Phase 4 verification: the committed Rust component is instantiated by
//! Wasmtime and called with typed s64 args; T5 is exercised through a real
//! shell-out build/run cycle.
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
fn t5_toolchain_fallback_compiles_and_runs_a_region() -> Result<()> {
    // JUDGMENT CALL 55: use PATH-resolved `sh` for both stages. The old
    // `/bin/sh` program name does not exist on Windows, and CreateProcess
    // cannot execute the generated shebang script directly there. Invoking
    // `sh` explicitly keeps the same POSIX build/run semantics on every host
    // while letting Windows find Git Bash through PATH.
    let backend = T5Backend::new(
        "sh",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "cp {src} {out} && chmod +x {out}".to_string(),
        ],
        vec!["sh".to_string(), "{out}".to_string()],
        PathBuf::from("."),
    );
    let code = "#!/bin/sh\necho t5-ok\necho '@meld \"7\"'\n";
    let (stdout, value) = backend.compile_and_run(code, &[])?;
    assert_eq!(stdout, vec!["t5-ok".to_string()]);
    assert_eq!(value, Some(Value::int(7)));
    Ok(())
}
