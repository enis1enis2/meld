//! Minimal stdio LSP client used to forward one region to its own language
//! server (project.md §7).
//!
//! **JUDGMENT CALL:** a pack's `lsp.toml` names the external server command.
//! The client is deliberately small: initialize, didOpen/didChange, completion,
//! shutdown. That is the smallest honest implementation of "virtual document
//! forwarding" and is testable with a deterministic mock server. Richer
//! features (hover, rename, incremental sync) are future work per language.
//!
//! Requests have a bounded lifetime. A stalled server is killed after the
//! timeout so a blocked `read` cannot hold the client mutex forever; the
//! backend drops that client and starts a fresh one on the next request.
use crate::packs::LspSpec;
use serde_json::{json, Value};
use std::env;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Upper bound for a single external-server request (initialize/completion).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(750);

/// Environment keys forwarded to an external language server. Everything else
/// is cleared so a pack cannot read unrelated host secrets through its server.
#[cfg(not(windows))]
pub const EXTERNAL_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "NODE_PATH",
    "LUA_PATH",
];

/// Windows runtimes additionally need the system root and scratch directories.
#[cfg(windows)]
pub const EXTERNAL_ENV_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "PYTHONPATH",
    "NODE_PATH",
    "LUA_PATH",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
];

pub struct ExternalLsp {
    child: Child,
    stdin: ChildStdin,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    next_id: i64,
    initialized: bool,
    root_uri: String,
    request_timeout: Duration,
    broken: bool,
}

impl ExternalLsp {
    pub fn spawn(spec: &LspSpec, root: &Path) -> Result<Self, String> {
        Self::with_timeout(spec, root, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn with_timeout(
        spec: &LspSpec,
        root: &Path,
        request_timeout: Duration,
    ) -> Result<Self, String> {
        let program = spec.command.as_ref().ok_or("pack has no lsp command")?;
        let mut child = Command::new(program)
            .args(&spec.args)
            .current_dir(root)
            .env_clear()
            .envs(external_environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // LSP owns stdout; stderr is deliberately discarded to avoid a
            // shared pipe that could corrupt framing. Diagnostics from the
            // external server are intentionally not surfaced by this client.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            next_id: 1,
            initialized: false,
            root_uri: file_uri(root),
            request_timeout,
            broken: false,
        })
    }

    /// True after a timeout killed the server; the owner should discard it.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn ensure_initialized(&mut self) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": self.root_uri,
                "capabilities": {},
                "workspaceFolders": null,
            }),
        )?;
        self.notify("initialized", json!({}))?;
        self.initialized = true;
        Ok(())
    }

    pub fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), String> {
        self.ensure_initialized()?;
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": language_id, "version": version, "text": text,
            }}),
        )
    }

    pub fn did_change(&mut self, uri: &str, version: i32, text: &str) -> Result<(), String> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }],
            }),
        )
    }

    pub fn completion(
        &mut self,
        uri: &str,
        position: Value,
        context: Option<Value>,
    ) -> Result<Value, String> {
        self.ensure_initialized()?;
        self.request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": position,
                "context": context.unwrap_or(Value::Null),
            }),
        )
    }

    fn ensure_usable(&self) -> Result<(), String> {
        if self.broken {
            Err("external language server was terminated after a timeout".to_string())
        } else {
            Ok(())
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.ensure_usable()?;
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;

        let stdout = Arc::clone(&self.stdout);
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || -> Result<(), String> {
            let response = read_response(&stdout, id)?;
            let _ = tx.send(response);
            Ok(())
        });

        match rx.recv_timeout(self.request_timeout) {
            Ok(msg) => {
                let _ = reader.join();
                if let Some(err) = msg.get("error") {
                    return Err(format!("{method}: {err}"));
                }
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.broken = true;
                // Killing the child closes its stdout pipe, unblocking the
                // reader thread without leaving a live server behind.
                let _ = self.child.kill();
                Err(format!(
                    "{method} timed out after {} ms; external language server terminated",
                    self.request_timeout.as_millis()
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let result = reader
                    .join()
                    .unwrap_or_else(|_| Err("reader thread panicked".to_string()));
                Err(result.err().unwrap_or_else(|| {
                    format!("{method}: language server closed stdout before responding")
                }))
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.ensure_usable()?;
        self.write_message(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn write_message(&mut self, msg: &Value) -> Result<(), String> {
        self.ensure_usable()?;
        let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        write!(
            self.stdin,
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}",
            body.len(),
            body
        )
        .map_err(|e| format!("write LSP message: {e}"))?;
        self.stdin.flush().map_err(|e| e.to_string())
    }
}

impl Drop for ExternalLsp {
    fn drop(&mut self) {
        if !self.broken {
            // Best-effort protocol shutdown with a short bound; `kill`+`wait`
            // below is the authoritative cleanup even for a rude server.
            let previous = self.request_timeout;
            self.request_timeout = SHUTDOWN_TIMEOUT;
            let _ = self.request("shutdown", Value::Null);
            self.request_timeout = previous;
            let _ = self.notify("exit", json!({}));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Allowlisted variables from the host environment, without inheriting
/// unrelated credentials or configuration.
pub fn external_environment() -> Vec<(String, OsString)> {
    EXTERNAL_ENV_KEYS
        .iter()
        .filter_map(|key| env::var_os(key).map(|value| ((*key).to_string(), value)))
        .collect()
}

/// RFC 3986-ish `file:` URI for a local path. Only unreserved characters plus
/// path separators and the Windows drive colon are left literal; everything
/// else (including `%`) is percent-encoded from its UTF-8 bytes.
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    let mut text = text;
    #[cfg(windows)]
    if text.len() >= 2 && text.as_bytes()[1] == b':' && !text.starts_with('/') {
        text.insert(0, '/');
    }
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    format!("file://{}", encode_uri(&text, true))
}

/// Percent-encode one URI component (e.g. a virtual-document region id).
pub fn uri_component(value: &str) -> String {
    encode_uri(value, false)
}

fn encode_uri(value: &str, keep_path_chars: bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        let literal = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (keep_path_chars && matches!(byte, b'/' | b':'));
        if literal {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

fn read_response(stdout: &Arc<Mutex<BufReader<ChildStdout>>>, id: i64) -> Result<Value, String> {
    loop {
        let msg = read_message(stdout)?;
        // Server-initiated requests/notifications are out of scope for this
        // client; wait only for the response matching this request id.
        if msg.get("id").and_then(Value::as_i64) == Some(id) {
            return Ok(msg);
        }
    }
}

fn read_message(stdout: &Arc<Mutex<BufReader<ChildStdout>>>) -> Result<Value, String> {
    let mut reader = stdout.lock().map_err(|e| format!("stdout lock: {e}"))?;
    read_message_from(&mut *reader)
}

fn read_message_from(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("language server closed stdout".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let len = length.ok_or("missing Content-Length")?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| format!("invalid LSP JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_percent_encodes_reserved_and_unicode_bytes() {
        #[cfg(not(windows))]
        let path = Path::new("/tmp/meld root/#1%ü");
        #[cfg(not(windows))]
        let expected = "file:///tmp/meld%20root/%231%25%C3%BC";

        #[cfg(windows)]
        let path = Path::new(r"C:\temp\meld root\#1%ü");
        #[cfg(windows)]
        let expected = "file:///C:/temp/meld%20root/%231%25%C3%BC";

        assert_eq!(file_uri(path), expected);
        assert_eq!(uri_component("a b/c#d"), "a%20b%2Fc%23d");
    }

    #[test]
    fn external_environment_is_an_allowlist() {
        std::env::set_var("MELD_LSP_ENV_ALLOWLIST_SENTINEL", "must-not-leak");
        let allowed = external_environment();
        std::env::remove_var("MELD_LSP_ENV_ALLOWLIST_SENTINEL");

        assert!(allowed
            .iter()
            .all(|(key, _)| EXTERNAL_ENV_KEYS.contains(&key.as_str())));
        assert!(allowed
            .iter()
            .all(|(key, _)| key != "MELD_LSP_ENV_ALLOWLIST_SENTINEL"));
    }

    #[test]
    fn completion_timeout_terminates_a_stalled_server() {
        const SCRIPT: &str = r#"
import json
import sys

def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))

def write_message(msg):
    body = json.dumps(msg).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: " + str(len(body)).encode("ascii") + b"\r\n\r\n" + body)
    sys.stdout.buffer.flush()

while True:
    msg = read_message()
    if msg is None:
        break
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method not in ("initialize", "shutdown"):
        continue  # completion is intentionally never answered
    result = {"capabilities": {}} if method == "initialize" else None
    write_message({"jsonrpc": "2.0", "id": msg["id"], "result": result})
"#;

        let dir = std::env::temp_dir().join(format!("meld-lsp-timeout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("stalled_server.py");
        std::fs::write(&script, SCRIPT).unwrap();

        let spec = LspSpec {
            language_id: "python".to_string(),
            extension: "py".to_string(),
            names: vec![],
            command: Some("python3".to_string()),
            args: vec![script.to_string_lossy().into_owned()],
            keywords: vec![],
            region_end: Default::default(),
        };
        let mut client = ExternalLsp::with_timeout(&spec, &dir, Duration::from_secs(2)).unwrap();
        let error = client
            .completion(
                "meld-virtual:///x.py",
                json!({"line": 0, "character": 0}),
                None,
            )
            .unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(client.is_broken());
        let _ = client.child.wait();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
