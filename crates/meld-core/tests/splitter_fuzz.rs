//! Splitter fuzz suite (project.md §8). Deterministic LCG so failures are
//! reproducible without an external property-testing dependency.
use meld_core::splitter::{self, BalancedBraceScanner, ScannerRegistry};
use std::sync::Arc;

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

fn scanners() -> ScannerRegistry {
    let mut registry = ScannerRegistry::empty();
    let brace: Arc<dyn splitter::EndScanner> = Arc::new(BalancedBraceScanner);
    registry.register(["python", "javascript"], brace);
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
    ];
    let mut out = String::new();
    let n = 4 + (next(&mut s) as usize % 6);
    for _ in 0..n {
        out.push_str(parts[next(&mut s) as usize % parts.len()]);
    }
    out
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
