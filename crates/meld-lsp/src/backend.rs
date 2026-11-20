//! Meld language server backend (Phase 5).
use crate::external::ExternalLsp;
use crate::packs::{self, LspSpec};
use crate::virtual_doc::{position_at_byte, VirtualDocument};
use meld_core::binding;
use meld_core::splitter::{self, Region};
use meld_core::syntax::SyntaxLayer;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tower_lsp_server::jsonrpc::{self, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

#[derive(Clone)]
struct DocumentState {
    text: String,
    version: i32,
    split: splitter::SplitDocument,
}

/// The Meld LSP backend. One instance serves the whole editor session.
pub struct Backend {
    client: Client,
    root: PathBuf,
    specs: HashMap<String, LspSpec>,
    documents: Arc<RwLock<HashMap<String, DocumentState>>>,
    external: Arc<RwLock<HashMap<String, Arc<Mutex<ExternalLsp>>>>>,
    opened: Arc<RwLock<HashMap<String, HashSet<String>>>>,
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
        Self {
            client,
            root,
            specs,
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
        let split = splitter::split(&text, &splitter::default_scanners())
            .unwrap_or_else(|_| splitter::SplitDocument {
                source: text.clone(),
                default_lang: String::new(),
                regions: Vec::new(),
            });
        DocumentState { text, version, split }
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
        let mut syntax = SyntaxLayer::new();
        meld_grammars::register_all(&mut syntax);
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
                        message: format!("meld: syntax error in {} region {}", region.lang, region.id),
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

    fn external_client(&self, lang: &str, spec: &LspSpec) -> Result<Arc<Mutex<ExternalLsp>>> {
        if let Some(existing) = self.external.read().unwrap().get(lang) {
            return Ok(Arc::clone(existing));
        }
        let client = ExternalLsp::spawn(spec, &self.root).map_err(Self::rpc_error)?;
        let client = Arc::new(Mutex::new(client));
        self.external
            .write()
            .unwrap()
            .insert(lang.to_string(), Arc::clone(&client));
        Ok(client)
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
}

impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                completion_provider: Some(CompletionOptions::default()),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let state = self.parse(params.text_document.text, params.text_document.version);
        let diagnostics = self.diagnostics(&state);
        self.documents.write().unwrap().insert(uri, state);
        self.client
            .publish_diagnostics(
                params.text_document.uri,
                diagnostics,
                Some(params.text_document.version),
            )
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let Some(text) = params.content_changes.first().map(|c| c.text.clone()) else {
            return;
        };
        let state = self.parse(text, params.text_document.version);
        let diagnostics = self.diagnostics(&state);
        self.documents.write().unwrap().insert(uri.clone(), state.clone());
        self.client
            .publish_diagnostics(
                params.text_document.uri,
                diagnostics,
                Some(params.text_document.version),
            )
            .await;

        // Mirror changed virtual documents into any external servers already
        // opened for this file. New regions are opened lazily by completion.
        let opened_langs: Vec<String> = self
            .opened
            .read()
            .unwrap()
            .iter()
            .filter(|(_, uris)| uris.contains(&uri))
            .map(|(lang, _)| lang.clone())
            .collect();
        for lang in opened_langs {
            let Some(spec) = self.specs.get(&lang) else { continue };
            let Some(ext) = self.external.read().unwrap().get(&lang).cloned() else { continue };
            for region in &state.split.regions {
                if region.lang != lang || region.kind == splitter::RegionKind::Header {
                    continue;
                }
                let vd = VirtualDocument::new(
                    &state.split,
                    region,
                    &spec.language_id,
                    &spec.extension,
                );
                let version = state.version;
                let text = vd.text.clone();
                let uri = vd.uri.clone();
                let ext = Arc::clone(&ext);
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut client) = ext.lock() {
                        let _ = client.did_change(&uri, version, &text);
                    }
                })
                .await;
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params
            .text_document_position
            .text_document
            .uri
            .to_string();
        let target = params.text_document_position.position;
        let state = {
            let documents = self.documents.read().unwrap();
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
            &state.split,
            &region,
            &spec.language_id,
            &spec.extension,
        );

        if spec.command.is_none() {
            return Ok(Some(CompletionResponse::Array(self.fallback_items(&state, spec))));
        }

        let ext = self.external_client(&region.lang, spec)?;
        let lang = region.lang.clone();
        let virtual_uri = vd.uri.clone();
        let should_open = self
            .opened
            .write()
            .unwrap()
            .entry(lang.clone())
            .or_default()
            .insert(virtual_uri.clone());
        let language_id = spec.language_id.clone();
        let open_version = state.version;
        let open_text = vd.text.clone();
        let open_ext = Arc::clone(&ext);
        let open_uri = virtual_uri.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if should_open {
                if let Ok(mut client) = open_ext.lock() {
                    let _ = client.did_open(&open_uri, &language_id, open_version, &open_text);
                }
            }
        })
        .await;

        let local = vd.local_position(&state.text, target);
        let local_json = json!({ "line": local.line, "character": local.character });
        let context = params
            .context
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok());
        let ext = Arc::clone(&ext);
        let completion_uri = virtual_uri.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut client = match ext.lock() {
                Ok(c) => c,
                Err(e) => return Err(e.to_string()),
            };
            client.completion(&completion_uri, local_json, context)
        })
        .await
        .map_err(|e| Self::rpc_error(e.to_string()))?
        .map_err(Self::rpc_error)?;

        let mut result = result;
        map_response_ranges(&mut result, &vd, &state.text);
        serde_json::from_value(result).map(Some).map_err(|e| {
            Self::rpc_error(format!("invalid completion response from {}: {e}", region.lang))
        })
    }
}

/// Rewrite virtual-document ranges in a completion response to `.meld`
/// coordinates. Items without text edits need no rewriting.
fn map_response_ranges(value: &mut serde_json::Value, vd: &VirtualDocument, source: &str) {
    if let Some(items) = value.as_array_mut() {
        for item in items {
            map_item(item, vd, source);
        }
    } else if let Some(items) = value.get_mut("items").and_then(serde_json::Value::as_array_mut) {
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
