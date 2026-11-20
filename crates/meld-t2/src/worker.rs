//! T2 worker host. Every language with a CLI can implement this protocol.
use crate::protocol::{decode_response, encode, ErrorObject, Request, PROTOCOL};
use meld_core::error::{MeldError, Result};
use meld_core::binding::Import;
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_value::Value;
use serde_json::{json, Value as Json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;

/// Worker-initiated host call (Phase 4). The worker stays language-agnostic:
/// it asks the host to resolve a reserved `meld:*` function name.
pub type HostCall = dyn Fn(&str, &[Value]) -> Result<Value> + Send + Sync + 'static;

pub struct T2Backend {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
    initialized: bool,
    caps: Vec<String>,
    host_call: Option<Arc<HostCall>>,
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
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            initialized: false,
            caps: Vec::new(),
            host_call: None,
        })
    }

    /// Register the host-side resolver for worker-initiated calls. **JUDGMENT
    /// CALL:** callbacks are opt-in so Phase 2/3 workers remain unchanged; the
    /// T3 bridge installs one for `meld:t3/*` names.
    pub fn set_host_call<F>(&mut self, callback: F)
    where
        F: Fn(&str, &[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        self.host_call = Some(Arc::new(callback));
    }

    fn send_json(&mut self, text: &str) -> Result<()> {
        self.stdin.write_all(text.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn handle_host_call(&self, params: &Json) -> Result<Value> {
        let name = params
            .get("fn")
            .and_then(Json::as_str)
            .ok_or_else(|| MeldError::TypeMismatch("host call missing fn".to_string()))?;
        let args = params
            .get("args")
            .and_then(Json::as_array)
            .ok_or_else(|| MeldError::TypeMismatch("host call args must be an array".to_string()))?
            .iter()
            .map(Value::from_json)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| MeldError::TypeMismatch(e.to_string()))?;
        match &self.host_call {
            Some(f) => f(name, &args),
            None => Err(MeldError::TypeMismatch(format!(
                "no host callback registered for {name:?}"
            ))),
        }
    }

    fn send(&mut self, req: &Request) -> Result<()> {
        let line = encode(req).map_err(|e| MeldError::WorkerCrashed(e.to_string()))?;
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line)?;
        if n == 0 {
            return Err(MeldError::WorkerCrashed("worker closed stdout".to_string()));
        }
        Ok(line.trim_end_matches('\n').to_string())
    }

    fn call(&mut self, method: &str, params: Json) -> Result<Json> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&Request {
            id,
            method: method.to_string(),
            params,
        })?;
        loop {
            let line = self.read_line()?;
            // JUDGMENT CALL: a worker may ask the host to resolve a callable
            // while a request is still in flight (used by `meld_call` in the
            // Python pack to reach T3 components). Handle it inline, then keep
            // waiting for the response to the original request.
            if let Ok(msg) = serde_json::from_str::<Json>(&line) {
                if msg.get("method").and_then(Json::as_str) == Some("__host_call") {
                    let callback_id = msg.get("id").cloned().unwrap_or(Json::Null);
                    let params = msg.get("params").cloned().unwrap_or(Json::Null);
                    let response = match self.handle_host_call(&params) {
                        Ok(value) => json!({ "id": callback_id, "result": { "value": value.to_json() } }),
                        Err(e) => json!({
                            "id": callback_id,
                            "error": {
                                "code": "HostCallError",
                                "message": e.to_string(),
                                "source": null
                            }
                        }),
                    };
                    self.send_json(&response.to_string())?;
                    continue;
                }
            }
            if let Ok(resp) = decode_response(&line) {
                if resp.id == id {
                    if let Some(err) = resp.error {
                        let e: ErrorObject = serde_json::from_value(err)
                            .unwrap_or(ErrorObject {
                                code: "E_PROTOCOL".to_string(),
                                message: "malformed error".to_string(),
                                source: None,
                            });
                        return Err(match e.code.as_str() {
                            "NumericOverflow" => MeldError::NumericOverflow(e.message),
                            "TypeMismatch" => MeldError::TypeMismatch(e.message),
                            "HandleExpired" => MeldError::HandleExpired(e.message),
                            "SandboxDenied" => MeldError::SandboxDenied(e.message),
                            _ => MeldError::WorkerCrashed(format!("{}: {}", e.code, e.message)),
                        });
                    }
                    return Ok(resp.result.unwrap_or(Json::Null));
                }
            }
            // Unknown lines are stderr-like/ignored; protocol exports ride in
            // the matching response (see JUDGMENT CALL above).
            let _ = line;
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
        let got = result.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
        if got != PROTOCOL {
            return Err(MeldError::WorkerCrashed("protocol mismatch".to_string()));
        }
        self.caps = result
            .get("caps")
            .and_then(|v| v.as_array())
            .map(|xs| xs.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
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
        let result = self.call(
            "eval",
            json!({ "region_id": region_id, "code": code }),
        )?;
        let value = result.get("value").cloned().unwrap_or(Json::Null);
        Value::from_json(&value).map_err(|e| MeldError::TypeMismatch(e.to_string()))
    }

    /// Read a native binding produced by a prior `execute` (spec/t2-protocol.md).
    pub fn fetch(&mut self, name: &str) -> Result<Value> {
        let result = self.call("fetch", json!({ "name": name }))?;
        let value = result.get("value").cloned().unwrap_or(Json::Null);
        Value::from_json(&value).map_err(|e| MeldError::TypeMismatch(e.to_string()))
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
        let stdout: Vec<String> = result
            .get("stdout")
            .and_then(|v| v.as_array())
            .map(|xs| xs.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();

        if let Some(exports) = result.get("exports").and_then(|v| v.as_array()) {
            for e in exports {
                if let (Some(name), Some(value)) = (
                    e.get("name").and_then(|v| v.as_str()),
                    e.get("value"),
                ) {
                    bridge.values.insert(
                        name.to_string(),
                        Value::from_json(value).map_err(|err| MeldError::TypeMismatch(err.to_string()))?,
                    );
                }
            }
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
        let _ = self.send(&Request {
            id: u64::MAX,
            method: "shutdown".to_string(),
            params: json!({}),
        });
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
