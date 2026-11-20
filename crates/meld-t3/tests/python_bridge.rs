//! End-to-end Phase 4 proof: a real Python T2 worker calls `meld_call`, the
//! host callback resolves the reserved name through T3, and the committed Rust
//! Wasm component computes the result with typed s64 arguments.
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_core::splitter::{Region, RegionKind};
use meld_t2::T2Backend;
use meld_t3::T3Backend;
use std::path::Path;

#[test]
fn python_meld_call_reaches_rust_wit_component() -> meld_core::error::Result<()> {
    let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/python/worker.py");
    let worker = worker.to_str().expect("worker path is UTF-8");
    let mut backend = T2Backend::spawn("python3", &["-u", worker])?;
    let t3 = T3Backend::demo()?;
    backend.set_host_call(move |name, args| t3.call_value(name, args));

    let region = Region {
        id: "wasm_demo".to_string(),
        lang: "python".to_string(),
        kind: RegionKind::Block,
        start: 0,
        end: 0,
        full_start: 0,
        full_end: 0,
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    };
    let job = Job {
        region,
        code: concat!(
            "print('add', meld_call('meld:t3/add', 20, 22))\n",
            "print('mul', meld_call('meld:t3/mul', 6, 7))\n"
        )
        .to_string(),
        imports: vec![],
        exports: vec![],
    };
    let mut bridge = Bridge::default();
    let lines = backend.run(&job, &mut bridge)?;
    assert_eq!(lines, vec!["add 42".to_string(), "mul 42".to_string()]);
    Ok(())
}
