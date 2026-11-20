//! Process spawning primitives: argv-only commands, process-tree guards and a
//! bounded pump for stdout/stderr.

use crate::error::{Result, SandboxError};
use crate::io::IoPolicy;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// argv-only command description. There is no `shell` field: a Meld program
/// cannot turn a grant into shell text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: BTreeMap::new(),
        }
    }

    pub fn with_cwd(mut self, cwd: impl AsRef<Path>) -> Self {
        self.cwd = cwd.as_ref().to_path_buf();
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Build a `Command` from the argv vector. `program` is never parsed by a
    /// shell; each argument stays a separate OS argument.
    pub fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).current_dir(&self.cwd);
        cmd.env_clear();
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_process_group(&mut cmd);
        cmd
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = vec![self.program.to_string_lossy().into_owned()];
        argv.extend(self.args.iter().cloned());
        argv
    }
}

/// Put the child in its own process group so the whole tree can be killed
/// without hunting descendants. Required for fail-closed timeouts.
#[cfg(unix)]
pub fn configure_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // process_group only sets an attribute before exec; it does not alter the
    // calling process.
    cmd.process_group(0);
}

#[cfg(not(unix))]
pub fn configure_process_group(_cmd: &mut Command) {}

/// RAII guard that always kills the process group on drop.
pub struct ProcessTreeGuard {
    child: Child,
    pid: u32,
    killed: bool,
}

impl ProcessTreeGuard {
    pub fn new(child: Child) -> Self {
        let pid = child.id();
        Self {
            child,
            pid,
            killed: false,
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub fn kill_tree(&mut self) -> Result<()> {
        if self.killed {
            return Ok(());
        }
        self.killed = true;
        kill_process_tree(self.pid)?;
        let _ = self.child.kill();
        Ok(())
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.child.wait().map_err(Into::into)
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        let _ = self.kill_tree();
        let _ = self.child.wait();
    }
}

/// Kill a process group by leader pid. Used by both the guard and the I/O pump
/// threads (which detect an oversized stream before the host can read it).
pub fn kill_process_tree(pid: u32) -> Result<()> {
    if pid == 0 {
        return Ok(());
    }
    #[cfg(unix)]
    {
        // SAFETY: killpg with a positive pgid sends a signal; the child was
        // spawned into its own group by `configure_process_group`.
        let result = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) };
        if result != 0 {
            let err = std::io::Error::last_os_error();
            // ESRCH is not mapped to `NotFound` on every Rust/libc target, so
            // match the raw errno as well. A process group that vanished before
            // teardown is success for fail-closed killing; other errors are not.
            if err.raw_os_error() != Some(libc::ESRCH) && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(err.into());
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
    Ok(())
}

enum Packet {
    Data(Vec<u8>),
    Eof,
    Error(SandboxError),
}

struct Pump {
    rx: Receiver<Packet>,
    overflow: bool,
}

impl Pump {
    fn recv(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        match self.rx.recv_timeout(timeout) {
            Ok(Packet::Data(bytes)) => Ok(Some(bytes)),
            Ok(Packet::Eof) => Ok(None),
            Ok(Packet::Error(e)) => {
                self.overflow = true;
                Err(e)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SandboxError::WorkerIo(
                "I/O pump stopped unexpectedly".to_string(),
            )),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(SandboxError::WorkerIo("worker idle timeout".to_string()))
            }
        }
    }
}

fn spawn_pump<R: Read + Send + 'static>(mut reader: R, max_bytes: u64, pid: u32) -> Pump {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("meld-sandbox-io".to_string())
        .spawn(move || {
            let mut total: u64 = 0;
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => {
                        let _ = tx.send(Packet::Eof);
                        break;
                    }
                    Ok(n) => {
                        total = total.saturating_add(n as u64);
                        if total > max_bytes {
                            let _ = kill_process_tree(pid);
                            let _ = tx.send(Packet::Error(SandboxError::Denied(format!(
                                "stream exceeded {max_bytes} byte cap; process killed"
                            ))));
                            break;
                        }
                        if tx.send(Packet::Data(chunk[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = kill_process_tree(pid);
                        let _ = tx.send(Packet::Error(SandboxError::Io(e)));
                        break;
                    }
                }
            }
        })
        .expect("spawn I/O pump");
    Pump {
        rx,
        overflow: false,
    }
}

/// A spawned process with bounded readers. Every read path enforces the idle
/// deadline, the overall deadline and the byte caps.
pub struct SpawnedProcess {
    guard: ProcessTreeGuard,
    stdout: Pump,
    stderr: Pump,
    io: IoPolicy,
    started: Instant,
    stdout_buf: Vec<u8>,
    stderr_seen: u64,
    stderr_text: String,
    lines_seen: usize,
    /// Extra backend-specific teardown (e.g. Windows Job Object termination).
    kill_hooks: Vec<Arc<dyn Fn() -> crate::error::Result<()> + Send + Sync>>,
    /// Backend resources (Job Objects, AppContainer profiles) kept alive until
    /// the child is reaped.
    keep_alive: Vec<Box<dyn Send + Sync>>,
}

impl SpawnedProcess {
    pub fn new(
        child: Child,
        stdout: ChildStdout,
        stderr: ChildStderr,
        io: IoPolicy,
    ) -> Result<Self> {
        let guard = ProcessTreeGuard::new(child);
        let pid = guard.pid();
        Ok(Self {
            stdout: spawn_pump(stdout, io.max_line_bytes as u64 * 4, pid),
            stderr: spawn_pump(stderr, io.max_stderr_bytes, pid),
            guard,
            io,
            started: Instant::now(),
            stdout_buf: Vec::new(),
            stderr_seen: 0,
            stderr_text: String::new(),
            lines_seen: 0,
            kill_hooks: Vec::new(),
            keep_alive: Vec::new(),
        })
    }

    /// Keep backend-owned resources alive until the child is reaped.
    pub fn with_keep_alive(mut self, value: impl Send + Sync + 'static) -> Self {
        self.keep_alive.push(Box::new(value));
        self
    }

    /// Attach a backend teardown hook. It runs on every `kill_tree` call and
    /// on drop, including the Windows Job Object terminator. Errors are
    /// returned by `kill_tree` so fail-closed teardown cannot be silently
    /// ignored; `Drop` still attempts every hook and discards its own errors.
    pub fn with_kill_hook(
        mut self,
        hook: impl Fn() -> crate::error::Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.kill_hooks.push(Arc::new(hook));
        self
    }

    pub fn pid(&self) -> u32 {
        self.guard.pid()
    }

    pub fn io_policy(&self) -> &IoPolicy {
        &self.io
    }

    fn timeouts(&self) -> Result<(Duration, Duration)> {
        let elapsed = self.started.elapsed();
        let overall = self
            .io
            .overall_timeout
            .checked_sub(elapsed)
            .ok_or_else(|| SandboxError::Denied("worker overall timeout exceeded".to_string()))?;
        Ok((self.io.idle_timeout.min(overall), overall))
    }

    /// Read one bounded line, enforcing idle and overall timeouts. On any
    /// violation the process tree is killed before the error is returned.
    pub fn read_line(&mut self) -> Result<Option<String>> {
        let (idle, overall) = match self.timeouts() {
            Ok(v) => v,
            Err(e) => {
                self.kill_tree()?;
                return Err(e);
            }
        };
        if self.lines_seen >= self.io.max_output_lines {
            self.kill_tree()?;
            return Err(SandboxError::Denied(
                "worker stdout line limit exceeded".to_string(),
            ));
        }
        let deadline = Instant::now() + overall;
        loop {
            if let Some(index) = self.stdout_buf.iter().position(|b| *b == b'\n') {
                let mut line = self.stdout_buf.drain(..=index).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.len() > self.io.max_line_bytes {
                    self.kill_tree()?;
                    return Err(SandboxError::Denied(format!(
                        "line exceeds {} byte limit",
                        self.io.max_line_bytes
                    )));
                }
                let text = std::str::from_utf8(&line)
                    .map_err(|e| SandboxError::Denied(format!("non-UTF-8 protocol line: {e}")))?;
                self.lines_seen += 1;
                return Ok(Some(text.to_string()));
            }
            if self.stdout_buf.len() > self.io.max_line_bytes {
                self.kill_tree()?;
                return Err(SandboxError::Denied(format!(
                    "line exceeds {} byte limit",
                    self.io.max_line_bytes
                )));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.kill_tree()?;
                return Err(SandboxError::Denied(
                    "worker overall timeout exceeded".to_string(),
                ));
            }
            let wait = idle.min(remaining);
            match self.stdout.recv(wait) {
                Ok(Some(bytes)) => self.stdout_buf.extend_from_slice(&bytes),
                Ok(None) => {
                    if self.stdout_buf.is_empty() {
                        return Ok(None);
                    }
                    let line = std::mem::take(&mut self.stdout_buf);
                    let text = std::str::from_utf8(&line).map_err(|e| {
                        SandboxError::Denied(format!("non-UTF-8 protocol line: {e}"))
                    })?;
                    self.lines_seen += 1;
                    return Ok(Some(text.to_string()));
                }
                Err(e) => {
                    self.kill_tree()?;
                    return Err(e);
                }
            }
        }
    }

    /// Best-effort, bounded look at captured stderr. It never blocks.
    pub fn drain_stderr(&mut self) -> String {
        while let Ok(Packet::Data(bytes)) = self.stderr.rx.try_recv() {
            if self.stderr_seen < self.io.max_stderr_bytes {
                let room = (self.io.max_stderr_bytes - self.stderr_seen) as usize;
                let take = room.min(bytes.len());
                self.stderr_text
                    .push_str(&String::from_utf8_lossy(&bytes[..take]));
                self.stderr_seen += take as u64;
            }
        }
        self.stderr_text.clone()
    }

    pub fn stderr_snapshot(&self) -> &str {
        &self.stderr_text
    }

    pub fn kill_tree(&mut self) -> Result<()> {
        let mut result = self.guard.kill_tree();
        for hook in &self.kill_hooks {
            if let Err(error) = hook() {
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        result
    }

    pub fn write_stdin(&mut self, bytes: &[u8]) -> Result<()> {
        let stdin = self
            .guard
            .child_mut()
            .stdin
            .as_mut()
            .ok_or_else(|| SandboxError::Denied("worker stdin is not piped".to_string()))?;
        stdin.write_all(bytes)?;
        stdin.flush()?;
        Ok(())
    }

    pub fn close_stdin(&mut self) {
        if let Some(stdin) = self.guard.child_mut().stdin.take() {
            drop(stdin);
        }
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.guard.child_mut().try_wait().map_err(Into::into)
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        self.guard.wait()
    }
}

impl Drop for SpawnedProcess {
    fn drop(&mut self) {
        let _ = self.kill_tree();
    }
}

/// Placeholder kept public so integration code can check that a backend's
/// kill mechanism is present without spawning a process.
pub const PROCESS_TREE_KILL_REQUIRED: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_keeps_argv_separate() {
        let spec = CommandSpec::new("/bin/echo", ["hello world", "$(id)"]);
        assert_eq!(spec.argv(), vec!["/bin/echo", "hello world", "$(id)"]);
        let cmd = spec.to_command();
        assert_eq!(cmd.get_args().collect::<Vec<_>>().len(), 2);
    }

    #[test]
    fn kill_hook_errors_are_surfaced() {
        let mut command = CommandSpec::new("/bin/true", Vec::<String>::new()).to_command();
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut spawned = SpawnedProcess::new(
            child,
            stdout,
            stderr,
            IoPolicy::from_limits(&Default::default()),
        )
        .unwrap()
        .with_kill_hook(|| Err(SandboxError::Denied("hook failed".to_string())));
        let error = spawned.kill_tree().unwrap_err();
        assert!(error.to_string().contains("hook failed"));
    }
}
