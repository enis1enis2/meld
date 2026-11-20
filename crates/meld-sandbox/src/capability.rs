//! Capability, filesystem, network and resource primitives.
//!
//! Design authority: effective permissions are an **intersection**
//! `pack maximum ∩ runner constraints ∩ file default grant ∩ region grant`.
//! A grant can only narrow. Missing `[sandbox]` means network denied and an
//! empty filesystem grant set. Unknown capabilities and `..`/symlink escapes
//! are hard errors.

use crate::error::{check_text, Result, SandboxError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// A boolean capability. Names are explicit; there is deliberately no
/// wildcard and no "all" value in manifests or grants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Capability {
    FsRead,
    FsWrite,
    FsExec,
    Network,
    Dns,
    Env,
    Spawn,
    HostCall,
    Time,
    Entropy,
    Stdio,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::FsExec => "fs.exec",
            Self::Network => "network",
            Self::Dns => "dns",
            Self::Env => "env",
            Self::Spawn => "spawn",
            Self::HostCall => "hostcall",
            Self::Time => "time",
            Self::Entropy => "entropy",
            Self::Stdio => "stdio",
        }
    }

    /// Unknown capability strings are a hard error, never ignored.
    pub fn parse(value: &str) -> Result<Self> {
        check_text("capability", value)?;
        Ok(match value {
            "fs.read" | "fs:read" | "fs-read" => Self::FsRead,
            "fs.write" | "fs:write" | "fs-write" => Self::FsWrite,
            "fs.exec" | "fs:exec" | "fs-exec" => Self::FsExec,
            "network" | "net" => Self::Network,
            "dns" => Self::Dns,
            "env" => Self::Env,
            "spawn" | "process.spawn" => Self::Spawn,
            "hostcall" | "host.call" => Self::HostCall,
            "time" => Self::Time,
            "entropy" => Self::Entropy,
            "stdio" => Self::Stdio,
            _ => return Err(SandboxError::Unknown("capability", value.to_string())),
        })
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Set of boolean capabilities. Empty means no capabilities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl CapabilitySet {
    pub fn deny_all() -> Self {
        Self(BTreeSet::new())
    }

    pub fn parse<I, S>(values: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Self::deny_all();
        for value in values {
            let value = value.as_ref();
            out.0.insert(Capability::parse(value)?);
        }
        Ok(out)
    }

    pub fn contains(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
    }

    pub fn allow(&mut self, cap: Capability) {
        self.0.insert(cap);
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }

    /// Grants can only narrow: intersection of two sets.
    pub fn intersection(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Remove a capability when the protocol-level grant is empty.
    pub(crate) fn retain_without(&mut self, cap: Capability) {
        self.0.remove(&cap);
    }

    /// Canonical, deterministic representation used by the policy fingerprint.
    pub(crate) fn canonical(&self) -> String {
        self.0
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Network protocols that can be requested independently. `none()` denies all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolCapability {
    pub tcp: bool,
    pub udp: bool,
    pub unix: bool,
    pub http: bool,
    pub https: bool,
    pub dns: bool,
}

impl ProtocolCapability {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn all() -> Self {
        Self {
            tcp: true,
            udp: true,
            unix: true,
            http: true,
            https: true,
            dns: true,
        }
    }

    pub fn any(&self) -> bool {
        self.tcp || self.udp || self.unix || self.http || self.https || self.dns
    }

    /// Unknown protocol names are a hard error.
    pub fn parse(value: &str) -> Result<Self> {
        check_text("protocol", value)?;
        let mut out = Self::none();
        match value {
            "none" => {}
            "all" => out = Self::all(),
            "tcp" => out.tcp = true,
            "udp" => out.udp = true,
            "unix" | "unixsocket" => out.unix = true,
            "http" => {
                out.http = true;
                out.tcp = true;
            }
            "https" => {
                out.https = true;
                out.tcp = true;
            }
            "dns" => {
                out.dns = true;
                out.udp = true;
            }
            _ => return Err(SandboxError::Unknown("protocol", value.to_string())),
        }
        Ok(out)
    }

    pub fn parse_list<I, S>(values: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Self::none();
        for value in values {
            let parsed = Self::parse(value.as_ref())?;
            out.tcp |= parsed.tcp;
            out.udp |= parsed.udp;
            out.unix |= parsed.unix;
            out.http |= parsed.http;
            out.https |= parsed.https;
            out.dns |= parsed.dns;
        }
        Ok(out)
    }

    /// Intersection: a protocol is allowed only if both layers allow it.
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            tcp: self.tcp && other.tcp,
            udp: self.udp && other.udp,
            unix: self.unix && other.unix,
            http: self.http && other.http,
            https: self.https && other.https,
            dns: self.dns && other.dns,
        }
    }

    pub(crate) fn canonical(&self) -> String {
        format!(
            "tcp={},udp={},unix={},http={},https={},dns={}",
            self.tcp, self.udp, self.unix, self.http, self.https, self.dns
        )
    }
}

impl fmt::Display for ProtocolCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names = Vec::new();
        if self.tcp {
            names.push("tcp");
        }
        if self.udp {
            names.push("udp");
        }
        if self.unix {
            names.push("unix");
        }
        if self.http {
            names.push("http");
        }
        if self.https {
            names.push("https");
        }
        if self.dns {
            names.push("dns");
        }
        let joined = names.join(",");
        f.write_str(if joined.is_empty() { "none" } else { &joined })
    }
}

/// Filesystem access mode. There is no implicit "write implies read".
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Write,
    Exec,
}

impl Access {
    pub fn parse(value: &str) -> Result<Self> {
        check_text("access", value)?;
        Ok(match value {
            "read" | "r" => Self::Read,
            "write" | "w" => Self::Write,
            "exec" | "x" => Self::Exec,
            _ => return Err(SandboxError::Unknown("access", value.to_string())),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Exec => "exec",
        }
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A canonicalized host directory exposed under a logical name. Construction
/// rejects symlinks and `..` so a later logical path cannot escape it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafeRoot {
    name: String,
    path: PathBuf,
}

impl SafeRoot {
    pub fn new(name: impl Into<String>, host_path: impl AsRef<Path>) -> Result<Self> {
        let name = name.into();
        check_text("root name", &name)?;
        if name.contains('/') || name.contains('\\') {
            return Err(SandboxError::Invalid(format!(
                "root name must be a single segment: {name}"
            )));
        }
        let declared = host_path.as_ref();
        reject_parent_components(declared)?;
        // Reject symlinks in the declaration itself before canonicalization:
        // `canonicalize` would silently follow them and hide the escape.
        reject_symlinks_in_path(declared)?;
        let path = std::fs::canonicalize(declared).map_err(|e| {
            SandboxError::Invalid(format!(
                "cannot canonicalize root {}: {e}",
                declared.display()
            ))
        })?;
        if !path.is_dir() {
            return Err(SandboxError::Invalid(format!(
                "sandbox root is not a directory: {}",
                path.display()
            )));
        }
        reject_symlinks_in_path(&path)?;
        Ok(Self { name, path })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve a logical, root-relative path to a host path. Absolute paths,
    /// `..` and symlink components are hard errors.
    pub fn resolve(&self, logical: &str) -> Result<PathBuf> {
        check_text("logical path", logical)?;
        let relative = Path::new(logical);
        for component in relative.components() {
            match component {
                Component::Normal(part) => {
                    let part = part.to_string_lossy();
                    if part.contains('\\') {
                        return Err(escape(logical, "backslash separator"));
                    }
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(escape(logical, "component leaves safe root"));
                }
            }
        }
        let joined = self.path.join(relative);
        if !joined.starts_with(&self.path) {
            return Err(escape(logical, "resolved outside safe root"));
        }
        reject_symlinks_in_path(&joined)?;
        Ok(joined)
    }

    pub(crate) fn canonical(&self) -> String {
        format!("{}={}", self.name, self.path.display())
    }
}

fn escape(path: impl Into<PathBuf>, reason: impl Into<String>) -> SandboxError {
    SandboxError::Escape {
        path: path.into(),
        reason: reason.into(),
    }
}

/// Reject `..` in declared manifest paths even before canonicalization.
pub fn reject_parent_components(path: &Path) -> Result<()> {
    for part in path.components() {
        if matches!(part, Component::ParentDir) {
            return Err(escape(path, "`..` in declared path"));
        }
    }
    Ok(())
}

/// Walk every existing component and refuse symlinks. For a not-yet-created
/// final component we validate its nearest existing ancestor; the caller must
/// re-check after creating files (see `SafeRoot::resolve`).
pub fn reject_symlinks_in_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let meta = match std::fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(SandboxError::Io(e)),
        };
        if meta.file_type().is_symlink() {
            return Err(escape(path, "symlink component"));
        }
    }
    Ok(())
}

/// One filesystem rule: a logical path under a named root and an access mode.
/// `path` may end in `/**` to cover a subtree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsRule {
    root: String,
    path: String,
    access: Access,
    subtree: bool,
}

impl FsRule {
    pub fn new(root: impl Into<String>, path: impl Into<String>, access: Access) -> Result<Self> {
        let root = root.into();
        let mut path = path.into();
        check_text("fs root", &root)?;
        check_text("fs path", &path)?;
        let subtree = path.ends_with("/**") || path == "**";
        if path == "**" {
            path.clear();
        } else if subtree {
            path.truncate(path.len() - 3);
        }
        if path == "." {
            path.clear();
        }
        let probe = SafeRoot {
            name: root.clone(),
            path: PathBuf::from("/sandbox-root"),
        };
        if path.is_empty() && !subtree {
            return Err(SandboxError::Invalid(
                "empty fs path must use the /** subtree suffix".to_string(),
            ));
        }
        if !path.is_empty() {
            probe.resolve(&path)?;
        }
        Ok(Self {
            root,
            path,
            access,
            subtree,
        })
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn access(&self) -> Access {
        self.access
    }

    /// Whether this rule covers `logical` under `root`.
    pub fn covers(&self, root: &str, logical: &str) -> bool {
        if self.root != root {
            return false;
        }
        if self.subtree && self.path.is_empty() {
            return true;
        }
        if self.subtree {
            logical == self.path || logical.starts_with(&format!("{}/", self.path))
        } else {
            logical == self.path
        }
    }

    pub(crate) fn canonical(&self) -> String {
        format!(
            "{}:{}:{}{}",
            self.root,
            self.path,
            self.access.as_str(),
            if self.subtree { "/**" } else { "" }
        )
    }
}

/// Filesystem policy: safe roots plus the only allowed (root, path, access)
/// triples. Empty rules means no filesystem access.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FsPolicy {
    roots: BTreeMap<String, SafeRoot>,
    rules: Vec<FsRule>,
}

impl FsPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_root(&mut self, root: SafeRoot) -> Result<()> {
        if let Some(existing) = self.roots.get(root.name()) {
            if *existing != root {
                return Err(SandboxError::Conflict(format!(
                    "root {} maps to both {} and {}",
                    root.name(),
                    existing.path().display(),
                    root.path().display()
                )));
            }
        }
        self.roots.insert(root.name().to_string(), root);
        Ok(())
    }

    pub fn root(&self, name: &str) -> Option<&SafeRoot> {
        self.roots.get(name)
    }

    pub fn roots(&self) -> impl Iterator<Item = &SafeRoot> {
        self.roots.values()
    }

    pub fn rules(&self) -> &[FsRule] {
        &self.rules
    }

    pub fn grant(&mut self, root: &str, path: &str, access: Access) -> Result<()> {
        if !self.roots.contains_key(root) {
            return Err(SandboxError::Invalid(format!("unknown fs root {root:?}")));
        }
        let rule = FsRule::new(root, path, access)?;
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
        Ok(())
    }

    /// Resolve a logical path and check that the requested access is granted.
    pub fn check(&self, root: &str, logical: &str, access: Access) -> Result<PathBuf> {
        let safe = self
            .roots
            .get(root)
            .ok_or_else(|| SandboxError::Denied(format!("fs root {root:?} is not declared")))?;
        if !self
            .rules
            .iter()
            .any(|rule| rule.covers(root, logical) && rule.access == access)
        {
            return Err(SandboxError::Denied(format!(
                "no {access} grant for {root}:{logical}"
            )));
        }
        safe.resolve(logical)
    }

    /// Grants can only narrow. Roots with the same name but different host
    /// paths are a hard conflict rather than a silent union.
    pub fn intersection(&self, other: &Self) -> Result<Self> {
        let mut out = Self::new();
        for (name, root) in &self.roots {
            if let Some(their) = other.roots.get(name) {
                if root != their {
                    return Err(SandboxError::Conflict(format!(
                        "root {name} maps to both {} and {}",
                        root.path().display(),
                        their.path().display()
                    )));
                }
                out.add_root(root.clone())?;
            }
        }
        // A rule survives only if it is present in both layers verbatim. This
        // is intersection, not path-prefix union: a narrower path must be
        // stated by the narrowing layer.
        for rule in &self.rules {
            if other.rules.contains(rule) {
                out.rules.push(rule.clone());
            }
        }
        Ok(out)
    }

    pub(crate) fn canonical(&self) -> String {
        let roots = self
            .roots
            .values()
            .map(SafeRoot::canonical)
            .collect::<Vec<_>>()
            .join(";");
        let rules = self
            .rules
            .iter()
            .map(FsRule::canonical)
            .collect::<Vec<_>>()
            .join(";");
        format!("roots[{roots}]/rules[{rules}]")
    }
}

/// Network policy: protocol bits plus an explicit host allowlist. If protocols
/// are enabled but no hosts are named, the policy fails closed at validation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkPolicy {
    pub protocols: ProtocolCapability,
    pub hosts: BTreeSet<String>,
}

impl NetworkPolicy {
    pub fn deny_all() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, protocol: &str, host: &str) -> Result<()> {
        check_text("host", host)?;
        let parsed = ProtocolCapability::parse(protocol)?;
        self.protocols.tcp |= parsed.tcp;
        self.protocols.udp |= parsed.udp;
        self.protocols.unix |= parsed.unix;
        self.protocols.http |= parsed.http;
        self.protocols.https |= parsed.https;
        self.protocols.dns |= parsed.dns;
        self.hosts.insert(host.to_string());
        Ok(())
    }

    /// Reject "allow all protocols to nobody" and wildcard hosts.
    pub fn validate(&self) -> Result<()> {
        for host in &self.hosts {
            if host == "*" || host.contains('*') {
                return Err(SandboxError::Invalid(format!(
                    "wildcard host is not allowed: {host}"
                )));
            }
        }
        if self.protocols.any() && self.hosts.is_empty() {
            return Err(SandboxError::Denied(
                "network protocols enabled without an explicit host allowlist".to_string(),
            ));
        }
        Ok(())
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            protocols: self.protocols.intersection(&other.protocols),
            hosts: self.hosts.intersection(&other.hosts).cloned().collect(),
        }
    }

    pub(crate) fn canonical(&self) -> String {
        format!(
            "{}/hosts[{}]",
            self.protocols.canonical(),
            self.hosts.iter().cloned().collect::<Vec<_>>().join(",")
        )
    }
}

/// Numeric limits. `None` means "this layer has no opinion"; the intersection
/// keeps the most restrictive `Some`. No value means unbounded at this layer,
/// but the backend still applies conservative built-in defaults through
/// `effective_*` helpers for I/O and timeouts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceLimits {
    pub memory_bytes: Option<u64>,
    pub cpu_time_ms: Option<u64>,
    pub wall_time_ms: Option<u64>,
    pub file_size_bytes: Option<u64>,
    pub processes: Option<u64>,
    pub open_files: Option<u64>,
    pub stack_bytes: Option<u64>,
    pub max_line_bytes: Option<u64>,
    pub max_stderr_bytes: Option<u64>,
    pub max_output_lines: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub overall_timeout_ms: Option<u64>,
}

pub const DEFAULT_MAX_LINE_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_MAX_STDERR_BYTES: u64 = 64 * 1024;
pub const DEFAULT_MAX_OUTPUT_LINES: u64 = 4096;
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_OVERALL_TIMEOUT_MS: u64 = 60_000;

impl ResourceLimits {
    fn tighten(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        }
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            memory_bytes: Self::tighten(self.memory_bytes, other.memory_bytes),
            cpu_time_ms: Self::tighten(self.cpu_time_ms, other.cpu_time_ms),
            wall_time_ms: Self::tighten(self.wall_time_ms, other.wall_time_ms),
            file_size_bytes: Self::tighten(self.file_size_bytes, other.file_size_bytes),
            processes: Self::tighten(self.processes, other.processes),
            open_files: Self::tighten(self.open_files, other.open_files),
            stack_bytes: Self::tighten(self.stack_bytes, other.stack_bytes),
            max_line_bytes: Self::tighten(self.max_line_bytes, other.max_line_bytes),
            max_stderr_bytes: Self::tighten(self.max_stderr_bytes, other.max_stderr_bytes),
            max_output_lines: Self::tighten(self.max_output_lines, other.max_output_lines),
            idle_timeout_ms: Self::tighten(self.idle_timeout_ms, other.idle_timeout_ms),
            overall_timeout_ms: Self::tighten(self.overall_timeout_ms, other.overall_timeout_ms),
        }
    }

    pub fn effective_max_line_bytes(&self) -> u64 {
        self.max_line_bytes.unwrap_or(DEFAULT_MAX_LINE_BYTES)
    }

    pub fn effective_max_stderr_bytes(&self) -> u64 {
        self.max_stderr_bytes.unwrap_or(DEFAULT_MAX_STDERR_BYTES)
    }

    pub fn effective_max_output_lines(&self) -> u64 {
        self.max_output_lines.unwrap_or(DEFAULT_MAX_OUTPUT_LINES)
    }

    pub fn effective_idle_timeout_ms(&self) -> u64 {
        self.idle_timeout_ms.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS)
    }

    pub fn effective_overall_timeout_ms(&self) -> u64 {
        self.overall_timeout_ms
            .unwrap_or(DEFAULT_OVERALL_TIMEOUT_MS)
    }

    pub(crate) fn canonical(&self) -> String {
        format!(
            "mem:{:?},cpu:{:?},wall:{:?},file:{:?},proc:{:?},nofile:{:?},stack:{:?},line:{:?},stderr:{:?},lines:{:?},idle:{:?},overall:{:?}",
            self.memory_bytes,
            self.cpu_time_ms,
            self.wall_time_ms,
            self.file_size_bytes,
            self.processes,
            self.open_files,
            self.stack_bytes,
            self.max_line_bytes,
            self.max_stderr_bytes,
            self.max_output_lines,
            self.idle_timeout_ms,
            self.overall_timeout_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_capability_is_an_error() {
        assert!(matches!(
            Capability::parse("execute_anything"),
            Err(SandboxError::Unknown("capability", _))
        ));
    }

    #[test]
    fn intersections_only_narrow() {
        let mut a = CapabilitySet::parse(["fs.read", "network"]).unwrap();
        a.allow(Capability::Spawn);
        let b = CapabilitySet::parse(["fs.read", "spawn"]).unwrap();
        let c = a.intersection(&b);
        assert!(c.contains(Capability::FsRead));
        assert!(c.contains(Capability::Spawn));
        assert!(!c.contains(Capability::Network));

        let p = ProtocolCapability::parse_list(["tcp", "dns"]).unwrap();
        let q = ProtocolCapability::parse("dns").unwrap();
        let r = p.intersection(&q);
        assert!(r.dns && r.udp);
        assert!(!r.tcp);
    }

    #[test]
    fn resource_limits_keep_strictest_value() {
        let a = ResourceLimits {
            memory_bytes: Some(128),
            wall_time_ms: Some(2000),
            ..Default::default()
        };
        let b = ResourceLimits {
            memory_bytes: Some(64),
            max_line_bytes: Some(16),
            ..Default::default()
        };
        let c = a.intersection(&b);
        assert_eq!(c.memory_bytes, Some(64));
        assert_eq!(c.wall_time_ms, Some(2000));
        assert_eq!(c.max_line_bytes, Some(16));
    }
}
