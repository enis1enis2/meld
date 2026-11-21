//! `pack.toml` / `runner.toml` sandbox, resources and limits parsing.
//!
//! Only the security-relevant tables are read: `[sandbox]`, `[sandbox.roots]`,
//! `[sandbox.network]`, `[sandbox.resources]`, `[resources]` and `[limits]`.
//! Other manifest keys are intentionally ignored. Unknown keys *inside* those
//! tables are hard errors so a typo cannot silently widen a sandbox.
//!
//! A missing `[sandbox]` table yields the deny-all policy: no network and an
//! empty filesystem grant set.

use crate::capability::{
    Access, Capability, CapabilitySet, FsPolicy, ProtocolCapability, ResourceLimits, SafeRoot,
};
use crate::error::{check_text, Result, SandboxError};
use crate::grant::{parse_duration, parse_size};
use crate::policy::SandboxPolicy;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Parse the pack's maximum policy. `base` is the project directory used to
/// resolve the conventional `project` root.
pub fn parse_pack(text: &str, base: &Path) -> Result<SandboxPolicy> {
    parse_manifest(text, base, "pack")
}

/// Parse the runner's constraints. A runner with no `[sandbox]` still
/// contributes its `[limits]` table, but never grants network or files.
pub fn parse_runner(text: &str, base: &Path) -> Result<SandboxPolicy> {
    parse_manifest(text, base, "runner")
}

pub fn parse_manifest(text: &str, base: &Path, source: &str) -> Result<SandboxPolicy> {
    let root: toml::Value =
        toml::from_str(text).map_err(|e| SandboxError::Invalid(format!("{source} TOML: {e}")))?;
    let table = root
        .as_table()
        .ok_or_else(|| SandboxError::Invalid(format!("{source} manifest is not a table")))?;

    let mut policy = SandboxPolicy::deny_all(source);
    let top_limits = parse_limit_table(table, source, "top-level")?;
    if let Some(limits) = &top_limits {
        parse_limits(&mut policy.limits, limits, source)?;
    }

    let Some(sandbox) = table.get("sandbox") else {
        return Ok(policy);
    };
    let sandbox = sandbox
        .as_table()
        .ok_or_else(|| SandboxError::Invalid(format!("{source} [sandbox] must be a table")))?;
    reject_unknown(
        sandbox,
        &[
            "fs",
            "roots",
            "network",
            "capabilities",
            "env",
            "resources",
            "limits",
        ],
        &format!("{source} [sandbox]"),
    )?;
    let sandbox_limits = parse_limit_table(sandbox, source, "[sandbox]")?;
    if top_limits.is_some() && sandbox_limits.is_some() {
        return Err(SandboxError::Conflict(format!(
            "{source} declares limits both at the top level and in [sandbox]"
        )));
    }
    if let Some(limits) = sandbox_limits {
        parse_limits(
            &mut policy.limits,
            &limits,
            &format!("{source} [sandbox.resources]"),
        )?;
    }

    let roots = parse_roots(sandbox.get("roots"), base, source)?;
    parse_fs(&mut policy.fs, sandbox.get("fs"), &roots, base, source)?;
    parse_network(&mut policy, sandbox.get("network"), source)?;
    parse_env(&mut policy, sandbox.get("env"), source)?;

    let explicit_caps = match sandbox.get("capabilities") {
        Some(value) => Some(CapabilitySet::parse(
            as_strings(value, "capabilities")?.iter(),
        )?),
        None => None,
    };
    if let Some(caps) = explicit_caps {
        policy.capabilities = caps;
    } else {
        // Derive only from concrete grants; no implicit spawn/env/network.
        let mut caps = CapabilitySet::deny_all();
        for rule in policy.fs.rules() {
            match rule.access() {
                Access::Read => caps.allow(Capability::FsRead),
                Access::Write => caps.allow(Capability::FsWrite),
                Access::Exec => caps.allow(Capability::FsExec),
            }
        }
        if policy.network.protocols.any() {
            caps.allow(Capability::Network);
        }
        policy.capabilities = caps;
    }
    policy.validate()?;
    Ok(policy)
}

fn parse_roots(
    value: Option<&toml::Value>,
    base: &Path,
    source: &str,
) -> Result<BTreeMap<String, PathBuf>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let table = value.as_table().ok_or_else(|| {
        SandboxError::Invalid(format!("{source} [sandbox.roots] must be a table"))
    })?;
    let mut names = BTreeMap::new();
    for (name, path) in table {
        check_text("root name", name)?;
        let path_text = path.as_str().ok_or_else(|| {
            SandboxError::Invalid(format!("{source} root {name} must be a path string"))
        })?;
        let candidate = PathBuf::from(path_text);
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            base.join(candidate)
        };
        // `SafeRoot::new` canonicalizes and rejects symlink components.
        let safe = SafeRoot::new(name, &candidate)?;
        names.insert(name.clone(), safe.path().to_path_buf());
    }
    Ok(names)
}

fn parse_fs(
    fs_policy: &mut FsPolicy,
    value: Option<&toml::Value>,
    declared_roots: &BTreeMap<String, PathBuf>,
    base: &Path,
    source: &str,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };

    // Conventional default: `fs = ["project"]` maps to the project directory.
    if !declared_roots.contains_key("project") {
        fs_policy.add_root(SafeRoot::new("project", base)?)?;
    }
    for (name, path) in declared_roots {
        fs_policy.add_root(SafeRoot::new(name, path)?)?;
    }

    match value {
        toml::Value::Array(items) => {
            for item in items {
                let spec = item.as_str().ok_or_else(|| {
                    SandboxError::Invalid(format!("{source} fs entries must be strings"))
                })?;
                add_fs_spec(fs_policy, spec, source)?;
            }
        }
        toml::Value::Table(table) => {
            // Table keys are logical root names, so there is no fixed allowlist.
            for (root, spec) in table {
                if root == "roots" {
                    continue;
                }
                match spec {
                    toml::Value::Array(paths) => {
                        for path in paths {
                            let path = path.as_str().ok_or_else(|| {
                                SandboxError::Invalid(format!("{source} fs path must be a string"))
                            })?;
                            let (path, access) = split_path_access(path)?;
                            for access in access {
                                fs_policy.grant(root, &path, access)?;
                            }
                        }
                    }
                    toml::Value::Table(inner) => {
                        reject_unknown(
                            inner,
                            &["paths", "access"],
                            &format!("{source} [sandbox.fs.{root}]"),
                        )?;
                        let paths = inner
                            .get("paths")
                            .map(|v| as_strings(v, "paths"))
                            .transpose()?
                            .unwrap_or_default();
                        let accesses = inner
                            .get("access")
                            .map(|v| as_strings(v, "access"))
                            .transpose()?
                            .unwrap_or_else(|| vec!["read".to_string(), "write".to_string()]);
                        let accesses = accesses
                            .iter()
                            .map(|a| Access::parse(a))
                            .collect::<Result<Vec<_>>>()?;
                        for path in paths {
                            for access in &accesses {
                                fs_policy.grant(root, &path, *access)?;
                            }
                        }
                    }
                    toml::Value::String(spec) => {
                        let (path, accesses) = split_path_access(spec)?;
                        for access in accesses {
                            fs_policy.grant(root, &path, access)?;
                        }
                    }
                    _ => {
                        return Err(SandboxError::Invalid(format!(
                            "{source} fs grant for {root} must be a list or table"
                        )))
                    }
                }
            }
        }
        _ => {
            return Err(SandboxError::Invalid(format!(
                "{source} fs must be a list or table"
            )))
        }
    }
    Ok(())
}

fn add_fs_spec(policy: &mut FsPolicy, spec: &str, source: &str) -> Result<()> {
    let (root, rest) = spec.split_once('/').unwrap_or((spec, ""));
    let (path, accesses) = split_path_access(rest)?;
    let path = if path.is_empty() {
        "**".to_string()
    } else {
        path
    };
    for access in accesses {
        policy.grant(root, &path, access)?;
    }
    check_text("fs spec", spec)
        .map_err(|_| SandboxError::Invalid(format!("{source} invalid fs spec: {spec:?}")))?;
    Ok(())
}

fn split_path_access(spec: &str) -> Result<(String, Vec<Access>)> {
    let (path, access_text) = match spec.rsplit_once(':') {
        Some((path, access)) if !access.is_empty() => (path, access),
        _ => (spec, ""),
    };
    let accesses = if access_text.is_empty() {
        vec![Access::Read, Access::Write]
    } else {
        access_text
            .split('+')
            .map(Access::parse)
            .collect::<Result<Vec<_>>>()?
    };
    Ok((path.to_string(), accesses))
}

fn parse_network(
    policy: &mut SandboxPolicy,
    value: Option<&toml::Value>,
    source: &str,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    match value {
        toml::Value::Boolean(false) => {}
        toml::Value::Boolean(true) => {
            // `network = true` without explicit hosts fails closed at validate.
            policy.network.protocols = ProtocolCapability::all();
        }
        toml::Value::String(s) => {
            policy.network.protocols = ProtocolCapability::parse(s)?;
            if policy.network.protocols.any() {
                policy.capabilities.allow(Capability::Network);
            }
        }
        toml::Value::Array(values) => {
            policy.network.protocols =
                ProtocolCapability::parse_list(as_strings(value, "network")?.iter())?;
            if policy.network.protocols.any() {
                policy.capabilities.allow(Capability::Network);
            }
            let _ = values;
        }
        toml::Value::Table(table) => {
            reject_unknown(
                table,
                &["protocols", "hosts"],
                &format!("{source} [sandbox.network]"),
            )?;
            let protocols = table
                .get("protocols")
                .map(|v| as_strings(v, "protocols"))
                .transpose()?
                .unwrap_or_default();
            policy.network.protocols = ProtocolCapability::parse_list(protocols.iter())?;
            if let Some(hosts) = table.get("hosts") {
                for host in as_strings(hosts, "hosts")? {
                    check_text("host", &host)?;
                    policy.network.hosts.insert(host);
                }
            }
            if policy.network.protocols.any() {
                policy.capabilities.allow(Capability::Network);
            }
        }
        _ => {
            return Err(SandboxError::Invalid(format!(
                "{source} network must be bool, string, list or table"
            )))
        }
    }
    Ok(())
}

fn parse_env(policy: &mut SandboxPolicy, value: Option<&toml::Value>, _source: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    for name in as_strings(value, "env")? {
        check_text("env name", &name)?;
        policy.env.insert(name);
    }
    // An explicit empty list grants no environment access; do not leave a
    // stale `env` capability behind.
    if policy.env.is_empty() {
        policy.capabilities.retain_without(Capability::Env);
    } else {
        policy.capabilities.allow(Capability::Env);
    }
    Ok(())
}

/// Return the single allowed limit table in `table`, rejecting `resources` and
/// `limits` together. The two spellings are aliases, not layers: accepting both
/// made key order decide which limit survived (M).
fn parse_limit_table(
    table: &toml::map::Map<String, toml::Value>,
    source: &str,
    context: &str,
) -> Result<Option<toml::Value>> {
    match (table.get("resources"), table.get("limits")) {
        (Some(_), Some(_)) => Err(SandboxError::Conflict(format!(
            "{source} {context} declares both [resources] and [limits]"
        ))),
        (Some(value), None) | (None, Some(value)) => {
            if !value.is_table() {
                return Err(SandboxError::Invalid(format!(
                    "{source} {context} resources/limits must be a table"
                )));
            }
            Ok(Some(value.clone()))
        }
        (None, None) => Ok(None),
    }
}

/// Known limit keys. A misspelled key is an error, never a silent no-op.
fn parse_limits(limits: &mut ResourceLimits, value: &toml::Value, source: &str) -> Result<()> {
    let table = value.as_table().ok_or_else(|| {
        SandboxError::Invalid(format!("{source} resources/limits must be a table"))
    })?;
    for (key, raw) in table {
        let number = |name: &str| -> Result<u64> {
            match raw {
                toml::Value::Integer(n) if *n >= 0 => Ok(*n as u64),
                toml::Value::String(s) if name == "time" || name == "timeout_ms" => {
                    parse_duration(s)
                }
                toml::Value::String(s)
                    if name.contains("mem")
                        || name.contains("size")
                        || name.contains("line")
                        || name.contains("stderr")
                        || name.contains("stack") =>
                {
                    parse_size(s)
                }
                toml::Value::String(s) => s.parse().map_err(|_| {
                    SandboxError::Invalid(format!("{source} {key} is not a number: {s}"))
                }),
                _ => Err(SandboxError::Invalid(format!(
                    "{source} {key} must be a non-negative integer"
                ))),
            }
        };
        let value = number(key)?;
        match key.as_str() {
            "memory_bytes" | "memory" | "rss_bytes" => limits.memory_bytes = Some(value),
            "memory_mb" => limits.memory_bytes = Some(value.saturating_mul(1024 * 1024)),
            "cpu_time_ms" | "cpu_ms" => limits.cpu_time_ms = Some(value),
            "wall_time_ms" | "time_ms" | "time" => limits.wall_time_ms = Some(value),
            "file_size_bytes" | "file_size" => limits.file_size_bytes = Some(value),
            "processes" | "pids" | "proc" => limits.processes = Some(value),
            "open_files" | "nofile" => limits.open_files = Some(value),
            "stack_bytes" | "stack" => limits.stack_bytes = Some(value),
            "max_line_bytes" => limits.max_line_bytes = Some(value),
            "max_stderr_bytes" | "stderr_bytes" => limits.max_stderr_bytes = Some(value),
            "max_output_lines" => limits.max_output_lines = Some(value),
            "idle_timeout_ms" => limits.idle_timeout_ms = Some(value),
            "overall_timeout_ms" | "timeout_ms" => limits.overall_timeout_ms = Some(value),
            _ => return Err(SandboxError::Unknown("limit key", key.clone())),
        }
    }
    Ok(())
}

fn as_strings(value: &toml::Value, field: &str) -> Result<Vec<String>> {
    let array = value
        .as_array()
        .ok_or_else(|| SandboxError::Invalid(format!("{field} must be an array of strings")))?;
    array
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| SandboxError::Invalid(format!("{field} must contain strings only")))
        })
        .collect()
}

fn reject_unknown(
    table: &toml::map::Map<String, toml::Value>,
    known: &[&str],
    context: &str,
) -> Result<()> {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            return Err(SandboxError::Unknown(
                "manifest key",
                format!("{context}.{key}"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_base(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "meld-sandbox-manifest-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn missing_sandbox_is_deny_all_but_keeps_limits() {
        let base = temp_base("missing");
        let policy = parse_runner(
            "tier = \"T2\"\n[limits]\nmax_line_bytes = 4096\nidle_timeout_ms = 1000\n",
            &base,
        )
        .unwrap();
        assert!(policy.fs.rules().is_empty());
        assert!(!policy.network.protocols.any());
        assert!(policy.network.hosts.is_empty());
        assert_eq!(policy.limits.max_line_bytes, Some(4096));
        assert_eq!(policy.limits.idle_timeout_ms, Some(1000));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn existing_pack_manifest_parses() {
        let base = temp_base("pack");
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/python/pack.toml"),
        )
        .unwrap();
        let policy = parse_pack(&text, &base).unwrap();
        assert!(policy.fs.root("project").is_some());
        assert!(!policy.network.protocols.any());
        assert!(policy.capabilities.contains(Capability::FsRead));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn unknown_limit_key_is_an_error() {
        let base = temp_base("unknown");
        let result = parse_runner("[limits]\nmax_lines_bytes = 5\n", &base);
        assert!(matches!(result, Err(SandboxError::Unknown("limit key", _))));
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn resources_and_limits_cannot_both_be_declared() {
        let base = temp_base("duplicate-limits");
        let top = "[resources]\nmemory_bytes = 1024\n[limits]\nmemory_bytes = 2048\n";
        assert!(matches!(
            parse_runner(top, &base),
            Err(SandboxError::Conflict(_))
        ));

        let inner = "[sandbox]\n[sandbox.resources]\nmemory_bytes = 1024\n[sandbox.limits]\nmemory_bytes = 2048\n";
        assert!(matches!(
            parse_pack(inner, &base),
            Err(SandboxError::Conflict(_))
        ));

        let split =
            "[limits]\nmemory_bytes = 1024\n[sandbox]\n[sandbox.resources]\nmemory_bytes = 2048\n";
        assert!(matches!(
            parse_pack(split, &base),
            Err(SandboxError::Conflict(_))
        ));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn empty_env_list_does_not_grant_env_capability() {
        let base = temp_base("empty-env");
        let policy = parse_pack("[sandbox]\nenv = []\n", &base).unwrap();
        assert!(policy.env.is_empty());
        assert!(!policy.capabilities.contains(Capability::Env));
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn rich_sandbox_table_narrows() {
        let base = temp_base("rich");
        let policy = parse_pack(
            "[sandbox]\nfs = [\"project/src/**\"]\nnetwork = { protocols = [\"dns\"], hosts = [\"example.com\"] }\nenv = [\"HOME\"]\n[sandbox.roots]\nproject = \".\"\n[sandbox.resources]\nmemory_bytes = 33554432\ntime = \"2s\"\n",
            &base,
        )
        .unwrap();
        assert!(policy.network.protocols.dns);
        assert!(policy.network.hosts.contains("example.com"));
        assert_eq!(policy.limits.memory_bytes, Some(32 * 1024 * 1024));
        assert_eq!(policy.limits.wall_time_ms, Some(2000));
        assert_eq!(policy.env.len(), 1);
        std::fs::remove_dir_all(base).ok();
    }
}
