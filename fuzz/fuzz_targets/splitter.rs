#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_core::splitter;

fuzz_target!(|data: &[u8]| {
    // The splitter operates on UTF-8 source text. InvalidUTF-8 is normalized
    // here so the target exercises region parsing rather than string decoding.
    let source = String::from_utf8_lossy(data);
    let scanners = splitter::ScannerRegistry::default_registry();

    if let Ok(doc) = splitter::split(&source, &scanners) {
        // A successful split must cover the source exactly once: rendering the
        // document is byte-for-byte idempotent (same invariant as the checked-in
        // deterministic splitter test, now driven by arbitrary input).
        assert_eq!(
            splitter::render(&doc),
            source.as_ref(),
            "split/render changed the source"
        );

        // Splitting the rendered source again must preserve region boundaries
        // and language tags.
        let again = splitter::split(&splitter::render(&doc), &scanners)
            .expect("rendered source must split again");
        assert_eq!(doc.regions.len(), again.regions.len(), "region count drift");
        for (first, second) in doc.regions.iter().zip(again.regions.iter()) {
            assert_eq!(first.lang, second.lang, "language drift");
            assert_eq!(first.kind, second.kind, "region kind drift");
            assert_eq!(
                first.body(&doc.source),
                second.body(&doc.source),
                "body drift"
            );
        }
    }
});
