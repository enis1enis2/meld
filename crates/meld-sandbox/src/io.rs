//! Bounded I/O helpers shared by every backend: capped line reads, capped
//! stderr buffering, idle and overall timeouts.

use crate::capability::ResourceLimits;
use crate::error::{Result, SandboxError};
use std::io::Read;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct IoPolicy {
    pub max_line_bytes: usize,
    pub max_stderr_bytes: u64,
    pub max_output_lines: usize,
    pub idle_timeout: Duration,
    pub overall_timeout: Duration,
}

impl IoPolicy {
    pub fn from_limits(limits: &ResourceLimits) -> Self {
        Self {
            max_line_bytes: limits.effective_max_line_bytes() as usize,
            max_stderr_bytes: limits.effective_max_stderr_bytes(),
            max_output_lines: limits.effective_max_output_lines() as usize,
            idle_timeout: Duration::from_millis(limits.effective_idle_timeout_ms()),
            overall_timeout: Duration::from_millis(limits.effective_overall_timeout_ms()),
        }
    }
}

/// Read one line from any `Read`, rejecting oversized lines instead of
/// allocating without bound. Returns `Ok(None)` at EOF with no partial data.
pub fn read_bounded_line<R: Read>(reader: &mut R, max_bytes: usize) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Ok(Some(decode_and_trim(&bytes, max_bytes)?));
        }
        if let Some(end) = chunk[..n].iter().position(|b| *b == b'\n') {
            bytes.extend_from_slice(&chunk[..end]);
            return Ok(Some(decode_and_trim(&bytes, max_bytes)?));
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > max_bytes {
            return Err(SandboxError::Denied(format!(
                "line exceeds {max_bytes} byte limit"
            )));
        }
    }
}

fn decode_and_trim(bytes: &[u8], max_bytes: usize) -> Result<String> {
    if bytes.len() > max_bytes {
        return Err(SandboxError::Denied(format!(
            "line exceeds {max_bytes} byte limit"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|e| SandboxError::Denied(format!("non-UTF-8 protocol line: {e}")))?;
    Ok(text
        .strip_suffix('\r')
        .unwrap_or(text)
        .trim_end_matches('\n')
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_line_read_handles_eof_and_limits() {
        assert_eq!(
            read_bounded_line(&mut Cursor::new(b"hello\nnext"), 8).unwrap(),
            Some("hello".to_string())
        );
        assert_eq!(read_bounded_line(&mut Cursor::new(b""), 8).unwrap(), None);
        assert!(matches!(
            read_bounded_line(&mut Cursor::new(vec![b'a'; 64]), 8),
            Err(SandboxError::Denied(_))
        ));
    }

    #[test]
    fn non_utf8_is_denied() {
        assert!(matches!(
            read_bounded_line(&mut Cursor::new(vec![0xff, b'\n']), 8),
            Err(SandboxError::Denied(_))
        ));
    }
}
