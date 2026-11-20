//! Compose inline islands with their containing fragment (Phase 1/5 fix).
//!
//! The splitter keeps inline regions separate so each language can be parsed
//! and evaluated on its own. Runners, however, need a syntactically complete
//! program: `print(` and `)` are not valid Python jobs by themselves. This
//! module groups adjacent untagged fragments and inline holes into one
//! execution unit while leaving block/fence regions as ordinary jobs.
//!
//! **JUDGMENT CALL:** grouping is limited to untagged runs around inline
//! markers. A top-level inline island with no containing fragment uses the
//! file's default language as its anchor. Inline markers inside `{ ... }`
//! blocks are still literal text in Phase 1; the raw delimiter form is the
//! collision-free workaround and grammar-level composition is Phase 6 research.
use crate::binding::{Bindings, Export, Import};
use crate::error::Result;
use crate::scheduler::{strip_glue, Job};
use crate::splitter::{Region, RegionKind, SplitDocument};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct InlineExpr {
    pub id: String,
    pub lang: String,
    pub code: String,
}

#[derive(Clone, Debug)]
pub enum Segment {
    Source(String),
    Inline(InlineExpr),
}

#[derive(Clone, Debug)]
pub enum Unit {
    /// One ordinary region job.
    Job(Job),
    /// Several adjacent fragments plus inline holes that must be rendered
    /// before the containing language sees the source.
    Composite {
        anchor: Region,
        segments: Vec<Segment>,
        imports: Vec<Import>,
        exports: Vec<Export>,
    },
}

impl Unit {
    pub fn anchor(&self) -> &Region {
        match self {
            Unit::Job(job) => &job.region,
            Unit::Composite { anchor, .. } => anchor,
        }
    }
}

fn find<'a>(doc: &'a SplitDocument, id: &str) -> &'a Region {
    doc.regions
        .iter()
        .find(|r| r.id == id)
        .expect("execution plan references a known region")
}

fn synthetic_anchor(doc: &SplitDocument, inline: &Region) -> Region {
    Region {
        id: format!("c{}", inline.id),
        lang: if doc.default_lang.is_empty() {
            "text".to_string()
        } else {
            doc.default_lang.clone()
        },
        kind: RegionKind::Untagged,
        start: inline.full_start,
        end: inline.full_start,
        full_start: inline.full_start,
        full_end: inline.full_start,
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    }
}

fn glue_for(bindings: &Bindings, ids: &HashSet<String>) -> (Vec<Import>, Vec<Export>) {
    let imports = bindings
        .imports
        .iter()
        .filter(|i| ids.contains(&i.region_id))
        .cloned()
        .collect();
    let exports = bindings
        .exports
        .iter()
        .filter(|e| ids.contains(&e.region_id))
        .cloned()
        .collect();
    (imports, exports)
}

/// Turn an execution plan into runner units. Inline regions never appear as
/// standalone units; they are rendered into the composite that owns them.
pub fn build_units(doc: &SplitDocument, plan: &[String], bindings: &Bindings) -> Result<Vec<Unit>> {
    let mut units = Vec::new();
    let mut i = 0usize;
    while i < plan.len() {
        let current = find(doc, &plan[i]);
        if current.kind == RegionKind::Inline {
            let anchor = synthetic_anchor(doc, current);
            let (segments, ids, next) = collect_inline_group(doc, plan, i, Some(anchor.clone()));
            let (imports, exports) = glue_for(bindings, &ids);
            units.push(Unit::Composite {
                anchor,
                segments,
                imports,
                exports,
            });
            i = next;
            continue;
        }

        // JUDGMENT CALL: headers, blocks and fences are complete jobs. Only
        // untagged runs may be joined with inline holes; otherwise the #meld
        // header would accidentally become the anchor for the whole file.
        if current.kind != RegionKind::Untagged {
            let imports = bindings
                .imports
                .iter()
                .filter(|x| x.region_id == current.id)
                .cloned()
                .collect();
            let exports = bindings
                .exports
                .iter()
                .filter(|x| x.region_id == current.id)
                .cloned()
                .collect();
            units.push(Unit::Job(Job {
                region: current.clone(),
                code: strip_glue(current.body(&doc.source)),
                imports,
                exports,
            }));
            i += 1;
            continue;
        }

        let mut segments = vec![Segment::Source(strip_glue(current.body(&doc.source)))];
        let mut ids = HashSet::new();
        ids.insert(current.id.clone());
        let mut j = i + 1;
        while j < plan.len() {
            let next = find(doc, &plan[j]);
            if next.kind == RegionKind::Inline {
                segments.push(Segment::Inline(InlineExpr {
                    id: next.id.clone(),
                    lang: next.lang.clone(),
                    // JUDGMENT CALL: the delimiter whitespace around an inline
                    // expression is Meld syntax, not part of the expression.
                    // Python's `eval` rejects a leading indent, so trim here
                    // and keep worker `eval` methods expression-only.
                    code: next.body(&doc.source).trim().to_string(),
                }));
                ids.insert(next.id.clone());
                j += 1;
            } else if next.kind == RegionKind::Untagged && next.lang == current.lang {
                segments.push(Segment::Source(strip_glue(next.body(&doc.source))));
                ids.insert(next.id.clone());
                j += 1;
            } else {
                break;
            }
        }
        if segments.iter().any(|s| matches!(s, Segment::Inline(_))) {
            let (imports, exports) = glue_for(bindings, &ids);
            units.push(Unit::Composite {
                anchor: current.clone(),
                segments,
                imports,
                exports,
            });
            i = j;
            continue;
        }

        let imports = bindings
            .imports
            .iter()
            .filter(|x| x.region_id == current.id)
            .cloned()
            .collect();
        let exports = bindings
            .exports
            .iter()
            .filter(|x| x.region_id == current.id)
            .cloned()
            .collect();
        units.push(Unit::Job(Job {
            region: current.clone(),
            code: strip_glue(current.body(&doc.source)),
            imports,
            exports,
        }));
        i += 1;
    }
    Ok(units)
}

fn collect_inline_group(
    doc: &SplitDocument,
    plan: &[String],
    start: usize,
    anchor: Option<Region>,
) -> (Vec<Segment>, HashSet<String>, usize) {
    let mut segments = Vec::new();
    let mut ids = HashSet::new();
    if let Some(a) = &anchor {
        ids.insert(a.id.clone());
    }
    let mut j = start;
    while j < plan.len() {
        let r = find(doc, &plan[j]);
        if r.kind == RegionKind::Inline {
            segments.push(Segment::Inline(InlineExpr {
                id: r.id.clone(),
                lang: r.lang.clone(),
                code: r.body(&doc.source).trim().to_string(),
            }));
            ids.insert(r.id.clone());
            j += 1;
        } else if r.kind == RegionKind::Untagged
            && anchor.as_ref().is_some_and(|a| a.lang == r.lang)
        {
            segments.push(Segment::Source(strip_glue(r.body(&doc.source))));
            ids.insert(r.id.clone());
            j += 1;
        } else {
            break;
        }
    }
    (segments, ids, j)
}

/// Render a composite unit's inline holes. The callback evaluates an inline
/// expression and returns source text in the containing language.
pub fn render_segments<F, E>(segments: &[Segment], mut render: F) -> std::result::Result<String, E>
where
    F: FnMut(&InlineExpr) -> std::result::Result<String, E>,
{
    let mut out = String::new();
    for segment in segments {
        match segment {
            Segment::Source(text) => out.push_str(text),
            Segment::Inline(expr) => out.push_str(&render(expr)?),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding;
    use crate::splitter::{default_scanners, split};

    #[test]
    fn stitches_adjacent_fragments_around_an_inline_island() {
        let src = "#meld python\nprint(⟦python: 2 + 3⟧)\n";
        let doc = split(src, &default_scanners()).unwrap();
        let bindings = binding::resolve(&doc).unwrap();
        let plan = binding::execution_plan(&doc, &bindings).unwrap();
        let units = build_units(&doc, &plan, &bindings).unwrap();
        assert_eq!(units.len(), 2); // header + one composite
        let Unit::Composite { segments, .. } = &units[1] else {
            panic!("expected composite");
        };
        let code = render_segments(segments, |expr| {
            assert_eq!(expr.code.trim(), "2 + 3");
            Ok::<String, crate::error::MeldError>("5".to_string())
        })
        .unwrap();
        assert!(code.contains("print(5)"));
        assert!(!code.contains('⟦'));
    }
}
