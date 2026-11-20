//! Indentation-sensitive block scanner: Python triple strings/dedents and
//! YAML block scalars. Meld's closing marker is still `}`; indentation only
//! decides which lines are opaque data.
use super::{host_end, line_end, Cursor, HostOptions};
use crate::splitter::{EndScanner, RegionEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentDialect {
    Python,
    Yaml,
}

#[derive(Clone, Copy, Debug)]
pub struct IndentScanner {
    dialect: IndentDialect,
}

impl IndentScanner {
    pub fn python() -> Self {
        Self {
            dialect: IndentDialect::Python,
        }
    }

    pub fn yaml() -> Self {
        Self {
            dialect: IndentDialect::Yaml,
        }
    }

    pub fn with_dialect(dialect: IndentDialect) -> Self {
        Self { dialect }
    }

    fn options(&self) -> HostOptions {
        match self.dialect {
            IndentDialect::Python => HostOptions {
                hash_comments: true,
                slash_comments: false,
                backticks: false,
                triple_quotes: true,
            },
            IndentDialect::Yaml => HostOptions {
                hash_comments: true,
                slash_comments: false,
                backticks: false,
                triple_quotes: false,
            },
        }
    }
}

fn indent_of(b: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }
    j - start
}

fn line_start(b: &[u8], i: usize) -> usize {
    b[..i]
        .iter()
        .rposition(|c| *c == b'\n')
        .map_or(0, |p| p + 1)
}

impl EndScanner for IndentScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        let b = text.as_bytes();
        // The dedent baseline is the first nonblank body line. A closing `}`
        // is only accepted when it is not deeper than that baseline, which
        // keeps nested Python dicts from looking like Meld region ends.
        let mut body_indent = None;
        let mut probe = body_start;
        while probe < b.len() {
            let end = line_end(b, probe);
            if end > probe && b[probe..end].iter().any(|c| !c.is_ascii_whitespace()) {
                body_indent = Some(indent_of(b, probe));
                break;
            }
            probe = if end < b.len() { end + 1 } else { end };
        }
        let mut block_scalar_indent: Option<usize> = None;
        host_end(b, body_start, self.options(), |cur: &mut Cursor<'_>| {
            if self.dialect == IndentDialect::Yaml {
                if let Some(parent) = block_scalar_indent {
                    let end = line_end(cur.b, cur.i);
                    let indent = indent_of(cur.b, cur.i);
                    if end == cur.i || indent > parent {
                        cur.i = if end < cur.b.len() { end + 1 } else { end };
                        return true;
                    }
                    block_scalar_indent = None;
                }
                // JUDGMENT CALL: a block scalar indicator is recognised when
                // `|`/`>` is the last non-indicator text on its line. The Meld
                // `{` may precede it on the first body line, so requiring a
                // column-zero indicator would miss the common `@yaml { key: |`
                // spelling.
                if matches!(cur.b[cur.i], b'|' | b'>') {
                    let end = line_end(cur.b, cur.i);
                    let rest = &cur.b[cur.i + 1..end];
                    if rest
                        .iter()
                        .all(|c| c.is_ascii_whitespace() || matches!(c, b'+' | b'-' | b'0'..=b'9'))
                    {
                        block_scalar_indent = Some(indent_of(cur.b, line_start(cur.b, cur.i)));
                        cur.i = if end < cur.b.len() { end + 1 } else { end };
                        return true;
                    }
                }
            }
            if self.dialect == IndentDialect::Python && cur.depth == 1 && cur.b[cur.i] == b'}' {
                if let Some(baseline) = body_indent {
                    let start = line_start(cur.b, cur.i);
                    if indent_of(cur.b, start) > baseline {
                        cur.i += 1;
                        return true;
                    }
                }
            }
            false
        })
    }
}
