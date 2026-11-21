//! Phase 5 done-when test: completion works inside a non-default region.
use meld_lsp::Backend;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tower_lsp_server::ls_types::{
    CompletionParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
};
use tower_lsp_server::{LanguageServer, LspService};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn as_value(response: tower_lsp_server::ls_types::CompletionResponse) -> Value {
    serde_json::to_value(response).expect("completion response serializes")
}

fn refresh_checksums(pack: &Path) {
    let root = root();
    let pydeps = std::fs::canonicalize(&root)
        .expect("canonical workspace root")
        .parent()
        .expect("workspace parent")
        .join("pydeps");
    let script = "import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); import packtool; packtool.write_checksums(Path(sys.argv[2]))";
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(root.join("tools"))
        .arg(pack)
        .env("PYTHONPATH", pydeps)
        .output()
        .expect("refresh checksums");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn labels(value: &Value) -> Vec<String> {
    let items = value
        .as_array()
        .or_else(|| value.get("items").and_then(Value::as_array))
        .unwrap();
    items
        .iter()
        .map(|i| i.get("label").and_then(Value::as_str).unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn completion_inside_non_default_region_forwards_and_maps_back() {
    let root = root();
    let extra = vec![root.join("tests/fixtures/pack-lsp-mock")];
    let (service, _socket) =
        LspService::new(|client| Backend::with_pack_roots(client, root.clone(), extra.clone()));
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
        "uri=meld-virtual:///file%3A%2F%2F%2Ftest.meld/js.js"
    );
    // The mock echoed the virtual cursor. After mapping it must be the exact
    // original .meld position, not the padded virtual-file coordinate.
    assert_eq!(
        items[0]["textEdit"]["range"]["start"],
        json!({"line": 3, "character": 11})
    );
    assert_eq!(
        items[0]["textEdit"]["range"]["end"],
        json!({"line": 3, "character": 11})
    );
}

#[tokio::test]
async fn identical_region_ids_in_two_documents_get_distinct_virtual_uris() {
    let root = root();
    let extra = vec![root.join("tests/fixtures/pack-lsp-mock")];
    let (service, _socket) =
        LspService::new(|client| Backend::with_pack_roots(client, root.clone(), extra.clone()));
    let backend = service.inner();
    let source = "#meld python\n@mocklang as js {\n  console.l\n}\n";

    let mut uris = Vec::new();
    for name in ["file:///one.meld", "file:///two.meld"] {
        let opened: DidOpenTextDocumentParams = serde_json::from_value(json!({
            "textDocument": {
                "uri": name,
                "languageId": "meld",
                "version": 1,
                "text": source,
            }
        }))
        .unwrap();
        backend.did_open(opened).await;
        let params: CompletionParams = serde_json::from_value(json!({
            "textDocument": { "uri": name },
            "position": { "line": 2, "character": 11 },
        }))
        .unwrap();
        let response = backend.completion(params).await.unwrap().unwrap();
        let detail = as_value(response)[0]["detail"]
            .as_str()
            .unwrap()
            .to_string();
        uris.push(detail);
    }
    assert_ne!(uris[0], uris[1], "virtual documents must not collide");
    assert!(uris[0].contains("one.meld"), "{:?}", uris[0]);
    assert!(uris[1].contains("two.meld"), "{:?}", uris[1]);

    // Closing one document must not disturb the other document's virtual URI.
    let closed: DidCloseTextDocumentParams = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///one.meld" }
    }))
    .unwrap();
    backend.did_close(closed).await;
}

#[tokio::test]
async fn a_failed_external_server_falls_back_to_pack_keywords() {
    let root = root();
    let dir = std::env::temp_dir().join(format!(
        "meld-lsp-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let pack = dir.join("packs/broken");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.toml"),
        "id = \"broken\"\naliases = []\ntier = \"L0\"\n",
    )
    .unwrap();
    std::fs::write(
        pack.join("lsp.toml"),
        "[lsp]\nlanguage_id = \"broken\"\nextension = \"br\"\ncommand = \"python3\"\nargs = [\"-c\", \"import sys; sys.exit(1)\"]\nkeywords = [\"fallbackKeyword\"]\n",
    )
    .unwrap();
    refresh_checksums(&pack);
    let (service, _socket) = LspService::new(|client| {
        Backend::with_pack_roots(client, root.clone(), vec![dir.join("packs")])
    });
    let backend = service.inner();
    let opened: DidOpenTextDocumentParams = serde_json::from_value(json!({
        "textDocument": {
            "uri": "file:///fallback-server.meld",
            "languageId": "meld",
            "version": 1,
            "text": "#meld python\n@broken as b {\n  x\n}\n",
        }
    }))
    .unwrap();
    backend.did_open(opened).await;
    let params: CompletionParams = serde_json::from_value(json!({
        "textDocument": { "uri": "file:///fallback-server.meld" },
        "position": { "line": 2, "character": 3 },
    }))
    .unwrap();
    let response = backend.completion(params).await.unwrap().unwrap();
    let found = labels(&as_value(response));
    assert!(found.contains(&"fallbackKeyword".to_string()), "{found:?}");
    std::fs::remove_dir_all(dir).unwrap();
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
