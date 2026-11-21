//! Seatbelt SBPL generation, compiled on every host so profile shape can be
//! tested without macOS. The macOS backend is the only runtime consumer.

use crate::capability::Access;
use crate::error::{Result, SandboxError};
use crate::policy::SandboxPolicy;
use crate::process::CommandSpec;

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

fn access_lines(
    lines: &mut Vec<String>,
    operation: &str,
    root_path: &std::path::Path,
    rule: &crate::capability::FsRule,
) {
    let path = if rule.path().is_empty() {
        root_path.to_path_buf()
    } else {
        root_path.join(rule.path())
    };
    let quoted = sbpl_quote(&path.to_string_lossy());
    // JUDGMENT CALL: an exact grant must not silently become a subtree grant.
    // `(subpath ...)` is emitted only for rules that were declared with `/**`.
    let selector = if rule.is_subtree() {
        format!("(subpath {quoted})")
    } else {
        format!("(literal {quoted})")
    };
    lines.push(format!("(allow {operation} {selector})"));
}

/// Generate a deny-by-default SBPL profile for the policy.
pub fn seatbelt_profile(spec: &CommandSpec, policy: &SandboxPolicy) -> Result<String> {
    policy.validate()?;
    if policy.requires_host_allowlist_enforcement() {
        return Err(SandboxError::Unsupported(
            "Seatbelt cannot enforce hostname allowlists; use a proxy or deny the grant"
                .to_string(),
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
        for rule in policy.fs.rules().iter().filter(|r| r.root() == root.name()) {
            match rule.access() {
                Access::Read => access_lines(&mut lines, "file-read*", root.path(), rule),
                Access::Write => access_lines(&mut lines, "file-write*", root.path(), rule),
                Access::Exec => access_lines(&mut lines, "file-execute", root.path(), rule),
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{ProtocolCapability, SafeRoot};
    use std::path::Path;

    fn policy() -> (SandboxPolicy, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("meld-sbpl-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut policy = SandboxPolicy::deny_all("sbpl");
        policy
            .fs
            .add_root(SafeRoot::new("project", &root).unwrap())
            .unwrap();
        policy
            .fs
            .grant("project", "src/exact.txt", Access::Read)
            .unwrap();
        policy.fs.grant("project", "docs/**", Access::Read).unwrap();
        policy
            .capabilities
            .allow(crate::capability::Capability::FsRead);
        (policy, root)
    }

    #[test]
    fn exact_rules_use_literal_and_subtrees_use_subpath() {
        let (policy, root) = policy();
        let spec = CommandSpec::new("/usr/bin/python3", ["-u"]);
        let profile = seatbelt_profile(&spec, &policy).unwrap();
        let exact = root.join("src/exact.txt");
        let subtree = root.join("docs");
        assert!(profile.contains(&format!(
            "(allow file-read* (literal {}))",
            sbpl_quote(&exact.to_string_lossy())
        )));
        assert!(profile.contains(&format!(
            "(allow file-read* (subpath {}))",
            sbpl_quote(&subtree.to_string_lossy())
        )));
        assert!(!profile.contains(&format!(
            "(allow file-read* (subpath {}))",
            sbpl_quote(&exact.to_string_lossy())
        )));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hostname_allowlists_fail_closed() {
        let (mut policy, root) = policy();
        policy.network.protocols = ProtocolCapability::parse("tcp").unwrap();
        policy
            .capabilities
            .allow(crate::capability::Capability::Network);
        policy.network.hosts.insert("example.com".to_string());
        let spec = CommandSpec::new("/usr/bin/python3", ["-u"]);
        assert!(matches!(
            seatbelt_profile(&spec, &policy),
            Err(SandboxError::Unsupported(_))
        ));
        assert!(Path::new(&root).exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn profile_quotes_paths() {
        assert_eq!(sbpl_quote("/tmp/a\"b"), "\"/tmp/a\\\"b\"");
    }
}
