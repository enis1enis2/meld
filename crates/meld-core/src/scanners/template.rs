//! JavaScript/TypeScript template-literal scanner.
//!
//! JUDGMENT CALL: `${...}` interpolation is a real nested language island, so
//! a bare "backtick string is opaque" rule is not enough when an interpolation
//! contains another template, a quoted `}`, or a comment. This scanner keeps
//! an interpolation depth and returns to the template at its matching `}`.
use super::{host_end, skip_line, skip_quoted, Cursor, HostOptions};
use crate::splitter::{EndScanner, RegionEnd};

fn skip_interpolation(b: &[u8], i: usize) -> usize {
    let mut j = i;
    let mut depth = 1usize;
    while j < b.len() {
        let c = b[j];
        if c == b'"' || c == b'\'' {
            j = skip_quoted(b, j, c);
            continue;
        }
        if c == b'`' {
            j = skip_template(b, j);
            continue;
        }
        if c == b'/' && j + 1 < b.len() && b[j + 1] == b'/' {
            j = skip_line(b, j);
            continue;
        }
        if c == b'/' && j + 1 < b.len() && b[j + 1] == b'*' {
            if let Some(rel) = b[j + 2..].windows(2).position(|w| w == b"*/") {
                j += 4 + rel;
            } else {
                return b.len();
            }
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return j + 1;
            }
        }
        j += 1;
    }
    b.len()
}

fn skip_template(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j = (j + 2).min(b.len()),
            b'`' => return j + 1,
            b'$' if j + 1 < b.len() && b[j + 1] == b'{' => {
                j = skip_interpolation(b, j + 2);
            }
            _ => j += 1,
        }
    }
    b.len()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TemplateScanner;

impl EndScanner for TemplateScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        let options = HostOptions {
            hash_comments: false,
            slash_comments: true,
            backticks: false,
            triple_quotes: false,
        };
        host_end(
            text.as_bytes(),
            body_start,
            options,
            |cur: &mut Cursor<'_>| {
                if cur.b[cur.i] == b'`' {
                    cur.i = skip_template(cur.b, cur.i);
                    return true;
                }
                false
            },
        )
    }
}
