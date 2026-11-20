//! macOS backend (cfg-gated; COMPILE-ONLY on Linux).
//!
//! Generates a Seatbelt SBPL profile and launches the worker through
//! `/usr/bin/sandbox-exec`. Resource limits use `setrlimit`. There is no
//! runtime verification of this module on Linux.

use crate::backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
use crate::capability::Access;
use crate::error::{Result, SandboxError};
use crate::io::IoPolicy;
use crate::policy::SandboxPolicy;
use crate::process::{CommandSpec, SpawnedProcess};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Escape a path for an SBPL string literal. Only literal argv is used; the
/// host never builds a shell command.
pub fn sbpl_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => continue,
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Generate a deny-by-default SBPL profile for the policy.
pub fn seatbelt_profile(spec: &CommandSpec, policy: &SandboxPolicy) -> Result<String> {
    if !policy.network.hosts.is_empty() {
        return Err(SandboxError::Unsupported(
            "Seatbelt cannot enforce hostname allowlists; use the OCI backend".to_string(),
        ));
    }
    let mut lines = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        format!(
            "(allow process-exec (literal {}))",
            sbpl_quote(&spec.program.to_string_lossy())
        ),
        "(allow process-fork)".to_string(),
        "(allow sysctl-read)".to_string(),
        "(allow mach-lookup (global-name \"com.apple.system.notificationcenter\"))".to_string(),
    ];
    // Read-only base system access needed by dynamic loaders and interpreters.
    lines.push(
        "(allow file-read* (subpath \"/usr\") (subpath \"/System\") (subpath \"/lib\"))"
            .to_string(),
    );
    for root in policy.fs.roots() {
        let path = root.path().to_string_lossy();
        let read = policy
            .fs
            .rules()
            .iter()
            .any(|r| r.root() == root.name() && r.access() == Access::Read);
        let write = policy
            .fs
            .rules()
            .iter()
            .any(|r| r.root() == root.name() && r.access() == Access::Write);
        let exec = policy
            .fs
            .rules()
            .iter()
            .any(|r| r.root() == root.name() && r.access() == Access::Exec);
        if read {
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                sbpl_quote(&path)
            ));
        }
        if write {
            lines.push(format!(
                "(allow file-write* (subpath {}))",
                sbpl_quote(&path)
            ));
        }
        if exec {
            lines.push(format!(
                "(allow file-execute (subpath {}))",
                sbpl_quote(&path)
            ));
        }
    }
    if policy.network.protocols.any() {
        if policy.network.protocols.tcp
            || policy.network.protocols.http
            || policy.network.protocols.https
        {
            lines.push("(allow network-outbound (remote tcp))".to_string());
            lines.push("(allow network-inbound (local tcp))".to_string());
        }
        if policy.network.protocols.udp || policy.network.protocols.dns {
            lines.push("(allow network-outbound (remote udp))".to_string());
        }
        if policy.network.protocols.unix {
            lines.push("(allow network-outbound (remote unix-socket))".to_string());
        }
    } else {
        lines.push("(deny network*)".to_string());
    }
    Ok(lines.join("\n"))
}

fn apply_rlimits(command: &mut Command, policy: &SandboxPolicy) -> Result<()> {
    // pre_exec closures must be 'static, so capture the limit values by copy.
    let limits = policy.limits.clone();
    // SAFETY: pre_exec runs in the child before exec. Use the platform's
    // setrlimit through the standard library's libc dependency.
    unsafe {
        command.pre_exec(move || {
            let mut failed = None;
            if let Some(mem) = limits.memory_bytes {
                if set_limit(libc::RLIMIT_AS, mem) {
                    failed = Some("RLIMIT_AS");
                }
            }
            if let Some(cpu) = limits.cpu_time_ms {
                if set_limit(libc::RLIMIT_CPU, cpu.div_ceil(1000)) {
                    failed = Some("RLIMIT_CPU");
                }
            }
            if let Some(file) = limits.file_size_bytes {
                if set_limit(libc::RLIMIT_FSIZE, file) {
                    failed = Some("RLIMIT_FSIZE");
                }
            }
            if let Some(proc) = limits.processes {
                if set_limit(libc::RLIMIT_NPROC, proc) {
                    failed = Some("RLIMIT_NPROC");
                }
            }
            if let Some(files) = limits.open_files {
                if set_limit(libc::RLIMIT_NOFILE, files) {
                    failed = Some("RLIMIT_NOFILE");
                }
            }
            match failed {
                Some(name) => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{name} failed: {}", std::io::Error::last_os_error()),
                )),
                None => Ok(()),
            }
        });
    }
    Ok(())
}

#[cfg(target_os = "macos")]
unsafe fn set_limit(resource: libc::c_int, value: u64) -> bool {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    libc::setrlimit(resource, &limit) != 0
}

pub struct MacosBackend;

impl MacosBackend {
    pub fn probe(&self) -> BackendProbe {
        let executable = std::path::Path::new("/usr/bin/sandbox-exec");
        let available = executable.is_file();
        let mut notes = vec!["macOS Seatbelt: compile-only on Linux".to_string()];
        if !available {
            notes.push("/usr/bin/sandbox-exec not found".to_string());
        }
        BackendProbe {
            kind: BackendKind::MacosSeatbelt,
            available,
            features: BackendFeatures {
                seatbelt: available,
                rlimits: true,
                process_group_kill: true,
                ..Default::default()
            },
            notes,
        }
    }
}

impl SandboxBackend for MacosBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::MacosSeatbelt
    }

    fn probe(&self) -> BackendProbe {
        MacosBackend::probe(self)
    }

    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess> {
        require_enforcement(self, policy)?;
        let profile = seatbelt_profile(spec, policy)?;
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .arg("-p")
            .arg(profile)
            .arg("--")
            .arg(&spec.program)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear();
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::configure_process_group(&mut command);
        apply_rlimits(&mut command, policy)?;
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Denied("stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Denied("stderr pipe missing".to_string()))?;
        SpawnedProcess::new(child, stdout, stderr, IoPolicy::from_limits(&policy.limits))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn profile_denies_network_by_default() {
        let spec = CommandSpec::new("/usr/bin/python3", ["-u"]);
        let policy = SandboxPolicy::deny_all("test");
        let profile = seatbelt_profile(&spec, &policy).unwrap();
        assert!(profile.starts_with("(version 1)\n(deny default)"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(literal \"/usr/bin/python3\")"));
    }

    #[test]
    fn profile_quotes_paths() {
        assert_eq!(sbpl_quote("/tmp/a\"b"), "\"/tmp/a\\\"b\"");
    }

    #[test]
    #[ignore = "runtime test only on a macOS host with sandbox-exec"]
    fn seatbelt_runtime_spawn() {
        let backend = MacosBackend;
        let probe = backend.probe();
        assert!(probe.available, "{}", probe.notes.join("; "));
        let spec = CommandSpec::new("/bin/echo", ["ok"]);
        let mut child = backend
            .spawn(&spec, &SandboxPolicy::deny_all("runtime"))
            .unwrap();
        assert_eq!(child.read_line().unwrap().as_deref(), Some("ok"));
    }
}
