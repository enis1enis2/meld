#![no_main]

use libfuzzer_sys::fuzz_target;
use meld_sandbox::{
    effective_policy, parse_grant, Access, Capability, ProtocolCapability, Result, SafeRoot,
    SandboxPolicy,
};
use std::path::Path;

/// A deliberately permissive authority layer. Fuzzing from deny-all cannot
/// detect a grant that widens authority, because there is nothing to widen.
fn permissive_baseline() -> Result<SandboxPolicy> {
    let mut policy = SandboxPolicy::deny_all("fuzz-base");
    policy.fs.add_root(SafeRoot::new("fuzz", Path::new("."))?)?;
    policy.fs.grant("fuzz", "**", Access::Read)?;
    policy.fs.grant("fuzz", "scratch/**", Access::Write)?;
    policy.fs.grant("fuzz", "bin/**", Access::Exec)?;
    for capability in [
        Capability::FsRead,
        Capability::FsWrite,
        Capability::FsExec,
        Capability::Network,
        Capability::Env,
    ] {
        policy.capabilities.allow(capability);
    }
    policy.network.protocols = ProtocolCapability::all();
    policy.network.hosts.insert("example.com".to_string());
    policy.env.insert("HOME".to_string());
    policy.limits.memory_bytes = Some(1024 * 1024 * 1024);
    policy.limits.cpu_time_ms = Some(60_000);
    policy.limits.processes = Some(64);
    policy.validate()?;
    Ok(policy)
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let Ok(grant) = parse_grant(&text) else {
        return;
    };

    // Applying an arbitrary grant to a permissive layer can only narrow
    // authority. Re-intersecting the result with the same layers must be
    // idempotent; drift here means a grant silently widened the policy.
    let Ok(base) = permissive_baseline() else {
        return;
    };
    if let Ok(granted) = base.apply_grant(&grant) {
        let _ = granted.validate().expect("grant result must validate");
        let again = effective_policy(&base, &base, &granted, Some(&grant))
            .expect("re-intersecting a valid policy must succeed");
        let _ = again.validate().expect("intersection must validate");
        assert_eq!(
            again.fingerprint(),
            granted.fingerprint(),
            "intersection drift after grant: {text}"
        );
        assert!(
            again.limits.memory_bytes.unwrap_or(u64::MAX)
                <= base.limits.memory_bytes.unwrap_or(u64::MAX),
            "grant widened the memory limit: {text}"
        );
    }
});
