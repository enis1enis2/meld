//! Linux backend: namespaces + seccomp-bpf (seccompiler), cgroup v2 writes,
//! best-effort Landlock, rlimits fallback and process-group kill.
//!
//! HONESTY NOTE: Landlock is never claimed as enforced by `probe()`. On hosts
//! where the syscall returns ENOSYS (including the development host) we report
//! it as unavailable and rely on mount namespaces/seccomp. No Linux runtime
//! test in this crate assumes namespaces or cgroups are writable.

use crate::backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
use crate::capability::{Access, ResourceLimits};
use crate::error::{Result, SandboxError};
use crate::policy::SandboxPolicy;
use crate::process::{CommandSpec, SpawnedProcess};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_FILE: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;

#[derive(Clone, Debug)]
pub struct LinuxBackend {
    /// Overridable so cgroup writes can be unit-tested with a temp directory.
    pub cgroup_root: PathBuf,
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self {
            cgroup_root: std::env::var_os("MELD_CGROUP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup")),
        }
    }
}

/// Query Landlock ABI without enforcing anything. `Ok(None)` means ENOSYS or
/// an unsupported ABI: never report Landlock as enforced in that case.
pub fn query_landlock() -> Result<Option<u32>> {
    // SAFETY: version query with a null attribute pointer and the VERSION flag
    // is the documented way to discover support; it has no side effects.
    let version = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<libc::c_void>(),
            0,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if version < 0 {
        let err = io::Error::last_os_error();
        if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) {
            return Ok(None);
        }
        return Err(SandboxError::Unsupported(format!(
            "Landlock version query failed: {err}"
        )));
    }
    Ok(Some(version as u32))
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn supports_unprivileged_userns() -> bool {
    match read_trimmed(Path::new("/proc/sys/kernel/unprivileged_userns_clone")) {
        Some(value) => value == "1",
        // Sysctl absent on many kernels; that does not mean disabled.
        None => Path::new("/proc/self/ns/user").exists(),
    }
}

fn seccomp_supported() -> bool {
    // SAFETY: PR_GET_SECCOMP only reads the calling thread's mode.
    let result = unsafe { libc::prctl(libc::PR_GET_SECCOMP, 0, 0, 0, 0) };
    result >= 0
}

impl LinuxBackend {
    pub fn probe(&self) -> BackendProbe {
        let mut notes = Vec::new();
        let landlock = match query_landlock() {
            Ok(Some(version)) => {
                notes.push(format!(
                    "Landlock ABI {version} present; not used as authority"
                ));
                true
            }
            Ok(None) => {
                notes.push("Landlock syscall ENOSYS/unsupported: not enforced".to_string());
                false
            }
            Err(e) => {
                notes.push(e.to_string());
                false
            }
        };

        let controllers =
            read_trimmed(&self.cgroup_root.join("cgroup.controllers")).unwrap_or_default();
        let cgroup_v2 = controllers.contains("memory") && controllers.contains("pids");
        if !cgroup_v2 {
            notes.push(format!(
                "cgroup v2 memory/pids controllers not available at {}",
                self.cgroup_root.display()
            ));
        }

        let userns = supports_unprivileged_userns();
        let namespaces_present = Path::new("/proc/self/ns/mnt").exists()
            && Path::new("/proc/self/ns/net").exists()
            && userns;
        if !namespaces_present {
            notes.push("required Linux namespaces not present/unprivileged".to_string());
        }
        if !seccomp_supported() {
            notes.push("seccomp not available".to_string());
        }

        let features = BackendFeatures {
            user_namespace: userns,
            mount_namespace: namespaces_present,
            pid_namespace: namespaces_present,
            network_namespace: namespaces_present,
            ipc_namespace: namespaces_present,
            // Probe checks kernel support; a failed installation in pre_exec
            // aborts spawn, so we never run unconfined.
            seccomp: seccomp_supported(),
            cgroup_v2,
            rlimits: true,
            landlock_available: landlock,
            // Deliberately false: only a successful in-child installation can
            // set this, and probe cannot make that claim.
            landlock_enforced: false,
            process_group_kill: true,
            seatbelt: false,
            job_object: false,
            appcontainer: false,
        };
        BackendProbe {
            kind: BackendKind::LinuxNamespaces,
            available: namespaces_present || features.seccomp,
            features,
            notes,
        }
    }

    /// Apply best-effort Landlock. Returns `Ok(false)` when the kernel does
    /// not implement it; a caller must not treat that as enforcement.
    pub fn apply_landlock_best_effort(&self, allowed: &[PathBuf]) -> Result<bool> {
        if query_landlock()?.is_none() {
            return Ok(false);
        }
        let handled = LANDLOCK_ACCESS_FS_EXECUTE
            | LANDLOCK_ACCESS_FS_WRITE_FILE
            | LANDLOCK_ACCESS_FS_READ_FILE
            | LANDLOCK_ACCESS_FS_READ_DIR
            | LANDLOCK_ACCESS_FS_MAKE_DIR
            | LANDLOCK_ACCESS_FS_MAKE_FILE
            | LANDLOCK_ACCESS_FS_REMOVE_DIR
            | LANDLOCK_ACCESS_FS_REMOVE_FILE;
        let ruleset_attr = LandlockRulesetAttr {
            handled_access_fs: handled,
        };
        // SAFETY: attr pointer and size match the kernel ABI for ABI v1.
        let fd = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &ruleset_attr as *const LandlockRulesetAttr,
                std::mem::size_of::<LandlockRulesetAttr>(),
                0u32,
            )
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            if matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EOPNOTSUPP)) {
                return Ok(false);
            }
            return Err(SandboxError::Denied(format!(
                "Landlock ruleset creation failed: {err}"
            )));
        }
        let ruleset_fd = fd as libc::c_int;
        let result = (|| -> Result<()> {
            for path in allowed {
                if !path.is_dir() {
                    continue;
                }
                // SAFETY: O_PATH|O_DIRECTORY opens the directory for add_rule.
                let dir_fd = unsafe {
                    libc::open(
                        path_to_cstring(path)?.as_ptr(),
                        libc::O_PATH | libc::O_CLOEXEC | libc::O_DIRECTORY,
                    )
                };
                if dir_fd < 0 {
                    return Err(SandboxError::Denied(format!(
                        "Landlock cannot open {}: {}",
                        path.display(),
                        io::Error::last_os_error()
                    )));
                }
                let attr = LandlockPathBeneathAttr {
                    allowed_access: handled,
                    parent_fd: dir_fd,
                };
                // SAFETY: attr layout matches landlock_path_beneath_attr v1.
                let rc = unsafe {
                    libc::syscall(
                        SYS_LANDLOCK_ADD_RULE,
                        ruleset_fd,
                        LANDLOCK_RULE_PATH_BENEATH,
                        &attr as *const LandlockPathBeneathAttr,
                        0u32,
                    )
                };
                // SAFETY: close a fd we just opened.
                unsafe { libc::close(dir_fd) };
                if rc < 0 {
                    return Err(SandboxError::Denied(format!(
                        "Landlock add_rule failed: {}",
                        io::Error::last_os_error()
                    )));
                }
            }
            // SAFETY: PR_SET_NO_NEW_PRIVS is required by restrict_self.
            if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
                return Err(SandboxError::Denied(format!(
                    "no_new_privs failed: {}",
                    io::Error::last_os_error()
                )));
            }
            // SAFETY: ruleset_fd is a valid Landlock ruleset fd.
            let rc = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd, 0u32) };
            if rc < 0 {
                return Err(SandboxError::Denied(format!(
                    "Landlock restrict_self failed: {}",
                    io::Error::last_os_error()
                )));
            }
            Ok(())
        })();
        // SAFETY: close the fd we created.
        unsafe { libc::close(ruleset_fd) };
        result?;
        Ok(true)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: libc::c_int,
}

fn path_to_cstring(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SandboxError::Invalid(format!("path has NUL byte: {}", path.display())))
}

fn rule_map(policy: &SandboxPolicy) -> Result<BTreeMap<i64, Vec<seccompiler::SeccompRule>>> {
    let mut rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
    // Minimal startup/runtime baseline. Deliberately does not include
    // network, process creation, ptrace, keyctl, mount or module syscalls.
    let baseline: &[libc::c_long] = &[
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_newfstatat,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_getrandom,
        libc::SYS_futex,
        libc::SYS_clock_gettime,
        libc::SYS_sched_yield,
        libc::SYS_openat,
        libc::SYS_getdents64,
        libc::SYS_faccessat,
        libc::SYS_pipe2,
        libc::SYS_ppoll,
        libc::SYS_dup,
        libc::SYS_dup3,
        libc::SYS_fcntl,
        libc::SYS_prlimit64,
        libc::SYS_uname,
        // execve is needed once for the initial program; process creation is
        // additionally blocked by pids cgroup/rlimits and by omitting clone.
        libc::SYS_execve,
    ];
    for syscall in baseline {
        rules.insert(*syscall as i64, Vec::new());
    }
    for syscall in network_syscalls(policy) {
        rules.insert(syscall, Vec::new());
    }
    Ok(rules)
}

/// Syscalls required for declared network protocols. Empty means deny network.
pub fn network_syscalls(policy: &SandboxPolicy) -> Vec<i64> {
    if !policy.network.protocols.any() {
        return Vec::new();
    }
    let mut out = vec![
        libc::SYS_socket as i64,
        libc::SYS_connect as i64,
        libc::SYS_getsockopt as i64,
        libc::SYS_setsockopt as i64,
        libc::SYS_shutdown as i64,
        libc::SYS_recvfrom as i64,
        libc::SYS_sendto as i64,
    ];
    if policy.network.protocols.unix
        || policy.network.protocols.http
        || policy.network.protocols.https
    {
        out.push(libc::SYS_socketpair as i64);
    }
    out
}

/// Compile the seccomp-bpf program. Default action is `EPERM`: any syscall not
/// explicitly allowlisted is denied.
pub fn compile_seccomp(policy: &SandboxPolicy) -> Result<BpfProgram> {
    let arch: TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(|_| SandboxError::Unsupported("unsupported seccomp architecture".to_string()))?;
    let filter = SeccompFilter::new(
        rule_map(policy)?,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch,
    )
    .map_err(|e| SandboxError::Unsupported(format!("seccomp filter: {e}")))?;
    filter
        .try_into()
        .map_err(|e| SandboxError::Unsupported(format!("seccomp BPF compile: {e}")))
}

/// Build (but do not apply) the rlimits that will be installed in pre_exec.
pub fn rlimits_for(limits: &ResourceLimits) -> Vec<(i32, libc::rlim_t)> {
    let mut out = Vec::new();
    if let Some(mem) = limits.memory_bytes {
        out.push((libc::RLIMIT_AS as i32, mem as libc::rlim_t));
    }
    if let Some(cpu) = limits.cpu_time_ms {
        // RLIMIT_CPU is whole seconds; round up so a declared 1ms cap never
        // becomes more than 1s of CPU. cgroup v2 is preferred when available.
        out.push((libc::RLIMIT_CPU as i32, cpu.div_ceil(1000) as libc::rlim_t));
    }
    if let Some(file) = limits.file_size_bytes {
        out.push((libc::RLIMIT_FSIZE as i32, file as libc::rlim_t));
    }
    if let Some(proc) = limits.processes {
        out.push((libc::RLIMIT_NPROC as i32, proc as libc::rlim_t));
    }
    if let Some(files) = limits.open_files {
        out.push((libc::RLIMIT_NOFILE as i32, files as libc::rlim_t));
    }
    if let Some(stack) = limits.stack_bytes {
        out.push((libc::RLIMIT_STACK as i32, stack as libc::rlim_t));
    }
    out
}

fn apply_rlimits(limits: &ResourceLimits) -> Result<()> {
    for (resource, value) in rlimits_for(limits) {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // SAFETY: valid resource id and rlimit struct in the child process.
        if unsafe { libc::setrlimit(resource as _, &limit) } != 0 {
            return Err(SandboxError::Denied(format!(
                "setrlimit({resource}) failed: {}",
                io::Error::last_os_error()
            )));
        }
    }
    Ok(())
}

/// Write cgroup v2 limits for a group. `root` is injectable for tests.
pub fn write_cgroup_limits(root: &Path, group: &str, limits: &ResourceLimits) -> Result<()> {
    if !group
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || group.is_empty()
    {
        return Err(SandboxError::Invalid(format!(
            "unsafe cgroup name: {group:?}"
        )));
    }
    let dir = root.join(group);
    fs::create_dir_all(&dir)?;
    let mut written = Vec::new();
    let write = |name: &str, value: String, written: &mut Vec<String>| -> Result<()> {
        fs::write(dir.join(name), value.as_bytes())?;
        written.push(name.to_string());
        Ok(())
    };
    let result = (|| -> Result<()> {
        if let Some(mem) = limits.memory_bytes {
            write("memory.max", mem.to_string(), &mut written)?;
            write("memory.swap.max", "0".to_string(), &mut written)?;
        }
        if let Some(cpu) = limits.cpu_time_ms {
            // cpu.max: quota (us) per 100000us period. A short wall-clock job
            // gets an equivalent burst quota; rlimits cover the hard CPU cap.
            write(
                "cpu.max",
                format!("{} 100000", cpu.saturating_mul(1000).max(1000)),
                &mut written,
            )?;
        }
        if let Some(pids) = limits.processes {
            write("pids.max", pids.to_string(), &mut written)?;
        }
        Ok(())
    })();
    if result.is_err() {
        for name in written {
            let _ = fs::remove_file(dir.join(name));
        }
        let _ = fs::remove_dir(&dir);
    }
    result
}

pub fn assign_pid_to_cgroup(root: &Path, group: &str, pid: u32) -> Result<()> {
    fs::write(root.join(group).join("cgroup.procs"), pid.to_string())?;
    Ok(())
}

pub fn cleanup_cgroup(root: &Path, group: &str) {
    let _ = fs::remove_dir_all(root.join(group));
}

fn mount_worker_roots(policy: &SandboxPolicy) -> Result<()> {
    // Make all mounts private first so the worker cannot affect the host.
    let c_empty = path_to_cstring(Path::new(""))?;
    let c_slash = path_to_cstring(Path::new("/"))?;
    // SAFETY: standard recursive private mount in the child's new mount ns.
    if unsafe {
        libc::mount(
            c_empty.as_ptr(),
            c_slash.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    } != 0
    {
        return Err(io_error("mount private"));
    }
    for root in policy.fs.roots() {
        let host = path_to_cstring(root.path())?;
        // SAFETY: bind-mount a canonical safe root onto itself in the child ns.
        if unsafe {
            libc::mount(
                host.as_ptr(),
                host.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            )
        } != 0
        {
            return Err(io_error("bind mount"));
        }
        let writable = policy
            .fs
            .rules()
            .iter()
            .any(|r| r.root() == root.name() && r.access() == Access::Write);
        if !writable {
            // SAFETY: remount the same path read-only inside the child ns.
            if unsafe {
                libc::mount(
                    c_empty.as_ptr(),
                    host.as_ptr(),
                    std::ptr::null(),
                    libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY,
                    std::ptr::null(),
                )
            } != 0
            {
                return Err(io_error("read-only remount"));
            }
        }
    }
    Ok(())
}

fn io_error(operation: &str) -> SandboxError {
    SandboxError::Denied(format!(
        "{operation} failed in sandbox setup: {}",
        io::Error::last_os_error()
    ))
}

fn child_setup(
    policy: &SandboxPolicy,
    filter: Arc<BpfProgram>,
    backend: LinuxBackend,
) -> Result<()> {
    let flags = libc::CLONE_NEWUSER
        | libc::CLONE_NEWNS
        | libc::CLONE_NEWPID
        | libc::CLONE_NEWNET
        | libc::CLONE_NEWIPC
        | libc::CLONE_NEWUTS;
    // SAFETY: called in pre_exec, before exec, in a single-threaded child.
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(io_error("unshare"));
    }
    // Map the child's uid/gid to the host's so file ownership stays usable
    // inside the user namespace. Failure aborts the spawn.
    fs::write("/proc/self/setgroups", b"deny")?;
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    fs::write("/proc/self/uid_map", format!("0 {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("0 {gid} 1"))?;

    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(io_error("no_new_privs"));
    }
    let allowed: Vec<PathBuf> = policy.fs.roots().map(|r| r.path().to_path_buf()).collect();
    let _landlock_enforced = backend.apply_landlock_best_effort(&allowed)?;
    mount_worker_roots(policy)?;
    apply_rlimits(&policy.limits)?;
    // SAFETY: filter is a compiled BPF program kept alive by the Arc.
    seccompiler::apply_filter(&filter)
        .map_err(|e| SandboxError::Denied(format!("seccomp install failed: {e}")))?;
    Ok(())
}

impl SandboxBackend for LinuxBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::LinuxNamespaces
    }

    fn probe(&self) -> BackendProbe {
        self.probe()
    }

    fn spawn(&self, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<SpawnedProcess> {
        require_enforcement(self, policy)?;
        let filter = Arc::new(compile_seccomp(policy)?);
        let mut command = spec.to_command();
        let backend = self.clone();
        let child_policy = policy.clone();
        // SAFETY: pre_exec runs in the forked child before exec; all captured
        // values are owned and no non-async-signal-safe Rust allocation occurs
        // after the first syscall. io::Error construction is the only path out.
        unsafe {
            command.pre_exec(move || {
                child_setup(&child_policy, Arc::clone(&filter), backend.clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))
            });
        }
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Denied("stdout pipe missing".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Denied("stderr pipe missing".to_string()))?;
        let io = crate::io::IoPolicy::from_limits(&policy.limits);
        let mut spawned = SpawnedProcess::new(child, stdout, stderr, io)?;
        if self.probe().features.cgroup_v2 {
            let group = format!("meld-{}", spawned.pid());
            if let Err(e) = write_cgroup_limits(&self.cgroup_root, &group, &policy.limits)
                .and_then(|_| assign_pid_to_cgroup(&self.cgroup_root, &group, spawned.pid()))
            {
                spawned.kill_tree()?;
                cleanup_cgroup(&self.cgroup_root, &group);
                return Err(e);
            }
        }
        Ok(spawned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cgroup(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "meld-sandbox-cgroup-{label}-{}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).ok();
        }
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn seccomp_filter_is_deny_by_default_and_network_is_opt_in() {
        let deny = SandboxPolicy::deny_all("deny");
        assert!(network_syscalls(&deny).is_empty());
        assert!(!compile_seccomp(&deny).unwrap().is_empty());

        let mut allow = deny.clone();
        allow.network.protocols = crate::capability::ProtocolCapability::parse("dns").unwrap();
        allow
            .capabilities
            .allow(crate::capability::Capability::Network);
        assert!(network_syscalls(&allow).contains(&(libc::SYS_socket as i64)));
        assert!(!compile_seccomp(&allow).unwrap().is_empty());
    }

    #[test]
    fn cgroup_writes_are_bounded_and_reject_unsafe_names() {
        let root = temp_cgroup("write");
        let limits = ResourceLimits {
            memory_bytes: Some(64 * 1024 * 1024),
            cpu_time_ms: Some(2000),
            processes: Some(4),
            ..Default::default()
        };
        write_cgroup_limits(&root, "meld-test", &limits).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("meld-test/memory.max")).unwrap(),
            "67108864"
        );
        assert_eq!(
            fs::read_to_string(root.join("meld-test/pids.max")).unwrap(),
            "4"
        );
        assert!(write_cgroup_limits(&root, "../escape", &limits).is_err());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn probe_never_claims_landlock_enforcement() {
        let probe = LinuxBackend::default().probe();
        assert!(!probe.features.landlock_enforced);
        assert!(probe
            .notes
            .iter()
            .any(|n| n.contains("Landlock") || n.contains("landlock")));
    }

    #[test]
    fn rlimits_are_derived_without_applying_them() {
        let limits = ResourceLimits {
            memory_bytes: Some(1024),
            cpu_time_ms: Some(1),
            file_size_bytes: Some(10),
            processes: Some(2),
            open_files: Some(8),
            stack_bytes: Some(4096),
            ..Default::default()
        };
        let values = rlimits_for(&limits);
        assert!(values.contains(&(libc::RLIMIT_AS as i32, 1024)));
        assert!(values.contains(&(libc::RLIMIT_CPU as i32, 1)));
    }

    #[test]
    #[ignore = "requires namespace privileges and writable cgroup v2; not available in CI"]
    fn runtime_namespace_spawn_is_optional() {
        let backend = LinuxBackend::default();
        let probe = backend.probe();
        assert!(probe.available, "{}", probe.notes.join("; "));
        let policy = SandboxPolicy::deny_all("runtime");
        let spec = CommandSpec::new("/bin/echo", ["ok"]);
        let mut child = backend.spawn(&spec, &policy).unwrap();
        assert_eq!(child.read_line().unwrap().as_deref(), Some("ok"));
        assert!(child.wait().unwrap().success());
    }
}
