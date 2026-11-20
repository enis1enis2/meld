//! Byte/line source maps between `.meld` and region-local code.
use crate::splitter::{Region, SplitDocument};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct SourceSpan {
    pub region_id: String,
    pub lang: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line: usize,
    pub col: usize,
}

/// Maps a region-local byte offset to a `.meld` byte offset and line/col.
pub struct SourceMap<'a> {
    doc: &'a SplitDocument,
}

impl<'a> SourceMap<'a> {
    pub fn new(doc: &'a SplitDocument) -> Self {
        Self { doc }
    }

    pub fn region(&self, id: &str) -> Option<&Region> {
        self.doc.regions.iter().find(|r| r.id == id)
    }

    pub fn local_to_global(&self, region_id: &str, local: usize) -> Option<usize> {
        let r = self.region(region_id)?;
        if local <= r.end - r.start {
            Some(r.start + local)
        } else {
            None
        }
    }

    pub fn global_to_local(&self, global: usize) -> Option<(&Region, usize)> {
        self.doc
            .regions
            .iter()
            .find(|r| global >= r.full_start && global < r.full_end)
            .map(|r| (r, global.saturating_sub(r.start)))
    }

    pub fn point_at(&self, byte: usize) -> Point {
        let before = &self.doc.source[..byte.min(self.doc.source.len())];
        let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
        let col = before
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count() + 1)
            .unwrap_or(1);
        Point { line, col }
    }

    pub fn span(&self, region_id: &str, local_start: usize, local_end: usize) -> Option<SourceSpan> {
        let r = self.region(region_id)?;
        let global = self.local_to_global(region_id, local_start)?;
        let p = self.point_at(global);
        Some(SourceSpan {
            region_id: r.id.clone(),
            lang: r.lang.clone(),
            byte_start: global,
            byte_end: global + (local_end - local_start),
            line: p.line,
            col: p.col,
        })
    }
}
