//! Optional OCI fallback (`--features oci`): generate an OCI runtime spec from
//! the effective policy and run it with a discovered `runc` binary. No shell is
//! ever used; all invocations are argv vectors.

use crate::backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
use crate::error::{Result, SandboxError};
use crate::io::IoPolicy;
use crate::policy::SandboxPolicy;
use crate::process::{CommandSpec, SpawnedProcess};
use oci_spec::runtime::{Linux, LinuxMemory, LinuxPids, LinuxResources, Process, Spec};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static BUNDLE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Discover `runc` on PATH without invoking a shell.
pub fn find_runc() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("runc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&candidate) {
                if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(unix))]
        {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Build an OCI spec that follows the effective policy. Network denial is
/// expressed by requiring the runtime to create a fresh network namespace.
pub fn build_spec(spec: &CommandSpec, policy: &SandboxPolicy) -> Result<Spec> {
    let mut config = Spec::default();
    config.set_version("1.0.2");

    let mut process = Process::default();
    process
        .set_terminal(false)
        .set_args(spec.argv())
        .set_cwd(spec.cwd.clone())
        .set_no_new_privileges(true);
    let env: Vec<String> = spec
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    process.set_env(env);
    config.set_process(process);

    let mut linux = Linux::default();
    let mut resources = LinuxResources::default();
    if let Some(mem) = policy.limits.memory_bytes {
        let mut memory = LinuxMemory::default();
        memory.set_limit(mem as i64);
        memory.set_swap(0);
        resources.set_memory(memory);
    }
    if let Some(pids) = policy.limits.processes {
        let mut pids_limit = LinuxPids::default();
        pids_limit.set_limit(pids as i64);
        resources.set_pids(pids_limit);
    }
    linux.set_resources(resources);

    // OCI defaults include all standard namespaces; keep them so the worker
    // gets no host network unless the policy explicitly grants protocols.
    if !policy.network.protocols.any() {
        let annotations = config
            .annotations_mut()
            .get_or_insert_with(Default::default);
        annotations.insert(
            "meld.sandbox/network".to_string(),
            "denied-by-policy".to_string(),
        );
    }
    let fs_roots: Vec<String> = policy
        .fs
        .roots()
        .map(|r| r.path().to_string_lossy().into_owned())
        .collect();
    config
        .annotations_mut()
        .get_or_insert_with(Default::default)
        .insert("meld.sandbox/fs-roots".to_string(), fs_roots.join(","));
    config
        .annotations_mut()
        .get_or_insert_with(Default::default)
        .insert(
            "meld.sandbox/fingerprint".to_string(),
            policy.fingerprint().to_string(),
        );
    config.set_linux(linux);
    Ok(config)
}

pub fn build_bundle(dir: &Path, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<()> {
    fs::create_dir_all(dir.join("rootfs"))?;
    let config = build_spec(spec, policy)?;
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| SandboxError::Invalid(format!("OCI spec serialization: {e}")))?;
    fs::write(dir.join("config.json"), json.as_bytes())?;
    Ok(())
}

pub struct OciBackend {
    pub runc: Option<PathBuf>,
}

impl Default for OciBackend {
    fn default() -> Self {
        Self { runc: find_runc() }
    }
}

fn run_runc(runc: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(runc)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(SandboxError::Denied(format!(
            "runc {} exited with {status}",
            args.join(" ")
        )));
    }
    Ok(())
}

impl OciBackend {
    pub fn probe(&self) -> BackendProbe {
        let available = self.runc.is_some();
        let mut notes = vec!["OCI fallback requires a host runc installation".to_string()];
        if !available {
            notes.push("runc not found on PATH".to_string());
        }
        BackendProbe {
            kind: BackendKind::OciRunc,
            available,
            features: BackendFeatures {
                network_namespace: available,
                mount_namespace: available,
                pid_namespace: available,
                seccomp: available,
                cgroup_v2: available,
                rlimits: available,
                process_group_kill: false,
                job_object: false,
                ..Default::default()
            },
            notes,
        }
    }
}

impl SandboxBackend for OciBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::OciRunc
    }

    fn probe(&self) -> BackendProbe {
        OciBackend::probe(self)
    }

    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess> {
        require_enforcement(self, policy)?;
        let runc = self
            .runc
            .clone()
            .ok_or_else(|| SandboxError::Unsupported("runc not found on PATH".to_string()))?;
        let seq = BUNDLE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let bundle = std::env::temp_dir().join(format!("meld-oci-{}-{seq}", policy.fingerprint()));
        build_bundle(&bundle, spec, policy)?;
        let id = format!("meld-{}-{}", policy.fingerprint(), seq);
        let mut command = Command::new(&runc);
        command
            .arg("run")
            .arg("--bundle")
            .arg(&bundle)
            .arg(&id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Denied("stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Denied("stderr pipe missing".to_string()))?;
        let cleanup_runc = runc.clone();
        let cleanup_id = id.clone();
        let cleanup_bundle = bundle.clone();
        Ok(
            SpawnedProcess::new(child, stdout, stderr, IoPolicy::from_limits(&policy.limits))?
                .with_kill_hook(move || {
                    let _ = run_runc(&cleanup_runc, &["kill", &cleanup_id, "SIGKILL"]);
                    let _ = run_runc(&cleanup_runc, &["delete", "--force", &cleanup_id]);
                    let _ = fs::remove_dir_all(&cleanup_bundle);
                }),
        )
    }
}

#[cfg(all(test, feature = "oci"))]
mod tests {
    use super::*;

    #[test]
    fn spec_follows_policy() {
        let spec = CommandSpec::new("/bin/echo", ["ok"]);
        let mut policy = SandboxPolicy::deny_all("oci");
        policy.limits.memory_bytes = Some(32 * 1024 * 1024);
        let config = build_spec(&spec, &policy).unwrap();
        assert!(config
            .process()
            .as_ref()
            .unwrap()
            .no_new_privileges()
            .unwrap());
        assert_eq!(
            config
                .linux()
                .as_ref()
                .unwrap()
                .resources()
                .as_ref()
                .unwrap()
                .memory()
                .as_ref()
                .unwrap()
                .limit()
                .unwrap(),
            32 * 1024 * 1024
        );
    }

    #[test]
    #[ignore = "requires a runc runtime on the host"]
    fn oci_runtime_spawn() {
        let backend = OciBackend::default();
        assert!(backend.probe().available);
        let spec = CommandSpec::new("/bin/echo", ["ok"]);
        let mut child = backend
            .spawn(&spec, &SandboxPolicy::deny_all("runtime"))
            .unwrap();
        assert!(child.read_line().unwrap().is_some());
    }
}
