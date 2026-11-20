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
use meld_sandbox::{
    CommandSpec, IoPolicy, ResourceLimits, SandboxBackend, SandboxError, SandboxPolicy,
    SpawnedProcess,
};
use meld_value::Value;
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Worker-initiated host call (Phase 4). The worker stays language-agnostic:
/// it asks the host to resolve a reserved `meld:*` function name.
pub type HostCall = dyn Fn(&str, &[Value]) -> Result<Value> + Send + Sync + 'static;

const T2_MAX_LINE_BYTES: usize = 1024 * 1024;
const T2_STDERR_CAP_BYTES: u64 = 64 * 1024;
const T2_ERROR_STDERR_BYTES: usize = 4 * 1024;
const T2_IDLE_TIMEOUT_MS: u64 = 10_000;
const T2_OVERALL_TIMEOUT_MS: u64 = 60_000;
const T2_CALL_TIMEOUT: Duration = Duration::from_secs(10);
const T2_HOST_CALL_TIMEOUT: Duration = Duration::from_secs(2);
const T2_MAX_HOST_CALLS_PER_REQUEST: usize = 64;
const KNOWN_CAPS: &[&str] = &["values", "stdout", "eval"];
const RESERVED_T3_PREFIX: &str = "meld:t3/";

/// Names a worker may ask the host to resolve. The reserved `meld:t3/*` slice
/// is enabled by [`T2Backend::set_host_call`]; other names require an explicit
/// [`T2Backend::allow_host_call`].
#[derive(Clone, Debug, Default)]
struct HostCallPolicy {
    allow_reserved_t3: bool,
    exact: HashSet<String>,
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
    /// Every worker, sandboxed or not, is wrapped in the same bounded I/O and
    /// process-tree guard. The launcher decides whether the child is confined.
    process: Option<SpawnedProcess>,
    next_id: u64,
    initialized: bool,
    level: String,
    caps: Vec<String>,
    host_call: Option<Arc<HostCall>>,
    host_call_policy: HostCallPolicy,
    reaped: bool,
}

fn sandbox_error(error: SandboxError) -> MeldError {
    match error {
        SandboxError::Denied(message) => MeldError::SandboxDenied(message),
        other => MeldError::WorkerCrashed(other.to_string()),
    }
}

fn bounded_limits(mut limits: ResourceLimits) -> ResourceLimits {
    limits.max_line_bytes = Some(
        limits
            .effective_max_line_bytes()
            .min(T2_MAX_LINE_BYTES as u64),
    );
    limits.max_stderr_bytes = Some(limits.effective_max_stderr_bytes().min(T2_STDERR_CAP_BYTES));
    limits.idle_timeout_ms = Some(limits.effective_idle_timeout_ms().min(T2_IDLE_TIMEOUT_MS));
    limits.overall_timeout_ms = Some(
        limits
            .effective_overall_timeout_ms()
            .min(T2_OVERALL_TIMEOUT_MS),
    );
    limits
}

impl T2Backend {
    fn from_spawned(process: SpawnedProcess) -> Self {
        Self {
            process: Some(process),
            next_id: 1,
            initialized: false,
            level: "L2".to_string(),
            caps: Vec::new(),
            host_call: None,
            host_call_policy: HostCallPolicy::default(),
            reaped: false,
        }
    }

    /// Spawn a worker without OS confinement. Only used by trusted fixtures and
    /// legacy callers; the CLI always uses [`Self::spawn_sandboxed`].
    pub fn spawn(program: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (stdout, stderr) = match (stdout, stderr) {
            (Some(stdout), Some(stderr)) => (stdout, stderr),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MeldError::WorkerCrashed(
                    "worker did not expose all protocol pipes".to_string(),
                ));
            }
        };
        let io = IoPolicy::from_limits(&bounded_limits(ResourceLimits::default()));
        let process = SpawnedProcess::new(child, stdout, stderr, io).map_err(sandbox_error)?;
        Ok(Self::from_spawned(process))
    }

    /// Spawn a worker from an explicit argv spec without OS confinement. The
    /// CLI uses this only for debug-build fixture runs when the test-only opt-in
    /// is set; production launches go through [`Self::spawn_sandboxed`].
    pub fn spawn_with_spec(spec: &CommandSpec) -> Result<Self> {
        let mut child = spec.to_command().spawn()?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (stdout, stderr) = match (stdout, stderr) {
            (Some(stdout), Some(stderr)) => (stdout, stderr),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MeldError::WorkerCrashed(
                    "worker did not expose all protocol pipes".to_string(),
                ));
            }
        };
        let io = IoPolicy::from_limits(&bounded_limits(ResourceLimits::default()));
        let process = SpawnedProcess::new(child, stdout, stderr, io).map_err(sandbox_error)?;
        Ok(Self::from_spawned(process))
    }

    /// Spawn a worker through an OS sandbox backend. The backend has already
    /// been asked to enforce the effective four-layer policy; it must fail
    /// closed instead of returning an unconfined child.
    pub fn spawn_sandboxed(
        backend: &dyn SandboxBackend,
        spec: &CommandSpec,
        policy: &SandboxPolicy,
    ) -> Result<Self> {
        let mut effective = policy.clone();
        effective.limits = bounded_limits(effective.limits);
        let process = backend.spawn(spec, &effective).map_err(sandbox_error)?;
        Ok(Self::from_spawned(process))
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

    fn with_stderr(&mut self, message: String) -> String {
        if let Some(process) = self.process.as_mut() {
            let _ = process.drain_stderr();
        }
        let log = self
            .process
            .as_ref()
            .map(SpawnedProcess::stderr_snapshot)
            .unwrap_or("");
        let start = log.len().saturating_sub(T2_ERROR_STDERR_BYTES);
        let tail = log[start..].trim_end();
        if tail.is_empty() {
            message
        } else {
            format!("{message}; stderr: {tail}")
        }
    }

    fn terminate(&mut self) {
        if !self.reaped {
            if let Some(process) = self.process.as_mut() {
                let _ = process.kill_tree();
                let _ = process.wait();
            }
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
        let mut line = bytes;
        line.push(b'\n');
        let result = match self.process.as_mut() {
            Some(process) => process.write_stdin(&line),
            None => Err(SandboxError::Denied(
                "T2 worker process already reaped".to_string(),
            )),
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = format!("T2 stdin write failed: {error}");
                Err(self.deny(message))
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
        let received = match self.process.as_mut() {
            Some(process) => process.read_line(),
            None => return Err(self.crash(format!("worker process gone while waiting for {what}"))),
        };
        match received {
            Ok(Some(line)) => {
                if Instant::now() > deadline {
                    Err(self.timeout(what))
                } else {
                    Ok(line)
                }
            }
            Ok(None) => Err(self.crash(format!("worker closed stdout while waiting for {what}"))),
            Err(SandboxError::Denied(message)) => {
                Err(self.deny(format!("worker stdout violates protocol: {message}")))
            }
            Err(error) => Err(self.crash(format!(
                "worker stdout read failed while waiting for {what}: {error}"
            ))),
        }
    }

    fn send_host_call_response(
        &mut self,
        id: &Json,
        result: std::result::Result<Value, MeldError>,
    ) -> Result<()> {
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
        if let Err(error) = self.require_l2("worker-initiated host calls") {
            return Err(HostCallFailure::Respond(error));
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
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| MeldError::WorkerCrashed("T2 request id space exhausted".to_string()))?;
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
                    return Err(self.deny(format!("host call id {callback_num} must be negative")));
                }
                if !host_call_ids.insert(callback_num) {
                    return Err(self.deny(format!("duplicate host call id {callback_num}")));
                }
                // Worker-initiated callbacks are a privilege of fully capable
                // workers. The handshake has not assigned a level yet, so any
                // call during initialization is denied unconditionally.
                if !self.initialized {
                    let _ = self.send_host_call_response(
                        &callback_id,
                        Err(MeldError::SandboxDenied(
                            "host calls before worker initialization are denied".to_string(),
                        )),
                    );
                    continue;
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
                        _ => MeldError::WorkerCrashed(format!("{}: {}", error.code, error.message)),
                    });
                }
                _ => {
                    return Err(self.deny("T2 response must contain exactly one of result or error"))
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
        // JUDGMENT CALL: L1 is a stdout-only compatibility level used by the
        // installed shell pack. It is accepted for `run`, but value imports,
        // exports, inline eval and host calls remain L2-only so the level is a
        // real capability boundary rather than an ignored label.
        let level = match result.get("level") {
            Some(value) => value
                .as_str()
                .ok_or_else(|| self.deny("worker level must be a string"))?
                .to_string(),
            None => "L2".to_string(),
        };
        if level != "L1" && level != "L2" {
            return Err(self.deny(format!("unsupported worker level {level:?}")));
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
        if !caps.iter().any(|cap| cap == "stdout") {
            return Err(self.deny("worker must advertise stdout"));
        }
        if level == "L2" && !caps.iter().any(|cap| cap == "values") {
            return Err(self.deny("L2 worker must advertise values"));
        }
        if level == "L1" && caps.iter().any(|cap| cap == "eval" || cap == "values") {
            return Err(self.deny("L1 worker must not advertise values or eval"));
        }
        self.level = level;
        self.caps = caps;
        self.initialized = true;
        Ok(())
    }

    fn require_l2(&self, operation: &str) -> Result<()> {
        if self.level == "L1" {
            Err(MeldError::SandboxDenied(format!(
                "{operation} requires an L2 worker; this worker is L1"
            )))
        } else {
            Ok(())
        }
    }

    /// Push imported bridge values into this worker before a job or inline
    /// expression runs (spec/value-model.md §4).
    pub fn apply_imports(&mut self, imports: &[Import], bridge: &Bridge) -> Result<()> {
        if !imports.is_empty() {
            self.require_l2("value imports")?;
        }
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
        self.require_l2("inline eval")?;
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
        self.require_l2("value exports")?;
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
        if !exports.is_empty() || !job.exports.is_empty() {
            self.require_l2("value exports")?;
        }
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
        if let Ok(shutdown) = encode(&Request {
            id: u64::MAX,
            method: "shutdown".to_string(),
            params: json!({}),
        }) {
            let mut bytes = shutdown.into_bytes();
            bytes.push(b'\n');
            if let Some(process) = self.process.as_mut() {
                let _ = process.write_stdin(&bytes);
                process.close_stdin();
            }
        }
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Property-style check for the host-side caps that keep T2 bounded before
    /// a worker is spawned: every requested value is clamped to the T2 ceiling,
    /// while already-stricter values survive unchanged.
    #[test]
    fn bounded_limits_always_tighten_to_t2_caps() {
        let capped = bounded_limits(ResourceLimits {
            max_line_bytes: Some(T2_MAX_LINE_BYTES as u64 * 4),
            max_stderr_bytes: Some(T2_STDERR_CAP_BYTES * 4),
            idle_timeout_ms: Some(T2_IDLE_TIMEOUT_MS * 6),
            overall_timeout_ms: Some(T2_OVERALL_TIMEOUT_MS * 2),
            ..Default::default()
        });
        assert_eq!(capped.max_line_bytes, Some(T2_MAX_LINE_BYTES as u64));
        assert_eq!(capped.max_stderr_bytes, Some(T2_STDERR_CAP_BYTES));
        assert_eq!(capped.idle_timeout_ms, Some(T2_IDLE_TIMEOUT_MS));
        assert_eq!(capped.overall_timeout_ms, Some(T2_OVERALL_TIMEOUT_MS));

        let stricter = bounded_limits(ResourceLimits {
            max_line_bytes: Some(1024),
            max_stderr_bytes: Some(2048),
            idle_timeout_ms: Some(250),
            overall_timeout_ms: Some(500),
            ..Default::default()
        });
        assert_eq!(stricter.max_line_bytes, Some(1024));
        assert_eq!(stricter.max_stderr_bytes, Some(2048));
        assert_eq!(stricter.idle_timeout_ms, Some(250));
        assert_eq!(stricter.overall_timeout_ms, Some(500));
    }

    /// The deadlines are enforced at read time, but the configuration must also
    /// reject an inverted idle/overall pair before a worker starts.
    #[test]
    fn bounded_limits_do_not_relax_inverted_deadlines() {
        let capped = bounded_limits(ResourceLimits {
            idle_timeout_ms: Some(T2_OVERALL_TIMEOUT_MS * 4),
            overall_timeout_ms: Some(T2_IDLE_TIMEOUT_MS),
            ..Default::default()
        });
        assert_eq!(capped.idle_timeout_ms, Some(T2_IDLE_TIMEOUT_MS));
        assert_eq!(capped.overall_timeout_ms, Some(T2_IDLE_TIMEOUT_MS));
    }
}
