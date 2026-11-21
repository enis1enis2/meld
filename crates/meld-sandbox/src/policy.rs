//! Effective policy: the intersection of all declarations, with a stable
//! fingerprint suitable for audit logs and cached enforcement decisions.

use crate::capability::{
    Access, Capability, CapabilitySet, FsPolicy, NetworkPolicy, ResourceLimits,
};
use crate::error::{Result, SandboxError};
use std::collections::BTreeSet;
use std::fmt;

/// What a backend must actually enforce for a policy to be honoured. These are
/// mechanisms, not capabilities: a backend that cannot provide them must
/// refuse to launch rather than run unconfined.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EnforcementRequirements {
    pub network_denial: bool,
    /// Destination hostname allowlists require a real host-aware mechanism.
    pub host_allowlist: bool,
    pub fs_isolation: bool,
    pub syscall_filter: bool,
    pub resource_limits: bool,
    pub process_tree_kill: bool,
}

/// 64-bit FNV-1a over the canonical policy description. Deterministic across
/// platforms and independent of TOML key order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Fingerprint(pub u64);

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fp:{:016x}", self.0)
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxPolicy {
    pub name: String,
    pub capabilities: CapabilitySet,
    pub fs: FsPolicy,
    pub network: NetworkPolicy,
    /// Environment variable names a region may read. Empty means none.
    pub env: BTreeSet<String>,
    pub limits: ResourceLimits,
}

impl SandboxPolicy {
    /// Missing `[sandbox]`: no network, no filesystem, no capabilities.
    pub fn deny_all(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            capabilities: CapabilitySet::deny_all(),
            fs: FsPolicy::new(),
            network: NetworkPolicy::deny_all(),
            env: BTreeSet::new(),
            limits: ResourceLimits::default(),
        }
    }

    /// Validate internal consistency. Fail closed on an incoherent declaration.
    pub fn validate(&self) -> Result<()> {
        self.network.validate()?;
        if self.network.protocols.any() && !self.capabilities.contains(Capability::Network) {
            return Err(SandboxError::Denied(
                "network protocols declared without the network capability".to_string(),
            ));
        }
        if !self.fs.rules().is_empty() {
            for rule in self.fs.rules() {
                if self.fs.root(rule.root()).is_none() {
                    return Err(SandboxError::Invalid(format!(
                        "fs rule names undeclared root {:?}",
                        rule.root()
                    )));
                }
                if (rule.access() == Access::Exec
                    && !self.capabilities.contains(Capability::FsExec))
                    || (rule.access() == Access::Write
                        && !self.capabilities.contains(Capability::FsWrite))
                    || (rule.access() == Access::Read
                        && !self.capabilities.contains(Capability::FsRead))
                {
                    return Err(SandboxError::Denied(format!(
                        "fs rule {} has no matching capability",
                        rule.canonical()
                    )));
                }
            }
        }
        for (a, b) in [
            (self.limits.idle_timeout_ms, self.limits.overall_timeout_ms),
            (self.limits.cpu_time_ms, self.limits.wall_time_ms),
        ] {
            if let (Some(a), Some(b)) = (a, b) {
                if a > b {
                    return Err(SandboxError::Invalid(format!(
                        "shorter timeout {a}ms exceeds overall limit {b}ms"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Grants can only narrow. Conflicting roots or impossible combinations are
    /// errors rather than a silently widened policy.
    pub fn intersection(&self, other: &Self) -> Result<Self> {
        let mut out = Self {
            name: format!("{}∩{}", self.name, other.name),
            capabilities: self.capabilities.intersection(&other.capabilities),
            fs: self.fs.intersection(&other.fs)?,
            network: self.network.intersection(&other.network),
            env: self.env.intersection(&other.env).cloned().collect(),
            limits: self.limits.intersection(&other.limits),
        };
        // A protocol must also be present in the boolean capability set of both
        // layers if the narrower layer forgot to repeat it.
        if !out.network.protocols.any() {
            out.capabilities.retain_without(Capability::Network);
        }
        out.validate()?;
        Ok(out)
    }

    /// A deterministic digest of the enforcement-relevant policy. Names are
    /// intentionally excluded so the same rules always produce the same pin.
    pub fn fingerprint(&self) -> Fingerprint {
        let canonical = format!(
            "meld-sandbox/v2|caps[{}]|fs[{}]|net[{}]|env[{}]|limits[{}]",
            self.capabilities.canonical(),
            self.fs.canonical(),
            self.network.canonical(),
            self.env.iter().cloned().collect::<Vec<_>>().join(","),
            self.limits.canonical()
        );
        Fingerprint(fnv1a(canonical.as_bytes()))
    }

    /// All declarations that must be intersected to produce the effective
    /// policy for a region.
    pub fn intersection_of(layers: &[Self]) -> Result<Self> {
        let mut iter = layers.iter();
        let mut acc = iter
            .next()
            .cloned()
            .ok_or_else(|| SandboxError::Invalid("no policy layers supplied".to_string()))?;
        for layer in iter {
            acc = acc.intersection(layer)?;
        }
        Ok(acc)
    }

    /// Mechanisms a backend must actually enforce for this policy.
    ///
    /// Untrusted workers are always subject to a syscall filter and a
    /// process-tree kill, even for a deny-all policy. Every declared security
    /// limit (including I/O and deadlines) contributes a resource-limit
    /// requirement, so adding a new limit cannot silently weaken `requires()`.
    pub fn requires(&self) -> EnforcementRequirements {
        let limits = &self.limits;
        EnforcementRequirements {
            // Denying network by default needs a real mechanism, not just an
            // assumption that the worker will behave.
            network_denial: !self.capabilities.contains(Capability::Network)
                && !self.network.protocols.any(),
            host_allowlist: self.requires_host_allowlist_enforcement(),
            fs_isolation: !self.fs.rules().is_empty()
                || !self.capabilities.contains(Capability::FsWrite)
                || !self.capabilities.contains(Capability::FsRead),
            // `spawn` is no longer a grantable capability (M): the seccomp
            // baseline is default-EPERM for every worker.
            syscall_filter: true,
            resource_limits: [
                limits.memory_bytes,
                limits.cpu_time_ms,
                limits.wall_time_ms,
                limits.file_size_bytes,
                limits.processes,
                limits.open_files,
                limits.stack_bytes,
                limits.max_line_bytes,
                limits.max_stderr_bytes,
                limits.max_output_lines,
                limits.idle_timeout_ms,
                limits.overall_timeout_ms,
            ]
            .into_iter()
            .any(|value| value.is_some()),
            // Every untrusted worker must be killable as a process tree.
            process_tree_kill: true,
        }
    }

    /// Whether the policy depends on hostname-level network filtering that a
    /// mechanistic backend cannot express.
    pub fn requires_host_allowlist_enforcement(&self) -> bool {
        self.network.requires_host_enforcement()
    }

    /// Narrow this policy with a region grant. The grant must be covered by the
    /// current policy; it can never add permission.
    pub fn apply_grant(&self, grant: &crate::grant::Grant) -> Result<Self> {
        grant.evaluate(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{ProtocolCapability, SafeRoot};
    use std::path::Path;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "meld-sandbox-policy-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn missing_policy_is_deny_all_and_stable() {
        let a = SandboxPolicy::deny_all("pack");
        let b = SandboxPolicy::deny_all("runner");
        a.validate().unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert!(a.requires().network_denial);
        assert!(a.fs.rules().is_empty());
    }

    #[test]
    fn intersection_never_widens_and_fingerprint_is_deterministic() {
        let base = temp_dir("intersect");
        let mut pack = SandboxPolicy::deny_all("pack");
        pack.fs
            .add_root(SafeRoot::new("project", &base).unwrap())
            .unwrap();
        pack.fs.grant("project", "src/**", Access::Read).unwrap();
        pack.capabilities.allow(Capability::FsRead);
        let mut runner = SandboxPolicy::deny_all("runner");
        runner
            .fs
            .add_root(SafeRoot::new("project", &base).unwrap())
            .unwrap();
        runner.fs.grant("project", "src/**", Access::Read).unwrap();
        runner.capabilities.allow(Capability::FsRead);

        let effective = pack.intersection(&runner).unwrap();
        assert_eq!(effective, runner_rename(&runner));
        assert_eq!(effective.fingerprint(), effective.fingerprint());
        let empty = SandboxPolicy::deny_all("file-default");
        assert!(effective
            .intersection(&empty)
            .unwrap()
            .fs
            .rules()
            .is_empty());
        assert_ne!(effective.fingerprint(), empty.fingerprint());
        std::fs::remove_dir_all(base).ok();
    }

    fn runner_rename(p: &SandboxPolicy) -> SandboxPolicy {
        let mut p = p.clone();
        p.name = "pack∩runner".to_string();
        p
    }

    #[test]
    fn conflicting_root_paths_are_hard_errors() {
        let a = temp_dir("root-a");
        let b = temp_dir("root-b");
        let mut first = SandboxPolicy::deny_all("a");
        first
            .fs
            .add_root(SafeRoot::new("project", &a).unwrap())
            .unwrap();
        let mut second = SandboxPolicy::deny_all("b");
        second
            .fs
            .add_root(SafeRoot::new("project", &b).unwrap())
            .unwrap();
        assert!(matches!(
            first.intersection(&second),
            Err(SandboxError::Conflict(_))
        ));
        assert!(Path::new(&a).exists());
        std::fs::remove_dir_all(a).ok();
        std::fs::remove_dir_all(b).ok();
    }

    #[test]
    fn network_without_hosts_fails_closed() {
        let mut p = SandboxPolicy::deny_all("p");
        p.capabilities.allow(Capability::Network);
        p.network.protocols = ProtocolCapability::parse("tcp").unwrap();
        assert!(matches!(p.validate(), Err(SandboxError::Denied(_))));
    }
}
