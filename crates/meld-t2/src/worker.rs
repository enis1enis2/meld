//! T2 worker host. Every language with a CLI can implement this protocol.
//!
//! JUDGMENT CALL: this is the host side of the stdio protocol, not a sandbox
//! backend. OS confinement is the launcher's job (spec/t2-protocol.md §6), but
//! the host still fails closed on protocol abuse: every line is bounded, every
//! wait has a deadline, stderr is drained so a worker cannot deadlock on a full
//! pipe, and worker-initiated host calls are gated by an allowlist.
use crate::protocol::{encode, ErrorObject, Request, Response, PROTOCOL};
use meld_core::binding::Import;
use meld_core::error::{MeldError, Result};
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_value::Value;
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Worker-initiated host call (Phase 4). The worker stays language-agnostic:
/// it asks the host to resolve a reserved `meld:*` function name.
pub type HostCall = dyn Fn(&str, &[Value]) -> Result<Value> + Send + Sync + 'static;

/// One protocol line, with EOF represented by `None`.
type Line = io::Result<Option<String>>;

const T2_MAX_LINE_BYTES: usize = 1024 * 1024;
const T2_MAX_PENDING_LINES: usize = 64;
const T2_MAX_PENDING_REQUESTS: usize = 64;
const T2_STDERR_CAP_BYTES: usize = 64 * 1024;
const T2_ERROR_STDERR_BYTES: usize = 4 * 1024;
const T2_CALL_TIMEOUT: Duration = Duration::from_secs(10);
const T2_HOST_CALL_TIMEOUT: Duration = Duration::from_secs(2);
const T2_MAX_HOST_CALLS_PER_REQUEST: usize = 64;
const KNOWN_CAPS: &[&str] = &["values", "stdout", "eval"];
const RESERVED_T3_PREFIX: &str = "meld:t3/";

/// Names a worker may ask the host to resolve. The reserved `meld:t3/*` slice
/// is enabled by [`T2Backend::set_host_call`]; other names require an explicit
/// [`T2Backend::allow_host_call`].
#[derive(Clone, Debug)]
struct HostCallPolicy {
    allow_reserved_t3: bool,
    exact: HashSet<String>,
}

impl Default for HostCallPolicy {
    fn default() -> Self {
        Self {
            allow_reserved_t3: false,
            exact: HashSet::new(),
        }
    }
}

impl HostCallPolicy {
    fn permits(&self, name: &str) -> bool {
        self.exact.contains(name)
            || (self.allow_reserved_t3
                && name
                    .strip_prefix(RESERVED_T3_PREFIX)
                    .is_some_and(|tail| !tail.is_empty() && !tail.contains('/')))
    }
}

/// Why a worker-initiated host call did not produce a value. `Respond` returns
/// a structured protocol error and keeps the worker alive; `Timeout` means the
/// callback is stuck and the worker must be terminated.
enum HostCallFailure {
    Respond(MeldError),
    Timeout(MeldError),
}

pub struct T2Backend {
    child: Child,
    requests: Option<SyncSender<Vec<u8>>>,
    lines: Receiver<Line>,
    writer_thread: Option<JoinHandle<()>>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_log: Arc<Mutex<Vec<u8>>>,
    next_id: u64,
    initialized: bool,
    caps: Vec<String>,
    host_call: Option<Arc<HostCall>>,
    host_call_policy: HostCallPolicy,
    reaped: bool,
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Option<String>> {
    let mut out = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if out.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|&b| b == b'\n');
        let take = newline.map_or(available.len(), |i| i + 1);
        if out.len().saturating_add(take) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("T2 stdout line exceeds {limit} bytes"),
            ));
        }
        out.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    if out.last() == Some(&b'\n') {
        out.pop();
    }
    if out.last() == Some(&b'\r') {
        out.pop();
    }
    String::from_utf8(out)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("T2 stdout is not UTF-8: {e}")))
}

fn stdout_reader(stdout: ChildStdout, tx: SyncSender<Line>) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let line = read_bounded_line(&mut stdout, T2_MAX_LINE_BYTES);
        let channel_open = match tx.try_send(line) {
            Ok(()) => true,
            Err(TrySendError::Full(line)) => {
                // JUDGMENT CALL: never grow an unbounded queue for a worker that
                // floods stdout. Report the violation and stop reading; the
                // host's deadline terminates the child.
                let overflow = io::Error::new(
                    io::ErrorKind::InvalidData,
                    "T2 stdout queue full".to_string(),
                );
                drop(line);
                let _ = tx.try_send(Err(overflow));
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        };
        if !channel_open {
            break;
        }
    }
}

fn stderr_drainer(mut stderr: ChildStderr, log: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
                let overflow = log.len().saturating_add(n).saturating_sub(T2_STDERR_CAP_BYTES);
                if overflow > 0 {
                    log.drain(..overflow);
                }
                log.extend_from_slice(&buf[..n]);
            }
        }
    }
}

fn stdin_writer(mut stdin: ChildStdin, rx: mpsc::Receiver<Vec<u8>>) {
    while let Ok(mut line) = rx.recv() {
        line.push(b'\n');
        if stdin.write_all(&line).is_err() || stdin.flush().is_err() {
            break;
        }
    }
}

impl T2Backend {
    /// Spawn a worker. Sandbox enforcement is the launcher's job (spec/t2-protocol.md §6).
    pub fn spawn(program: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if child.stdin.is_none() || child.stdout.is_none() || child.stderr.is_none() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MeldError::WorkerCrashed(
                "worker did not expose all protocol pipes".to_string(),
            ));
        }
        let stdin = child.stdin.take().expect("piped stdin checked");
        let stdout = child.stdout.take().expect("piped stdout checked");
        let stderr = child.stderr.take().expect("piped stderr checked");

        let stderr_log = Arc::new(Mutex::new(Vec::new()));
        let stderr_thread = match thread::Builder::new()
            .name("meld-t2-stderr".to_string())
            .spawn({
                let log = Arc::clone(&stderr_log);
                move || stderr_drainer(stderr, log)
            }) {
            Ok(handle) => handle,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MeldError::WorkerCrashed(format!(
                    "T2 stderr drain spawn: {e}"
                )));
            }
        };

        let (line_tx, line_rx) = mpsc::sync_channel(T2_MAX_PENDING_LINES);
        let reader_thread = match thread::Builder::new()
            .name("meld-t2-stdout".to_string())
            .spawn(move || stdout_reader(stdout, line_tx)) {
            Ok(handle) => handle,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MeldError::WorkerCrashed(format!(
                    "T2 stdout reader spawn: {e}"
                )));
            }
        };

        let (request_tx, request_rx) = mpsc::sync_channel(T2_MAX_PENDING_REQUESTS);
        let writer_thread = match thread::Builder::new()
            .name("meld-t2-stdin".to_string())
            .spawn(move || stdin_writer(stdin, request_rx)) {
            Ok(handle) => handle,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MeldError::WorkerCrashed(format!(
                    "T2 stdin writer spawn: {e}"
                )));
            }
        };

        Ok(Self {
            child,
            requests: Some(request_tx),
            lines: line_rx,
            writer_thread: Some(writer_thread),
            reader_thread: Some(reader_thread),
            stderr_thread: Some(stderr_thread),
            stderr_log,
            next_id: 1,
            initialized: false,
            caps: Vec::new(),
            host_call: None,
            host_call_policy: HostCallPolicy::default(),
            reaped: false,
        })
    }

    /// Register the host-side resolver for worker-initiated calls. **JUDGMENT
    /// CALL:** callbacks are opt-in so Phase 2/3 workers remain unchanged; the
    /// T3 bridge installs one for `meld:t3/*` names. Registering a callback
    /// allowlists only that reserved namespace; use [`Self::allow_host_call`]
    /// for any other name.
    pub fn set_host_call<F>(&mut self, callback: F)
    where
        F: Fn(&str, &[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        self.host_call = Some(Arc::new(callback));
        self.host_call_policy = HostCallPolicy {
            allow_reserved_t3: true,
            exact: HashSet::new(),
        };
    }

    /// Add one exact `meld:*` name to the host-call allowlist. Returns an error
    /// for empty or non-reserved names so a typo cannot silently widen policy.
    pub fn allow_host_call(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if !name.starts_with("meld:") || name.len() == "meld:".len() {
            return Err(MeldError::SandboxDenied(format!(
                "invalid host call name {name:?}"
            )));
        }
        self.host_call_policy.exact.insert(name);
        Ok(())
    }

    fn with_stderr(&self, message: String) -> String {
        let log = self.stderr_log.lock().unwrap_or_else(|e| e.into_inner());
        let start = log.len().saturating_sub(T2_ERROR_STDERR_BYTES);
        let tail = String::from_utf8_lossy(&log[start..]);
        let tail = tail.trim_end();
        if tail.is_empty() {
            message
        } else {
            format!("{message}; stderr: {tail}")
        }
    }

    fn terminate(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.reaped = true;
        }
    }

    fn deny(&mut self, message: impl Into<String>) -> MeldError {
        let message = message.into();
        self.terminate();
        MeldError::SandboxDenied(self.with_stderr(message))
    }

    fn crash(&mut self, message: impl Into<String>) -> MeldError {
        let message = message.into();
        self.terminate();
        MeldError::WorkerCrashed(self.with_stderr(message))
    }

    fn timeout(&mut self, what: &str) -> MeldError {
        self.crash(format!("T2 {what} deadline exceeded"))
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() || bytes.len() > T2_MAX_LINE_BYTES {
            return Err(self.deny(format!(
                "T2 request size {} outside 1..={T2_MAX_LINE_BYTES} bytes",
                bytes.len()
            )));
        }
        let requests = self.requests.as_ref().cloned().ok_or_else(|| {
            MeldError::WorkerCrashed("T2 request channel already closed".to_string())
        })?;
        match requests.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(self.deny("T2 request queue full")),
            Err(TrySendError::Disconnected(_)) => {
                let message = self.with_stderr("T2 stdin writer exited".to_string());
                Err(MeldError::WorkerCrashed(message))
            }
        }
    }

    fn send_json(&mut self, value: &Json) -> Result<()> {
        let text =
            serde_json::to_string(value).map_err(|e| MeldError::WorkerCrashed(e.to_string()))?;
        self.send_bytes(text.into_bytes())
    }

    fn send(&mut self, req: &Request) -> Result<()> {
        let line = encode(req).map_err(|e| MeldError::WorkerCrashed(e.to_string()))?;
        self.send_bytes(line.into_bytes())
    }

    fn read_line(&mut self, deadline: Instant, what: &str) -> Result<String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(self.timeout(what));
        }
        let received = self.lines.recv_timeout(remaining);
        match received {
            Ok(Ok(Some(line))) => Ok(line),
            Ok(Ok(None)) => Err(self.crash(format!("worker closed stdout while waiting for {what}"))),
            Ok(Err(e)) if e.kind() == io::ErrorKind::InvalidData => {
                Err(self.deny(format!("worker stdout violates protocol: {e}")))
            }
            Ok(Err(e)) => Err(self.crash(format!(
                "worker stdout read failed while waiting for {what}: {e}"
            ))),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.timeout(what)),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.crash(format!(
                "worker stdout channel closed while waiting for {what}"
            ))),
        }
    }

    fn send_host_call_response(&mut self, id: &Json, result: std::result::Result<Value, MeldError>) -> Result<()> {
        let response = match result {
            Ok(value) => json!({ "id": id, "result": { "value": value.to_json() } }),
            Err(error) => {
                let code = if matches!(&error, MeldError::SandboxDenied(_)) {
                    "SandboxDenied"
                } else {
                    "HostCallError"
                };
                json!({
                    "id": id,
                    "error": {
                        "code": code,
                        "message": error.to_string(),
                        "source": null
                    }
                })
            }
        };
        self.send_json(&response)
    }

    fn handle_host_call(
        &self,
        name: &str,
        args: &[Value],
        deadline: Instant,
    ) -> std::result::Result<Value, HostCallFailure> {
        let callback = match &self.host_call {
            Some(callback) => Arc::clone(callback),
            None => {
                return Err(HostCallFailure::Respond(MeldError::SandboxDenied(format!(
                    "no host callback registered for {name:?}"
                ))))
            }
        };
        if !self.host_call_policy.permits(name) {
            return Err(HostCallFailure::Respond(MeldError::SandboxDenied(format!(
                "host call {name:?} is not allowlisted"
            ))));
        }

        let budget = deadline
            .saturating_duration_since(Instant::now())
            .min(T2_HOST_CALL_TIMEOUT);
        if budget.is_zero() {
            return Err(HostCallFailure::Timeout(MeldError::WorkerCrashed(
                "T2 host call deadline exceeded".to_string(),
            )));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let name = name.to_string();
        let args = args.to_vec();
        thread::Builder::new()
            .name("meld-t2-hostcall".to_string())
            .spawn(move || {
                let result = callback(&name, &args);
                // Never block the callback thread if the host has already
                // timed out and torn down the worker.
                let _ = tx.try_send(result);
            })
            .map_err(|e| {
                HostCallFailure::Respond(MeldError::WorkerCrashed(format!(
                    "T2 host call spawn: {e}"
                )))
            })?;
        match rx.recv_timeout(budget) {
            Ok(result) => result.map_err(HostCallFailure::Respond),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(HostCallFailure::Timeout(
                MeldError::WorkerCrashed("T2 host call deadline exceeded".to_string()),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(HostCallFailure::Respond(
                MeldError::WorkerCrashed("T2 host callback panicked".to_string()),
            )),
        }
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            MeldError::WorkerCrashed("T2 request id space exhausted".to_string())
        })?;
        self.send(&Request {
            id,
            method: method.to_string(),
            params,
        })?;
        let deadline = Instant::now() + T2_CALL_TIMEOUT;
        let mut host_calls = 0usize;
        let mut host_call_ids = HashSet::new();

        loop {
            let line = self.read_line(deadline, &format!("{method} response"))?;
            let msg: Json = match serde_json::from_str(&line) {
                Ok(msg) => msg,
                Err(_) => {
                    return Err(self.deny("worker wrote malformed T2 JSON on stdout"));
                }
            };

            if msg.get("method").is_some() {
                let call_method = msg
                    .get("method")
                    .and_then(Json::as_str)
                    .ok_or_else(|| self.deny("worker method must be a string"))?;
                if call_method != "__host_call" {
                    return Err(self.deny(format!("worker sent unknown method {call_method:?}")));
                }
                let callback_id = msg
                    .get("id")
                    .cloned()
                    .ok_or_else(|| self.deny("host call missing id"))?;
                let callback_num = callback_id
                    .as_i64()
                    .ok_or_else(|| self.deny("host call id must be a negative integer"))?;
                if callback_num >= 0 {
                    return Err(self.deny(format!(
                        "host call id {callback_num} must be negative"
                    )));
                }
                if !host_call_ids.insert(callback_num) {
                    return Err(self.deny(format!("duplicate host call id {callback_num}")));
                }
                if host_calls >= T2_MAX_HOST_CALLS_PER_REQUEST {
                    let _ = self.send_host_call_response(
                        &callback_id,
                        Err(MeldError::SandboxDenied(
                            "T2 host call limit exceeded".to_string(),
                        )),
                    );
                    return Err(self.deny("T2 host call limit exceeded"));
                }
                host_calls += 1;

                let params = msg
                    .get("params")
                    .ok_or_else(|| self.deny("host call missing params"))?;
                let name = params
                    .get("fn")
                    .and_then(Json::as_str)
                    .ok_or_else(|| self.deny("host call missing fn"))?
                    .to_string();
                let args = params
                    .get("args")
                    .and_then(Json::as_array)
                    .ok_or_else(|| self.deny("host call args must be an array"))?;
                let args = args
                    .iter()
                    .map(Value::from_json)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| self.deny(format!("host call arg: {e}")))?;

                match self.handle_host_call(&name, &args, deadline) {
                    Ok(value) => self.send_host_call_response(&callback_id, Ok(value))?,
                    Err(HostCallFailure::Respond(error)) => {
                        self.send_host_call_response(&callback_id, Err(error))?
                    }
                    Err(HostCallFailure::Timeout(error)) => {
                        let _ = self.send_host_call_response(&callback_id, Err(error));
                        return Err(self.crash("T2 host call deadline terminated the worker"));
                    }
                }
                continue;
            }

            let response: Response = match serde_json::from_value(msg) {
                Ok(response) => response,
                Err(_) => return Err(self.deny("malformed T2 response")),
            };
            if response.id != id {
                return Err(self.deny(format!(
                    "T2 response id {} does not match request {id}",
                    response.id
                )));
            }
            match (response.result, response.error) {
                (Some(result), None) => return Ok(result),
                (None, Some(error)) => {
                    let error: ErrorObject = match serde_json::from_value(error) {
                        Ok(error) => error,
                        Err(_) => return Err(self.deny("malformed T2 error object")),
                    };
                    return Err(match error.code.as_str() {
                        "NumericOverflow" => MeldError::NumericOverflow(error.message),
                        "TypeMismatch" => MeldError::TypeMismatch(error.message),
                        "HandleExpired" => MeldError::HandleExpired(error.message),
                        "SandboxDenied" => MeldError::SandboxDenied(error.message),
                        _ => MeldError::WorkerCrashed(format!(
                            "{}: {}",
                            error.code, error.message
                        )),
                    });
                }
                _ => {
                    return Err(self.deny(
                        "T2 response must contain exactly one of result or error",
                    ))
                }
            }
        }
    }

    /// Idempotent handshake. Inline evaluation and normal execution share the
    /// same persistent worker namespace, so both must go through this path.
    pub fn ensure_initialized(&mut self, lang: &str) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        let result = self.call(
            "initialize",
            json!({ "protocol": PROTOCOL, "lang": lang, "caps": ["values", "stdout", "eval"] }),
        )?;
        let got = result
            .get("protocol")
            .and_then(Json::as_str)
            .ok_or_else(|| self.deny("initialize response missing protocol"))?;
        if got != PROTOCOL {
            return Err(self.deny(format!("unsupported worker protocol {got:?}")));
        }
        if let Some(level) = result.get("level") {
            let level = level
                .as_str()
                .ok_or_else(|| self.deny("worker level must be a string"))?;
            if level != "L2" {
                return Err(self.deny(format!("unsupported worker level {level:?}")));
            }
        }

        let advertised = result
            .get("caps")
            .and_then(Json::as_array)
            .ok_or_else(|| self.deny("initialize response missing caps"))?;
        let mut caps = Vec::new();
        for cap in advertised {
            let cap = cap
                .as_str()
                .ok_or_else(|| self.deny("worker cap must be a string"))?;
            if !KNOWN_CAPS.contains(&cap) {
                return Err(self.deny(format!("worker advertises unknown capability {cap:?}")));
            }
            if !caps.iter().any(|known| known == cap) {
                caps.push(cap.to_string());
            }
        }
        if !caps.iter().any(|cap| cap == "values") || !caps.iter().any(|cap| cap == "stdout") {
            return Err(self.deny("worker must advertise values and stdout"));
        }
        self.caps = caps;
        self.initialized = true;
        Ok(())
    }

    /// Push imported bridge values into this worker before a job or inline
    /// expression runs (spec/value-model.md §4).
    pub fn apply_imports(&mut self, imports: &[Import], bridge: &Bridge) -> Result<()> {
        for imp in imports {
            if let Some(v) = bridge.values.get(&imp.bridge_name) {
                self.call("import", json!({ "name": imp.name, "value": v.to_json() }))?;
            }
        }
        Ok(())
    }

    /// Evaluate an inline expression island and decode its Meld Value.
    /// **JUDGMENT CALL:** `eval` is advertised in `caps`; failing clearly here
    /// is better than sending expression text to a worker that only supports
    /// statements.
    pub fn eval(&mut self, region_id: &str, code: &str) -> Result<Value> {
        if self.initialized && !self.caps.iter().any(|c| c == "eval") {
            return Err(MeldError::TypeMismatch(format!(
                "worker for {region_id} does not advertise eval"
            )));
        }
        let result = self.call("eval", json!({ "region_id": region_id, "code": code }))?;
        let value = result
            .get("value")
            .ok_or_else(|| self.deny("eval response missing value"))?;
        Value::from_json(value).map_err(|e| MeldError::TypeMismatch(e.to_string()))
    }

    /// Read a native binding produced by a prior `execute` (spec/t2-protocol.md).
    pub fn fetch(&mut self, name: &str) -> Result<Value> {
        let result = self.call("fetch", json!({ "name": name }))?;
        let value = result
            .get("value")
            .ok_or_else(|| self.deny("fetch response missing value"))?;
        Value::from_json(value).map_err(|e| MeldError::TypeMismatch(e.to_string()))
    }

    // JUDGMENT CALL: worker-initiated exports are returned in the `execute`
    // response rather than as out-of-band notifications. This keeps the
    // request/response loop single-threaded and deterministic (Phase 2/3).
}

impl Backend for T2Backend {
    fn tier(&self) -> &'static str {
        "T2"
    }

    fn run(&mut self, job: &Job, bridge: &mut Bridge) -> Result<Vec<String>> {
        self.ensure_initialized(&job.region.lang)?;
        self.apply_imports(&job.imports, bridge)?;

        let result = self.call(
            "execute",
            json!({
                "region_id": job.region.id,
                "code": job.code,
                "env": {}
            }),
        )?;
        let stdout_json = result
            .get("stdout")
            .ok_or_else(|| self.deny("execute response missing stdout"))?;
        let stdout_lines = stdout_json
            .as_array()
            .ok_or_else(|| self.deny("execute stdout must be an array"))?;
        let mut stdout = Vec::with_capacity(stdout_lines.len());
        for line in stdout_lines {
            stdout.push(
                line.as_str()
                    .ok_or_else(|| self.deny("execute stdout entries must be strings"))?
                    .to_string(),
            );
        }

        let exports: &[Json] = match result.get("exports") {
            Some(value) => value
                .as_array()
                .ok_or_else(|| self.deny("execute exports must be an array"))?,
            None => &[],
        };
        for e in exports {
            let name = e
                .get("name")
                .and_then(Json::as_str)
                .ok_or_else(|| self.deny("execute export missing name"))?;
            let value = e
                .get("value")
                .ok_or_else(|| self.deny("execute export missing value"))?;
            bridge.values.insert(
                name.to_string(),
                Value::from_json(value).map_err(|err| MeldError::TypeMismatch(err.to_string()))?,
            );
        }
        // Glue-declared exports are authoritative: `export x as y` means the
        // host reads native binding `x` from the worker and publishes it as
        // bridge name `y`. Worker-pushed values above are an optional shortcut.
        for export in &job.exports {
            let value = self.fetch(&export.name)?;
            bridge.values.insert(export.bridge_name.clone(), value);
        }
        Ok(stdout)
    }
}

impl Drop for T2Backend {
    fn drop(&mut self) {
        if let Some(requests) = self.requests.take() {
            if let Ok(shutdown) = encode(&Request {
                id: u64::MAX,
                method: "shutdown".to_string(),
                params: json!({}),
            }) {
                let _ = requests.try_send(shutdown.into_bytes());
            }
            // Closing the last sender lets the writer thread exit even when
            // the child ignores shutdown and has to be killed.
            drop(requests);
        }
        self.terminate();
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}
