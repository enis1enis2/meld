//! Meld Lang Libraries: installable, verifiable language packs.
//!
//! A Lang Library is a Lang Pack directory (`pack.toml`, grammar reference,
//! queries, optional runner/worker, integrity files) plus a `library.toml`
//! manifest. It is data, not executable code loaded at library-discovery time.
//! JUDGMENT CALL: "library" here follows the package-manager sense used by
//! `project.md` §4/§11 — an independently installable and importable unit. A
//! dynamically linked tree-sitter `cdylib` would require compiling arbitrary
//! downloaded code and is outside the trust model in `project.md` §9. This
//! crate provides the shared on-disk format, integrity verification and
//! discovery used by both the CLI and the terminal installer.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const LIBRARY_API: &str = "meld-lang-library/1";
pub const INDEX_SCHEMA: &str = "meld-lang-library-index/1";
pub const LOCK_SCHEMA: &str = "meld-project-libraries/1";
pub const MANIFEST_FILE: &str = "library.toml";
pub const PACK_MANIFEST: &str = "pack.toml";
pub const CHECKSUM_FILE: &str = "checksums.sha256";
pub const SIGNATURE_FILE: &str = "signature.hmac";
pub const LOCK_FILE: &str = "meld.lock";

const SKIP_DIRS: [&str; 5] = [
    ".git",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "target",
];
const MIN_KEY_BYTES: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryManifest {
    pub api: String,
    pub kind: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub exports: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledEntry {
    pub id: String,
    pub version: String,
    /// Canonical source, e.g. `github:enis1enis2/meld@main`.
    pub source: String,
    /// Installed path relative to the library root, e.g. `typescript/0.1.0`.
    pub path: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub level: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledIndex {
    pub schema: String,
    pub libraries: BTreeMap<String, InstalledEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLock {
    pub schema: String,
    /// Canonical library id -> exact installed version.
    pub libraries: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    pub pack_id: String,
    pub version: String,
    pub strict: bool,
    pub file_count: usize,
    pub signature: String,
    pub key_id: Option<String>,
}

impl InstalledIndex {
    pub fn empty() -> Self {
        Self {
            schema: INDEX_SCHEMA.to_string(),
            libraries: BTreeMap::new(),
        }
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let path = index_path(root);
        if !path.is_file() {
            return Ok(Self::empty());
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let index: Self = serde_json::from_str(&text)
            .map_err(|e| format!("invalid library index {}: {e}", path.display()))?;
        if index.schema != INDEX_SCHEMA {
            return Err(format!(
                "unsupported library index schema in {} (expected {INDEX_SCHEMA})",
                path.display()
            ));
        }
        for (key, entry) in &index.libraries {
            if key != &entry.id || !valid_name(&entry.id) || !valid_name(&entry.version) {
                return Err(format!("invalid library index entry for {key:?}"));
            }
            let rel = Path::new(&entry.path);
            if rel.is_absolute() || rel.components().any(|c| c.as_os_str() == "..") {
                return Err(format!("unsafe library path in index: {}", entry.path));
            }
        }
        Ok(index)
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let path = index_path(root);
        fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        write_canonical_json(&path, self)
    }

    pub fn upsert(&mut self, entry: InstalledEntry) {
        self.libraries.insert(entry.id.clone(), entry);
    }

    pub fn remove(&mut self, id: &str) -> Option<InstalledEntry> {
        self.libraries.remove(id)
    }

    /// id or alias -> installed entry, without guessing on alias collisions.
    pub fn resolve(&self, name: &str) -> Result<&InstalledEntry, String> {
        if let Some(entry) = self.libraries.get(name) {
            return Ok(entry);
        }
        let matches: Vec<_> = self
            .libraries
            .values()
            .filter(|entry| entry.aliases.iter().any(|a| a == name))
            .collect();
        match matches.len() {
            0 => Err(format!("unknown installed language or alias {name:?}")),
            1 => Ok(matches[0]),
            _ => Err(format!(
                "ambiguous alias {name:?}: {}",
                matches
                    .iter()
                    .map(|e| e.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

impl ProjectLock {
    pub fn load(project_root: &Path) -> Result<Self, String> {
        let path = project_root.join(LOCK_FILE);
        if !path.is_file() {
            return Ok(Self {
                schema: LOCK_SCHEMA.to_string(),
                libraries: BTreeMap::new(),
            });
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lock: Self = serde_json::from_str(&text)
            .map_err(|e| format!("invalid project lock {}: {e}", path.display()))?;
        if lock.schema != LOCK_SCHEMA {
            return Err(format!(
                "unsupported project lock schema in {} (expected {LOCK_SCHEMA})",
                path.display()
            ));
        }
        for (id, version) in &lock.libraries {
            if !valid_name(id) || !valid_name(version) {
                return Err(format!("invalid project lock entry: {id} {version}"));
            }
        }
        Ok(lock)
    }

    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        write_canonical_json(&project_root.join(LOCK_FILE), self)
    }
}

/// Default user-level library root. `MELD_LIB_ROOT` overrides it so tests and
/// CI can isolate installs; no system-wide write is attempted implicitly.
pub fn library_root() -> PathBuf {
    if let Ok(root) = std::env::var("MELD_LIB_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("meld").join("libraries");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local/share/meld/libraries");
        }
    }
    PathBuf::from(".meld-libraries")
}

pub fn index_path(root: &Path) -> PathBuf {
    root.join("index.json")
}

pub fn installed_dir(root: &Path, id: &str, version: &str) -> PathBuf {
    root.join(id).join(version)
}

/// Paths of installed libraries imported by a project.
///
/// JUDGMENT CALL: a lock entry whose installed directory is missing is skipped
/// rather than failing `meld check`; `meld-pack list` reports the drift, and
/// installing again repairs it. Missing imports never execute code.
pub fn discover_imported(
    project_root: &Path,
    extra_roots: &[PathBuf],
) -> Result<Vec<(String, PathBuf)>, String> {
    let lock = ProjectLock::load(project_root)?;
    if lock.libraries.is_empty() {
        return Ok(Vec::new());
    }
    let mut roots = vec![library_root()];
    for root in extra_roots {
        if !roots.contains(root) {
            roots.push(root.clone());
        }
    }
    let mut out = Vec::new();
    for (id, version) in &lock.libraries {
        for root in &roots {
            let dir = installed_dir(root, id, version);
            if dir.is_dir() {
                out.push((id.clone(), dir));
                break;
            }
        }
    }
    Ok(out)
}

pub fn read_library_manifest(pack_dir: &Path) -> Result<LibraryManifest, String> {
    let path = pack_dir.join(MANIFEST_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: toml::Value =
        toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    let get = |key: &str| -> Result<String, String> {
        doc.get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{}: missing string field {key}", path.display()))
    };
    let strings = |key: &str| -> Vec<String> {
        doc.get(key)
            .and_then(toml::Value::as_array)
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let manifest = LibraryManifest {
        api: get("api")?,
        kind: get("kind")?,
        id: get("id")?,
        name: doc
            .get("name")
            .and_then(toml::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("{}: missing non-empty string field name", path.display()))?
            .to_string(),
        version: get("version")?,
        aliases: strings("aliases"),
        exports: strings("exports"),
    };
    if manifest.api != LIBRARY_API {
        return Err(format!(
            "{}: unsupported api {:?} (expected {LIBRARY_API})",
            path.display(),
            manifest.api
        ));
    }
    if manifest.kind != "lang" {
        return Err(format!(
            "{}: unsupported library kind {:?}",
            path.display(),
            manifest.kind
        ));
    }
    if !valid_name(&manifest.id) || !valid_name(&manifest.version) {
        return Err(format!("{}: invalid id or version", path.display()));
    }
    for alias in &manifest.aliases {
        if !valid_name(alias) {
            return Err(format!("{}: invalid alias {alias:?}", path.display()));
        }
    }
    // JUDGMENT CALL: `exports` names the stable integration surface of a
    // library (relative paths such as `runner.toml` or `queries/`). The
    // installer refuses traversal so a manifest cannot copy or reference files
    // outside its own directory (project.md §9).
    for export in &manifest.exports {
        let rel = Path::new(export);
        if rel.is_absolute()
            || rel.components().any(|c| c.as_os_str() == "..")
            || export.contains('\\')
        {
            return Err(format!("{}: unsafe export path {export:?}", path.display()));
        }
    }
    Ok(manifest)
}

/// Check that a library manifest and its underlying Lang Pack agree. This is
/// the single consistency rule the installer needs before copying data.
pub fn validate_library_dir(pack_dir: &Path) -> Result<LibraryManifest, String> {
    let manifest = read_library_manifest(pack_dir)?;
    let (tier, level, pack_aliases) = read_pack_facts(pack_dir)?;
    let verification = verify_pack(pack_dir, None)?;
    if verification.pack_id != manifest.id {
        return Err(format!(
            "library id {:?} does not match pack id {:?}",
            manifest.id, verification.pack_id
        ));
    }
    let pack_version = if verification.version == "0.0.0" {
        "0.1.0".to_string()
    } else {
        verification.version
    };
    if pack_version != manifest.version {
        return Err(format!(
            "library version {:?} does not match pack version {:?}",
            manifest.version, pack_version
        ));
    }
    if pack_aliases != manifest.aliases {
        return Err(format!(
            "library aliases {:?} do not match pack aliases {:?}",
            manifest.aliases, pack_aliases
        ));
    }
    let _ = (tier, level);
    for export in &manifest.exports {
        let path = pack_dir.join(export);
        if !path.exists() {
            return Err(format!("library export is missing: {}", path.display()));
        }
    }
    Ok(manifest)
}

/// Read tier/level/aliases from the underlying Lang Pack manifest.
pub fn read_pack_facts(pack_dir: &Path) -> Result<(String, String, Vec<String>), String> {
    let path = pack_dir.join(PACK_MANIFEST);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: toml::Value =
        toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
    let string = |key: &str, default: &str| {
        doc.get(key)
            .and_then(toml::Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let aliases = doc
        .get("aliases")
        .and_then(toml::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((string("tier", "L0"), string("level", "L0"), aliases))
}

/// Verify a Lang Library using the same canonical rules as `packtool`.
/// `key` enables MAC verification; without it only checksums and signature
/// presence are checked, matching `spec/registry.md` §4.
pub fn verify_pack(pack_dir: &Path, key: Option<&Path>) -> Result<Verification, String> {
    let (tier, level, _aliases) = read_pack_facts(pack_dir)?;
    let _ = (tier, level);
    let manifest_text = fs::read_to_string(pack_dir.join(PACK_MANIFEST))
        .map_err(|e| format!("{}: {e}", pack_dir.join(PACK_MANIFEST).display()))?;
    let manifest: toml::Value =
        toml::from_str(&manifest_text).map_err(|e| format!("invalid pack.toml: {e}"))?;
    let strict = manifest
        .get("strict")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let pack_id = manifest
        .get("id")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "pack.toml: missing id".to_string())?
        .to_string();

    let checksum_path = pack_dir.join(CHECKSUM_FILE);
    let checksum_bytes =
        fs::read(&checksum_path).map_err(|e| format!("{}: {e}", checksum_path.display()))?;
    let expected = parse_checksums(&checksum_bytes)?;
    let actual = payload_files(pack_dir)?;
    let mut problems = Vec::new();
    for rel in expected.keys().filter(|rel| !actual.contains_key(*rel)) {
        problems.push(format!("missing file: {rel}"));
    }
    for (rel, digest) in &actual {
        match expected.get(rel) {
            None => problems.push(format!("unexpected file: {rel}")),
            Some(want) if want != digest => problems.push(format!("checksum mismatch: {rel}")),
            _ => {}
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }

    let signature = match fs::read_to_string(pack_dir.join(SIGNATURE_FILE)) {
        Ok(text) => Some(parse_signature(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if strict && signature.is_none() {
        return Err(format!("strict pack requires {SIGNATURE_FILE}"));
    }
    let mut signature_state = "absent".to_string();
    let mut signature_key_id = None;
    if let Some((key_id, digest)) = signature {
        signature_key_id = Some(key_id.clone());
        signature_state = "present-unverified".to_string();
        if let Some(key_path) = key {
            let key = fs::read(key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
            if key.len() < MIN_KEY_BYTES {
                return Err(format!("key must be at least {MIN_KEY_BYTES} bytes"));
            }
            if key_id != hex(&sha256(&key)[..8]) {
                return Err("signature key-id does not match the supplied key".to_string());
            }
            let want = hex(&hmac_sha256(&key, &checksum_bytes));
            if !constant_eq(&digest, &want) {
                return Err(
                    "signature mismatch: checksums were modified or the key is wrong".to_string(),
                );
            }
            signature_state = "verified".to_string();
        }
    }

    let version = manifest
        .get("version")
        .and_then(toml::Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();
    Ok(Verification {
        pack_id,
        version,
        strict,
        file_count: actual.len(),
        signature: signature_state,
        key_id: signature_key_id,
    })
}

fn parse_checksums(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("checksums are not UTF-8: {e}"))?;
    let mut entries = BTreeMap::new();
    for (line_no, line) in text.split('\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let rest = line.strip_suffix('\n').unwrap_or(line);
        let line_number = line_no + 1;
        let (digest, rel) = rest
            .split_once("  ")
            .ok_or_else(|| format!("malformed checksums.sha256:{line_number}"))?;
        if digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || digest != digest.to_lowercase()
        {
            return Err(format!("malformed checksums.sha256:{line_number}"));
        }
        let path = Path::new(rel);
        if path.is_absolute()
            || path.components().any(|c| c.as_os_str() == "..")
            || rel.contains('\\')
        {
            return Err(format!("unsafe path in checksums.sha256: {rel}"));
        }
        if entries
            .insert(rel.to_string(), digest.to_string())
            .is_some()
        {
            return Err(format!("duplicate path in checksums.sha256: {rel}"));
        }
    }
    if text != canonical_checksums(&entries) {
        return Err("checksums.sha256 is not canonical (sorted paths, LF, two spaces)".to_string());
    }
    Ok(entries)
}

fn canonical_checksums(entries: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (rel, digest) in entries {
        out.push_str(digest);
        out.push_str("  ");
        out.push_str(rel);
        out.push('\n');
    }
    out
}

fn payload_files(pack_dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    collect_files(pack_dir, pack_dir, &mut out)?;
    Ok(out)
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "symlink in pack is not allowed: {}",
                path.display()
            ));
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect_files(root, &path, out)?;
            }
            continue;
        }
        if name == CHECKSUM_FILE
            || name == SIGNATURE_FILE
            || name.ends_with(".pyc")
            || name.ends_with(".pyo")
        {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let digest = hex(&sha256(
            &fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        ));
        out.insert(rel, digest);
    }
    Ok(())
}

fn parse_signature(text: &str) -> Result<(String, String), String> {
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() != 3 || parts[0] != "hmac-sha256" || parts[1].len() != 16 || parts[2].len() != 64
    {
        return Err(
            "malformed signature.hmac (expected 'hmac-sha256 <key-id> <digest>')".to_string(),
        );
    }
    if !parts[1].bytes().all(|b| b.is_ascii_hexdigit())
        || !parts[2].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("malformed signature.hmac (non-hex field)".to_string());
    }
    Ok((parts[1].to_string(), parts[2].to_string()))
}

/// Recursively copy a verified pack. Symlinks are rejected so a source tree
/// cannot smuggle a file from outside the pack (project.md §9).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", from.display()))?;
        if kind.is_symlink() {
            return Err(format!(
                "symlink in pack is not allowed: {}",
                from.display()
            ));
        }
        if kind.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

pub fn write_canonical_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = canonical_json(value)?;
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Serialize with recursively sorted object keys and a trailing LF.
/// JUDGMENT CALL: `serde_json::Map` is a `BTreeMap` unless the optional
/// `preserve_order` feature is enabled, so converting through `Value` gives a
/// canonical representation independent of struct field declaration order.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<String, String> {
    let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    Ok(format!("{text}\n"))
}

pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let key = if key.len() > 64 {
        sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(&key);
    let inner: Vec<u8> = block
        .iter()
        .map(|b| b ^ 0x36)
        .chain(data.iter().copied())
        .collect();
    let inner_hash = sha256(&inner);
    let outer: Vec<u8> = block
        .iter()
        .map(|b| b ^ 0x5c)
        .chain(inner_hash.iter().copied())
        .collect();
    sha256(&outer)
}

fn constant_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_vector() {
        let key = vec![0x0b; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn canonical_json_is_sorted_and_newline_terminated() {
        let root = std::env::temp_dir().join(format!("meld-lib-test-{}", std::process::id()));
        let mut index = InstalledIndex::empty();
        index.upsert(InstalledEntry {
            id: "ruby".to_string(),
            version: "0.1.0".to_string(),
            source: "github:test@main".to_string(),
            path: "ruby/0.1.0".to_string(),
            aliases: vec!["rb".to_string()],
            tier: "L0".to_string(),
            level: "L0".to_string(),
        });
        index.save(&root).unwrap();
        let text = fs::read_to_string(index_path(&root)).unwrap();
        assert!(text.starts_with("{\n  \"libraries\": {"), "{text}");
        assert!(text.ends_with("}\n"));
        assert_eq!(InstalledIndex::load(&root).unwrap(), index);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_imports_are_skipped() {
        let root = std::env::temp_dir().join(format!("meld-lib-missing-{}", std::process::id()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let lock = ProjectLock {
            schema: LOCK_SCHEMA.to_string(),
            libraries: BTreeMap::from([("ruby".to_string(), "0.1.0".to_string())]),
        };
        lock.save(&project).unwrap();
        assert!(discover_imported(&project, &[]).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
