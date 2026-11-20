//! Phase 5 done-when test: completion works inside a non-default region.
use meld_lsp::Backend;
use serde_json::{json, Value};
use std::path::PathBuf;
use tower_lsp_server::ls_types::{CompletionParams, DidOpenTextDocumentParams};
use tower_lsp_server::{LanguageServer, LspService};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn as_value(response: tower_lsp_server::ls_types::CompletionResponse) -> Value {
    serde_json::to_value(response).expect("completion response serializes")
}

fn labels(value: &Value) -> Vec<String> {
    let items = value.as_array().or_else(|| value.get("items").and_then(Value::as_array)).unwrap();
    items
        .iter()
        .map(|i| i.get("label").and_then(Value::as_str).unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn completion_inside_non_default_region_forwards_and_maps_back() {
    let root = root();
    let extra = vec![root.join("tests/fixtures/pack-lsp-mock")];
    let (service, _socket) = LspService::new(|client| {
        Backend::with_pack_roots(client, root.clone(), extra.clone())
    });
    let backend = service.inner();
    let source = "#meld python\nconst setup = 1;\n@mocklang as js {\n  console.l\n}\n";

    let opened: DidOpenTextDocumentParams = serde_json::from_value(json!({
        "textDocument": {
            "uri": "file:///test.meld",
            "languageId": "meld",
            "version": 1,
            "text": source,
        }
    }))
    .unwrap();
    backend.did_open(opened).await;

    let params: CompletionParams = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///test.meld" },
        "position": { "line": 3, "character": 11 },
    }))
    .unwrap();
    let response = backend
        .completion(params)
        .await
        .expect("completion RPC succeeds")
        .expect("non-default region returns items");
    let value = as_value(response);
    let items = value.as_array().expect("mock returns an array");

    assert_eq!(items[0]["label"], "mockFromVirtualDoc");
    assert_eq!(
        items[0]["detail"].as_str().unwrap(),
        "uri=meld-virtual:///js.js"
    );
    // The mock echoed the virtual cursor. After mapping it must be the exact
    // original .meld position, not the padded virtual-file coordinate.
    assert_eq!(items[0]["textEdit"]["range"]["start"], json!({"line": 3, "character": 11}));
    assert_eq!(items[0]["textEdit"]["range"]["end"], json!({"line": 3, "character": 11}));
}

#[tokio::test]
async fn packs_without_an_external_server_get_deterministic_keywords() {
    let (service, _socket) = LspService::new(|client| Backend::with_root(client, root()));
    let backend = service.inner();
    let opened: DidOpenTextDocumentParams = serde_json::from_value(json!({
        "textDocument": {
            "uri": "file:///fallback.meld",
            "languageId": "meld",
            "version": 1,
            "text": "#meld python\nprint(1)\n",
        }
    }))
    .unwrap();
    backend.did_open(opened).await;

    let params: CompletionParams = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///fallback.meld" },
        "position": { "line": 1, "character": 5 },
    }))
    .unwrap();
    let response = backend.completion(params).await.unwrap().unwrap();
    let found = labels(&as_value(response));
    assert!(found.contains(&"enumerate".to_string()), "{found:?}");
}
