#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_t2::protocol::{decode_response, ErrorObject, Notification, Request};

fuzz_target!(|data: &[u8]| {
    // Request/response envelopes are newline-delimited JSON. Parsing arbitrary
    // bytes must never panic, and any accepted envelope must survive a
    // serialize/deserialize round trip without changing its identity fields.
    let line = String::from_utf8_lossy(data);
    let line = line.lines().next().unwrap_or("");

    if let Ok(request) = serde_json::from_str::<Request>(line) {
        let encoded = serde_json::to_string(&request).expect("request serializes");
        let again: Request =
            serde_json::from_str(&encoded).expect("encoded request deserializes");
        assert_eq!(again.id, request.id, "request id drift");
        assert_eq!(again.method, request.method, "request method drift");
    }

    if let Ok(response) = decode_response(line) {
        let encoded = serde_json::to_string(&response).expect("response serializes");
        let again = decode_response(&encoded).expect("encoded response deserializes");
        assert_eq!(again.id, response.id, "response id drift");
        assert_eq!(
            again.result.is_some(),
            response.result.is_some(),
            "response result presence drift"
        );
        assert_eq!(
            again.error.is_some(),
            response.error.is_some(),
            "response error presence drift"
        );
    }

    if let Ok(notification) = serde_json::from_str::<Notification>(line) {
        let encoded = serde_json::to_string(&notification).expect("notification serializes");
        let again: Notification =
            serde_json::from_str(&encoded).expect("encoded notification deserializes");
        assert_eq!(again.method, notification.method, "notification method drift");
    }

    // ErrorObject is deliberately parsed from the whole byte slice rather than
    // `line`: an error object can be pretty-printed across multiple lines, and
    // this also exercises serde_json's raw-byte UTF-8 validation path. The
    // other three envelopes are NDJSON-framed, so they intentionally use only
    // the first line. Most inputs are rejected here; only panics are bugs.
    if let Ok(error) = serde_json::from_slice::<ErrorObject>(data) {
        let encoded = serde_json::to_string(&error).expect("error serializes");
        let again: ErrorObject =
            serde_json::from_str(&encoded).expect("encoded error deserializes");
        assert_eq!(again.code, error.code, "error code drift");
        assert_eq!(again.message, error.message, "error message drift");
    }
});
