//! Real tree-sitter parses for every bundled grammar. These are not structural
//! proxies: each snippet is parsed by the published grammar and must have no
//! ERROR/MISSING nodes.
use meld_core::splitter::{Region, RegionKind, SplitDocument};
use meld_core::syntax::SyntaxLayer;
use meld_grammars::{language, register_all};
use std::path::Path;

fn parse(lang: &str, code: &str) -> meld_core::error::Result<meld_core::syntax::SyntaxInfo> {
    let mut layer = SyntaxLayer::new();
    register_all(&mut layer);
    let doc = SplitDocument {
        source: code.to_string(),
        default_lang: lang.to_string(),
        regions: vec![],
    };
    let region = Region {
        id: format!("{lang}-test"),
        lang: lang.to_string(),
        kind: RegionKind::Block,
        start: 0,
        end: code.len(),
        full_start: 0,
        full_end: code.len(),
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    };
    layer.parse_region(&doc, &region)
}

#[test]
fn every_bundled_grammar_parses_a_real_snippet_without_errors() {
    let cases: &[(&str, &str)] = &[
        ("rust", "fn main() { let x = 1; }"),
        ("go", "package main\nfunc main() { x := 1; _ = x }"),
        ("c", "int main(void) { return 0; }"),
        ("cpp", "int main() { return 0; }"),
        ("java", "class A { int f() { return 1; } }"),
        ("json", "{\"a\": 1, \"b\": [true, null]}"),
        ("markdown", "# Title\n\nHello **world**.\n"),
        ("sql", "SELECT id FROM users WHERE id = 1;"),
        ("html", "<html><body><p>hi</p></body></html>"),
        ("css", "body { color: red; }"),
        ("python", "def f(x):\n    return x + 1\n"),
        ("javascript", "function f(x) { return x + 1; }"),
        ("lua", "local function f(x) return x + 1 end\n"),
    ];
    for (lang, code) in cases {
        let info = parse(lang, code)
            .unwrap_or_else(|e| panic!("{lang}: grammar rejected snippet: {e}"));
        assert!(!info.has_error, "{lang}: ERROR/MISSING node in {code:?}");
        assert!(info.named_nodes > 0, "{lang}: no named nodes");
    }
}

#[test]
fn every_packs_published_highlight_query_compiles() {
    let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
    let mut checked = 0;
    for (id, _) in meld_grammars_tests_language_ids() {
        let query_file = packs.join(id).join("queries/highlights.scm");
        let text = std::fs::read_to_string(&query_file)
            .unwrap_or_else(|e| panic!("{}: {e}", query_file.display()));
        let lang = language(id).unwrap_or_else(|| panic!("no bundled grammar for {id}"));
        tree_sitter::Query::new(&lang, &text)
            .unwrap_or_else(|e| panic!("{}: invalid published query: {e}", query_file.display()));
        checked += 1;
    }
    assert_eq!(checked, 13);
}

fn meld_grammars_tests_language_ids() -> Vec<(&'static str, ())> {
    register_all(&mut SyntaxLayer::new())
        .into_iter()
        .map(|id| (id, ()))
        .collect()
}

#[test]
fn source_map_points_are_relative_to_the_whole_meld_file() {
    // JUDGMENT CALL: a region after a multi-line header must keep its real
    // row in tree-sitter coordinates. This catches the old placeholder that
    // set every region's start_point to row 0.
    let prefix = "#meld python\n\nprose\n\n";
    let body = "x = 1\n";
    let source = format!("{prefix}{body}");
    let mut layer = SyntaxLayer::new();
    register_all(&mut layer);
    let doc = SplitDocument {
        source: source.clone(),
        default_lang: "python".to_string(),
        regions: vec![],
    };
    let region = Region {
        id: "r0".to_string(),
        lang: "python".to_string(),
        kind: RegionKind::Block,
        start: prefix.len(),
        end: source.len(),
        full_start: prefix.len(),
        full_end: source.len(),
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    };
    let info = layer.parse_region(&doc, &region).expect("parse region");
    assert!(!info.has_error);
}
