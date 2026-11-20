//! Meld OS sandbox core: deny-by-default policy, manifest/grant parsing and
//! platform backends.
//!
//! # Design authority
//!
//! Effective permissions are the **intersection**
//! `pack maximum ∩ runner constraints ∩ file default grant ∩ region grant`.
//! Grants can only narrow. Missing `[sandbox]` means network denied and an
//! empty filesystem grant set. Unknown capabilities, unknown manifest keys and
//! `..`/symlink escapes are hard errors. A backend that cannot enforce a
//! declared policy must fail closed: it returns an error instead of spawning
//! an unconfined process.
//!
//! **JUDGMENT CALL:** each OS backend attests the mechanisms it uses. Linux
//! combines namespaces, seccomp, optional Landlock, cgroups, and an optional
//! probed OCI fallback; macOS uses Seatbelt and Windows uses Job Objects plus
//! AppContainer. A missing or unenforceable mechanism is an execution error,
//! not a weaker sandbox.
//!
//! The CLI is the integration point: it computes the effective policy and asks
//! the platform backend to enforce it before any T2 worker is spawned.

pub mod backend;
pub mod capability;
pub mod error;
pub mod grant;
pub mod io;
pub mod manifest;
pub mod policy;
pub mod process;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(feature = "oci")]
pub mod oci;
#[cfg(target_os = "windows")]
pub mod windows;

pub use backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
pub use capability::{
    Access, Capability, CapabilitySet, FsPolicy, FsRule, NetworkPolicy, ProtocolCapability,
    ResourceLimits, SafeRoot,
};
pub use error::{Result, SandboxError};
pub use grant::{parse_grant, Grant};
pub use io::{read_bounded_line, IoPolicy};
pub use manifest::{parse_manifest, parse_pack, parse_runner};
pub use policy::{EnforcementRequirements, Fingerprint, SandboxPolicy};
pub use process::{
    configure_process_group, kill_process_tree, CommandSpec, ProcessTreeGuard, SpawnedProcess,
};

#[cfg(target_os = "linux")]
pub use linux::LinuxBackend;
#[cfg(target_os = "macos")]
pub use macos::MacosBackend;
#[cfg(feature = "oci")]
pub use oci::{find_runc as find_oci_runtime, OciBackend};
#[cfg(target_os = "windows")]
pub use windows::WindowsBackend;

use std::path::Path;

/// Intersect all four authority layers. Callers pass deny-all policies for any
/// layer that did not declare a sandbox.
pub fn effective_policy(
    pack: &SandboxPolicy,
    runner: &SandboxPolicy,
    file_default: &SandboxPolicy,
    region_grant: Option<&Grant>,
) -> Result<SandboxPolicy> {
    let base =
        SandboxPolicy::intersection_of(&[pack.clone(), runner.clone(), file_default.clone()])?;
    match region_grant {
        Some(grant) => base.apply_grant(grant),
        None => Ok(base),
    }
}

/// Platform backend selected for the current target. On Linux this is the
/// namespace/seccomp backend; macOS uses Seatbelt and Windows a Job Object.
/// The OCI fallback is **never** selected automatically: enable it explicitly
/// when the platform backend cannot enforce the policy.
#[cfg(target_os = "linux")]
pub fn platform_backend() -> LinuxBackend {
    LinuxBackend::default()
}

#[cfg(target_os = "macos")]
pub fn platform_backend() -> MacosBackend {
    MacosBackend
}

#[cfg(target_os = "windows")]
pub fn platform_backend() -> WindowsBackend {
    WindowsBackend
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn unsupported_platform() -> SandboxError {
    SandboxError::Unsupported("no sandbox backend for this target".to_string())
}

/// Load the two manifest files from disk and intersect them with a deny-all
/// file default. Absolute paths and symlink escapes in `[sandbox.roots]` are
/// rejected by `SafeRoot`.
pub fn policy_from_files(
    pack_path: &Path,
    runner_path: &Path,
    base: &Path,
) -> Result<SandboxPolicy> {
    let pack_text = std::fs::read_to_string(pack_path)?;
    let runner_text = std::fs::read_to_string(runner_path)?;
    let pack = parse_pack(&pack_text, base)?;
    let runner = parse_runner(&runner_text, base)?;
    SandboxPolicy::intersection_of(&[pack, runner, SandboxPolicy::deny_all("file-default")])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_base(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("meld-sandbox-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn four_layer_intersection_never_widens() {
        let base = temp_base("layers");
        let mut pack = SandboxPolicy::deny_all("pack");
        pack.fs
            .add_root(SafeRoot::new("project", &base).unwrap())
            .unwrap();
        pack.fs.grant("project", "src/**", Access::Read).unwrap();
        pack.capabilities.allow(Capability::FsRead);
        pack.limits.memory_bytes = Some(128 * 1024 * 1024);

        let mut runner = SandboxPolicy::deny_all("runner");
        runner
            .fs
            .add_root(SafeRoot::new("project", &base).unwrap())
            .unwrap();
        runner.fs.grant("project", "src/**", Access::Read).unwrap();
        runner.capabilities.allow(Capability::FsRead);
        runner.limits.memory_bytes = Some(64 * 1024 * 1024);

        // The file default is an authority layer too: a file without a sandbox
        // declaration would contribute deny-all here, so give it the same read
        // grant before narrowing it with the region grant.
        let mut file_default = SandboxPolicy::deny_all("file-default");
        file_default
            .fs
            .add_root(SafeRoot::new("project", &base).unwrap())
            .unwrap();
        file_default
            .fs
            .grant("project", "src/**", Access::Read)
            .unwrap();
        file_default.capabilities.allow(Capability::FsRead);
        file_default.limits.memory_bytes = Some(96 * 1024 * 1024);

        let grant = parse_grant("grant(fs=[\"project/src/**\"], mem=32MiB)").unwrap();
        let effective = effective_policy(&pack, &runner, &file_default, Some(&grant)).unwrap();
        assert_eq!(effective.limits.memory_bytes, Some(32 * 1024 * 1024));
        assert_eq!(effective.fs.rules().len(), 1);

        let too_wide = parse_grant("grant(mem=512MiB)").unwrap();
        assert!(matches!(
            effective_policy(&pack, &runner, &file_default, Some(&too_wide)),
            Err(SandboxError::Denied(_))
        ));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn symlink_root_is_a_hard_error() {
        let base = temp_base("symlink-root");
        let outside = temp_base("symlink-outside");
        let link = base.join("escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(not(unix))]
        let _ = (&link, &outside);

        #[cfg(unix)]
        {
            assert!(matches!(
                SafeRoot::new("project", &link),
                Err(SandboxError::Escape { .. }) | Err(SandboxError::Invalid(_))
            ));
            let root = SafeRoot::new("project", &base).unwrap();
            assert!(matches!(
                root.resolve("escape/secret"),
                Err(SandboxError::Escape { .. })
            ));
        }
        std::fs::remove_dir_all(base).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn dotdot_in_manifest_root_is_a_hard_error() {
        let base = temp_base("dotdot");
        let text = "[sandbox]\nfs = [\"project\"]\n[sandbox.roots]\nproject = \"../outside\"\n";
        assert!(parse_pack(text, &base).is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn fixtures_are_available() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/host_path_validation.txt");
        let text = std::fs::read_to_string(fixture).unwrap();
        assert!(text.lines().any(|line| line.starts_with("../")));
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/echo_args.py");
        assert!(script.is_file());
    }
}
