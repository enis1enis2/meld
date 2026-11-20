//! Published tree-sitter grammars wired into Meld's syntax layer.
//!
//! JUDGMENT CALL: grammar crates are pinned to exact versions here and in each
//! pack's `pack.toml`. The core never links a grammar directly; this crate is
//! the registry of grammars the toolchain ships with. Adding a language still
//! only costs a pack + one entry here, never a core change.
use meld_core::syntax::SyntaxLayer;
use std::sync::LazyLock;
use tree_sitter::Language;

// JUDGMENT CALL: LanguageFn -> Language is not const, so the registry is a
// LazyLock. Initialisation happens once on first grammar lookup.
static LANGUAGES: LazyLock<Vec<(&'static str, Language)>> = LazyLock::new(|| {
    vec![
        ("rust", tree_sitter_rust::LANGUAGE.into()),
        ("go", tree_sitter_go::LANGUAGE.into()),
        ("c", tree_sitter_c::LANGUAGE.into()),
        ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        ("java", tree_sitter_java::LANGUAGE.into()),
        ("json", tree_sitter_json::LANGUAGE.into()),
        ("markdown", tree_sitter_md::LANGUAGE.into()),
        ("sql", tree_sitter_sequel::LANGUAGE.into()),
        ("html", tree_sitter_html::LANGUAGE.into()),
        ("css", tree_sitter_css::LANGUAGE.into()),
        ("python", tree_sitter_python::LANGUAGE.into()),
        ("javascript", tree_sitter_javascript::LANGUAGE.into()),
        ("lua", tree_sitter_lua::LANGUAGE.into()),
        // JUDGMENT CALL (language expansion, 2026-11-20): each id below matches a
        // strict Lang Pack in packs/ and an exact crate version in Cargo.toml.
        // Multi-grammar crates expose separate constants; TypeScript and TSX share
        // one pinned crate, as do PHP and OCaml's implementation grammar.
        (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        ("bash", tree_sitter_bash::LANGUAGE.into()),
        ("ruby", tree_sitter_ruby::LANGUAGE.into()),
        ("php", tree_sitter_php::LANGUAGE_PHP.into()),
        ("yaml", tree_sitter_yaml::LANGUAGE.into()),
        ("toml", tree_sitter_toml_ng::LANGUAGE.into()),
        ("csharp", tree_sitter_c_sharp::LANGUAGE.into()),
        ("haskell", tree_sitter_haskell::LANGUAGE.into()),
        ("ocaml", tree_sitter_ocaml::LANGUAGE_OCAML.into()),
        ("scala", tree_sitter_scala::LANGUAGE.into()),
        ("zig", tree_sitter_zig::LANGUAGE.into()),
        ("r", tree_sitter_r::LANGUAGE.into()),
        ("dart", tree_sitter_dart::LANGUAGE.into()),
        ("solidity", tree_sitter_solidity::LANGUAGE.into()),
    ]
});

fn language_entries() -> &'static [(&'static str, Language)] {
    LANGUAGES.as_slice()
}

/// Bundled grammar lookup, used by query validation and the CLI.
pub fn language(id: &str) -> Option<Language> {
    language_entries()
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, lang)| lang.clone())
}

/// Register every bundled grammar. Returns the ids that were registered.
pub fn register_all(layer: &mut SyntaxLayer) -> Vec<&'static str> {
    let entries = language_entries();
    for (id, lang) in entries {
        layer.register(id, lang.clone());
    }
    entries.iter().map(|(id, _)| *id).collect()
}
