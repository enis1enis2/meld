//! Built-in region-end scanners (Phase 6 GO decision).
//!
//! JUDGMENT CALL: packs declare a scanner *profile* in `region_end.toml`; the
//! algorithms live here, in the Rust host. Pack `region_end.py/js/lua/sh/wat`
//! files are inert data and are never loaded or executed. Raw hash delimiters
//! remain the normative escape hatch for bodies these scanners cannot cover.
pub mod brace;
pub mod counted;
pub mod delimited;
pub mod heredoc;
pub mod indent;
pub mod template;

use crate::splitter::RegionEnd;

/// Lexical features shared by the "host" (Meld block) half of each scanner.
#[derive(Clone, Copy)]
pub(crate) struct HostOptions {
    pub hash_comments: bool,
    pub slash_comments: bool,
    pub backticks: bool,
    pub triple_quotes: bool,
}

impl HostOptions {
    /// Exact Phase 1 fallback: C quotes/comments plus the original
    /// whitespace-prefixed `#` comment rule (including backtick strings).
    pub(crate) const fn c_like() -> Self {
        Self {
            hash_comments: true,
            slash_comments: true,
            backticks: true,
            triple_quotes: false,
        }
    }

    pub(crate) const fn script() -> Self {
        Self {
            hash_comments: true,
            slash_comments: true,
            backticks: true,
            triple_quotes: false,
        }
    }
}

/// The mutable state of one host-language scan. A dialect hook may consume
/// bytes (returning `true`) to impose embedded lexical rules before the
/// generic quote/comment/brace rules run.
pub(crate) struct Cursor<'a> {
    pub b: &'a [u8],
    pub i: usize,
    pub depth: usize,
}

impl Cursor<'_> {
    pub(crate) fn brace_end(&self, at: usize) -> Option<RegionEnd> {
        Some(RegionEnd {
            body_end: at,
            full_end: at + 1,
            close_marker: "}".to_string(),
        })
    }
}

pub(crate) fn is_id(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub(crate) fn is_ws(c: u8) -> bool {
    c.is_ascii_whitespace()
}

pub(crate) fn line_end(b: &[u8], i: usize) -> usize {
    b[i..].iter().position(|c| *c == b'\n').map_or(b.len(), |p| i + p)
}

/// Consume a quoted run starting at the opening quote, honouring backslash
/// escapes. JUDGMENT CALL: no interpolation is interpreted here; scanners that
/// need it (Ruby heredocs, JS templates) install their own dialect hook.
pub(crate) fn skip_quoted(b: &[u8], i: usize, q: u8) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        if b[j] == b'\\' {
            j = (j + 2).min(b.len());
            continue;
        }
        if b[j] == q {
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

pub(crate) fn skip_triple(b: &[u8], i: usize, q: u8) -> usize {
    let mut j = i + 3;
    while j + 2 < b.len() {
        if b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q && b[j + 1] == q && b[j + 2] == q {
            return j + 3;
        }
        j += 1;
    }
    b.len()
}

pub(crate) fn skip_line(b: &[u8], i: usize) -> usize {
    let end = line_end(b, i);
    if end < b.len() { end + 1 } else { end }
}

pub(crate) fn skip_block_comment(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j + 1 < b.len() {
        if b[j] == b'*' && b[j + 1] == b'/' {
            return j + 2;
        }
        j += 1;
    }
    b.len()
}

/// Host-language scanner over UTF-8 bytes. Only ASCII marker bytes are
/// inspected, so every returned offset is a byte offset and no slice crosses a
/// multibyte character. `hook` runs first, letting a dialect own embedded
/// constructs (heredocs, raw strings, templates, block scalars).
pub(crate) fn host_end<F>(b: &[u8], start: usize, opts: HostOptions, mut hook: F) -> Option<RegionEnd>
where
    F: FnMut(&mut Cursor<'_>) -> bool,
{
    let mut cur = Cursor {
        b,
        i: start,
        depth: 1,
    };
    while cur.i < b.len() {
        if hook(&mut cur) {
            continue;
        }
        let i = cur.i;
        let c = b[i];
        if opts.triple_quotes
            && (c == b'"' || c == b'\'')
            && i + 2 < b.len()
            && b[i + 1] == c
            && b[i + 2] == c
        {
            cur.i = skip_triple(b, i, c);
            continue;
        }
        if c == b'"' || c == b'\'' || (opts.backticks && c == b'`') {
            cur.i = skip_quoted(b, i, c);
            continue;
        }
        if opts.slash_comments && c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            cur.i = skip_line(b, i);
            continue;
        }
        if opts.slash_comments && c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            cur.i = skip_block_comment(b, i);
            continue;
        }
        if opts.hash_comments && c == b'#' && (i == 0 || is_ws(b[i - 1])) {
            cur.i = skip_line(b, i);
            continue;
        }
        match c {
            b'{' => {
                cur.depth += 1;
                cur.i += 1;
            }
            b'}' => {
                cur.depth -= 1;
                if cur.depth == 0 {
                    return cur.brace_end(i);
                }
                cur.i += 1;
            }
            _ => cur.i += 1,
        }
    }
    None
}
