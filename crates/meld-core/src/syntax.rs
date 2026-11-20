//! Tree-sitter syntax layer using per-region included ranges (project.md §3).
//!
//! JUDGMENT CALL: grammars are registered by Lang Packs at runtime; the core
//! does not link every tree-sitter grammar. This keeps the "adding a language
//! costs a pack, not a core change" property from project.md §1.
use crate::error::{MeldError, Result};
use crate::splitter::{Region, SplitDocument};
use std::collections::HashMap;
use tree_sitter::{Language, Parser, Point, Range};

#[derive(Clone, Debug)]
pub struct SyntaxInfo {
    pub region_id: String,
    pub root_kind: String,
    pub named_nodes: usize,
    pub has_error: bool,
}

pub struct SyntaxLayer {
    languages: HashMap<String, Language>,
}

impl SyntaxLayer {
    pub fn new() -> Self {
        Self {
            languages: HashMap::new(),
        }
    }

    /// Register a language from a pack. Packs own the compiled grammar.
    pub fn register(&mut self, id: &str, language: Language) {
        self.languages.insert(id.to_string(), language);
    }

    pub fn parse_region(&mut self, doc: &SplitDocument, region: &Region) -> Result<SyntaxInfo> {
        let language = self.languages.get(&region.lang).ok_or_else(|| {
            // Syntax-only (L0) packs are allowed; an unknown language is only
            // an error when the caller requires tree structure.
            MeldError::UnknownLang(region.lang.clone())
        })?;

        let mut parser = Parser::new();
        parser.set_language(language).map_err(|e| MeldError::Parse {
            region_id: region.id.clone(),
            message: format!("set_language: {e}"),
        })?;
        // JUDGMENT CALL: included ranges are expressed against the *whole*
        // .meld source, so their points must be real source coordinates.
        // The earlier placeholder (row 0 / row=byte-length) produced wrong
        // line:col data for every region after the header.
        let start = point_at(&doc.source, region.start);
        let mut end = point_at(&doc.source, region.end);
        let mut parse_source = sanitize_source(&doc.source, region);
        let mut end_byte = region.end;
        // JUDGMENT CALL: some grammars (notably tree-sitter-md's block
        // grammar) require a terminating newline. The parser copy gets a
        // synthetic one; the original document and source maps are untouched.
        if end_byte > region.start && parse_source.as_bytes()[end_byte - 1] != b'\n' {
            parse_source.insert(end_byte, '\n');
            end_byte += 1;
            end = Point {
                row: end.row + 1,
                column: 0,
            };
        }
        parser
            .set_included_ranges(&[Range {
                start_byte: region.start,
                end_byte,
                start_point: start,
                end_point: end,
            }])
            .map_err(|e| MeldError::Parse {
                region_id: region.id.clone(),
                message: format!("set_included_ranges: {e}"),
            })?;
        let tree = parser.parse(&parse_source, None).ok_or_else(|| MeldError::Parse {
            region_id: region.id.clone(),
            message: "tree-sitter returned no tree".to_string(),
        })?;
        let root = tree.root_node();
        Ok(SyntaxInfo {
            region_id: region.id.clone(),
            root_kind: root.kind().to_string(),
            named_nodes: count_named(root),
            has_error: root.has_error(),
        })
    }
}

/// Blank Meld glue lines while preserving every byte offset. **JUDGMENT
/// CALL:** `export`/`import` are host directives, not region source; parsing
/// them as Python/JS/Lua produces misleading ERROR nodes in `meld check`.
/// Replacing bytes with spaces keeps included ranges and source maps exact.
pub fn sanitize_source(source: &str, region: &Region) -> String {
    let mut bytes = source.as_bytes().to_vec();
    let mut line_start = region.start;
    while line_start < region.end {
        let mut line_end = line_start;
        while line_end < region.end && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        while line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        let text = std::str::from_utf8(&bytes[line_start..line_end]).unwrap_or("");
        let trimmed = text.trim_start();
        if trimmed.starts_with("export ") || trimmed.starts_with("import ") {
            for b in &mut bytes[line_start..line_end] {
                *b = b' ';
            }
        }
        line_start = line_end + 1;
    }
    String::from_utf8(bytes).expect("byte-preserving rewrite remains valid UTF-8")
}

/// Byte offset -> tree-sitter Point in the original source. Tree-sitter
/// columns are byte offsets, so `str::len()` is correct here (not char count).
fn point_at(source: &str, offset: usize) -> Point {
    let prefix = &source[..offset];
    let row = prefix.bytes().filter(|b| *b == b'\n').count();
    let column = prefix.rsplit('\n').next().unwrap_or("").len();
    Point { row, column }
}

fn count_named(node: tree_sitter::Node<'_>) -> usize {
    let mut n = usize::from(node.is_named());
    for i in 0..node.child_count() {
        if let Some(c) = node.child(i) {
            n += count_named(c);
        }
    }
    n
}
