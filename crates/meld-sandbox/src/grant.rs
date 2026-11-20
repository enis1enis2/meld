//! Region-grant parser for the `.meld` annotation form:
//!
//! ```text
//! grant(fs=["project/src/**", "project/data.txt:read"], net=none,
//!       env=["HOME"], host=["example.com"], mem=64MiB, time=2s)
//! ```
//!
//! This module only parses and evaluates text; it deliberately has no
//! dependency on the Meld splitter. Evaluation never widens a policy.

use crate::capability::Capability;
use crate::error::{check_text, Result, SandboxError};
use crate::policy::SandboxPolicy;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Grant {
    /// `None` means "not mentioned"; `Some(empty)` means explicitly none.
    pub fs: Option<Vec<String>>,
    pub net: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
    pub host: Option<Vec<String>>,
    pub caps: Option<Vec<String>>,
    pub mem: Option<u64>,
    pub time: Option<u64>,
    pub cpu: Option<u64>,
    pub proc: Option<u64>,
    pub files: Option<u64>,
    pub stack: Option<u64>,
    pub max_line: Option<u64>,
    pub stderr: Option<u64>,
    pub reason: Option<String>,
}

impl fmt::Display for Grant {
    #[allow(unused_assignments)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("grant(")?;
        let mut first = true;
        macro_rules! field {
            ($name:literal, $value:expr) => {
                if let Some(value) = $value {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    write!(f, "{}={}", $name, value)?;
                }
            };
        }
        field!("fs", self.fs.as_ref().map(|v| format!("[{}]", v.join(","))));
        field!(
            "net",
            self.net.as_ref().map(|v| format!("[{}]", v.join(",")))
        );
        field!(
            "env",
            self.env.as_ref().map(|v| format!("[{}]", v.join(",")))
        );
        field!(
            "host",
            self.host.as_ref().map(|v| format!("[{}]", v.join(",")))
        );
        field!(
            "caps",
            self.caps.as_ref().map(|v| format!("[{}]", v.join(",")))
        );
        field!("mem", self.mem);
        field!("time", self.time);
        field!("cpu", self.cpu);
        field!("proc", self.proc);
        field!("files", self.files);
        field!("stack", self.stack);
        field!("max_line", self.max_line);
        field!("stderr", self.stderr);
        field!("reason", self.reason.as_ref());
        f.write_str(")")
    }
}

/// Parse `grant(...)`. Unknown fields, `..` paths and malformed units are hard
/// errors. No shell text is ever constructed or executed.
pub fn parse_grant(text: &str) -> Result<Grant> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix("grant")
        .and_then(|s| s.trim_start().strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| SandboxError::Invalid("grant must be grant(...)".to_string()))?;

    let mut grant = Grant::default();
    for pair in split_top_level(inner, ',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = split_once_top_level(pair, '=')
            .ok_or_else(|| SandboxError::Invalid(format!("grant field missing '=': {pair:?}")))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "fs" => grant.fs = Some(parse_list(value)?),
            "net" => grant.net = Some(parse_list(value)?),
            "env" => grant.env = Some(parse_list(value)?),
            "host" => grant.host = Some(parse_list(value)?),
            "caps" | "capabilities" => grant.caps = Some(parse_list(value)?),
            "mem" => grant.mem = Some(parse_size(value)?),
            "time" | "wall" => grant.time = Some(parse_duration(value)?),
            "cpu" => grant.cpu = Some(parse_duration(value)?),
            "proc" | "processes" => grant.proc = Some(parse_u64(value)?),
            "files" | "open_files" => grant.files = Some(parse_u64(value)?),
            "stack" => grant.stack = Some(parse_size(value)?),
            "max_line" => grant.max_line = Some(parse_size(value)?),
            "stderr" => grant.stderr = Some(parse_size(value)?),
            "reason" => grant.reason = Some(unquote(value)?),
            _ => return Err(SandboxError::Unknown("grant field", key.to_string())),
        }
    }
    grant.validate()?;
    Ok(grant)
}

impl Grant {
    fn validate(&self) -> Result<()> {
        if let Some(fs) = &self.fs {
            for entry in fs {
                let path = entry.rsplit_once(':').map(|(p, _)| p).unwrap_or(entry);
                let path = path.strip_suffix("/**").unwrap_or(path);
                if path.split('/').any(|p| p == "..") {
                    return Err(SandboxError::Escape {
                        path: path.into(),
                        reason: "`..` in grant path".to_string(),
                    });
                }
                if path.starts_with('/') || path.contains('\\') {
                    return Err(SandboxError::Escape {
                        path: path.into(),
                        reason: "absolute or backslash path in grant".to_string(),
                    });
                }
            }
        }
        if let Some(caps) = &self.caps {
            crate::capability::CapabilitySet::parse(caps.iter())?;
        }
        if let Some(protocols) = &self.net {
            crate::capability::ProtocolCapability::parse_list(protocols.iter())?;
        }
        for env in self.env.iter().flatten() {
            check_text("grant env", env)?;
        }
        for host in self.host.iter().flatten() {
            check_text("grant host", host)?;
        }
        if let Some(reason) = &self.reason {
            check_text("grant reason", reason)?;
        }
        Ok(())
    }

    /// Apply the grant to a policy. Every requested item must already be
    /// present in the policy; the result can only be narrower.
    pub fn evaluate(&self, policy: &SandboxPolicy) -> Result<SandboxPolicy> {
        policy.validate()?;
        let mut out = policy.clone();
        out.name = format!("{}∩grant", policy.name);

        if let Some(caps) = &self.caps {
            let requested = crate::capability::CapabilitySet::parse(caps.iter())?;
            let narrowed = policy.capabilities.intersection(&requested);
            for cap in requested.iter() {
                if !policy.capabilities.contains(cap) {
                    return Err(SandboxError::Denied(format!(
                        "grant requests capability not held: {cap}"
                    )));
                }
            }
            out.capabilities = narrowed;
        }

        if let Some(fs) = &self.fs {
            let mut rules = Vec::new();
            for entry in fs {
                let (path, access_text) = match entry.rsplit_once(':') {
                    Some((path, access)) => (path, Some(access)),
                    None => (entry.as_str(), None),
                };
                let (root, relative) = path.split_once('/').unwrap_or((path, ""));
                if root.is_empty() {
                    return Err(SandboxError::Invalid(
                        "grant fs path has no root".to_string(),
                    ));
                }
                let relative = if relative.is_empty() {
                    "**".to_string()
                } else {
                    relative.to_string()
                };
                let logical = relative.strip_suffix("/**").unwrap_or(&relative);
                let check_logical = if logical.is_empty() { "." } else { logical };
                let accesses: Vec<_> = match access_text {
                    // Least privilege: a grant that omits `:access` requests
                    // read only. Write/exec must be stated explicitly.
                    None => vec![crate::capability::Access::Read],
                    Some("none") => Vec::new(),
                    Some(a) => a
                        .split('+')
                        .map(crate::capability::Access::parse)
                        .collect::<Result<_>>()?,
                };
                for access in accesses {
                    if !relative.ends_with("/**") && relative != "**" {
                        return Err(SandboxError::Invalid(format!(
                            "grant fs path must end in /** or name a root: {path}"
                        )));
                    }
                    let rule = crate::capability::FsRule::new(root, &relative, access)?;
                    // A grant path may be a subtree below a policy rule. Only
                    // accept it when the policy rule covers the requested path.
                    let covered = policy.fs.rules().iter().any(|r| {
                        r.root() == root && r.access() == access && r.covers(root, check_logical)
                    });
                    if !covered {
                        return Err(SandboxError::Denied(format!(
                            "grant fs request not covered by policy: {entry}"
                        )));
                    }
                    // Re-check against the real safe root (symlinks included).
                    policy.fs.check(root, check_logical, access)?;
                    rules.push(rule);
                }
            }
            out.fs = crate::capability::FsPolicy::new();
            for root in policy.fs.roots() {
                out.fs.add_root(root.clone())?;
            }
            for rule in rules {
                out.fs.grant(rule.root(), rule.path(), rule.access())?;
            }
        }

        if let Some(protocols) = &self.net {
            let requested = crate::capability::ProtocolCapability::parse_list(protocols.iter())?;
            if !(requested.tcp <= policy.network.protocols.tcp
                && requested.udp <= policy.network.protocols.udp
                && requested.unix <= policy.network.protocols.unix
                && requested.http <= policy.network.protocols.http
                && requested.https <= policy.network.protocols.https
                && requested.dns <= policy.network.protocols.dns)
            {
                return Err(SandboxError::Denied(
                    "grant requests a network protocol not held".to_string(),
                ));
            }
            out.network.protocols = policy.network.protocols.intersection(&requested);
            if !out.network.protocols.any() {
                out.capabilities.retain_without(Capability::Network);
            }
        }

        if let Some(hosts) = &self.host {
            let wanted: BTreeSet<_> = hosts.iter().cloned().collect();
            for host in &wanted {
                if !policy.network.hosts.contains(host) {
                    return Err(SandboxError::Denied(format!(
                        "grant requests host not allowlisted: {host}"
                    )));
                }
            }
            out.network.hosts = wanted;
        }

        if let Some(env) = &self.env {
            for name in env {
                if !policy.env.contains(name) {
                    return Err(SandboxError::Denied(format!(
                        "grant requests env not allowlisted: {name}"
                    )));
                }
            }
            out.env = env.iter().cloned().collect();
        }

        check_limit(policy.limits.memory_bytes, self.mem, "memory")?;
        check_limit(policy.limits.wall_time_ms, self.time, "wall time")?;
        check_limit(policy.limits.cpu_time_ms, self.cpu, "cpu time")?;
        check_limit(policy.limits.processes, self.proc, "processes")?;
        check_limit(policy.limits.open_files, self.files, "open files")?;
        check_limit(policy.limits.stack_bytes, self.stack, "stack")?;
        check_limit(policy.limits.max_line_bytes, self.max_line, "max line")?;
        check_limit(policy.limits.max_stderr_bytes, self.stderr, "stderr")?;

        if self.mem.is_some() {
            out.limits.memory_bytes = self.mem;
        }
        if self.time.is_some() {
            out.limits.wall_time_ms = self.time;
        }
        if self.cpu.is_some() {
            out.limits.cpu_time_ms = self.cpu;
        }
        if self.proc.is_some() {
            out.limits.processes = self.proc;
        }
        if self.files.is_some() {
            out.limits.open_files = self.files;
        }
        if self.stack.is_some() {
            out.limits.stack_bytes = self.stack;
        }
        if self.max_line.is_some() {
            out.limits.max_line_bytes = self.max_line;
        }
        if self.stderr.is_some() {
            out.limits.max_stderr_bytes = self.stderr;
        }
        out.validate()?;
        Ok(out)
    }
}

fn check_limit(allowed: Option<u64>, requested: Option<u64>, label: &str) -> Result<()> {
    if let (Some(allowed), Some(requested)) = (allowed, requested) {
        if requested > allowed {
            return Err(SandboxError::Denied(format!(
                "grant {label} {requested} exceeds policy maximum {allowed}"
            )));
        }
    }
    Ok(())
}

/// Parse `64MiB`, `2GiB`, `1024B`, `4096` (bytes).
pub fn parse_size(text: &str) -> Result<u64> {
    let text = text.trim();
    check_text("size", text)?;
    let digits = text
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return Err(SandboxError::Invalid(format!("invalid size: {text:?}")));
    }
    let number: u64 = digits
        .parse()
        .map_err(|_| SandboxError::Invalid(format!("invalid size: {text:?}")))?;
    let unit = &text[digits.len()..];
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1u64,
        "kib" => 1024,
        "mib" => 1024 * 1024,
        "gib" => 1024 * 1024 * 1024,
        "kb" => 1000,
        "mb" => 1000 * 1000,
        "gb" => 1000 * 1000 * 1000,
        _ => {
            return Err(SandboxError::Invalid(format!(
                "unknown size unit: {unit:?}"
            )))
        }
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| SandboxError::Invalid(format!("size overflows u64: {text:?}")))
}

/// Parse `2s`, `500ms`, `3m`, `1h`.
pub fn parse_duration(text: &str) -> Result<u64> {
    let text = text.trim();
    check_text("duration", text)?;
    let digits = text
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return Err(SandboxError::Invalid(format!("invalid duration: {text:?}")));
    }
    let number: u64 = digits
        .parse()
        .map_err(|_| SandboxError::Invalid(format!("invalid duration: {text:?}")))?;
    let multiplier = match &text[digits.len()..] {
        "ms" => 1u64,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        other => {
            return Err(SandboxError::Invalid(format!(
                "unknown duration unit: {other:?}"
            )))
        }
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| SandboxError::Invalid(format!("duration overflows u64: {text:?}")))
}

fn parse_u64(text: &str) -> Result<u64> {
    let text = unquote(text)?;
    text.parse()
        .map_err(|_| SandboxError::Invalid(format!("invalid integer: {text:?}")))
}

/// `none`, a single value, or `[a, b, c]`.
pub fn parse_list(text: &str) -> Result<Vec<String>> {
    let text = text.trim();
    if text == "none" || text == "[]" {
        return Ok(Vec::new());
    }
    let inner = if let Some(inner) = text.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner
    } else {
        text
    };
    split_top_level(inner, ',')
        .into_iter()
        .map(|item| unquote(item.trim()))
        .filter(|item| item.as_ref().map(|s| !s.is_empty()).unwrap_or(true))
        .collect()
}

fn unquote(text: &str) -> Result<String> {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    _ => return Err(SandboxError::Invalid("invalid escape in grant".to_string())),
                }
            } else {
                out.push(c);
            }
        }
        check_text("string", &out)?;
        Ok(out)
    } else {
        check_text("value", text)?;
        Ok(text.to_string())
    }
}

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            match ch {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            c if c == separator && depth == 0 => {
                parts.push(&text[start..index]);
                start = index + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn split_once_top_level<'a>(text: &'a str, separator: char) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted {
            match ch {
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            c if c == separator && depth == 0 => return Some((&text[..index], &text[index + 1..])),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_grant() {
        let grant = parse_grant(
            "grant(fs=[\"project/src/**\", \"project/data.txt:read\"], net=none, env=[\"HOME\"], host=[\"example.com\"], mem=64MiB, time=2s, cpu=1500ms, proc=4, max_line=1MiB)",
        )
        .unwrap();
        assert_eq!(grant.mem, Some(64 * 1024 * 1024));
        assert_eq!(grant.time, Some(2000));
        assert_eq!(grant.cpu, Some(1500));
        assert_eq!(grant.fs.unwrap().len(), 2);
        assert_eq!(grant.host.unwrap(), vec!["example.com"]);
        assert!(grant.net.unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_field_and_escape_attempts() {
        assert!(matches!(
            parse_grant("grant(exec=anything)"),
            Err(SandboxError::Unknown("grant field", _))
        ));
        assert!(matches!(
            parse_grant("grant(fs=[\"project/../../etc/**\"])"),
            Err(SandboxError::Escape { .. })
        ));
        assert!(matches!(
            parse_grant("grant(fs=[\"/etc/passwd:read\"])"),
            Err(SandboxError::Escape { .. })
        ));
        assert!(parse_grant("grant(fs=\"project/**\")").is_ok());
    }
}
