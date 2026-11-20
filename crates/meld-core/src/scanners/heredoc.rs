//! Heredoc scanner ported from `research/composition/prototype.py`.
//!
//! JUDGMENT CALL: this is the data-only, built-in Rust translation of the
//! prototype's Ruby slang stack. The opener is queued, activated at the
//! following newline, and its body is opaque: braces, quotes and comments
//! inside it are data. Multiple openers on one line are FIFO; a terminator
//! may be indented only for `<<-` / `<<~` (and PHP's flexible form).
use super::{host_end, is_id, line_end, Cursor, HostOptions};
use crate::splitter::{EndScanner, RegionEnd};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeredocDialect {
    Ruby,
    Bash,
    Php,
}

#[derive(Clone, Debug)]
struct Spec {
    terminator: Vec<u8>,
    indent_ok: bool,
    opener_end: usize,
}

#[derive(Default)]
struct State {
    pending: VecDeque<Spec>,
    active: VecDeque<Spec>,
}

/// Heredoc-aware block scanner. The fallback `}` balance stays in the host
/// grammar; only heredoc bodies are switched to an opaque grammar.
#[derive(Clone, Copy, Debug)]
pub struct HeredocScanner {
    dialect: HeredocDialect,
}

impl HeredocScanner {
    pub fn ruby() -> Self {
        Self {
            dialect: HeredocDialect::Ruby,
        }
    }

    pub fn bash() -> Self {
        Self {
            dialect: HeredocDialect::Bash,
        }
    }

    pub fn php() -> Self {
        Self {
            dialect: HeredocDialect::Php,
        }
    }

    pub fn with_dialect(dialect: HeredocDialect) -> Self {
        Self { dialect }
    }

    fn options(&self) -> HostOptions {
        match self.dialect {
            HeredocDialect::Ruby => HostOptions::script(),
            HeredocDialect::Bash => HostOptions {
                hash_comments: true,
                slash_comments: false,
                backticks: true,
                triple_quotes: false,
            },
            HeredocDialect::Php => HostOptions::script(),
        }
    }

    /// Recognise `<<ID`, `<<-ID`, `<<~ID`, `<<"ID"`, `<<'ID'` (Ruby/Bash) and
    /// `<<<ID` / `<<<'ID'` (PHP). The preceding byte must not be an identifier
    /// byte or `.`, so `flags<<mask` is not mistaken for an opener.
    fn opener(&self, b: &[u8], i: usize) -> Option<Spec> {
        let prefix: &[u8] = match self.dialect {
            HeredocDialect::Php => b"<<<",
            _ => b"<<",
        };
        if !b[i..].starts_with(prefix) {
            return None;
        }
        if i > 0 && (is_id(b[i - 1]) || b[i - 1] == b'.') {
            return None;
        }
        let mut j = i + prefix.len();
        let mut indent_ok = false;
        if j < b.len() && (b[j] == b'-' || b[j] == b'~') {
            indent_ok = true;
            j += 1;
        }
        // JUDGMENT CALL: PHP permits horizontal whitespace after `<<<`; Ruby
        // does not. Being lenient here cannot create an unterminated host
        // region, it just recognises one more opener shape.
        if self.dialect == HeredocDialect::Php {
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
        }
        let quote = if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
            let q = b[j];
            j += 1;
            Some(q)
        } else {
            None
        };
        let start = j;
        while j < b.len() && (is_id(b[j]) || (self.dialect == HeredocDialect::Php && b[j] == b'-')) {
            j += 1;
        }
        if j == start {
            return None;
        }
        // JUDGMENT CALL: a quoted delimiter still terminates on the plain
        // identifier; the quote only controls interpolation, which is opaque
        // to this scanner in every case. Capture the terminator *before*
        // consuming the closing quote.
        let terminator = b[start..j].to_vec();
        if let Some(q) = quote {
            if j >= b.len() || b[j] != q {
                return None;
            }
            j += 1;
        }
        Some(Spec {
            terminator,
            indent_ok,
            opener_end: j,
        })
    }
}

impl EndScanner for HeredocScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        let b = text.as_bytes();
        let mut state = State::default();
        host_end(b, body_start, self.options(), |cur: &mut Cursor<'_>| {
            if let Some(spec) = state.active.front() {
                let start = cur.i;
                let end = line_end(cur.b, start);
                let mut candidate = &cur.b[start..end];
                while candidate.last().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r')) {
                    candidate = &candidate[..candidate.len() - 1];
                }
                if spec.indent_ok {
                    while candidate.first().is_some_and(|c| matches!(c, b' ' | b'\t')) {
                        candidate = &candidate[1..];
                    }
                }
                if candidate == spec.terminator.as_slice() {
                    state.active.pop_front();
                }
                cur.i = if end < cur.b.len() { end + 1 } else { end };
                return true;
            }
            if let Some(spec) = self.opener(cur.b, cur.i) {
                state.pending.push_back(spec.clone());
                cur.i = spec.opener_end;
                return true;
            }
            // JUDGMENT CALL: activation happens at the newline after the
            // opener line, exactly as in prototype.py. All openers queued on
            // that line become active in FIFO order.
            if cur.b[cur.i] == b'\n' && !state.pending.is_empty() {
                state.active.extend(state.pending.drain(..));
                cur.i += 1;
                return true;
            }
            false
        })
    }
}
