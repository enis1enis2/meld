#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_value::{SourceStyle, Value};

fuzz_target!(|data: &[u8]| {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };

    if let Ok(value) = Value::from_json(&json) {
        // Decoding a Meld Value and re-encoding it must be stable: a second
        // decode/encode pass cannot change the canonical wire representation.
        let encoded = value.to_json();
        let decoded_again = Value::from_json(&encoded).expect("encoded value decodes");
        assert_eq!(decoded_again.to_json(), encoded, "wire encoding drifted");

        // Exercise source-literal rendering for every configured style. Some
        // tagged values are intentionally unrenderable; only panics are bugs.
        for style in [
            SourceStyle::Python,
            SourceStyle::JavaScript,
            SourceStyle::Json,
            SourceStyle::Lua,
        ] {
            let _ = value.to_source_literal(style);
        }
    }
});
