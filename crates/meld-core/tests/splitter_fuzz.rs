//! Splitter fuzz suite (project.md §8). Deterministic LCG so failures are
//! reproducible without an external property-testing dependency.
use meld_core::splitter::{self, BalancedBraceScanner, EndScanner, RegionKind, ScannerRegistry};
use std::sync::Arc;

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

fn scanners() -> ScannerRegistry {
    // Built-ins carry the Phase 6 profiles; the explicit registrations prove
    // the registry API is used by in-tree callers rather than a language map.
    let mut registry = ScannerRegistry::default_registry();
    let brace: Arc<dyn splitter::EndScanner> = Arc::new(BalancedBraceScanner);
    registry.register(["plain"], brace);
    registry
}

fn generate(seed: u64) -> String {
    let mut s = seed;
    let parts = [
        "#meld python\n",
        "@python { s = 'a } b'; print(s) }\n",
        "@python#\" body } inside raw \"#\n",
        "@javascript as j { const x = \"} not a close\"; console.log(x); }\n",
        "⟦python: 1 + 2⟧\n",
        "```python\nvalue = '``` inside a string'\n```\n",
        "plain text with an email a@b.com\n",
        "print('default tail')\n",
        "@ruby { html = <<~SQL\n  SELECT } FROM t\nSQL\nputs html }\n",
        "@python { s = \"\"\"a \" } b\"\"\"; print(s) }\n",
        "@javascript { const t = `a ${ `inner }` } b`; }\n",
    ];
    let mut out = String::new();
    let n = 4 + (next(&mut s) as usize % 6);
    for _ in 0..n {
        out.push_str(parts[next(&mut s) as usize % parts.len()]);
    }
    out
}

#[test]
fn deterministic_scanner_cases_keep_offsets_and_render() {
    let cases = [
        (
            "@ruby { html = <<~SQL\n  SELECT } FROM t\nSQL\nputs html }\n",
            "SELECT } FROM t",
        ),
        (
            "@ruby { a = <<A + <<B\n  } first\nA\n  } second\nB\nputs a }\n",
            "} second",
        ),
        (
            "@python { s = \"\"\"a \" } b\"\"\"; print(s) }\n",
            "a \" } b",
        ),
        (
            "@javascript { const t = `a ${ `inner }` } b`; }\n",
            "inner }",
        ),
        (
            "@rust { let s = r##\"a \"#} b\"##; /* /* } */ */ x }\n",
            "a \"#} b",
        ),
        (
            "@sql { SELECT $tag$ } $tag$ AS x; }\n",
            "$tag$ } $tag$",
        ),
    ];
    for (src, needle) in cases {
        let doc = splitter::split(src, &scanners()).unwrap_or_else(|e| panic!("{e}: {src:?}"));
        assert_eq!(splitter::render(&doc), src, "render changed {src:?}");
        let block = doc
            .regions
            .iter()
            .find(|r| r.kind == RegionKind::Block)
            .unwrap();
        assert_eq!(block.end, src.rfind('}').unwrap(), "wrong end: {src:?}");
        assert!(block.body(src).contains(needle), "missing data: {src:?}");
        let start = src.find('{').unwrap() + 1;
        let generic = BalancedBraceScanner.find_end(src, start);
        assert_ne!(
            generic,
            Some(block.end),
            "negative control passed: {src:?}"
        );
    }

    // Raw hash delimiter bypass: no scanner registration is needed.
    let raw = "@ruby#\" } literal \"#\n";
    let doc = splitter::split(raw, &ScannerRegistry::empty()).unwrap();
    assert_eq!(splitter::render(&doc), raw);
    assert!(doc.regions[0].body(raw).contains("} literal"));
}

#[test]
fn split_render_is_idempotent() {
    for seed in 0..1000u64 {
        let src = generate(seed);
        let doc = splitter::split(&src, &scanners()).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        let rendered = splitter::render(&doc);
        assert_eq!(rendered, src, "seed {seed} changed source");
        let again = splitter::split(&rendered, &scanners()).unwrap();
        assert_eq!(
            again.regions.len(),
            doc.regions.len(),
            "seed {seed} region count"
        );
        for (a, b) in doc.regions.iter().zip(again.regions.iter()) {
            assert_eq!(a.lang, b.lang, "seed {seed} lang drift");
            assert_eq!(a.kind, b.kind, "seed {seed} kind drift");
            assert_eq!(
                a.body(&rendered),
                b.body(&rendered),
                "seed {seed} body drift"
            );
        }
    }
}

#[test]
fn closing_marker_inside_string_does_not_end_region() {
    let src = "@python { s = 'a } b'; print(s) }\ntail\n";
    let doc = splitter::split(src, &scanners()).unwrap();
    let block = doc
        .regions
        .iter()
        .find(|r| r.kind == splitter::RegionKind::Block)
        .unwrap();
    assert!(block.body(src).contains("a } b"));
}
