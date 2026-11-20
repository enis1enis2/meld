//! Region splitter (Phase 1).
//!
//! JUDGMENT CALL (spec/syntax.md): Phase 1 uses deterministic delimiters first.
//! The pack-provided end scanner is used for plain `{ ... }` blocks; raw hash
//! delimiters are always accepted as the collision-free fallback.
use crate::error::{MeldError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Header,
    Untagged,
    Block,
    Inline,
    Fence,
}

#[derive(Clone, Debug)]
pub struct Region {
    pub id: String,
    pub lang: String,
    pub kind: RegionKind,
    /// Body start byte offset (inside markers).
    pub start: usize,
    /// Body end byte offset (exclusive).
    pub end: usize,
    /// Full span including markers.
    pub full_start: usize,
    pub full_end: usize,
    pub open_marker: String,
    pub close_marker: String,
    pub confidence: f32,
}

impl Region {
    pub fn body<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

#[derive(Clone, Debug)]
pub struct SplitDocument {
    pub source: String,
    pub default_lang: String,
    pub regions: Vec<Region>,
}

/// The end of one plain `{ ... }` region, as discovered by an end scanner.
///
/// `body_end` is the byte offset of the closing marker, `full_end` is the
/// exclusive end of that marker, and `close_marker` is the exact source text
/// substituted back by [`render`]. All three fields must describe a span in
/// the original source so rendering is byte-for-byte idempotent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionEnd {
    pub body_end: usize,
    pub full_end: usize,
    pub close_marker: String,
}

/// End scanner contract from project.md §3.2. Packs select a built-in scanner
/// by data; no pack code is ever loaded or executed.
pub trait EndScanner: Send + Sync {
    /// Return the closing marker span for a block beginning at `body_start`,
    /// or `None` if the region is unterminated.
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd>;

    /// Compatibility shim for the Phase 1 contract: only the body-end offset.
    fn find_end(&self, text: &str, body_start: usize) -> Option<usize> {
        self.find_region_end(text, body_start).map(|end| end.body_end)
    }
}

/// Allow registered scanners (held as `Arc<dyn EndScanner>`) to be used
/// wherever an `EndScanner` value is expected.
impl<T: EndScanner + ?Sized> EndScanner for std::sync::Arc<T> {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        self.as_ref().find_region_end(text, body_start)
    }
}

pub use crate::scanners::brace::BalancedBraceScanner;

fn take_id(b: &[u8], mut i: usize) -> (String, usize) {
    let start = i;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-') {
        i += 1;
    }
    (String::from_utf8_lossy(&b[start..i]).into_owned(), i)
}

fn skip_spaces(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    i
}

struct BlockHead {
    lang: String,
    id: Option<String>,
    hash: usize,
    body_start: usize,
    close: String,
    kind: RegionKind,
}

/// Recognises `@lang {...}` / `@lang as id {...}` / `@lang#"... "#`.
fn block_head(source: &str, at: usize) -> Option<BlockHead> {
    let b = source.as_bytes();
    if at >= b.len() || b[at] != b'@' {
        return None;
    }
    let mut i = at + 1;
    let (lang, mut k) = take_id(b, i);
    if lang.is_empty() {
        return None;
    }
    i = k;
    let mut id = None;
    let after_space = skip_spaces(b, i);
    if source[after_space..].starts_with("as ") {
        let (name, next) = take_id(b, after_space + 3);
        if name.is_empty() {
            return None;
        }
        id = Some(name);
        i = skip_spaces(b, next);
    } else {
        i = after_space;
    }
    if i >= b.len() {
        return None;
    }
    if b[i] == b'#' {
        let hash_start = i;
        while i < b.len() && b[i] == b'#' {
            i += 1;
        }
        let hash = i - hash_start;
        if i >= b.len() || b[i] != b'"' {
            return None;
        }
        let close = format!("\"{}", "#".repeat(hash));
        return Some(BlockHead {
            lang,
            id,
            hash,
            body_start: i + 1,
            close,
            kind: RegionKind::Block,
        });
    }
    if b[i] == b'{' {
        return Some(BlockHead {
            lang,
            id,
            hash: 0,
            body_start: i + 1,
            close: "}".to_string(),
            kind: RegionKind::Block,
        });
    }
    k = i;
    let _ = k; // keep the compiler from warning about unused mut binding
    None
}

/// Deterministic language → built-in scanner map. Built-ins are shared via
/// `Arc`; pack metadata can override any entry (CLI/LSP build the map from the
/// discovered packs, then register their declared profiles).
#[derive(Clone, Default)]
pub struct ScannerRegistry {
    scanners: std::collections::HashMap<String, std::sync::Arc<dyn EndScanner>>,
}

impl ScannerRegistry {
    /// No language-specific scanners; every plain block uses the fallback.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Core built-ins for the bundled languages. A language not listed here
    /// falls back to [`BalancedBraceScanner`] in [`split`].
    pub fn default_registry() -> Self {
        let mut registry = Self::empty();
        let brace: std::sync::Arc<dyn EndScanner> =
            std::sync::Arc::new(BalancedBraceScanner);
        registry.register(
            [
                "go", "c", "java", "json", "html", "css", "markdown", "toml",
                "csharp", "scala", "zig", "dart", "solidity",
            ],
            brace,
        );
        registry
    }

    /// Register one built-in scanner under every pack id/alias. Repeated
    /// registrations overwrite earlier ones, so callers control precedence.
    pub fn register<I, S>(&mut self, names: I, scanner: std::sync::Arc<dyn EndScanner>)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for name in names {
            let name = name.as_ref();
            if !name.is_empty() {
                self.scanners.insert(name.to_string(), scanner.clone());
            }
        }
    }

    /// Look up a language's scanner. Returns `None` for unknown languages;
    /// [`split`] applies the balanced-brace fallback in that case.
    pub fn get(&self, lang: &str) -> Option<std::sync::Arc<dyn EndScanner>> {
        self.scanners.get(lang).cloned()
    }
}

/// Compatibility helper for callers still holding a plain Phase 1 map.
/// In-tree callers use [`ScannerRegistry`] directly.
pub fn default_scanners() -> std::collections::HashMap<String, Box<dyn EndScanner>> {
    ScannerRegistry::default_registry()
        .scanners
        .into_iter()
        .map(|(lang, scanner)| (lang, Box::new(scanner) as Box<dyn EndScanner>))
        .collect()
}

fn line_start(b: &[u8], i: usize) -> bool {
    i == 0 || b[i - 1] == b'\n'
}

fn next_marker(source: &str, from: usize) -> Option<usize> {
    let b = source.as_bytes();
    let mut i = from;
    // JUDGMENT CALL: scan on char boundaries. Offsets are bytes, but slicing a
    // str in the middle of a multibyte character would panic; `⟦`/`⟧` are
    // multibyte, so a byte-wise scan cannot detect them safely.
    while i < b.len() {
        if !source.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let c = source[i..].chars().next().expect("char boundary");
        if c == '@' && block_head(source, i).is_some() {
            return Some(i);
        }
        if c == '⟦' || c == '⟧' {
            return Some(i);
        }
        if c == '`' && line_start(b, i) && source[i..].starts_with("```") {
            return Some(i);
        }
        i += c.len_utf8();
    }
    None
}

fn detect_confidence(body: &str, has_header: bool) -> (String, f32) {
    if has_header {
        return (String::new(), 1.0);
    }
    if body.contains("print(") || body.contains("def ") || body.contains("import ") {
        ("python".to_string(), 0.82)
    } else {
        (String::new(), 0.60)
    }
}

/// Split a `.meld` source into language-tagged regions.
pub fn split(source: &str, scanners: &ScannerRegistry) -> Result<SplitDocument> {
    let b = source.as_bytes();
    let mut regions = Vec::new();
    let mut i = 0usize;
    let mut default_lang = String::new();
    let mut counter = 0usize;
    let default_scanner: std::sync::Arc<dyn EndScanner> =
        std::sync::Arc::new(BalancedBraceScanner);

    // Header.
    if source.starts_with("#meld ") {
        let end = source.find('\n').unwrap_or(source.len());
        let lang = source[6..end].trim().to_string();
        if lang.is_empty() {
            return Err(MeldError::Split {
                byte_start: 0,
                message: "empty #meld language".to_string(),
            });
        }
        default_lang = lang.clone();
        regions.push(Region {
            id: "header".to_string(),
            lang: lang.clone(),
            kind: RegionKind::Header,
            start: 6,
            end,
            full_start: 0,
            full_end: if end < source.len() { end + 1 } else { end },
            open_marker: "#meld ".to_string(),
            close_marker: if end < source.len() {
                "\n".to_string()
            } else {
                String::new()
            },
            confidence: 1.0,
        });
        i = if end < source.len() { end + 1 } else { end };
    }

    while i < b.len() {
        if !source.is_char_boundary(i) {
            i += 1;
            continue;
        }
        // Fence.
        if b[i] == b'`' && line_start(b, i) && source[i..].starts_with("```") {
            let mut k = i + 3;
            let (lang, next) = take_id(b, k);
            k = next;
            while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                k += 1;
            }
            if k >= b.len() || b[k] != b'\n' {
                return Err(MeldError::Split {
                    byte_start: i,
                    message: "malformed fence opener".to_string(),
                });
            }
            let body_start = k + 1;
            let close_marker = "\n```".to_string();
            let rel = source[body_start..]
                .find(&close_marker)
                .ok_or_else(|| MeldError::Split {
                    byte_start: i,
                    message: "unterminated fence".to_string(),
                })?;
            let body_end = body_start + rel;
            let mut full_end = body_end + close_marker.len();
            if full_end < b.len() && b[full_end] == b'\n' {
                full_end += 1;
            }
            let id = format!("f{}", counter);
            counter += 1;
            regions.push(Region {
                id,
                lang: if lang.is_empty() {
                    default_lang.clone()
                } else {
                    lang
                },
                kind: RegionKind::Fence,
                start: body_start,
                end: body_end,
                full_start: i,
                full_end,
                open_marker: source[i..body_start].to_string(),
                close_marker: source[body_end..full_end].to_string(),
                confidence: 1.0,
            });
            i = full_end;
            continue;
        }

        // Block.
        if let Some(head) = block_head(source, i) {
            let (body_end, full_end, close_marker) = if head.hash > 0 {
                let rel = source[head.body_start..].find(&head.close).ok_or_else(|| {
                    MeldError::Split {
                        byte_start: i,
                        message: "unterminated raw block".to_string(),
                    }
                })?;
                let end = head.body_start + rel;
                (end, end + head.close.len(), head.close.clone())
            } else {
                let scanner = scanners.get(&head.lang).unwrap_or_else(|| default_scanner.clone());
                let end = scanner
                    .find_region_end(source, head.body_start)
                    .ok_or_else(|| MeldError::Split {
                        byte_start: i,
                        message: "unterminated block".to_string(),
                    })?;
                (end.body_end, end.full_end, end.close_marker)
            };
            debug_assert_eq!(close_marker, source[body_end..full_end]);
            let open_marker = source[i..head.body_start].to_string();
            regions.push(Region {
                id: head.id.unwrap_or_else(|| format!("r{}", counter)),
                lang: head.lang,
                kind: head.kind,
                start: head.body_start,
                end: body_end,
                full_start: i,
                full_end,
                open_marker,
                close_marker,
                confidence: 1.0,
            });
            counter += 1;
            i = full_end;
            continue;
        }

        // Inline.
        if source[i..].starts_with('⟦') {
            let close = "⟧";
            let colon = source[i + 3..].find(':').ok_or_else(|| MeldError::Split {
                byte_start: i,
                message: "inline region missing ':'".to_string(),
            })? + i
                + 3;
            let lang = source[i + 3..colon].trim().to_string();
            let body_start = colon + 1;
            let rel = source[body_start..]
                .find(close)
                .ok_or_else(|| MeldError::Split {
                    byte_start: i,
                    message: "unterminated inline region".to_string(),
                })?;
            let body_end = body_start + rel;
            let full_end = body_end + close.len();
            regions.push(Region {
                id: format!("i{}", counter),
                lang: if lang.is_empty() {
                    default_lang.clone()
                } else {
                    lang
                },
                kind: RegionKind::Inline,
                start: body_start,
                end: body_end,
                full_start: i,
                full_end,
                open_marker: source[i..body_start].to_string(),
                close_marker: close.to_string(),
                confidence: 1.0,
            });
            counter += 1;
            i = full_end;
            continue;
        }

        // Untagged run up to the next marker.
        let end = next_marker(source, i).unwrap_or(b.len());
        let body = &source[i..end];
        let (detected, confidence) = detect_confidence(body, !default_lang.is_empty());
        let lang = if detected.is_empty() {
            if default_lang.is_empty() {
                "text".to_string()
            } else {
                default_lang.clone()
            }
        } else {
            detected
        };
        regions.push(Region {
            id: format!("u{}", counter),
            lang,
            kind: RegionKind::Untagged,
            start: i,
            end,
            full_start: i,
            full_end: end,
            open_marker: String::new(),
            close_marker: String::new(),
            confidence,
        });
        counter += 1;
        i = end;
    }

    Ok(SplitDocument {
        source: source.to_string(),
        default_lang,
        regions,
    })
}

/// Rebuild source from regions. Used by the idempotence fuzz test.
pub fn render(doc: &SplitDocument) -> String {
    let mut out = String::new();
    for r in &doc.regions {
        out.push_str(&r.open_marker);
        out.push_str(r.body(&doc.source));
        out.push_str(&r.close_marker);
    }
    out
}
