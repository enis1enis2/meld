//! Virtual documents and position maps for one Meld region (project.md §7).
//!
//! An LSP client sees one `.meld` file; a language server sees one isolated
//! source file per region. This module builds that isolated file while keeping
//! a reversible byte/Position map back to the original file.
//!
//! **JUDGMENT CALL:** the virtual file is padded with one newline per earlier
//! line and spaces up to the region's starting column. That makes region-local
//! LSP line/column numbers identical to `.meld` coordinates for normal block
//! regions, and gives exact offsets for inline regions without rewriting the
//! grammar. Language servers that dislike leading blank lines still receive a
//! syntactically valid body because Meld glue lines are blanked byte-for-byte.
use meld_core::splitter::{Region, SplitDocument};
use meld_core::syntax::sanitize_source;
use tower_lsp_server::ls_types::Position;

#[derive(Clone, Debug)]
pub struct VirtualDocument {
    pub uri: String,
    pub language_id: String,
    pub text: String,
    pub region_id: String,
    /// Byte length of the newline/space padding at the start of `text`.
    prefix_len: usize,
    region_start: usize,
    region_end: usize,
}

impl VirtualDocument {
    /// `doc_uri` is the editor's URI for the containing `.meld` file. It is
    /// part of the virtual URI so two documents with the same region id cannot
    /// share an external server's virtual document (H9).
    pub fn new(
        doc_uri: &str,
        doc: &SplitDocument,
        region: &Region,
        language_id: &str,
        extension: &str,
    ) -> Self {
        let origin = position_at_byte(&doc.source, region.start);
        let mut text = "\n".repeat(origin.line as usize);
        text.push_str(&" ".repeat(origin.character as usize));
        // JUDGMENT CALL: `sanitize_source` returns a byte-preserving copy of
        // the whole file (tree-sitter included ranges need that). A virtual
        // document must contain only the region body.
        let sanitized = sanitize_source(&doc.source, region);
        text.push_str(&sanitized[region.start..region.end]);
        let ext = if extension.is_empty() {
            "txt"
        } else {
            extension
        };
        Self {
            uri: format!(
                "meld-virtual:///{}/{}.{}",
                crate::external::uri_component(doc_uri),
                crate::external::uri_component(&region.id),
                crate::external::uri_component(ext)
            ),
            language_id: language_id.to_string(),
            text,
            region_id: region.id.clone(),
            prefix_len: origin.line as usize + origin.character as usize,
            region_start: region.start,
            region_end: region.end,
        }
    }

    /// A global `.meld` cursor becomes a region-local cursor.
    pub fn local_position(&self, source: &str, global: Position) -> Position {
        let byte = byte_offset(source, global);
        let local_byte = byte
            .saturating_sub(self.region_start)
            .min(self.region_end - self.region_start);
        let virtual_byte = self.prefix_len + local_byte;
        position_at_byte(&self.text, virtual_byte)
    }

    /// A region-local cursor from a language server becomes a global cursor.
    pub fn global_position(&self, source: &str, local: Position) -> Position {
        let virtual_byte = byte_offset(&self.text, local);
        let body_byte = virtual_byte.saturating_sub(self.prefix_len);
        let global_byte = (self.region_start + body_byte).min(self.region_end.min(source.len()));
        position_at_byte(source, global_byte)
    }
}

/// Byte offset of an LSP Position. LSP columns count UTF-16 code units.
pub fn byte_offset(source: &str, position: Position) -> usize {
    let mut byte = line_start(source, position.line as usize);
    let mut units = 0usize;
    let target = position.character as usize;
    while byte < source.len() {
        if source[byte..].starts_with('\n') || source[byte..].starts_with("\r\n") {
            break;
        }
        let ch = match source[byte..].chars().next() {
            Some(c) => c,
            None => break,
        };
        if units + ch.len_utf16() > target {
            break;
        }
        units += ch.len_utf16();
        byte += ch.len_utf8();
        if units == target {
            break;
        }
    }
    byte
}

/// Inverse of [`byte_offset`]; clamps to a character boundary.
pub fn position_at_byte(source: &str, byte: usize) -> Position {
    let byte = byte.min(source.len());
    let byte = clamp_boundary(source, byte);
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    Position {
        line: line as u32,
        character: utf16_len(&source[line_start..byte]) as u32,
    }
}

fn line_start(source: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut seen = 0;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    source.len()
}

fn clamp_boundary(source: &str, mut byte: usize) -> usize {
    while byte > 0 && !source.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meld_core::splitter::{split, ScannerRegistry};

    const SRC: &str = "#meld python\nconst setup = 1;\n@javascript as js {\n  console.l\n}\n";

    fn region() -> (
        meld_core::splitter::SplitDocument,
        meld_core::splitter::Region,
    ) {
        let doc = split(SRC, &ScannerRegistry::default_registry()).unwrap();
        let r = doc
            .regions
            .iter()
            .find(|r| r.lang == "javascript")
            .cloned()
            .unwrap();
        (doc, r)
    }

    #[test]
    fn virtual_document_preserves_global_coordinates() {
        let (doc, r) = region();
        let vd = VirtualDocument::new("file:///a.meld", &doc, &r, "javascript", "js");
        let global = Position {
            line: 3,
            character: 11,
        };
        let local = vd.local_position(&doc.source, global);
        assert_eq!(local, global);
        assert_eq!(vd.global_position(&doc.source, local), global);
        assert!(vd.text.contains("console.l"));
        assert!(!vd.text.contains('@'));
    }

    #[test]
    fn utf16_columns_survive_a_multibyte_prefix() {
        let src = "#meld python\n⟦python: 'héllo'⟧\n";
        let doc = split(src, &ScannerRegistry::default_registry()).unwrap();
        let r = doc
            .regions
            .iter()
            .find(|r| r.kind == meld_core::splitter::RegionKind::Inline)
            .unwrap();
        let vd = VirtualDocument::new("file:///a.meld", &doc, r, "python", "py");
        // Character 12 is the `h` in `'héllo'`; LSP columns count `é` as one
        // UTF-16 unit, while Rust byte offsets need two bytes.
        let p = Position {
            line: 1,
            character: 12,
        };
        assert_eq!(
            vd.global_position(&doc.source, vd.local_position(&doc.source, p)),
            p
        );
    }

    #[test]
    fn utf16_byte_offsets_round_trip_across_a_surrogate_pair() {
        // 😀 is 4 UTF-8 bytes but 2 UTF-16 code units. A cursor after the emoji
        // must land on the byte after it, not two bytes later.
        let source = "a😀b";
        let position = Position {
            line: 0,
            character: 3, // a=1 + 😀=2
        };
        let byte = byte_offset(source, position);
        assert_eq!(&source[..byte], "a😀");
        assert_eq!(position_at_byte(source, byte), position);
    }

    #[test]
    fn virtual_region_round_trips_utf16_columns_after_an_emoji() {
        let src = "#meld python\n⟦python: x = '😀'; y = 1⟧\n";
        let doc = split(src, &ScannerRegistry::default_registry()).unwrap();
        let r = doc
            .regions
            .iter()
            .find(|r| r.kind == meld_core::splitter::RegionKind::Inline)
            .unwrap();
        let vd = VirtualDocument::new("file:///a.meld", &doc, r, "python", "py");

        // Byte just after the closing quote following the surrogate pair.
        let byte = src.find("'; y").unwrap() + 1;
        let global = position_at_byte(src, byte);
        assert_eq!(global.character, 17, "UTF-16 column should count 😀 as two");

        let local = vd.local_position(&doc.source, global);
        assert_eq!(vd.global_position(&doc.source, local), global);
    }

    #[test]
    fn virtual_uri_percent_encodes_extension_and_is_document_scoped() {
        let (doc, r) = region();
        let vd = VirtualDocument::new("file:///a b.meld", &doc, &r, "javascript", "j s#1");
        assert_eq!(
            vd.uri,
            "meld-virtual:///file%3A%2F%2F%2Fa%20b.meld/js.j%20s%231"
        );
        let other = VirtualDocument::new("file:///c.meld", &doc, &r, "javascript", "j s#1");
        assert_ne!(vd.uri, other.uri);
    }
}
