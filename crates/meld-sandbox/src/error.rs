//! Sandbox error taxonomy. Every failure is explicit: an unenforceable policy
//! is an error, never a silently weaker sandbox.

use std::io;
use std::path::PathBuf;

/// Errors raised while parsing, merging or enforcing sandbox policy.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The requested operation was not granted by the effective policy, or a
    /// declared policy cannot be enforced on this host.
    #[error("sandbox denied: {0}")]
    Denied(String),
    /// A capability, protocol, manifest key or grant field is not understood.
    #[error("unknown {0}: {1}")]
    Unknown(&'static str, String),
    /// Manifest or grant syntax error.
    #[error("invalid sandbox declaration: {0}")]
    Invalid(String),
    /// A path left its declared safe root (`..`, absolute path or symlink).
    #[error("path escape: {path}: {reason}")]
    Escape { path: PathBuf, reason: String },
    /// Two policy layers disagree in a way that cannot be narrowed safely.
    #[error("policy conflict: {0}")]
    Conflict(String),
    /// The required OS enforcement mechanism is not available.
    #[error("enforcement unavailable: {0}")]
    Unsupported(String),
    #[error("sandbox io error: {0}")]
    Io(#[from] io::Error),
    #[error("worker io failure: {0}")]
    WorkerIo(String),
}

pub type Result<T> = std::result::Result<T, SandboxError>;

/// Convenience for integration code that maps this error to
/// `meld_core::error::MeldError::SandboxDenied`.
impl SandboxError {
    pub fn denial_message(&self) -> String {
        self.to_string()
    }
}

/// Reject NUL bytes and other control characters before any path reaches an
/// OS API. This is also the first line of defence against injection through
/// shell-like strings (the sandbox never invokes a shell).
pub(crate) fn check_text(field: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(SandboxError::Invalid(format!("{field} is empty")));
    }
    if value.chars().any(|c| c.is_control() || c == '\u{7f}') {
        return Err(SandboxError::Invalid(format!(
            "{field} contains control characters"
        )));
    }
    Ok(())
}
