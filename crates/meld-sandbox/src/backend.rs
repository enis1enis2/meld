//! Backend contract. A backend that cannot enforce the declared policy must
//! refuse to spawn (`fail closed`); it must never fall back to an unconfined
//! `Command::spawn`.

use crate::error::{Result, SandboxError};
use crate::policy::{EnforcementRequirements, SandboxPolicy};
use crate::process::{CommandSpec, SpawnedProcess};
use std::fmt;

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
    pub process_group_kill: bool,
    pub seatbelt: bool,
    pub job_object: bool,
    pub appcontainer: bool,
}

impl BackendFeatures {
    /// Map mechanism requirements to the mechanisms that can satisfy them.
    /// Alternatives are explicit: e.g. network denial can come from a network
    /// namespace *or* seccomp, filesystem isolation from a mount namespace *or*
    /// an enforced Landlock ruleset.
    pub fn can_enforce(&self, req: &EnforcementRequirements) -> Result<()> {
        if req.network_denial
            && !(self.network_namespace || self.seccomp || self.seatbelt || self.appcontainer)
        {
            return Err(SandboxError::Unsupported(
                "network denial needs netns, seccomp, Seatbelt or AppContainer".to_string(),
            ));
        }
        if req.fs_isolation
            && !(self.mount_namespace
                || self.landlock_enforced
                || self.seatbelt
                || self.appcontainer)
        {
            return Err(SandboxError::Unsupported(
                "filesystem isolation needs mountns, enforced Landlock, Seatbelt or AppContainer"
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
