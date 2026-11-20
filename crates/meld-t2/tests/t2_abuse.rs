//! Host-side abuse tests for the T2 stdio protocol. These use tiny Python
//! workers because the point is the host's parsing/limits, not any language
//! pack. They skip when python3 is unavailable.
use meld_core::error::MeldError;
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_core::splitter::{Region, RegionKind};
use meld_t2::T2Backend;
use meld_value::Value;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn python3_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn spawn(script: &str) -> T2Backend {
    T2Backend::spawn("python3", &["-u", "-c", script]).expect("spawn fake worker")
}

fn region(id: &str) -> Region {
    Region {
        id: id.to_string(),
        lang: "python".to_string(),
        kind: RegionKind::Block,
        start: 0,
        end: 0,
        full_start: 0,
        full_end: 0,
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    }
}

fn job(code: &str) -> Job {
    Job {
        region: region("r0"),
        code: code.to_string(),
        exports: vec![],
        imports: vec![],
    }
}

const HANDSHAKE: &str = r#"
import json, sys
def emit(obj):
    sys.stdout.write(json.dumps(obj, separators=(',', ':')) + '\n')
    sys.stdout.flush()
"#;

#[test]
fn oversized_stdout_line_is_rejected() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let mut backend =
        spawn("import sys\nsys.stdout.write('x' * (2 * 1024 * 1024) + '\\n')\nsys.stdout.flush()\n");
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("stdout line exceeds"));
}

#[test]
fn unknown_capability_is_a_hard_error() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\nreq = json.loads(sys.stdin.readline())\nemit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout', 'evil']}}}})\n"
    );
    let mut backend = spawn(&script);
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("unknown capability"));
}

#[test]
fn missing_required_capability_is_a_hard_error() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\nreq = json.loads(sys.stdin.readline())\nemit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['eval']}}}})\n"
    );
    let mut backend = spawn(&script);
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("values and stdout"));
}

#[test]
fn protocol_mismatch_is_a_hard_error() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\nreq = json.loads(sys.stdin.readline())\nemit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/0', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n"
    );
    let mut backend = spawn(&script);
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("unsupported worker protocol"));
}

#[test]
fn unsolicited_response_is_a_hard_error() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\nreq = json.loads(sys.stdin.readline())\nemit({{'id': req['id'] + 1000, 'result': {{'protocol': 'meld-t2/1', 'caps': ['values', 'stdout']}}}})\n"
    );
    let mut backend = spawn(&script);
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("does not match request"));
}

#[test]
fn stderr_tail_is_included_in_crash_report() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let mut backend = spawn(
        "import sys\nsys.stderr.write('SENTINEL-42\\n')\nsys.stderr.flush()\nsys.exit(3)\n",
    );
    std::thread::sleep(Duration::from_millis(50));
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::WorkerCrashed(_)), "{error:?}");
    assert!(error.to_string().contains("SENTINEL-42"));
}

#[test]
fn host_call_allowlist_denies_unknown_name() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': -7, 'method': '__host_call', 'params': {{'fn': 'meld:evil/run', 'args': []}}}})\n\
         response = json.loads(sys.stdin.readline())\n\
         code = response.get('error', {{}}).get('code')\n\
         emit({{'id': req['id'], 'result': {{'stdout': [], 'exports': [{{'name': 'code', 'value': code}}]}}}})\n"
    );
    let mut backend = spawn(&script);
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    backend.set_host_call(move |_, _| {
        observed.store(true, Ordering::SeqCst);
        Ok(Value::int(1))
    });

    let mut bridge = Bridge::default();
    backend.run(&job("pass"), &mut bridge).expect("execute");
    assert!(!called.load(Ordering::SeqCst), "callback must not run");
    assert_eq!(
        bridge.values.get("code"),
        Some(&Value::String("SandboxDenied".to_string()))
    );
}

#[test]
fn host_call_timeout_terminates_worker() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': -1, 'method': '__host_call', 'params': {{'fn': 'meld:t3/add', 'args': []}}}})\n\
         sys.stdin.readline()\n"
    );
    let mut backend = spawn(&script);
    backend.set_host_call(|_, _| {
        std::thread::sleep(Duration::from_secs(3));
        Ok(Value::int(42))
    });

    let error = backend
        .run(&job("call"), &mut Bridge::default())
        .unwrap_err();
    assert!(matches!(error, MeldError::WorkerCrashed(_)), "{error:?}");
    assert!(error.to_string().contains("host call deadline"));
}

#[test]
fn host_call_limit_is_enforced() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         for i in range(70):\n\
         \x20   emit({{'id': -(i + 1), 'method': '__host_call', 'params': {{'fn': 'meld:t3/add', 'args': []}}}})\n\
         \x20   response = json.loads(sys.stdin.readline())\n\
         \x20   if 'error' in response:\n\
         \x20       break\n\
         emit({{'id': req['id'], 'result': {{'stdout': [], 'exports': []}}}})\n"
    );
    let mut backend = spawn(&script);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    backend.set_host_call(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::int(1))
    });

    let error = backend
        .run(&job("flood"), &mut Bridge::default())
        .unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("host call limit"));
    assert_eq!(calls.load(Ordering::SeqCst), 64);
}
