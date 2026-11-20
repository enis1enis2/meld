//! Delimiters whose close is *counted* from their opener: Rust raw strings,
//! C++ raw strings, R raw strings, and nested comments in OCaml/Haskell.
use super::{host_end, is_id, line_end, Cursor, HostOptions};
use crate::splitter::{EndScanner, RegionEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CountedDialect {
    Rust,
    Cpp,
    R,
    Ocaml,
    Haskell,
}

#[derive(Clone, Copy, Debug)]
pub struct CountedScanner {
    dialect: CountedDialect,
}

impl CountedScanner {
    pub fn rust() -> Self {
        Self {
            dialect: CountedDialect::Rust,
        }
    }

    pub fn cpp() -> Self {
        Self {
            dialect: CountedDialect::Cpp,
        }
    }

    pub fn r() -> Self {
        Self {
            dialect: CountedDialect::R,
        }
    }

    pub fn ocaml() -> Self {
        Self {
            dialect: CountedDialect::Ocaml,
        }
    }

    pub fn haskell() -> Self {
        Self {
            dialect: CountedDialect::Haskell,
        }
    }

    pub fn with_dialect(dialect: CountedDialect) -> Self {
        Self { dialect }
    }

    fn options(&self) -> HostOptions {
        match self.dialect {
            CountedDialect::R => HostOptions {
                hash_comments: true,
                slash_comments: false,
                backticks: false,
                triple_quotes: false,
            },
            CountedDialect::Ocaml | CountedDialect::Haskell => HostOptions {
                hash_comments: false,
                slash_comments: false,
                backticks: false,
                triple_quotes: false,
            },
            CountedDialect::Rust | CountedDialect::Cpp => HostOptions::c_like(),
        }
    }
}

/// Rust `r#"..."#` / `br##"..."##`. Returns the byte after the close.
fn rust_raw(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if b[j..].starts_with(b"br") || b[j..].starts_with(b"rb") {
        j += 2;
    } else if b[j] == b'r' {
        j += 1;
    } else {
        return None;
    }
    let hashes_at = j;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    if j >= b.len() || b[j] != b'"' {
        return None;
    }
    let hashes = j - hashes_at;
    let needle: Vec<u8> = [b'"']
        .into_iter()
        .chain(std::iter::repeat_n(b'#', hashes))
        .collect();
    j += 1;
    b[j..]
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|p| j + p + needle.len())
}

/// C++ `R"tag(...)tag"`; `tag` is at most 16 non-space, non-`()` bytes.
fn cpp_raw(b: &[u8], i: usize) -> Option<usize> {
    if !b[i..].starts_with(b"R\"") {
        return None;
    }
    let start = i + 2;
    let open = b[start..].iter().position(|c| *c == b'(')?;
    if open == 0 || open > 16 {
        return None;
    }
    let tag = &b[start..start + open];
    if tag
        .iter()
        .any(|c| c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'\\'))
    {
        return None;
    }
    let body = start + open + 1;
    let mut needle = Vec::with_capacity(tag.len() + 2);
    needle.push(b')');
    needle.extend_from_slice(tag);
    needle.push(b'"');
    b[body..]
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|p| body + p + needle.len())
}

/// R `r"---(...)---"`; the dash run is the counted delimiter.
fn r_raw(b: &[u8], i: usize) -> Option<usize> {
    if !b[i..].starts_with(b"r\"") && !b[i..].starts_with(b"R\"") {
        return None;
    }
    let mut j = i + 2;
    let dashes_at = j;
    while j < b.len() && b[j] == b'-' {
        j += 1;
    }
    if j >= b.len() || b[j] != b'(' {
        return None;
    }
    let dashes = j - dashes_at;
    let body = j + 1;
    let mut needle = Vec::with_capacity(dashes + 2);
    needle.push(b')');
    needle.extend(std::iter::repeat_n(b'-', dashes));
    needle.push(b'"');
    b[body..]
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|p| body + p + needle.len())
}

fn nested_comment(b: &[u8], i: usize, open: &[u8], close: &[u8]) -> usize {
    let mut j = i;
    let mut depth = 0usize;
    while j < b.len() {
        if b[j..].starts_with(open) {
            depth += 1;
            j += open.len();
        } else if b[j..].starts_with(close) {
            depth -= 1;
            j += close.len();
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    b.len()
}

impl EndScanner for CountedScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        let b = text.as_bytes();
        host_end(b, body_start, self.options(), |cur: &mut Cursor<'_>| {
            let i = cur.i;
            match self.dialect {
                CountedDialect::Rust => {
                    // Rust lifetimes (`'a`) are not quoted strings. JUDGMENT
                    // CALL: skip an identifier after `'` unless it is closed
                    // on the same token, which is a char literal.
                    if b[i] == b'\'' {
                        let mut j = i + 1;
                        while j < b.len() && is_id(b[j]) {
                            j += 1;
                        }
                        if j > i + 1 && (j >= b.len() || b[j] != b'\'') {
                            cur.i = j;
                            return true;
                        }
                    }
                    if let Some(end) = rust_raw(b, i) {
                        cur.i = end;
                        return true;
                    }
                    if b[i..].starts_with(b"/*") {
                        cur.i = nested_comment(b, i, b"/*", b"*/");
                        return true;
                    }
                }
                CountedDialect::Cpp => {
                    if let Some(end) = cpp_raw(b, i) {
                        cur.i = end;
                        return true;
                    }
                }
                CountedDialect::R => {
                    if let Some(end) = r_raw(b, i) {
                        cur.i = end;
                        return true;
                    }
                }
                CountedDialect::Ocaml => {
                    if b[i..].starts_with(b"(*") {
                        cur.i = nested_comment(b, i, b"(*", b"*)");
                        return true;
                    }
                }
                CountedDialect::Haskell => {
                    if b[i..].starts_with(b"{-") {
                        cur.i = nested_comment(b, i, b"{-", b"-}");
                        return true;
                    }
                    if b[i..].starts_with(b"--") {
                        let end = line_end(b, i);
                        cur.i = if end < b.len() { end + 1 } else { end };
                        return true;
                    }
                }
            }
            false
        })
    }
}
