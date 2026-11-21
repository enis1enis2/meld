//! Host-side escape-attempt tests. All inputs are inert strings; no shell is
//! invoked and no platform enforcement is assumed.

use meld_sandbox::{
    parse_grant, parse_pack, Access, BackendFeatures, BackendKind, BackendProbe, Capability,
    CapabilitySet, Grant, Result, SafeRoot, SandboxError, SandboxPolicy,
};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_base(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "meld-sandbox-escape-{label}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn fixture_lines() -> Vec<String> {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/host_path_validation.txt"),
    )
    .unwrap();
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_dotdot_fixture_is_rejected_by_safe_root() -> Result<()> {
    let base = temp_base("dotdot");
    let root = SafeRoot::new("project", &base)?;
    for value in fixture_lines() {
        if value.contains("..") || value.starts_with('/') {
            assert!(
                root.resolve(&value).is_err(),
                "path must be rejected: {value}"
            );
        }
    }
    fs::remove_dir_all(base).ok();
    Ok(())
}

#[test]
fn manifest_and_grant_reject_parent_components() {
    let base = temp_base("manifest");
    for line in fixture_lines() {
        if line.starts_with("../") {
            let manifest = format!("[sandbox]\nfs = [\"{line}\"]\n");
            assert!(
                parse_pack(&manifest, &base).is_err(),
                "manifest accepted {line}"
            );
        }
    }
    let grant = format!("grant(fs=[\"{}\"])", fixture_lines()[0]);
    assert!(matches!(
        parse_grant(&grant),
        Err(SandboxError::Escape { .. })
    ));
    fs::remove_dir_all(base).ok();
}

#[test]
fn grant_cannot_add_filesystem_or_memory() -> Result<()> {
    let base = temp_base("grant");
    let mut policy = SandboxPolicy::deny_all("pack");
    policy.fs.add_root(SafeRoot::new("project", &base)?)?;
    policy.fs.grant("project", "src/**", Access::Read)?;
    policy.capabilities = CapabilitySet::default();
    policy.capabilities.allow(Capability::FsRead);
    policy.limits.memory_bytes = Some(1024);

    let good = parse_grant("grant(fs=[\"project/src/**\"], mem=512)")?;
    let narrowed = policy.apply_grant(&good)?;
    assert_eq!(narrowed.limits.memory_bytes, Some(512));

    let wider = parse_grant("grant(fs=[\"project/**\"], mem=4096)")?;
    assert!(matches!(
        policy.apply_grant(&wider),
        Err(SandboxError::Denied(_))
    ));
    let no_permission = parse_grant("grant(fs=[\"project/other/**\"])")?;
    assert!(matches!(
        policy.apply_grant(&no_permission),
        Err(SandboxError::Denied(_))
    ));
    fs::remove_dir_all(base).ok();
    Ok(())
}

#[test]
fn unknown_capabilities_and_protocols_are_errors() -> Result<()> {
    assert!(parse_grant("grant(caps=[\"root\"])").is_err());
    assert!(parse_pack("[sandbox]\nnetwork = [\"raw-socket\"]\n", Path::new(".")).is_err());
    let mut policy = SandboxPolicy::deny_all("p");
    assert!(Grant::default().evaluate(&policy).is_ok());
    policy.network.protocols = meld_sandbox::ProtocolCapability::parse("tcp")?;
    assert!(policy.validate().is_err());
    Ok(())
}

#[test]
fn grant_preserves_exact_and_subtree_flags() -> Result<()> {
    let base = temp_base("grant-flags");
    let mut policy = SandboxPolicy::deny_all("pack");
    policy.fs.add_root(SafeRoot::new("project", &base)?)?;
    policy.fs.grant("project", "src/exact.txt", Access::Read)?;
    policy.fs.grant("project", "docs/**", Access::Read)?;
    policy.capabilities.allow(Capability::FsRead);

    let grant = parse_grant("grant(fs=[\"project/src/exact.txt\", \"project/docs/**\"])")?;
    let narrowed = policy.apply_grant(&grant)?;
    let exact = narrowed
        .fs
        .rules()
        .iter()
        .find(|rule| rule.path() == "src/exact.txt")
        .expect("exact rule preserved");
    let subtree = narrowed
        .fs
        .rules()
        .iter()
        .find(|rule| rule.path() == "docs")
        .expect("subtree rule preserved");
    assert!(!exact.is_subtree(), "exact grant widened to a subtree");
    assert!(subtree.is_subtree(), "subtree grant lost its flag");
    fs::remove_dir_all(base).ok();
    Ok(())
}

#[test]
fn hostname_allowlist_cannot_be_honoured_by_protocol_only_backends() -> Result<()> {
    let mut policy = SandboxPolicy::deny_all("net");
    policy.network.protocols = meld_sandbox::ProtocolCapability::parse("tcp")?;
    policy.capabilities.allow(Capability::Network);
    policy.network.hosts.insert("example.com".to_string());
    policy.validate()?;

    let probe = BackendProbe {
        kind: BackendKind::LinuxNamespaces,
        available: true,
        features: BackendFeatures {
            network_namespace: true,
            seccomp: true,
            process_group_kill: true,
            ..Default::default()
        },
        notes: Vec::new(),
    };
    assert!(matches!(
        probe.require(&policy),
        Err(SandboxError::Unsupported(_))
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_inside_root_is_a_hard_error() -> Result<()> {
    let base = temp_base("symlink");
    let outside = temp_base("symlink-outside");
    let link = base.join("link");
    std::os::unix::fs::symlink(&outside, &link)?;
    let root = SafeRoot::new("project", &base)?;
    assert!(matches!(
        root.resolve("link/secret"),
        Err(SandboxError::Escape { .. })
    ));
    fs::remove_dir_all(base).ok();
    fs::remove_dir_all(outside).ok();
    Ok(())
}
