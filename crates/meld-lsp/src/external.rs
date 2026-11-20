//! Minimal stdio LSP client used to forward one region to its own language
//! server (project.md §7).
//!
//! **JUDGMENT CALL:** a pack's `lsp.toml` names the external server command.
//! The client is deliberately small: initialize, didOpen/didChange, completion,
//! shutdown. That is the smallest honest implementation of "virtual document
//! forwarding" and is testable with a deterministic mock server. Richer
//! features (hover, rename, incremental sync) are future work per language.
use crate::packs::LspSpec;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct ExternalLsp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    initialized: bool,
    root_uri: String,
}

impl ExternalLsp {
    pub fn spawn(spec: &LspSpec, root: &Path) -> Result<Self, String> {
        let program = spec.command.as_ref().ok_or("pack has no lsp command")?;
        let mut child = Command::new(program)
            .args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            initialized: false,
            root_uri: format!("file://{}", root.to_string_lossy().replace(' ', "%20")),
        })
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

    pub fn did_open(&mut self, uri: &str, language_id: &str, version: i32, text: &str) -> Result<(), String> {
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

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let msg = self.read_message()?;
            if msg.get("id").and_then(Value::as_i64) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(format!("{method}: {err}"));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Server-initiated requests are out of scope for this client; a
            // mock or a minimal server will only send responses/notifications.
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn write_message(&mut self, msg: &Value) -> Result<(), String> {
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

    fn read_message(&mut self) -> Result<Value, String> {
        let mut length = None;
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).map_err(|e| e.to_string())?;
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
        self.stdout.read_exact(&mut buf).map_err(|e| e.to_string())?;
        serde_json::from_slice(&buf).map_err(|e| format!("invalid LSP JSON: {e}"))
    }
}

impl Drop for ExternalLsp {
    fn drop(&mut self) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", json!({}));
        let _ = self.child.kill();
    }
}
