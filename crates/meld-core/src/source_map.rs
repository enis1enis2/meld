//! Byte/line source maps between `.meld` and region-local code.
//!
//! Coordinate convention: region-local offsets are relative to the region
//! *body* (`Region::start..Region::end`). Marker bytes in the full span
//! (`Region::full_start..Region::start` and `Region::end..Region::full_end`)
//! are addressable through [`SourceMap::global_to_local`], but they clamp to
//! the nearest body edge because they have no distinct local coordinate.
//! [`SourceMap::local_to_global`] and [`SourceMap::span`] never expose marker
//! bytes. All arithmetic uses checked/saturating operations so malformed or
//! reversed spans fail closed instead of underflowing.
use crate::splitter::{Region, SplitDocument};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// Map a body-relative offset to the whole-document byte offset.
    ///
    /// Returns `None` for offsets outside `0..=body_len` and for malformed
    /// regions whose body end precedes its body start.
    pub fn local_to_global(&self, region_id: &str, local: usize) -> Option<usize> {
        let r = self.region(region_id)?;
        let body_len = r.end.checked_sub(r.start)?;
        if local > body_len {
            return None;
        }
        r.start.checked_add(local)
    }

    /// Map a whole-document byte offset to its containing region and a
    /// body-relative offset. Marker bytes clamp to `0` (opening marker) or
    /// `body_len` (closing marker), keeping both directions consistent.
    pub fn global_to_local(&self, global: usize) -> Option<(&Region, usize)> {
        self.doc
            .regions
            .iter()
            .find(|r| global >= r.full_start && global < r.full_end)
            .map(|r| {
                let body_len = r.end.saturating_sub(r.start);
                let local = global.saturating_sub(r.start).min(body_len);
                (r, local)
            })
    }

    pub fn point_at(&self, byte: usize) -> Point {
        let mut byte = byte.min(self.doc.source.len());
        while byte > 0 && !self.doc.source.is_char_boundary(byte) {
            byte -= 1;
        }
        let before = &self.doc.source[..byte];
        let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
        let col = before
            .rsplit('\n')
            .next()
            .map(|s| s.chars().count() + 1)
            .unwrap_or(1);
        Point { line, col }
    }

    pub fn span(
        &self,
        region_id: &str,
        local_start: usize,
        local_end: usize,
    ) -> Option<SourceSpan> {
        if local_end < local_start {
            return None;
        }
        let r = self.region(region_id)?;
        let global = self.local_to_global(region_id, local_start)?;
        let len = local_end - local_start;
        let global_end = global.checked_add(len)?;
        if global_end > r.end || global_end > self.doc.source.len() {
            return None;
        }
        let p = self.point_at(global);
        Some(SourceSpan {
            region_id: r.id.clone(),
            lang: r.lang.clone(),
            byte_start: global,
            byte_end: global_end,
            line: p.line,
            col: p.col,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splitter::{split, RegionKind, ScannerRegistry};

    fn sample_document() -> SplitDocument {
        let source = "#meld python\n@python as demo { x = 1 }\n⟦python: x + 1⟧\n";
        split(source, &ScannerRegistry::default_registry()).expect("sample source splits")
    }

    fn block_region(doc: &SplitDocument) -> &Region {
        doc.regions
            .iter()
            .find(|r| r.id == "demo")
            .expect("named block region")
    }

    #[test]
    fn body_offsets_round_trip_exactly() {
        let doc = sample_document();
        let map = SourceMap::new(&doc);
        let block = block_region(&doc);
        let body_len = block.end - block.start;

        for local in 0..=body_len {
            let global = map
                .local_to_global(&block.id, local)
                .unwrap_or_else(|| panic!("local {local} should map"));
            assert_eq!(global, block.start + local);
            let (back, local_back) = map
                .global_to_local(global)
                .unwrap_or_else(|| panic!("global {global} should map back"));
            assert_eq!(back.id, block.id);
            assert_eq!(local_back, local);
        }
    }

    #[test]
    fn marker_bytes_clamp_to_the_nearest_body_edge() {
        let doc = sample_document();
        let map = SourceMap::new(&doc);
        let block = block_region(&doc);
        let body_len = block.end - block.start;

        for global in block.full_start..block.start {
            let (region, local) = map
                .global_to_local(global)
                .unwrap_or_else(|| panic!("opening marker byte {global} should map"));
            assert_eq!(region.id, block.id);
            assert_eq!(local, 0);
            assert_eq!(map.local_to_global(&block.id, local), Some(block.start));
        }

        for global in block.end..block.full_end {
            let (region, local) = map
                .global_to_local(global)
                .unwrap_or_else(|| panic!("closing marker byte {global} should map"));
            assert_eq!(region.id, block.id);
            assert_eq!(local, body_len);
            assert_eq!(map.local_to_global(&block.id, local), Some(block.end));
        }
    }

    #[test]
    fn inline_regions_use_the_same_body_relative_convention() {
        let doc = sample_document();
        let map = SourceMap::new(&doc);
        let inline = doc
            .regions
            .iter()
            .find(|r| r.kind == RegionKind::Inline)
            .expect("inline region");
        let body_len = inline.end - inline.start;

        assert_eq!(inline.close_marker, "⟧");
        assert_eq!(map.local_to_global(&inline.id, 0), Some(inline.start));
        assert_eq!(map.local_to_global(&inline.id, body_len), Some(inline.end));
        assert_eq!(map.local_to_global(&inline.id, body_len + 1), None);
        assert_eq!(
            map.global_to_local(inline.full_start)
                .map(|(r, local)| (r.id.as_str(), local)),
            Some((inline.id.as_str(), 0))
        );
        assert_eq!(
            map.global_to_local(inline.end)
                .map(|(r, local)| (r.id.as_str(), local)),
            Some((inline.id.as_str(), body_len))
        );
    }

    #[test]
    fn spans_reject_reversed_and_out_of_body_ranges() {
        let doc = sample_document();
        let map = SourceMap::new(&doc);
        let block = block_region(&doc);
        let body_len = block.end - block.start;

        assert_eq!(map.span(&block.id, 2, 1), None);
        assert_eq!(map.span(&block.id, 0, body_len + 1), None);
        assert_eq!(map.span("missing", 0, 1), None);

        let span = map.span(&block.id, 0, body_len).expect("body span");
        assert_eq!(span.byte_start, block.start);
        assert_eq!(span.byte_end, block.end);
    }
}
