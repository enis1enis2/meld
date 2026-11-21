//! Meld language server backend (Phase 5).
use crate::external::ExternalLsp;
use crate::packs::{self, LspSpec};
use crate::virtual_doc::{position_at_byte, VirtualDocument};
use meld_core::binding;
use meld_core::splitter::{self, Region};
use meld_core::syntax::SyntaxLayer;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tower_lsp_server::jsonrpc::{self, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

#[derive(Clone)]
struct DocumentState {
    text: String,
    version: i32,
    split: splitter::SplitDocument,
}

/// One open virtual document in an external server. The virtual URI is the key
/// because that is what the external server tracks; `doc_uri` lets `didClose`
/// and `didChange` find all virtual documents belonging to an editor tab.
#[derive(Clone)]
struct OpenRegion {
    lang: String,
    doc_uri: String,
}

/// The Meld LSP backend. One instance serves the whole editor session.
pub struct Backend {
    client: Client,
    root: PathBuf,
    specs: HashMap<String, LspSpec>,
    scanners: splitter::ScannerRegistry,
    /// Parsing trees are expensive to rebuild on every keystroke; keep one
    /// syntax layer for the session and guard it against concurrent requests.
    syntax: Arc<Mutex<SyntaxLayer>>,
    documents: Arc<RwLock<HashMap<String, DocumentState>>>,
    external: Arc<RwLock<HashMap<String, Arc<Mutex<ExternalLsp>>>>>,
    opened: Arc<RwLock<HashMap<String, OpenRegion>>>,
}

/// A poisoned lock means another task panicked while holding it. The state is
/// still internally consistent for these plain maps, so recover the guard
/// instead of turning a panic into a cascade of LSP request failures (H10).
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self::with_root(client, crate::project_root())
    }

    /// Test-friendly constructor: pack-relative commands are rebased to `root`.
    pub fn with_root(client: Client, root: PathBuf) -> Self {
        let specs = packs::discover(&root);
        Self::with_specs(client, root, specs)
    }

    /// Constructor with explicit extra pack roots, avoiding process-global env
    /// mutation in tests (JUDGMENT CALL: same semantics as CLI `MELD_PACKS`).
    pub fn with_pack_roots(client: Client, root: PathBuf, extra: Vec<PathBuf>) -> Self {
        let specs = packs::discover_with(&root, &extra);
        Self::with_specs(client, root, specs)
    }

    fn with_specs(client: Client, root: PathBuf, specs: HashMap<String, LspSpec>) -> Self {
        let scanners = packs::scanner_registry(&specs);
        let mut syntax = SyntaxLayer::new();
        meld_grammars::register_all(&mut syntax);
        Self {
            client,
            root,
            specs,
            scanners,
            syntax: Arc::new(Mutex::new(syntax)),
            documents: Arc::new(RwLock::new(HashMap::new())),
            external: Arc::new(RwLock::new(HashMap::new())),
            opened: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn rpc_error(message: impl Into<String>) -> jsonrpc::Error {
        jsonrpc::Error {
            code: jsonrpc::ErrorCode::InternalError,
            message: std::borrow::Cow::Owned(message.into()),
            data: None,
        }
    }

    fn parse(&self, text: String, version: i32) -> DocumentState {
        let split =
            splitter::split(&text, &self.scanners).unwrap_or_else(|_| splitter::SplitDocument {
                source: text.clone(),
                default_lang: String::new(),
                regions: Vec::new(),
            });
        DocumentState {
            text,
            version,
            split,
        }
    }

    fn diagnostics(&self, state: &DocumentState) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if state.split.regions.is_empty() && !state.text.trim().is_empty() {
            out.push(Diagnostic {
                range: Range::new(Position::new(0, 0), Position::new(0, 1)),
                severity: Some(DiagnosticSeverity::ERROR),
                message: "meld: could not split document".to_string(),
                source: Some("meld".to_string()),
                ..Default::default()
            });
            return out;
        }
        let mut syntax = lock(&self.syntax);
        for region in &state.split.regions {
            if region.kind == splitter::RegionKind::Header
                || region.kind == splitter::RegionKind::Untagged
                || region.lang == "text"
            {
                continue;
            }
            if let Ok(info) = syntax.parse_region(&state.split, region) {
                if info.has_error {
                    let p = position_at_byte(&state.text, region.start);
                    out.push(Diagnostic {
                        range: Range::new(p, Position::new(p.line, p.character + 1)),
                        severity: Some(DiagnosticSeverity::WARNING),
                        message: format!(
                            "meld: syntax error in {} region {}",
                            region.lang, region.id
                        ),
                        source: Some("meld".to_string()),
                        ..Default::default()
                    });
                }
            }
        }
        out
    }

    fn region_at(split: &splitter::SplitDocument, byte: usize) -> Option<Region> {
        split
            .regions
            .iter()
            .find(|r| byte >= r.start && byte < r.end)
            .or_else(|| {
                split
                    .regions
                    .iter()
                    .find(|r| byte >= r.full_start && byte < r.full_end)
            })
            .cloned()
    }

    /// Create at most one client per language, even when several completions
    /// race (the check-then-insert pattern alone can spawn duplicates).
    fn external_client(&self, lang: &str, spec: &LspSpec) -> Result<Arc<Mutex<ExternalLsp>>> {
        if let Some(existing) = read(&self.external).get(lang) {
            return Ok(Arc::clone(existing));
        }
        let mut clients = write(&self.external);
        if let Some(existing) = clients.get(lang) {
            return Ok(Arc::clone(existing));
        }
        let client = Arc::new(Mutex::new(
            ExternalLsp::spawn(spec, &self.root).map_err(Self::rpc_error)?,
        ));
        clients.insert(lang.to_string(), Arc::clone(&client));
        Ok(client)
    }

    /// Forget a failed/stopped client and every virtual document it owned.
    /// Dropping the last Arc outside any lock also shuts the child down.
    fn drop_client(&self, lang: &str) {
        let client = write(&self.external).remove(lang);
        write(&self.opened).retain(|_, open| open.lang != lang);
        drop(client);
    }

    fn fallback_items(&self, state: &DocumentState, spec: &LspSpec) -> Vec<CompletionItem> {
        let mut labels: Vec<String> = spec.keywords.clone();
        if let Ok(b) = binding::resolve(&state.split) {
            for export in b.exports {
                labels.push(export.name);
            }
            for import in b.imports {
                labels.push(import.name);
            }
        }
        labels.sort();
        labels.dedup();
        labels
            .into_iter()
            .map(|label| CompletionItem::new_simple(label, "meld pack".to_string()))
            .collect()
    }

    fn fallback_response(&self, state: &DocumentState, spec: &LspSpec) -> CompletionResponse {
        CompletionResponse::Array(self.fallback_items(state, spec))
    }

    /// Send `didClose` for virtual documents that no longer have a region.
    async fn close_virtual_documents(&self, mut by_lang: HashMap<String, Vec<String>>) {
        for (lang, uris) in by_lang.drain() {
            let Some(client) = read(&self.external).get(&lang).cloned() else {
                continue;
            };
            let _ = tokio::task::spawn_blocking(move || {
                let mut client = lock(&client);
                for uri in uris {
                    let _ = client.did_close(&uri);
                }
            })
            .await;
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions::default()),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn shutdown(&self) -> Result<()> {
        // End every external process instead of leaking children when the
        // editor closes the LSP session (H10).
        let clients: Vec<_> = write(&self.external).drain().map(|(_, c)| c).collect();
        write(&self.opened).clear();
        drop(clients);
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let state = self.parse(params.text_document.text, params.text_document.version);
        let diagnostics = self.diagnostics(&state);
        write(&self.documents).insert(uri, state);
        self.client
            .publish_diagnostics(
                params.text_document.uri,
                diagnostics,
                Some(params.text_document.version),
            )
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        write(&self.documents).remove(&uri);
        // Clear published diagnostics so stale errors do not linger in the UI.
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;

        let mut closes: HashMap<String, Vec<String>> = HashMap::new();
        write(&self.opened).retain(|virtual_uri, open| {
            if open.doc_uri != uri {
                return true;
            }
            closes
                .entry(open.lang.clone())
                .or_default()
                .push(virtual_uri.clone());
            false
        });
        self.close_virtual_documents(closes).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let Some(text) = params.content_changes.first().map(|c| c.text.clone()) else {
            return;
        };
        let state = self.parse(text, params.text_document.version);
        let diagnostics = self.diagnostics(&state);
        write(&self.documents).insert(uri.clone(), state.clone());
        self.client
            .publish_diagnostics(
                params.text_document.uri,
                diagnostics,
                Some(params.text_document.version),
            )
            .await;

        // Build the virtual URIs that are still live in the new document.
        let mut active: HashMap<String, (String, String)> = HashMap::new();
        for region in &state.split.regions {
            if region.kind == splitter::RegionKind::Header || region.lang == "text" {
                continue;
            }
            let Some(spec) = self.specs.get(&region.lang) else {
                continue;
            };
            let vd = VirtualDocument::new(
                &uri,
                &state.split,
                region,
                &spec.language_id,
                &spec.extension,
            );
            active.insert(vd.uri, (region.lang.clone(), vd.text));
        }

        let opened: Vec<(String, OpenRegion)> = read(&self.opened)
            .iter()
            .filter(|(_, open)| open.doc_uri == uri)
            .map(|(virtual_uri, open)| (virtual_uri.clone(), open.clone()))
            .collect();
        let mut closes: HashMap<String, Vec<String>> = HashMap::new();
        let mut changes: HashMap<String, Vec<(String, String, i32)>> = HashMap::new();
        for (virtual_uri, open) in opened {
            match active.get(&virtual_uri) {
                Some((lang, text)) if lang == &open.lang => {
                    changes.entry(open.lang).or_default().push((
                        virtual_uri,
                        text.clone(),
                        state.version,
                    ));
                }
                _ => {
                    closes
                        .entry(open.lang)
                        .or_default()
                        .push(virtual_uri.clone());
                    write(&self.opened).remove(&virtual_uri);
                }
            }
        }
        self.close_virtual_documents(closes).await;

        for (lang, updates) in changes {
            let Some(client) = read(&self.external).get(&lang).cloned() else {
                continue;
            };
            let _ = tokio::task::spawn_blocking(move || {
                let mut client = lock(&client);
                for (virtual_uri, text, version) in updates {
                    let _ = client.did_change(&virtual_uri, version, &text);
                }
            })
            .await;
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let target = params.text_document_position.position;
        let state = {
            let documents = read(&self.documents);
            documents.get(&uri).cloned()
        };
        let Some(state) = state else {
            return Ok(None);
        };
        let byte = crate::virtual_doc::byte_offset(&state.text, target);
        let Some(region) = Self::region_at(&state.split, byte) else {
            return Ok(None);
        };
        if region.kind == splitter::RegionKind::Header || region.lang == "text" {
            return Ok(None);
        }
        let Some(spec) = self.specs.get(&region.lang) else {
            return Ok(None);
        };
        let vd = VirtualDocument::new(
            &uri,
            &state.split,
            &region,
            &spec.language_id,
            &spec.extension,
        );

        if spec.command.is_none() {
            return Ok(Some(self.fallback_response(&state, spec)));
        }

        let lang = region.lang.clone();
        let virtual_uri = vd.uri.clone();
        // Record the virtual document before spawning so concurrent requests
        // for the same URI cannot both send didOpen.
        let should_open = write(&self.opened)
            .insert(
                virtual_uri.clone(),
                OpenRegion {
                    lang: lang.clone(),
                    doc_uri: uri.clone(),
                },
            )
            .is_none();

        let ext = match self.external_client(&lang, spec) {
            Ok(client) => client,
            Err(e) => {
                if should_open {
                    write(&self.opened).remove(&virtual_uri);
                }
                eprintln!("meld-lsp: {e}");
                return Ok(Some(self.fallback_response(&state, spec)));
            }
        };
        let language_id = spec.language_id.clone();
        let open_version = state.version;
        let open_text = vd.text.clone();
        let local = vd.local_position(&state.text, target);
        let local_json = json!({ "line": local.line, "character": local.character });
        let context = params
            .context
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok());
        let completion_uri = virtual_uri.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut client = lock(&ext);
            if should_open {
                client.did_open(&completion_uri, &language_id, open_version, &open_text)?;
            }
            client.completion(&completion_uri, local_json, context)
        })
        .await;

        let mut result = match result {
            Ok(Ok(value)) => value,
            Ok(Err(e)) => {
                self.drop_client(&lang);
                eprintln!(
                    "meld-lsp: external language server for {lang} failed: {e}; restarted on next request"
                );
                return Ok(Some(self.fallback_response(&state, spec)));
            }
            Err(e) => {
                // A timed-out/stalled server is killed by ExternalLsp; drop it
                // so the next request starts a fresh process, and answer from
                // the pack's deterministic fallback list instead of failing.
                self.drop_client(&lang);
                eprintln!(
                    "meld-lsp: external language server for {lang} failed: {e}; restarted on next request"
                );
                return Ok(Some(self.fallback_response(&state, spec)));
            }
        };
        map_response_ranges(&mut result, &vd, &state.text);
        match serde_json::from_value(result) {
            Ok(response) => Ok(Some(response)),
            Err(e) => {
                self.drop_client(&lang);
                eprintln!("meld-lsp: invalid completion response from {lang}: {e}");
                Ok(Some(self.fallback_response(&state, spec)))
            }
        }
    }
}

/// Rewrite virtual-document ranges in a completion response to `.meld`
/// coordinates. Items without text edits need no rewriting.
fn map_response_ranges(value: &mut serde_json::Value, vd: &VirtualDocument, source: &str) {
    if let Some(items) = value.as_array_mut() {
        for item in items {
            map_item(item, vd, source);
        }
    } else if let Some(items) = value
        .get_mut("items")
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in items {
            map_item(item, vd, source);
        }
    }
}

fn map_item(item: &mut serde_json::Value, vd: &VirtualDocument, source: &str) {
    for path in [
        "/textEdit/range",
        "/textEdit/insertAndReplace/range",
        "/textEdit/replace/range",
    ] {
        if let Some(range) = item.pointer_mut(path) {
            for side in ["start", "end"] {
                if let Some(pos) = range.get_mut(side) {
                    if let Ok(local) = serde_json::from_value::<Position>(pos.clone()) {
                        let global = vd.global_position(source, local);
                        *pos = json!({ "line": global.line, "character": global.character });
                    }
                }
            }
        }
    }
}
