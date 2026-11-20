//! Delimiter-aware scanner: Lua long brackets and SQL dollar quotes.
use super::{host_end, is_id, line_end, Cursor, HostOptions};
use crate::splitter::{EndScanner, RegionEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelimitedDialect {
    Lua,
    Sql,
}

#[derive(Clone, Copy, Debug)]
pub struct DelimitedScanner {
    dialect: DelimitedDialect,
}

impl DelimitedScanner {
    pub fn lua() -> Self {
        Self {
            dialect: DelimitedDialect::Lua,
        }
    }

    pub fn sql() -> Self {
        Self {
            dialect: DelimitedDialect::Sql,
        }
    }

    pub fn with_dialect(dialect: DelimitedDialect) -> Self {
        Self { dialect }
    }

    fn options(&self) -> HostOptions {
        match self.dialect {
            DelimitedDialect::Lua => HostOptions {
                hash_comments: false,
                slash_comments: false,
                backticks: false,
                triple_quotes: false,
            },
            DelimitedDialect::Sql => HostOptions::c_like(),
        }
    }
}

/// Lua `[=[ ... ]=]`: the number of `=` bytes is the close delimiter.
/// JUDGMENT CALL 37: delimiter probes accept an index at the end of input,
/// because a Lua `--` comment may end exactly at EOF. Bounds-checking inside
/// the helper keeps every scan total instead of relying on each caller to
/// prove that an offset derived from a marker is still in range.
fn lua_long(b: &[u8], i: usize) -> Option<usize> {
    if i >= b.len() || b[i] != b'[' {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && b[j] == b'=' {
        j += 1;
    }
    if j >= b.len() || b[j] != b'[' {
        return None;
    }
    let eq = j - (i + 1);
    let mut needle = Vec::with_capacity(eq + 2);
    needle.push(b']');
    needle.extend(std::iter::repeat_n(b'=', eq));
    needle.push(b']');
    b[j + 1..]
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map(|p| j + 1 + p + needle.len())
}

/// SQL `$tag$ ... $tag$` (or `$$ ... $$`). JUDGMENT CALL: tags follow the
/// PostgreSQL identifier rule so a stray `$` in arithmetic is not a opener.
fn sql_dollar(b: &[u8], i: usize) -> Option<usize> {
    if i >= b.len() || b[i] != b'$' {
        return None;
    }
    let mut j = i + 1;
    if j < b.len() && is_id(b[j]) && !b[j].is_ascii_digit() {
        j += 1;
        while j < b.len() && (is_id(b[j]) || b[j] == b'_') {
            j += 1;
        }
    }
    if j >= b.len() || b[j] != b'$' {
        return None;
    }
    let delimiter = &b[i..=j];
    b[j + 1..]
        .windows(delimiter.len())
        .position(|w| w == delimiter)
        .map(|p| j + 1 + p + delimiter.len())
}

impl EndScanner for DelimitedScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        let b = text.as_bytes();
        host_end(b, body_start, self.options(), |cur: &mut Cursor<'_>| {
            let i = cur.i;
            match self.dialect {
                DelimitedDialect::Lua => {
                    if b[i..].starts_with(b"--") {
                        // A long comment `--[[ ... ]]` uses the same delimiter
                        // rule as a long string and may hide a brace.
                        if let Some(end) = lua_long(b, i + 2) {
                            cur.i = end;
                        } else {
                            let end = line_end(b, i);
                            cur.i = if end < b.len() { end + 1 } else { end };
                        }
                        return true;
                    }
                    if let Some(end) = lua_long(b, i) {
                        cur.i = end;
                        return true;
                    }
                }
                DelimitedDialect::Sql => {
                    if b[i..].starts_with(b"--") {
                        let end = line_end(b, i);
                        cur.i = if end < b.len() { end + 1 } else { end };
                        return true;
                    }
                    if let Some(end) = sql_dollar(b, i) {
                        cur.i = end;
                        return true;
                    }
                }
            }
            false
        })
    }
}
