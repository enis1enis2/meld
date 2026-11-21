//! Host-side abuse tests for the T2 stdio protocol. These use tiny Python
//! workers because the point is the host's parsing/limits, not any language
//! pack. They skip when python3 is unavailable.
use meld_core::binding::{Export, Import};
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

/// Minimal stdout-only L1 worker. It answers value-shaped methods so tests can
/// prove the host rejects them before they reach the worker.
fn l1_script() -> String {
    format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L1', 'caps': ['stdout']}}}})\n\
         while True:\n\
         \x20   req = json.loads(sys.stdin.readline())\n\
         \x20   method = req['method']\n\
         \x20   if method == 'execute':\n\
         \x20       emit({{'id': req['id'], 'result': {{'stdout': ['ok'], 'exports': []}}}})\n\
         \x20   elif method in ('eval', 'fetch', 'import'):\n\
         \x20       emit({{'id': req['id'], 'result': {{'value': 1}}}})\n\
         \x20   else:\n\
         \x20       emit({{'id': req['id'], 'error': {{'code': 'TypeMismatch', 'message': 'unexpected method', 'source': None}}}})\n"
    )
}

fn job_with_exports(code: &str) -> Job {
    let mut job = job(code);
    job.exports.push(Export {
        region_id: "r0".to_string(),
        name: "native".to_string(),
        bridge_name: "value".to_string(),
    });
    job
}

#[test]
fn oversized_stdout_line_is_rejected() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    // Stay alive after flushing so the host detects the oversized line while
    // the worker is still killable, instead of racing its normal exit.
    let mut backend = spawn(
        "import sys\nsys.stdout.write('x' * (2 * 1024 * 1024) + '\\n')\nsys.stdout.flush()\nsys.stdin.readline()\n",
    );
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("line exceeds"));
}

#[test]
fn oversized_stdout_lines_are_rejected_across_nearby_sizes() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    // Property-style coverage around the 1 MiB line ceiling. The worker stays
    // alive after flushing so the host observes the cap while it can still kill
    // the process tree.
    for extra in [1usize, 4096, 64 * 1024] {
        let size = 1024 * 1024 + extra;
        let script = format!(
            "import sys\nsys.stdout.write('x' * {size} + '\\n')\nsys.stdout.flush()\nsys.stdin.readline()\n"
        );
        let mut backend = spawn(&script);
        let error = backend.ensure_initialized("python").unwrap_err();
        assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
        assert!(error.to_string().contains("line exceeds"));
    }
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
        "{HANDSHAKE}\nreq = json.loads(sys.stdin.readline())\nemit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['stdout', 'eval']}}}})\n"
    );
    let mut backend = spawn(&script);
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error
        .to_string()
        .contains("L2 worker must advertise values"));
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
        "import sys\nsys.stderr.write('SENTINEL-42\\n')\nsys.stderr.flush()\nsys.stdin.readline()\nsys.exit(3)\n",
    );
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::WorkerCrashed(_)), "{error:?}");
    assert!(error.to_string().contains("SENTINEL-42"));
}

#[test]
fn stderr_cap_keeps_only_the_tail() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    // The host must drain a large stderr stream without deadlocking, retain at
    // most the bounded snapshot, and report the most recent diagnostic.
    let mut backend = spawn(
        "import sys\nhead = 'HEAD-SENTINEL\\n'\ntail = 'TAIL-SENTINEL\\n'\nsys.stderr.write(head + 'A' * (64 * 1024 - len(head) - len(tail)) + tail)\nsys.stderr.flush()\nsys.stdin.readline()\nsys.exit(3)\n",
    );
    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::WorkerCrashed(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("TAIL-SENTINEL"), "{message}");
    assert!(!message.contains("HEAD-SENTINEL"), "{message}");
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
fn allowlisted_host_calls_are_resolved_and_unknown_names_are_denied() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         names = ['meld:t3/add', 'meld:custom/echo', 'meld:evil/run']\n\
         observed = []\n\
         for index, name in enumerate(names):\n\
         \x20   emit({{'id': -(index + 1), 'method': '__host_call', 'params': {{'fn': name, 'args': []}}}})\n\
         \x20   response = json.loads(sys.stdin.readline())\n\
         \x20   if 'error' in response:\n\
         \x20       observed.append(response['error']['code'])\n\
         \x20   else:\n\
         \x20       observed.append(response['result']['value'])\n\
         emit({{'id': req['id'], 'result': {{'stdout': observed, 'exports': []}}}})\n"
    );
    let mut backend = spawn(&script);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    backend.set_host_call(move |name, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::String(format!("ok:{name}")))
    });
    backend
        .allow_host_call("meld:custom/echo")
        .expect("exact allowlist entry");

    let stdout = backend
        .run(&job("allowlist"), &mut Bridge::default())
        .expect("execute");
    assert_eq!(
        stdout,
        vec![
            "ok:meld:t3/add".to_string(),
            "ok:meld:custom/echo".to_string(),
            "SandboxDenied".to_string(),
        ]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn set_host_call_preserves_exact_allowlist_entries() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': -1, 'method': '__host_call', 'params': {{'fn': 'meld:custom/echo', 'args': []}}}})\n\
         response = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'stdout': [response['result']['value']], 'exports': []}}}})\n"
    );
    let mut backend = spawn(&script);
    backend
        .allow_host_call("meld:custom/echo")
        .expect("exact entry before callback");
    backend.set_host_call(|name, _| Ok(Value::String(format!("ok:{name}"))));
    let stdout = backend
        .run(&job("order"), &mut Bridge::default())
        .expect("execute");
    assert_eq!(stdout, vec!["ok:meld:custom/echo".to_string()]);
}

#[test]
fn pre_initialization_host_calls_are_charged_to_the_cap() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         for i in range(70):\n\
         \x20   emit({{'id': -(i + 1), 'method': '__host_call', 'params': {{'fn': 'meld:t3/add', 'args': []}}}})\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         sys.stdin.readline()\n"
    );
    let mut backend = spawn(&script);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    backend.set_host_call(move |_, _| {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Value::int(1))
    });

    let error = backend.ensure_initialized("python").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error.to_string().contains("host call limit"));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "pre-init calls must never reach the callback"
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

#[test]
fn l1_worker_runs_stdout_only_jobs() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let mut backend = spawn(&l1_script());
    let stdout = backend
        .run(&job("pass"), &mut Bridge::default())
        .expect("L1 stdout-only run");
    assert_eq!(stdout, vec!["ok"]);
}

#[test]
fn l1_worker_cannot_eval_or_fetch_values() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let mut backend = spawn(&l1_script());
    backend.ensure_initialized("python").expect("initialize");
    let error = backend.eval("r0", "1").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error
        .to_string()
        .contains("inline eval requires an L2 worker"));
    let error = backend.fetch("native").unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error
        .to_string()
        .contains("value exports requires an L2 worker"));
}

#[test]
fn l1_worker_cannot_import_or_export_values() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let import = Import {
        region_id: "r0".to_string(),
        name: "x".to_string(),
        source_region: "r1".to_string(),
        bridge_name: "x".to_string(),
    };
    let mut backend = spawn(&l1_script());
    backend.ensure_initialized("python").expect("initialize");
    let error = backend
        .apply_imports(&[import], &Bridge::default())
        .unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error
        .to_string()
        .contains("value imports requires an L2 worker"));

    let mut backend = spawn(&l1_script());
    let error = backend
        .run(&job_with_exports("pass"), &mut Bridge::default())
        .unwrap_err();
    assert!(matches!(error, MeldError::SandboxDenied(_)), "{error:?}");
    assert!(error
        .to_string()
        .contains("value exports requires an L2 worker"));
}

#[test]
fn l1_worker_cannot_issue_host_calls() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L1', 'caps': ['stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': -7, 'method': '__host_call', 'params': {{'fn': 'meld:t3/add', 'args': []}}}})\n\
         response = json.loads(sys.stdin.readline())\n\
         code = response.get('error', {{}}).get('code')\n\
         emit({{'id': req['id'], 'result': {{'stdout': [code], 'exports': []}}}})\n"
    );
    let mut backend = spawn(&script);
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    backend.set_host_call(move |_, _| {
        observed.store(true, Ordering::SeqCst);
        Ok(Value::int(1))
    });
    let stdout = backend
        .run(&job("call"), &mut Bridge::default())
        .expect("execute");
    assert!(!called.load(Ordering::SeqCst), "L1 host call must not run");
    assert_eq!(stdout, vec!["SandboxDenied"]);
}

#[test]
fn host_calls_during_initialization_are_denied() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let script = format!(
        "{HANDSHAKE}\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': -1, 'method': '__host_call', 'params': {{'fn': 'meld:t3/add', 'args': []}}}})\n\
         response = json.loads(sys.stdin.readline())\n\
         code = response.get('error', {{}}).get('code')\n\
         emit({{'id': req['id'], 'result': {{'protocol': 'meld-t2/1', 'level': 'L2', 'caps': ['values', 'stdout']}}}})\n\
         req = json.loads(sys.stdin.readline())\n\
         emit({{'id': req['id'], 'result': {{'stdout': [code], 'exports': []}}}})\n"
    );
    let mut backend = spawn(&script);
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    backend.set_host_call(move |_, _| {
        observed.store(true, Ordering::SeqCst);
        Ok(Value::int(42))
    });

    let stdout = backend
        .run(&job("pre-init"), &mut Bridge::default())
        .expect("execute after denied pre-init call");
    assert!(
        !called.load(Ordering::SeqCst),
        "pre-init host call must not run"
    );
    assert_eq!(stdout, vec!["SandboxDenied"]);
}
