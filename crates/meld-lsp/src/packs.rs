//! Pack-owned LSP metadata. No language id is hardcoded in the core.
use meld_core::region_end_spec::RegionEndSpec;
use meld_core::splitter::ScannerRegistry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct LspSpec {
    pub language_id: String,
    pub extension: String,
    /// Pack id plus aliases; used to register the data-only scanner profile.
    pub names: Vec<String>,
    /// External language server command, if the pack ships one.
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Deterministic fallback completions when no external server is configured.
    pub keywords: Vec<String>,
    pub region_end: RegionEndSpec,
}

/// JUDGMENT CALL: `lsp.toml` is optional. A pack may provide only static
/// keywords (the common case for L0 packs), an external LSP command, or both.
/// The server command is rebased like runner commands so an installed pack
/// keeps working without a core edit (project.md §7, spec/registry.md §6).
pub fn discover(root: &Path) -> HashMap<String, LspSpec> {
    let extra: Vec<PathBuf> = std::env::var("MELD_PACKS")
        .map(|v| {
            v.split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    discover_with(root, &extra)
}

pub fn discover_with(root: &Path, extra: &[PathBuf]) -> HashMap<String, LspSpec> {
    let mut out = HashMap::new();
    let mut roots = vec![root.join("packs")];
    roots.extend(extra.iter().cloned());
    for scan_root in roots {
        let mut dirs = Vec::new();
        if scan_root.join("pack.toml").is_file() {
            dirs.push(scan_root.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&scan_root) {
            dirs.extend(entries.flatten().map(|e| e.path()));
        }
        for dir in dirs {
            let Some((spec, names)) = load(root, &dir) else {
                continue;
            };
            for name in names {
                out.insert(name, spec.clone());
            }
        }
    }
    out
}

/// Built-ins plus pack-declared profiles, in deterministic pack-id order.
pub fn scanner_registry(specs: &HashMap<String, LspSpec>) -> ScannerRegistry {
    let mut registry = ScannerRegistry::default_registry();
    let mut values: Vec<&LspSpec> = specs.values().collect();
    values.sort_by(|a, b| a.names.cmp(&b.names));
    let mut seen: Vec<String> = Vec::new();
    for spec in values {
        let Some(id) = spec.names.first() else {
            continue;
        };
        if seen.contains(id) {
            continue;
        }
        seen.push(id.clone());
        spec.region_end.register(&mut registry, spec.names.iter().cloned());
    }
    registry
}

fn load(root: &Path, dir: &Path) -> Option<(LspSpec, Vec<String>)> {
    let manifest: toml::Value =
        toml::from_str(&std::fs::read_to_string(dir.join("pack.toml")).ok()?).ok()?;
    let id = manifest.get("id")?.as_str()?.to_string();
    let mut names = vec![id.clone()];
    names.extend(
        manifest
            .get("aliases")
            .and_then(toml::Value::as_array)
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );

    let lsp: toml::Value = match std::fs::read_to_string(dir.join("lsp.toml")) {
        Ok(text) => toml::from_str(&text).ok()?,
        Err(_) => toml::Value::Table(toml::map::Map::new()),
    };
    let section = lsp.get("lsp");
    let str_field = |key: &str| -> Option<String> {
        section
            .and_then(|v| v.get(key))
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    };
    let command = str_field("command").map(|c| rebase_arg(&c, root, dir));
    let args = section
        .and_then(|v| v.get("args"))
        .and_then(toml::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str())
                .map(|x| rebase_arg(x, root, dir))
                .collect()
        })
        .unwrap_or_default();
    let keywords = section
        .and_then(|v| v.get("keywords"))
        .and_then(toml::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let spec = LspSpec {
        language_id: str_field("language_id").unwrap_or_else(|| names.first().cloned().unwrap_or_default()),
        extension: str_field("extension").unwrap_or_default(),
        names: names.clone(),
        command,
        args,
        keywords,
        region_end: RegionEndSpec::from_dir(dir).unwrap_or_default(),
    };
    Some((spec, names))
}

pub fn rebase_arg(arg: &str, root: &Path, pack_dir: &Path) -> String {
    let p = Path::new(arg);
    if p.is_absolute() {
        return arg.to_string();
    }
    let rooted = root.join(p);
    if rooted.exists() {
        return rooted.to_string_lossy().into_owned();
    }
    if let Some(name) = p.file_name() {
        let local = pack_dir.join(name);
        if local.exists() {
            return local.to_string_lossy().into_owned();
        }
    }
    arg.to_string()
}
