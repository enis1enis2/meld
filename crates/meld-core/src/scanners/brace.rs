//! Generic balanced-brace fallback (Phase 1 behaviour, unchanged semantics).
use super::{host_end, HostOptions};
use crate::splitter::RegionEnd;

/// Skip quoted strings, escapes, line/block comments and balance braces.
/// This is the strict fallback: it knows no embedded language rules.
#[derive(Clone, Copy, Debug, Default)]
pub struct BalancedBraceScanner;

impl crate::splitter::EndScanner for BalancedBraceScanner {
    fn find_region_end(&self, text: &str, body_start: usize) -> Option<RegionEnd> {
        host_end(text.as_bytes(), body_start, HostOptions::c_like(), |_| {
            false
        })
    }
}
