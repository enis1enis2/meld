//! Optional OCI/runc fallback (`--features oci`): generate an OCI runtime spec from
//! the effective policy and run it with an operator-configured `runc` binary.
//! No shell is ever used; all invocations are argv vectors. The runtime path
//! comes only from absolute `MELD_OCI_RUNC`; PATH is never trusted.
//!
//! The fallback is deliberately strict. It creates fresh mount, network, IPC,
//! UTS, PID and cgroup namespaces; installs a deny-by-default seccomp filter;
//! drops every Linux capability; applies cgroup/rlimit limits; and uses
//! `runc kill --all` for process-tree teardown. A policy that grants network
//! access is rejected: this backend cannot provide a controlled network, so it
//! must not pretend to honour one.

use crate::backend::{
    require_enforcement, BackendFeatures, BackendKind, BackendProbe, SandboxBackend,
};
use crate::capability::Access;
use crate::error::{Result, SandboxError};
use crate::io::IoPolicy;
use crate::linux::{baseline_syscalls, network_syscalls, seccomp_supported};
use crate::policy::SandboxPolicy;
use crate::process::{configure_process_group, CommandSpec, SpawnedProcess};
use oci_spec::runtime::{
    get_default_namespaces, Arch, Capabilities, Linux, LinuxCapabilities, LinuxCpu, LinuxMemory,
    LinuxPids, LinuxResources, LinuxSeccomp, LinuxSeccompAction, LinuxSyscall, Mount, PosixRlimit,
    PosixRlimitBuilder, PosixRlimitType, Process, Root, Spec, User,
};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const RUNC_ENV: &str = "MELD_OCI_RUNC";
const RUNC_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CAPTURE_BYTES: u64 = 64 * 1024;

// Linux capability bits from <linux/capability.h>. Hardcoded so the OCI probe
// does not depend on libc exposing CAP_* constants on every supported target.
const CAP_KILL: u64 = 4;
const CAP_SETGID: u64 = 6;
const CAP_SETUID: u64 = 7;
const CAP_SYS_CHROOT: u64 = 18;
const CAP_SYS_ADMIN: u64 = 21;
const CAP_SYS_RESOURCE: u64 = 24;

/// Resolve the operator-configured OCI runtime.
///
/// JUDGMENT CALL 41: `runc` is trusted code that Meld executes with full host
/// privilege, so a PATH lookup would let a caller-controlled directory become
/// the runtime. The backend accepts only an absolute path from `MELD_OCI_RUNC`,
/// canonicalizes it, and requires a regular executable file. Without that
/// setting the probe fails closed.
pub fn find_runc() -> Option<PathBuf> {
    trusted_runc(&std::env::var_os(RUNC_ENV)?)
}

fn trusted_runc(configured: &OsStr) -> Option<PathBuf> {
    let candidate = PathBuf::from(configured);
    if !candidate.is_absolute() {
        return None;
    }
    let canonical = fs::canonicalize(candidate).ok()?;
    let meta = fs::symlink_metadata(&canonical).ok()?;
    use std::os::unix::fs::PermissionsExt;
    if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
        Some(canonical)
    } else {
        None
    }
}

fn effective_capabilities() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(hex) = line.strip_prefix("CapEff:") {
            return u64::from_str_radix(hex.trim(), 16).ok();
        }
    }
    None
}

fn has_cap(caps: u64, bit: u64) -> bool {
    caps & (1u64 << bit) != 0
}

fn cgroup_v2_writable() -> bool {
    let controllers = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers").unwrap_or_default();
    for required in ["cpu", "memory", "pids"] {
        if !controllers.split_whitespace().any(|name| name == required) {
            return false;
        }
    }
    fs::OpenOptions::new()
        .write(true)
        .open("/sys/fs/cgroup/cgroup.procs")
        .is_ok()
}

fn required_namespaces_present() -> bool {
    [
        "/proc/self/ns/mnt",
        "/proc/self/ns/pid",
        "/proc/self/ns/net",
        "/proc/self/ns/ipc",
        "/proc/self/ns/uts",
        "/proc/self/ns/cgroup",
    ]
    .iter()
    .all(|path| Path::new(path).exists())
}

/// Map a syscall number from the shared baseline to the name libseccomp expects
/// in an OCI seccomp profile.
fn syscall_name(number: i64) -> Result<String> {
    let name = match number {
        libc::SYS_read => "read",
        libc::SYS_write => "write",
        libc::SYS_writev => "writev",
        libc::SYS_close => "close",
        libc::SYS_fstat => "fstat",
        libc::SYS_newfstatat => "newfstatat",
        libc::SYS_lseek => "lseek",
        libc::SYS_mmap => "mmap",
        libc::SYS_mprotect => "mprotect",
        libc::SYS_munmap => "munmap",
        libc::SYS_brk => "brk",
        libc::SYS_rt_sigaction => "rt_sigaction",
        libc::SYS_rt_sigprocmask => "rt_sigprocmask",
        libc::SYS_rt_sigreturn => "rt_sigreturn",
        libc::SYS_exit => "exit",
        libc::SYS_exit_group => "exit_group",
        libc::SYS_getpid => "getpid",
        libc::SYS_gettid => "gettid",
        libc::SYS_getrandom => "getrandom",
        libc::SYS_futex => "futex",
        libc::SYS_clock_gettime => "clock_gettime",
        libc::SYS_sched_yield => "sched_yield",
        libc::SYS_openat => "openat",
        libc::SYS_getdents64 => "getdents64",
        libc::SYS_faccessat => "faccessat",
        libc::SYS_pipe2 => "pipe2",
        libc::SYS_ppoll => "ppoll",
        libc::SYS_dup => "dup",
        libc::SYS_dup3 => "dup3",
        libc::SYS_fcntl => "fcntl",
        libc::SYS_prlimit64 => "prlimit64",
        libc::SYS_uname => "uname",
        libc::SYS_execve => "execve",
        libc::SYS_socket => "socket",
        libc::SYS_connect => "connect",
        libc::SYS_getsockopt => "getsockopt",
        libc::SYS_setsockopt => "setsockopt",
        libc::SYS_shutdown => "shutdown",
        libc::SYS_recvfrom => "recvfrom",
        libc::SYS_sendto => "sendto",
        libc::SYS_socketpair => "socketpair",
        // runc stage-1 and glibc/aarch64 startup need these in addition to
        // the native seccomp baseline. None of them create processes or open
        // network sockets.
        libc::SYS_readlinkat => "readlinkat",
        libc::SYS_fstatfs => "fstatfs",
        libc::SYS_statfs => "statfs",
        libc::SYS_statx => "statx",
        libc::SYS_close_range => "close_range",
        libc::SYS_prctl => "prctl",
        libc::SYS_capget => "capget",
        libc::SYS_sched_getaffinity => "sched_getaffinity",
        libc::SYS_getcwd => "getcwd",
        libc::SYS_set_tid_address => "set_tid_address",
        libc::SYS_set_robust_list => "set_robust_list",
        libc::SYS_rseq => "rseq",
        libc::SYS_sigaltstack => "sigaltstack",
        libc::SYS_pread64 => "pread64",
        libc::SYS_faccessat2 => "faccessat2",
        libc::SYS_openat2 => "openat2",
        libc::SYS_mremap => "mremap",
        libc::SYS_madvise => "madvise",
        libc::SYS_setgroups => "setgroups",
        libc::SYS_getgroups => "getgroups",
        libc::SYS_setresuid => "setresuid",
        libc::SYS_setresgid => "setresgid",
        libc::SYS_setuid => "setuid",
        libc::SYS_setgid => "setgid",
        libc::SYS_getuid => "getuid",
        libc::SYS_getgid => "getgid",
        libc::SYS_geteuid => "geteuid",
        libc::SYS_getegid => "getegid",
        #[cfg(target_arch = "x86_64")]
        libc::SYS_arch_prctl => "arch_prctl",
        other => {
            return Err(SandboxError::Unsupported(format!(
                "no OCI seccomp name for syscall {other}"
            )))
        }
    };
    Ok(name.to_string())
}

/// Extra syscalls needed by the OCI runtime's own init phase and by the
/// dynamic loader on aarch64. This list never includes process creation,
/// mount, ptrace, keyctl, module or network syscalls.
fn oci_runtime_syscalls() -> Vec<i64> {
    vec![
        libc::SYS_readlinkat,
        libc::SYS_fstatfs,
        libc::SYS_statfs,
        libc::SYS_statx,
        libc::SYS_close_range,
        libc::SYS_prctl,
        libc::SYS_capget,
        libc::SYS_sched_getaffinity,
        libc::SYS_getcwd,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_sigaltstack,
        libc::SYS_pread64,
        libc::SYS_faccessat2,
        libc::SYS_openat2,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_setgroups,
        libc::SYS_getgroups,
        libc::SYS_setresuid,
        libc::SYS_setresgid,
        libc::SYS_setuid,
        libc::SYS_setgid,
        libc::SYS_getuid,
        libc::SYS_getgid,
        libc::SYS_geteuid,
        libc::SYS_getegid,
    ]
}

fn allowed_syscalls(policy: &SandboxPolicy) -> Result<Vec<String>> {
    let mut numbers = baseline_syscalls();
    numbers.extend(network_syscalls(policy));
    numbers.extend(oci_runtime_syscalls());
    numbers.sort_unstable();
    numbers.dedup();
    numbers.into_iter().map(syscall_name).collect()
}

fn rlimit(typ: PosixRlimitType, value: u64) -> PosixRlimit {
    PosixRlimitBuilder::default()
        .typ(typ)
        .hard(value)
        .soft(value)
        .build()
        .expect("fixed rlimit fields are always valid")
}

fn process_rlimits(policy: &SandboxPolicy) -> Vec<PosixRlimit> {
    let limits = &policy.limits;
    let mut out = Vec::new();
    if let Some(memory) = limits.memory_bytes {
        out.push(rlimit(PosixRlimitType::RlimitAs, memory));
    }
    if let Some(cpu) = limits.cpu_time_ms {
        out.push(rlimit(PosixRlimitType::RlimitCpu, cpu.div_ceil(1000)));
    }
    if let Some(file) = limits.file_size_bytes {
        out.push(rlimit(PosixRlimitType::RlimitFsize, file));
    }
    if let Some(processes) = limits.processes {
        out.push(rlimit(PosixRlimitType::RlimitNproc, processes));
    }
    if let Some(open_files) = limits.open_files {
        out.push(rlimit(PosixRlimitType::RlimitNofile, open_files));
    }
    if let Some(stack) = limits.stack_bytes {
        out.push(rlimit(PosixRlimitType::RlimitStack, stack));
    }
    out
}

/// Resolve the container working directory. The OCI rootfs is empty except for
/// the mounts derived from the policy, so a host cwd that is not granted to the
/// workload would either not exist or silently change execution semantics.
/// JUDGMENT CALL: fail closed instead of remapping an ungranted cwd to `/`;
/// callers must pass a cwd inside a declared root, `/`, or the tmpfs `/tmp`.
fn container_cwd(spec: &CommandSpec, policy: &SandboxPolicy) -> Result<PathBuf> {
    let declared = if spec.cwd.is_absolute() {
        spec.cwd.clone()
    } else {
        std::env::current_dir()?.join(&spec.cwd)
    };
    let cwd = fs::canonicalize(&declared).map_err(|e| {
        SandboxError::Invalid(format!(
            "OCI cwd {} cannot be resolved: {e}",
            declared.display()
        ))
    })?;
    if cwd == Path::new("/") || cwd == Path::new("/tmp") {
        return Ok(cwd);
    }
    if policy.fs.roots().any(|root| cwd.starts_with(root.path())) {
        return Ok(cwd);
    }
    Err(SandboxError::Invalid(format!(
        "OCI cwd {} is not mounted by the effective policy; pass a cwd inside a declared root",
        cwd.display()
    )))
}

fn mount(destination: &str, kind: &str, source: &str, options: &[&str]) -> Mount {
    let mut entry = Mount::default();
    entry.set_destination(PathBuf::from(destination));
    entry.set_typ(Some(kind.to_string()));
    entry.set_source(Some(PathBuf::from(source)));
    entry.set_options(Some(
        options.iter().map(|value| value.to_string()).collect(),
    ));
    entry
}

/// Mount plan. The bundle rootfs is empty except for read-only system binaries
/// and the exact project roots named by the effective policy, so an unrelated
/// host file cannot be read merely because it exists on the host.
fn mounts_for(policy: &SandboxPolicy) -> Result<Vec<Mount>> {
    let mut mounts = vec![
        mount("/proc", "proc", "proc", &["nosuid", "noexec", "nodev"]),
        mount(
            "/dev",
            "tmpfs",
            "tmpfs",
            &["nosuid", "strictatime", "mode=755", "size=65536k"],
        ),
        // Bind the four stdio devices instead of asking runc to mknod them:
        // the final process has no device-creation capability.
        mount("/dev/null", "bind", "/dev/null", &["bind"]),
        mount("/dev/zero", "bind", "/dev/zero", &["bind"]),
        mount("/dev/random", "bind", "/dev/random", &["bind"]),
        mount("/dev/urandom", "bind", "/dev/urandom", &["bind"]),
        mount(
            "/tmp",
            "tmpfs",
            "tmpfs",
            &["nosuid", "noexec", "nodev", "mode=1777", "size=65536k"],
        ),
    ];

    for dir in [
        "/bin",
        "/usr/bin",
        "/lib",
        "/lib64",
        "/usr/lib",
        "/usr/local/bin",
        "/usr/local/lib",
    ] {
        let host = Path::new(dir);
        if !host.exists() {
            continue;
        }
        let canonical = fs::canonicalize(host).map_err(|e| {
            SandboxError::Invalid(format!("cannot canonicalize OCI mount source {dir}: {e}"))
        })?;
        mounts.push(mount(
            dir,
            "bind",
            &canonical.to_string_lossy(),
            &["rbind", "ro", "nosuid", "nodev"],
        ));
    }

    for root in policy.fs.roots() {
        let writable = policy
            .fs
            .rules()
            .iter()
            .any(|rule| rule.root() == root.name() && rule.access() == Access::Write);
        let mut options = vec!["rbind"];
        options.push(if writable { "rw" } else { "ro" });
        options.extend(["nosuid", "nodev"]);
        mounts.push(mount(
            &root.path().to_string_lossy(),
            "bind",
            &root.path().to_string_lossy(),
            &options,
        ));
    }
    Ok(mounts)
}

/// Build an OCI spec that follows the effective policy. Network grants are
/// refused because the fallback always creates an empty network namespace.
pub fn build_spec(spec: &CommandSpec, policy: &SandboxPolicy) -> Result<Spec> {
    if policy.network.protocols.any() {
        return Err(SandboxError::Unsupported(
            "OCI fallback cannot grant network access; use the native backend".to_string(),
        ));
    }

    let mut config = Spec::default();
    config.set_version("1.0.2".to_string());
    config.set_hostname(Some("meld".to_string()));

    let mut root = Root::default();
    root.set_path(PathBuf::from("rootfs"));
    root.set_readonly(Some(true));
    config.set_root(Some(root));

    let mut process = Process::default();
    process.set_terminal(Some(false));
    process.set_args(Some(spec.argv()));
    process.set_cwd(container_cwd(spec, policy)?);
    // Run at the caller's uid/gid instead of container root. There is no user
    // namespace here, so uid 0 would be host root even with empty capabilities.
    // SAFETY: geteuid/getegid only read the calling process identity.
    let mut user = User::default();
    user.set_uid(unsafe { libc::geteuid() });
    user.set_gid(unsafe { libc::getegid() });
    process.set_user(user);
    // JUDGMENT CALL: no_new_privileges stays enabled. Early runc failures were
    // caused by an incomplete syscall allowlist and missing stdio mount points,
    // not by NNP. Once those were fixed, runc 1.1.15 started the process with
    // NNP set; disabling it would weaken the fallback for no benefit.
    process.set_no_new_privileges(Some(true));
    let env: Vec<String> = spec
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    process.set_env(Some(env));
    process.set_rlimits(Some(process_rlimits(policy)));

    let empty: Capabilities = Capabilities::new();
    let mut capabilities = LinuxCapabilities::default();
    capabilities.set_bounding(Some(empty.clone()));
    capabilities.set_effective(Some(empty.clone()));
    capabilities.set_inheritable(Some(empty.clone()));
    capabilities.set_permitted(Some(empty.clone()));
    capabilities.set_ambient(Some(empty));
    process.set_capabilities(Some(capabilities));
    config.set_process(Some(process));

    let mut resources = LinuxResources::default();
    if let Some(mem) = policy.limits.memory_bytes {
        let mut memory = LinuxMemory::default();
        memory.set_limit(Some(mem as i64));
        memory.set_swap(Some(0));
        resources.set_memory(Some(memory));
    }
    if let Some(cpu_ms) = policy.limits.cpu_time_ms {
        let mut cpu = LinuxCpu::default();
        cpu.set_quota(Some(cpu_ms as i64 * 1000));
        cpu.set_period(Some(1_000_000));
        resources.set_cpu(Some(cpu));
    }
    if let Some(pids) = policy.limits.processes {
        let mut pids_limit = LinuxPids::default();
        pids_limit.set_limit(pids as i64);
        resources.set_pids(Some(pids_limit));
    }

    let mut seccomp = LinuxSeccomp::default();
    seccomp.set_default_action(LinuxSeccompAction::ScmpActErrno);
    seccomp.set_default_errno_ret(Some(libc::EPERM as u32));
    #[cfg(target_arch = "aarch64")]
    let seccomp_arch = Arch::ScmpArchAarch64;
    #[cfg(target_arch = "x86_64")]
    let seccomp_arch = Arch::ScmpArchX86_64;
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    let seccomp_arch = Arch::ScmpArchNative;
    seccomp.set_architectures(Some(vec![seccomp_arch]));
    let mut allowed = LinuxSyscall::default();
    allowed.set_names(allowed_syscalls(policy)?);
    allowed.set_action(LinuxSeccompAction::ScmpActAllow);
    seccomp.set_syscalls(Some(vec![allowed]));

    let mut linux = Linux::default();
    // Do not depend on the current oci-spec default: the policy requires a
    // fresh network namespace, and the probe attests each namespace below.
    linux.set_namespaces(Some(get_default_namespaces()));
    linux.set_resources(Some(resources));
    linux.set_seccomp(Some(seccomp));
    config.set_linux(Some(linux));
    config.set_mounts(Some(mounts_for(policy)?));

    let annotations = config
        .annotations_mut()
        .get_or_insert_with(Default::default);
    annotations.insert(
        "meld.sandbox/network".to_string(),
        "denied-by-empty-netns".to_string(),
    );
    annotations.insert(
        "meld.sandbox/fs-roots".to_string(),
        policy
            .fs
            .roots()
            .map(|root| root.path().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(","),
    );
    annotations.insert(
        "meld.sandbox/fingerprint".to_string(),
        policy.fingerprint().to_string(),
    );
    Ok(config)
}

pub fn build_bundle(dir: &Path, spec: &CommandSpec, policy: &SandboxPolicy) -> Result<()> {
    let rootfs = dir.join("rootfs");
    fs::create_dir_all(&rootfs)?;
    for entry in mounts_for(policy)? {
        let relative = entry
            .destination()
            .strip_prefix("/")
            .unwrap_or_else(|_| entry.destination().as_path());
        let destination = rootfs.join(relative);
        if destination.exists() {
            continue;
        }
        let device_file = relative.starts_with("dev/")
            && matches!(
                entry
                    .destination()
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("null" | "zero" | "random" | "urandom")
            );
        if device_file {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            write_new(&destination, b"")?;
        } else {
            fs::create_dir_all(&destination)?;
        }
    }
    let config = build_spec(spec, policy)?;
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| SandboxError::Invalid(format!("OCI spec serialization: {e}")))?;
    write_new(&dir.join("config.json"), json.as_bytes())
}

/// Create a private, non-predictable bundle with exclusive create. A 0700 mode
/// prevents another user from planting symlinks inside it, even when the system
/// temp directory is world-writable.
fn create_bundle_dir() -> Result<PathBuf> {
    let mut entropy = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let token: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("meld-oci-{token}"));
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(&dir)?;
    Ok(dir)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
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

fn read_bounded<R: Read>(reader: R) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = reader.take(MAX_CAPTURE_BYTES + 1).read_to_end(&mut bytes);
    bytes.truncate(MAX_CAPTURE_BYTES as usize);
    bytes
}

fn run_runc_capture(
    runc: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(ExitStatus, String, String)> {
    let mut child = Command::new(runc)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SandboxError::Denied(format!("runc invocation failed: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SandboxError::Denied("runc stdout pipe missing".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SandboxError::Denied("runc stderr pipe missing".to_string()))?;
    let stdout_thread = thread::spawn(move || read_bounded(stdout));
    let stderr_thread = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap_or_default();
                let stderr = stderr_thread.join().unwrap_or_default();
                return Ok((
                    status,
                    String::from_utf8_lossy(&stdout).into_owned(),
                    String::from_utf8_lossy(&stderr).into_owned(),
                ));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(SandboxError::Denied(format!(
                    "runc {} timed out after {:?}",
                    args.join(" "),
                    timeout
                )));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(error.into());
            }
        }
    }
}

fn run_runc(runc: &Path, args: &[&str]) -> Result<()> {
    let (status, stdout, stderr) = run_runc_capture(runc, args, RUNC_TIMEOUT)?;
    if !status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(SandboxError::Denied(format!(
            "runc {} exited with {}: {}",
            args.join(" "),
            status,
            detail
        )));
    }
    Ok(())
}

fn runc_supports_kill_all(runc: &Path) -> bool {
    match run_runc_capture(runc, &["kill", "--help"], RUNC_TIMEOUT) {
        Ok((status, stdout, stderr)) => {
            status.success() && format!("{stdout}\n{stderr}").contains("--all")
        }
        Err(_) => false,
    }
}

fn teardown(runc: &Path, id: &str, bundle: &Path) -> Result<()> {
    let kill = run_runc(runc, &["kill", "--all", id, "SIGKILL"]);
    let delete = run_runc(runc, &["delete", "--force", id]);
    let remove = fs::remove_dir_all(bundle)
        .map_err(|e| SandboxError::Denied(format!("OCI bundle cleanup failed: {e}")));
    let mut errors = Vec::new();
    for result in [kill, delete, remove] {
        if let Err(error) = result {
            errors.push(error.to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(SandboxError::Denied(format!(
            "OCI teardown failed: {}",
            errors.join("; ")
        )))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MechanismPresence {
    seccomp: bool,
    namespaces: bool,
    cgroup_v2: bool,
    kill_all: bool,
}

impl OciBackend {
    pub fn probe(&self) -> BackendProbe {
        let mut notes = vec![format!(
            "OCI fallback requires {RUNC_ENV} as an absolute path"
        )];
        if self.runc.is_none() {
            notes.push(format!("{RUNC_ENV} is unset, relative or not executable"));
        }
        let capabilities = match effective_capabilities() {
            Some(caps) => {
                for (bit, name) in [
                    (CAP_SYS_ADMIN, "CAP_SYS_ADMIN"),
                    (CAP_SYS_CHROOT, "CAP_SYS_CHROOT"),
                    (CAP_SETUID, "CAP_SETUID"),
                    (CAP_SETGID, "CAP_SETGID"),
                    (CAP_SYS_RESOURCE, "CAP_SYS_RESOURCE"),
                    (CAP_KILL, "CAP_KILL"),
                ] {
                    if !has_cap(caps, bit) {
                        notes.push(format!("missing {name}"));
                    }
                }
                Some(caps)
            }
            None => {
                notes.push("cannot read CapEff from /proc/self/status".to_string());
                None
            }
        };
        let cgroup_v2 = cgroup_v2_writable();
        if !cgroup_v2 {
            notes.push("writable cgroup v2 cpu/memory/pids controllers not found".to_string());
        }
        let seccomp = seccomp_supported();
        if !seccomp {
            notes.push("seccomp not available".to_string());
        }
        let namespaces = required_namespaces_present();
        if !namespaces {
            notes.push("required OCI namespaces not present".to_string());
        }
        let kill_all = self
            .runc
            .as_deref()
            .map(runc_supports_kill_all)
            .unwrap_or(false);
        if self.runc.is_some() && !kill_all {
            notes.push("runc kill --all not advertised".to_string());
        }
        let probe = self.probe_with(
            capabilities,
            MechanismPresence {
                seccomp,
                namespaces,
                cgroup_v2,
                kill_all,
            },
            notes,
        );
        if probe.available {
            probe
        } else {
            BackendProbe {
                kind: probe.kind,
                available: probe.available,
                features: probe.features,
                notes: [
                    probe.notes,
                    vec!["OCI fallback failed closed; use a native backend".to_string()],
                ]
                .concat(),
            }
        }
    }

    fn probe_with(
        &self,
        capabilities: Option<u64>,
        mechanisms: MechanismPresence,
        notes: Vec<String>,
    ) -> BackendProbe {
        let privileged = capabilities.is_some_and(|caps| {
            has_cap(caps, CAP_SYS_ADMIN)
                && has_cap(caps, CAP_SYS_CHROOT)
                && has_cap(caps, CAP_SETUID)
                && has_cap(caps, CAP_SETGID)
                && has_cap(caps, CAP_SYS_RESOURCE)
                && has_cap(caps, CAP_KILL)
        });
        let available = self.runc.is_some()
            && privileged
            && mechanisms.seccomp
            && mechanisms.namespaces
            && mechanisms.cgroup_v2
            && mechanisms.kill_all;
        let namespace_admin = available
            && privileged
            && mechanisms.namespaces
            && capabilities.is_some_and(|caps| has_cap(caps, CAP_SYS_ADMIN));
        let features = BackendFeatures {
            mount_namespace: namespace_admin,
            pid_namespace: namespace_admin,
            network_namespace: namespace_admin,
            ipc_namespace: namespace_admin,
            seccomp: available && mechanisms.seccomp,
            cgroup_v2: available && mechanisms.cgroup_v2,
            rlimits: available,
            process_group_kill: available && mechanisms.kill_all,
            ..Default::default()
        };
        BackendProbe {
            kind: BackendKind::OciRunc,
            available,
            features,
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
        let runc = self.runc.clone().ok_or_else(|| {
            SandboxError::Unsupported(format!("{RUNC_ENV} must name an absolute runc path"))
        })?;
        let bundle = create_bundle_dir()?;
        let result = (|| -> Result<SpawnedProcess> {
            build_bundle(&bundle, spec, policy)?;
            let token = bundle
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| SandboxError::Invalid("OCI bundle name is not UTF-8".to_string()))?;
            let id = token.strip_prefix("meld-").unwrap_or(token).to_string();
            let mut command = Command::new(&runc);
            configure_process_group(&mut command);
            command
                .arg("run")
                .arg("--bundle")
                .arg(&bundle)
                .arg(&id)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command
                .spawn()
                .map_err(|e| SandboxError::Denied(format!("runc spawn failed: {e}")))?;
            let stdout = match child.stdout.take() {
                Some(pipe) => pipe,
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SandboxError::Denied("stdout pipe missing".to_string()));
                }
            };
            let stderr = match child.stderr.take() {
                Some(pipe) => pipe,
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SandboxError::Denied("stderr pipe missing".to_string()));
                }
            };
            let spawned = match SpawnedProcess::new(
                child,
                stdout,
                stderr,
                IoPolicy::from_limits(&policy.limits),
            ) {
                Ok(spawned) => spawned,
                Err(error) => {
                    let _ = teardown(&runc, &id, &bundle);
                    return Err(error);
                }
            };
            let cleanup_runc = runc.clone();
            let cleanup_id = id.clone();
            let cleanup_bundle = bundle.clone();
            Ok(spawned
                .with_kill_hook(move || teardown(&cleanup_runc, &cleanup_id, &cleanup_bundle)))
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&bundle);
        }
        result
    }
}

#[cfg(all(test, feature = "oci"))]
mod tests {
    use super::*;
    use crate::capability::SafeRoot;
    use oci_spec::runtime::LinuxNamespaceType;
    use std::os::unix::fs::PermissionsExt;

    fn all_caps() -> u64 {
        (1u64 << CAP_SYS_ADMIN)
            | (1u64 << CAP_SYS_CHROOT)
            | (1u64 << CAP_SETUID)
            | (1u64 << CAP_SETGID)
            | (1u64 << CAP_SYS_RESOURCE)
            | (1u64 << CAP_KILL)
    }

    fn all_mechanisms() -> MechanismPresence {
        MechanismPresence {
            seccomp: true,
            namespaces: true,
            cgroup_v2: true,
            kill_all: true,
        }
    }

    fn test_root(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("meld-oci-spec-{label}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn spec_follows_policy() {
        let spec = CommandSpec::new("/bin/echo", ["ok"]).with_cwd("/");
        let mut policy = SandboxPolicy::deny_all("oci");
        policy.limits.memory_bytes = Some(32 * 1024 * 1024);
        let config = build_spec(&spec, &policy).unwrap();
        let process = config.process().as_ref().unwrap();
        assert_eq!(process.cwd(), Path::new("/"));
        assert!(process.no_new_privileges().unwrap());
        assert_eq!(process.user().uid(), unsafe { libc::geteuid() });
        assert_eq!(process.user().gid(), unsafe { libc::getegid() });
        assert_eq!(
            process
                .capabilities()
                .as_ref()
                .unwrap()
                .effective()
                .as_ref()
                .unwrap()
                .len(),
            0
        );
        assert!(process
            .rlimits()
            .as_ref()
            .unwrap()
            .iter()
            .any(|limit| limit.typ() == PosixRlimitType::RlimitAs));
        let linux = config.linux().as_ref().unwrap();
        let namespaces = linux.namespaces().as_ref().unwrap();
        assert!(namespaces
            .iter()
            .any(|namespace| namespace.typ() == LinuxNamespaceType::Network));
        assert!(namespaces
            .iter()
            .any(|namespace| namespace.typ() == LinuxNamespaceType::Mount));
        let seccomp = linux.seccomp().as_ref().unwrap();
        assert_eq!(seccomp.default_action(), LinuxSeccompAction::ScmpActErrno);
        let allowed = seccomp
            .syscalls()
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .names();
        assert!(allowed.contains(&"write".to_string()));
        for network in ["socket", "connect", "sendto", "recvfrom", "socketpair"] {
            assert!(!allowed.contains(&network.to_string()));
        }
        assert_eq!(
            linux
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
        assert!(config
            .mounts()
            .as_ref()
            .unwrap()
            .iter()
            .any(|entry| entry.destination() == Path::new("/proc")));
    }

    #[test]
    fn read_only_root_is_mounted_read_only() {
        let root = test_root("readonly");
        let spec = CommandSpec::new("/bin/echo", ["ok"]).with_cwd(&root);
        let mut policy = SandboxPolicy::deny_all("oci");
        let safe = SafeRoot::new("code", &root).unwrap();
        policy.fs.add_root(safe).unwrap();
        let config = build_spec(&spec, &policy).unwrap();
        let mount = config
            .mounts()
            .as_ref()
            .unwrap()
            .iter()
            .find(|entry| entry.destination() == root.as_path())
            .unwrap();
        assert!(mount
            .options()
            .as_ref()
            .unwrap()
            .contains(&"ro".to_string()));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cwd_outside_policy_fails_closed() {
        let spec = CommandSpec::new("/bin/echo", ["ok"]).with_cwd("/etc");
        let policy = SandboxPolicy::deny_all("oci");
        assert!(matches!(
            build_spec(&spec, &policy),
            Err(SandboxError::Invalid(_))
        ));
    }

    #[test]
    fn network_grant_fails_closed() {
        let spec = CommandSpec::new("/bin/echo", ["ok"]);
        let mut policy = SandboxPolicy::deny_all("oci");
        policy.network.protocols.dns = true;
        assert!(matches!(
            build_spec(&spec, &policy),
            Err(SandboxError::Unsupported(_))
        ));
    }

    #[test]
    fn trusted_runc_requires_an_absolute_executable() {
        let current = std::env::current_exe().unwrap();
        assert_eq!(
            trusted_runc(current.as_os_str()),
            Some(fs::canonicalize(&current).unwrap())
        );
        assert!(trusted_runc(OsStr::new("runc")).is_none());
    }

    #[test]
    fn bundle_dir_is_private_and_exclusive() {
        let dir = create_bundle_dir().unwrap();
        let meta = fs::metadata(&dir).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn probe_fails_closed_when_any_mechanism_is_missing() {
        let backend = OciBackend {
            runc: Some(PathBuf::from("/bin/true")),
        };
        let full = backend.probe_with(Some(all_caps()), all_mechanisms(), Vec::new());
        assert!(full.available);
        for missing in [
            MechanismPresence {
                seccomp: false,
                ..all_mechanisms()
            },
            MechanismPresence {
                namespaces: false,
                ..all_mechanisms()
            },
            MechanismPresence {
                cgroup_v2: false,
                ..all_mechanisms()
            },
            MechanismPresence {
                kill_all: false,
                ..all_mechanisms()
            },
        ] {
            let probe = backend.probe_with(Some(all_caps()), missing, Vec::new());
            assert!(!probe.available);
            assert!(!probe.features.seccomp);
            assert!(!probe.features.network_namespace);
            assert!(!probe.features.process_group_kill);
        }
        let no_caps = backend.probe_with(None, all_mechanisms(), Vec::new());
        assert!(!no_caps.available);
        assert!(!no_caps.features.mount_namespace);
        assert!(!no_caps.features.cgroup_v2);
        let no_runc = OciBackend { runc: None };
        assert!(
            !no_runc
                .probe_with(Some(all_caps()), all_mechanisms(), Vec::new())
                .available
        );
    }

    #[test]
    fn teardown_failures_are_surfaced_and_bundle_removal_is_attempted() {
        let bundle = create_bundle_dir().unwrap();
        let error = teardown(Path::new("/bin/false"), "probe-id", &bundle).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("kill --all"), "{text}");
        assert!(text.contains("delete --force"), "{text}");
        assert!(!bundle.exists());
    }

    #[test]
    fn runc_invocation_is_bounded() {
        let dir = create_bundle_dir().unwrap();
        let script = dir.join("slow-runc");
        fs::write(&script, b"#!/bin/sh\nsleep 5\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let error = run_runc_capture(
            &script,
            &["kill", "--all", "probe-id", "SIGKILL"],
            Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        fs::remove_dir_all(&dir).unwrap();
    }

    fn privileged_backend() -> OciBackend {
        let configured = std::env::var_os(RUNC_ENV)
            .unwrap_or_else(|| panic!("set {RUNC_ENV} to an absolute runc path"));
        let runc = trusted_runc(&configured)
            .unwrap_or_else(|| panic!("{RUNC_ENV} is not a trusted absolute runc path"));
        let backend = OciBackend { runc: Some(runc) };
        let probe = backend.probe();
        assert!(probe.available, "{}", probe.notes.join("; "));
        backend
    }

    #[test]
    #[ignore = "requires MELD_OCI_RUNC and a privileged runc runtime on the host"]
    fn oci_runtime_spawn() {
        let backend = privileged_backend();
        let spec = CommandSpec::new("/bin/echo", ["ok"]).with_cwd("/");
        let mut child = backend
            .spawn(&spec, &SandboxPolicy::deny_all("runtime"))
            .unwrap();
        let line = child.read_line().unwrap();
        assert_eq!(
            line.as_deref(),
            Some("ok"),
            "runc stderr: {}",
            child.drain_stderr()
        );
        assert!(child.wait().unwrap().success());
    }

    #[test]
    #[ignore = "requires MELD_OCI_RUNC and a privileged runc runtime on the host"]
    fn oci_runtime_confinement_facts() {
        let backend = privileged_backend();
        let policy = SandboxPolicy::deny_all("confinement");
        let host_net = fs::read_link("/proc/self/ns/net").unwrap();

        let mut net = backend
            .spawn(
                &CommandSpec::new("/usr/bin/readlink", ["/proc/self/ns/net"]).with_cwd("/"),
                &policy,
            )
            .unwrap();
        let container_net = net.read_line().unwrap().unwrap();
        assert!(net.wait().unwrap().success(), "{}", net.drain_stderr());
        drop(net);
        assert_ne!(container_net.trim(), host_net.to_string_lossy().trim());

        let mut caps = backend
            .spawn(
                &CommandSpec::new("/usr/bin/grep", ["CapEff:", "/proc/self/status"]).with_cwd("/"),
                &policy,
            )
            .unwrap();
        let cap_line = caps.read_line().unwrap().unwrap();
        assert!(caps.wait().unwrap().success(), "{}", caps.drain_stderr());
        drop(caps);
        assert_eq!(
            cap_line.split_whitespace().nth(1),
            Some("0000000000000000"),
            "container capabilities were not empty"
        );

        let mut seccomp = backend
            .spawn(
                &CommandSpec::new("/usr/bin/grep", ["Seccomp:", "/proc/self/status"]).with_cwd("/"),
                &policy,
            )
            .unwrap();
        let seccomp_line = seccomp.read_line().unwrap().unwrap();
        assert!(
            seccomp.wait().unwrap().success(),
            "{}",
            seccomp.drain_stderr()
        );
        drop(seccomp);
        assert_eq!(
            seccomp_line.split_whitespace().nth(1),
            Some("2"),
            "OCI seccomp filter was not active"
        );

        let mut hidden = backend
            .spawn(
                &CommandSpec::new("/usr/bin/test", ["!", "-e", "/etc/passwd"]).with_cwd("/"),
                &policy,
            )
            .unwrap();
        assert!(
            hidden.wait().unwrap().success(),
            "ungranted host file was visible: {}",
            hidden.drain_stderr()
        );
        drop(hidden);
    }
}
