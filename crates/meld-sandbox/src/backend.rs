//! Backend contract. A backend that cannot enforce the declared policy must
//! refuse to spawn (`fail closed`); it must never fall back to an unconfined
//! `Command::spawn`.

use crate::error::{Result, SandboxError};
use crate::policy::{EnforcementRequirements, SandboxPolicy};
use crate::process::{CommandSpec, SpawnedProcess};
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    LinuxNamespaces,
    MacosSeatbelt,
    WindowsJobObject,
    OciRunc,
    /// No OS confinement: only usable for trusted test fixtures. Never
    /// selected automatically for untrusted code.
    UnconfinedTestOnly,
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LinuxNamespaces => "linux-namespaces",
            Self::MacosSeatbelt => "macos-seatbelt",
            Self::WindowsJobObject => "windows-job-object",
            Self::OciRunc => "oci-runc",
            Self::UnconfinedTestOnly => "unconfined-test-only",
        })
    }
}

/// Concrete mechanisms this backend can actually enforce *right now*.
/// `landlock_enforced` is only set after a successful ruleset installation;
/// the ENOSYS case must leave it false.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendFeatures {
    pub user_namespace: bool,
    pub mount_namespace: bool,
    pub pid_namespace: bool,
    pub network_namespace: bool,
    pub ipc_namespace: bool,
    pub seccomp: bool,
    pub cgroup_v2: bool,
    pub rlimits: bool,
    pub landlock_available: bool,
    pub landlock_enforced: bool,
    /// A real filesystem root boundary (e.g. an OCI container rootfs). A mount
    /// namespace that merely makes mounts private is **not** root confinement:
    /// declared paths still refer to the host filesystem.
    pub root_confinement: bool,
    pub process_group_kill: bool,
    pub seatbelt: bool,
    pub job_object: bool,
    pub appcontainer: bool,
    /// A backend that can actually restrict destinations by hostname (for
    /// example a policy-aware proxy). No current backend claims this: seccomp,
    /// Seatbelt and an empty netns can only filter protocols or deny all.
    pub host_allowlist: bool,
}

impl BackendFeatures {
    /// Map mechanism requirements to the mechanisms that can satisfy them.
    /// Alternatives are explicit. Filesystem isolation needs a real root
    /// boundary, an enforced Landlock ruleset, Seatbelt or AppContainer; a
    /// bare mount namespace is deliberately not accepted (C2).
    pub fn can_enforce(&self, req: &EnforcementRequirements) -> Result<()> {
        if req.network_denial
            && !(self.network_namespace || self.seccomp || self.seatbelt || self.appcontainer)
        {
            return Err(SandboxError::Unsupported(
                "network denial needs netns, seccomp, Seatbelt or AppContainer".to_string(),
            ));
        }
        if req.host_allowlist && !self.host_allowlist {
            return Err(SandboxError::Unsupported(
                "hostname allowlists need a backend that can enforce destination hosts; protocol bits are not a host filter"
                    .to_string(),
            ));
        }
        if req.fs_isolation
            && !(self.root_confinement
                || self.landlock_enforced
                || self.seatbelt
                || self.appcontainer)
        {
            return Err(SandboxError::Unsupported(
                "filesystem isolation needs root confinement, enforced Landlock, Seatbelt or AppContainer (a mount namespace alone is not enough)"
                    .to_string(),
            ));
        }
        if req.syscall_filter && !(self.seccomp || self.seatbelt || self.appcontainer) {
            return Err(SandboxError::Unsupported(
                "syscall filtering needs seccomp, Seatbelt or AppContainer".to_string(),
            ));
        }
        if req.resource_limits && !(self.cgroup_v2 || self.rlimits || self.job_object) {
            return Err(SandboxError::Unsupported(
                "resource limits need cgroup v2, rlimits or a Job Object".to_string(),
            ));
        }
        if req.process_tree_kill && !(self.process_group_kill || self.job_object) {
            return Err(SandboxError::Unsupported(
                "process-tree kill needs process groups or a Job Object".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct BackendProbe {
    pub kind: BackendKind,
    pub available: bool,
    pub features: BackendFeatures,
    pub notes: Vec<String>,
}

impl BackendProbe {
    pub fn unavailable(kind: BackendKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            available: false,
            features: BackendFeatures::default(),
            notes: vec![reason.into()],
        }
    }

    pub fn require(&self, policy: &SandboxPolicy) -> Result<()> {
        if !self.available {
            return Err(SandboxError::Unsupported(format!(
                "{} backend is unavailable: {}",
                self.kind,
                self.notes.join("; ")
            )));
        }
        self.features.can_enforce(&policy.requires())?;
        Ok(())
    }
}

pub trait SandboxBackend: Send + Sync {
    fn kind(&self) -> BackendKind;
    fn probe(&self) -> BackendProbe;

    /// Spawn exactly the declared argv under the effective policy. Must call
    /// `probe().require(policy)` first and never spawn unconfined.
    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess>;
}

/// Shared backends are first-class backends. This lets hosts attach one probed
/// backend to multiple workers without wrapping it in a bespoke adapter.
impl<T: SandboxBackend + ?Sized> SandboxBackend for Arc<T> {
    fn kind(&self) -> BackendKind {
        (**self).kind()
    }

    fn probe(&self) -> BackendProbe {
        (**self).probe()
    }

    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess> {
        (**self).spawn(spec, policy)
    }
}

/// Validate before doing any side effect. Shared by all backend `spawn`s.
pub fn require_enforcement(backend: &dyn SandboxBackend, policy: &SandboxPolicy) -> Result<()> {
    policy.validate()?;
    backend.probe().require(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_probe_never_claims_enforcement() {
        let probe = BackendProbe::unavailable(BackendKind::LinuxNamespaces, "test");
        let err = probe.require(&SandboxPolicy::deny_all("p")).unwrap_err();
        assert!(matches!(err, SandboxError::Unsupported(_)));
    }

    #[test]
    fn landlock_available_does_not_imply_enforced() {
        let features = BackendFeatures {
            landlock_available: true,
            landlock_enforced: false,
            process_group_kill: true,
            ..Default::default()
        };
        let req = EnforcementRequirements {
            fs_isolation: true,
            process_tree_kill: true,
            ..Default::default()
        };
        assert!(matches!(
            features.can_enforce(&req),
            Err(SandboxError::Unsupported(_))
        ));
    }

    #[test]
    fn mount_namespace_alone_never_satisfies_filesystem_isolation() {
        let features = BackendFeatures {
            mount_namespace: true,
            landlock_available: true,
            ..Default::default()
        };
        let req = EnforcementRequirements {
            fs_isolation: true,
            ..Default::default()
        };
        assert!(matches!(
            features.can_enforce(&req),
            Err(SandboxError::Unsupported(_))
        ));

        let confined = BackendFeatures {
            root_confinement: true,
            ..features
        };
        confined.can_enforce(&req).unwrap();
    }

    #[test]
    fn protocol_bits_never_satisfy_a_hostname_allowlist() {
        let features = BackendFeatures {
            seccomp: true,
            seatbelt: true,
            network_namespace: true,
            ..Default::default()
        };
        let req = EnforcementRequirements {
            host_allowlist: true,
            ..Default::default()
        };
        assert!(matches!(
            features.can_enforce(&req),
            Err(SandboxError::Unsupported(_))
        ));
        let proxy = BackendFeatures {
            host_allowlist: true,
            ..features
        };
        proxy.can_enforce(&req).unwrap();
    }

    #[test]
    fn alternative_mechanisms_satisfy_requirements() {
        let features = BackendFeatures {
            seatbelt: true,
            job_object: true,
            ..Default::default()
        };
        let req = EnforcementRequirements {
            network_denial: true,
            fs_isolation: true,
            syscall_filter: true,
            process_tree_kill: true,
            ..Default::default()
        };
        features.can_enforce(&req).unwrap();
    }
}
