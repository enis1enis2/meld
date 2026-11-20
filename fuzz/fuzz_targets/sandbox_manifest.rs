#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_sandbox::{parse_pack, parse_runner};
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    // Manifest parsers accept UTF-8 text only; lossy conversion keeps the fuzz
    // input focused on table/key/value handling rather than UTF-8 decoding.
    let text = String::from_utf8_lossy(data);
    let base = Path::new(".");

    for policy in [parse_pack(&text, base), parse_runner(&text, base)] {
        if let Ok(policy) = policy {
            // A successfully parsed policy must be internally consistent and
            // have a stable audit fingerprint.
            let _ = policy.fingerprint();
            let _ = policy.validate().expect("parsed policy must validate");
        }
    }
});
