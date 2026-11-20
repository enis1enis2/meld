#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_sandbox::{effective_policy, parse_grant, SandboxPolicy};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let Ok(grant) = parse_grant(&text) else {
        return;
    };

    // Applying a grant to a deny-all baseline can only narrow authority. The
    // resulting policy must remain valid and must not become more permissive
    // when intersected with the same layer again.
    let base = SandboxPolicy::deny_all("fuzz-base");
    if let Ok(granted) = base.apply_grant(&grant) {
        let _ = granted.validate().expect("grant result must validate");
        let again = effective_policy(&base, &base, &granted, Some(&grant))
            .expect("re-intersecting a valid policy must succeed");
        let _ = again.validate().expect("intersection must validate");
        assert_eq!(
            again.fingerprint(),
            granted.fingerprint(),
            "intersection drift after grant"
        );
    }
});
