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
//!
//! JUDGMENT CALL (signing): HMAC was replaced by Ed25519 because a symmetric
//! key cannot be a public trust root. Signatures are verified against explicit
//! trust roots and rejected when their key-id is revoked. Private keys stay
//! outside the repository; this crate only ever handles public key bytes.
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const LIBRARY_API: &str = "meld-lang-library/1";
pub const INDEX_SCHEMA: &str = "meld-lang-library-index/1";
pub const LOCK_SCHEMA: &str = "meld-project-libraries/1";
pub const MANIFEST_FILE: &str = "library.toml";
pub const PACK_MANIFEST: &str = "pack.toml";
pub const CHECKSUM_FILE: &str = "checksums.sha256";
pub const SIGNATURE_FILE: &str = "signature.ed25519";
pub const SIGNATURE_SCHEME: &str = "meld-pack/ed25519/v1";
pub const TRUST_ROOTS_SCHEMA: &str = "meld-trust-roots/1";
pub const REVOCATIONS_SCHEMA: &str = "meld-revocations/1";
pub const ATTESTATION_SCHEME: &str = "meld-registry/attestation/ed25519/v1";
pub const ATTESTATION_FILE: &str = "attestation.ed25519";
pub const LOCK_FILE: &str = "meld.lock";
/// Out-of-band trust store files written next to `index.json` by an install
/// that used pinned roots. They are metadata, never pack payload.
pub const TRUST_STORE_ROOTS: &str = "trust-roots.toml";
pub const TRUST_STORE_REVOCATIONS: &str = "revocations.json";
/// Insecure escape hatch for imported packs whose signer is not in a local
/// trust store. Only this exact value disables the trusted-signature check.
pub const ALLOW_UNTRUSTED_IMPORTS_ENV: &str = "MELD_ALLOW_UNTRUSTED_IMPORTS";

/// Keep the payload directory set byte-for-byte identical to `packtool.py`.
/// In particular `target/` is *payload*, not a cache: skipping it in Rust let
/// a build directory change the signed checksum set without detection.
const SKIP_DIRS: [&str; 4] = [".git", "__pycache__", ".mypy_cache", ".pytest_cache"];

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
    /// "absent", "present-unverified", or "verified".
    pub signature: String,
    pub key_id: Option<String>,
}

/// Public-key inputs to pack verification. No private key belongs here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerificationPolicy {
    /// TOML file with `meld-trust-roots/1` public keys. `None` means presence
    /// only: a signature can be reported as present but not authenticated.
    pub trust_roots: Option<PathBuf>,
    /// JSON file with `meld-revocations/1` key ids that must be rejected.
    pub revocations: Option<PathBuf>,
    /// Remote installs set this so an absent/unknown/unverified signature is
    /// a hard failure instead of a presence-only report.
    pub require_trusted: bool,
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
            validate_entry_path(entry)?;
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

/// The only directory an installed entry may own. Deriving the target from
/// validated id/version (instead of trusting `entry.path`) is what keeps a
/// hostile index from deleting arbitrary host directories.
pub fn entry_installed_dir(root: &Path, entry: &InstalledEntry) -> Result<PathBuf, String> {
    validate_entry_path(entry)?;
    Ok(installed_dir(root, &entry.id, &entry.version))
}

fn validate_entry_path(entry: &InstalledEntry) -> Result<(), String> {
    let rel = Path::new(&entry.path);
    let expected = Path::new(&entry.id).join(&entry.version);
    let unsafe_component = |c: Component<'_>| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) | Component::CurDir
        )
    };
    if entry.path.is_empty()
        || rel.components().any(unsafe_component)
        || entry.path.contains('\\')
        || rel != expected
    {
        return Err(format!(
            "unsafe library path in index for {}: {:?} (expected {})",
            entry.id,
            entry.path,
            expected.display()
        ));
    }
    Ok(())
}

/// Re-confine a path at the point of a destructive operation. Canonicalizing
/// the existing parent catches symlink swaps that happened after the index was
/// loaded; a missing path is still checked lexically against the root.
pub fn confined_under(root: &Path, target: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let existing_parent = target
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| format!("cannot resolve target {}", target.display()))?;
    let canonical_parent = existing_parent
        .canonicalize()
        .map_err(|e| format!("{}: {e}", existing_parent.display()))?;
    let suffix: PathBuf = target
        .strip_prefix(existing_parent)
        .map_err(|e| e.to_string())?
        .to_path_buf();
    let resolved = canonical_parent.join(suffix);
    if !resolved.starts_with(&root) {
        return Err(format!(
            "refusing to touch {} outside library root {}",
            target.display(),
            root.display()
        ));
    }
    Ok(resolved)
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

/// Verification policy used when a pack is about to be *executed* rather than
/// merely listed. Trust roots are pinned out-of-band (env or the local trust
/// store written by an authenticated install), never taken from the project
/// tree or from the pack itself.
pub fn verification_policy_from_env(library_root: &Path) -> VerificationPolicy {
    let env_path = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
    };
    let trust_roots = env_path("MELD_TRUST_ROOTS")
        .or_else(|| env_path("MELD_PACK_TRUST_ROOTS"))
        .or_else(|| {
            let path = library_root.join(TRUST_STORE_ROOTS);
            path.is_file().then_some(path)
        });
    let revocations = env_path("MELD_REVOCATIONS")
        .or_else(|| env_path("MELD_PACK_REVOCATIONS"))
        .or_else(|| {
            let path = library_root.join(TRUST_STORE_REVOCATIONS);
            path.is_file().then_some(path)
        });
    VerificationPolicy {
        require_trusted: trust_roots.is_some(),
        trust_roots,
        revocations,
    }
}

/// Discover imports and re-verify them *at the trust boundary*.
///
/// Checksums are always checked. A signed pack with no local trust anchor is
/// refused unless the caller explicitly opts into insecure operation; with a
/// trust anchor configured, only a verified signature is accepted. This closes
/// the gap where a pack was verified once at install time and then modified or
/// replaced before `meld run`/the LSP launched its worker.
pub fn discover_imported_verified(
    project_root: &Path,
    extra_roots: &[PathBuf],
    policy: &VerificationPolicy,
) -> Result<Vec<(String, PathBuf)>, String> {
    let found = discover_imported(project_root, extra_roots)?;
    let allow_untrusted = std::env::var(ALLOW_UNTRUSTED_IMPORTS_ENV).as_deref() == Ok("1");
    let mut problems = Vec::new();
    for (id, dir) in &found {
        match verify_pack(dir, policy) {
            Ok(verification) => {
                if policy.require_trusted && verification.signature != "verified" {
                    problems.push(format!(
                        "{id} at {}: signature is not verified against configured trust roots",
                        dir.display()
                    ));
                } else if verification.signature == "present-unverified" && !allow_untrusted {
                    problems.push(format!(
                        "{id} at {}: signed but unverified; set MELD_TRUST_ROOTS (or install \
                         with --trust-roots), or set {ALLOW_UNTRUSTED_IMPORTS_ENV}=1 to accept it \
                         without provenance",
                        dir.display()
                    ));
                }
            }
            Err(e) => problems.push(format!("{id} at {}: {e}", dir.display())),
        }
    }
    if problems.is_empty() {
        Ok(found)
    } else {
        Err(problems.join("; "))
    }
}

/// One workspace-root resolver shared by the CLI, the LSP and the installer.
///
/// `MELD_ROOT` is normalized (empty values are ignored) and an explicit
/// argument wins. Otherwise the nearest ancestor containing `project.md` or a
/// real `[workspace]` manifest is used. Member `Cargo.toml` files are skipped
/// so running from a workspace member cannot silently narrow the policy root.
pub fn workspace_root(explicit: Option<&Path>) -> PathBuf {
    let normalize =
        |path: PathBuf| std::fs::canonicalize(&path).unwrap_or_else(|_| normalize_lexically(path));
    if let Some(root) = explicit {
        if !root.as_os_str().is_empty() {
            return resolve_explicit_root(&normalize(root.to_path_buf()));
        }
    }
    if let Ok(root) = std::env::var("MELD_ROOT") {
        if !root.trim().is_empty() {
            return resolve_explicit_root(&normalize(PathBuf::from(root)));
        }
    }
    find_workspace_root(&normalize(
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    ))
}

/// Honor an explicit root, except when it names a Cargo workspace *member*.
/// In that case the enclosing workspace is the policy root; otherwise we must
/// not silently climb from an explicitly chosen directory (e.g. a test fixture
/// containing only `meld.lock`).
fn resolve_explicit_root(root: &Path) -> PathBuf {
    let dir = if root.is_file() {
        root.parent().unwrap_or(root)
    } else {
        root
    };
    let member_manifest = dir.join("Cargo.toml").is_file() && !is_workspace_manifest(dir);
    if member_manifest {
        find_workspace_root(dir)
    } else {
        root.to_path_buf()
    }
}

fn find_workspace_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|p| p.join("project.md").is_file() || is_workspace_manifest(p))
        .map(Path::to_path_buf)
        .unwrap_or_else(|| start.to_path_buf())
}

fn is_workspace_manifest(dir: &Path) -> bool {
    let path = dir.join("Cargo.toml");
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines().any(|line| {
                let line = line.trim();
                line == "[workspace]" || line.starts_with("[workspace.")
            })
        })
        .unwrap_or(false)
}

fn normalize_lexically(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.is_absolute() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
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

/// Check that a library manifest and its underlying Lang Pack agree, using an
/// explicit verification policy. Remote installs pass trust roots and require a
/// verified signature; local read-only checks may use the default policy.
pub fn validate_library_dir_with_policy(
    pack_dir: &Path,
    policy: &VerificationPolicy,
) -> Result<LibraryManifest, String> {
    let manifest = read_library_manifest(pack_dir)?;
    let (tier, level, pack_aliases) = read_pack_facts(pack_dir)?;
    let verification = verify_pack(pack_dir, policy)?;
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

/// Presence-only compatibility wrapper: use [`validate_library_dir_with_policy`]
/// whenever provenance matters.
pub fn validate_library_dir(pack_dir: &Path) -> Result<LibraryManifest, String> {
    validate_library_dir_with_policy(pack_dir, &VerificationPolicy::default())
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
///
/// Without trust roots this reports a well-formed signature as
/// `present-unverified`; with trust roots it verifies the exact Ed25519
/// message and rejects unknown or revoked key ids. `policy.require_trusted`
/// turns an unauthenticated result into a hard error for remote installs.
pub fn verify_pack(pack_dir: &Path, policy: &VerificationPolicy) -> Result<Verification, String> {
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
    check_pinned_rev(&manifest, pack_dir)?;
    let pack_id = manifest
        .get("id")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "pack.toml: missing id".to_string())?
        .to_string();
    let version = manifest
        .get("version")
        .and_then(toml::Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();
    if !valid_name(&pack_id) || !valid_name(&version) {
        return Err(format!(
            "invalid id or version in {}/pack.toml",
            pack_dir.display()
        ));
    }

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
    // Revocations are an independent input: a revoked key must be rejected
    // even in presence-only mode or when no trust roots are configured.
    let revoked = match &policy.revocations {
        Some(path) => load_revocations(path)?,
        None => BTreeSet::new(),
    };
    if let Some((key_id, signature_bytes)) = signature {
        if revoked.contains(&key_id) {
            return Err(format!("signature key-id {key_id} is revoked"));
        }
        signature_key_id = Some(key_id.clone());
        signature_state = "present-unverified".to_string();
        if let Some(roots_path) = &policy.trust_roots {
            let roots = load_trust_roots(roots_path)?;
            let public = roots.get(&key_id).ok_or_else(|| {
                format!(
                    "signature key-id {key_id} is not in the trust roots; rotate trust roots \
                     for a new key instead of accepting an unknown signer"
                )
            })?;
            let verifying_key = VerifyingKey::from_bytes(public)
                .map_err(|e| format!("invalid trust root public key for {key_id}: {e}"))?;
            let signature = Signature::from_bytes(&signature_bytes);
            let message = pack_message(&pack_id, &version, &checksum_bytes);
            verifying_key
                .verify_strict(&message, &signature)
                .map_err(|_| {
                    "signature mismatch: checksums, pack id, version or signer were modified"
                        .to_string()
                })?;
            signature_state = "verified".to_string();
        }
    }
    if policy.require_trusted && signature_state != "verified" {
        if signature_state == "absent" {
            return Err("trusted Ed25519 signature required but none is present".to_string());
        }
        return Err("trusted Ed25519 signature required; pass --trust-roots".to_string());
    }
    Ok(Verification {
        pack_id,
        version,
        strict,
        file_count: actual.len(),
        signature: signature_state,
        key_id: signature_key_id,
    })
}

/// Parity with `packtool._check_pinned_rev`: strict packs must pin their
/// grammar source. A bare `rev = "main"` or `rev = "pinned"` is mutable and
/// therefore not a stable supply-chain claim.
fn check_pinned_rev(manifest: &toml::Value, pack_dir: &Path) -> Result<(), String> {
    if !manifest
        .get("strict")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let rev = manifest
        .get("tree_sitter")
        .and_then(|v| v.get("rev"))
        .and_then(toml::Value::as_str)
        .unwrap_or("");
    let pinned_ref = rev
        .strip_prefix("pinned:")
        .is_some_and(|rest| !rest.is_empty());
    if pinned_ref || is_exact_version(rev) {
        return Ok(());
    }
    Err(format!(
        "strict pack requires a pinned [tree_sitter].rev (`pinned:<ref>` or exact version) in {}/pack.toml",
        pack_dir.display()
    ))
}

fn is_exact_version(text: &str) -> bool {
    let text = text.strip_prefix('=').unwrap_or(text);
    let bytes = text.as_bytes();
    let split = bytes
        .iter()
        .position(|b| *b == b'-' || *b == b'+')
        .unwrap_or(bytes.len());
    let core = &bytes[..split];
    let parts: Vec<&[u8]> = core.split(|b| *b == b'.').collect();
    let valid_core = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit));
    let valid_suffix = split == bytes.len()
        || (bytes.len() > split + 1
            && bytes[split + 1..]
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'.' | b'-' | b'+')));
    valid_core && valid_suffix
}

/// Exact bytes signed by `packtool sign`; keep in sync with Python.
pub fn pack_message(pack_id: &str, version: &str, checksum_bytes: &[u8]) -> Vec<u8> {
    let mut message = SIGNATURE_SCHEME.as_bytes().to_vec();
    message.push(b'\n');
    message.extend_from_slice(pack_id.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(version.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(checksum_bytes);
    message
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRootsFile {
    schema: String,
    keys: Vec<TrustRoot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRoot {
    key_id: String,
    public_key: String,
    label: String,
    created: String,
}

/// Load `meld-trust-roots/1` as key-id -> 32-byte Ed25519 public key.
pub fn load_trust_roots(path: &Path) -> Result<BTreeMap<String, [u8; 32]>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: TrustRootsFile = toml::from_str(&text)
        .map_err(|e| format!("invalid trust roots {}: {e}", path.display()))?;
    if file.schema != TRUST_ROOTS_SCHEMA {
        return Err(format!(
            "unsupported trust-root schema in {} (expected {TRUST_ROOTS_SCHEMA})",
            path.display()
        ));
    }
    let mut roots = BTreeMap::new();
    for entry in file.keys {
        if entry.label.trim().is_empty() {
            return Err("invalid trust root label".to_string());
        }
        if entry.created.len() != 10
            || !entry.created.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        {
            return Err(format!(
                "invalid trust root created date {:?}",
                entry.created
            ));
        }
        let public = decode_hex(&entry.public_key, 32, "public_key")?;
        let expected_id = hex(&sha256(&public)[..8]);
        if entry.key_id != expected_id {
            return Err(format!(
                "trust root key-id {} does not match public key (expected {expected_id})",
                entry.key_id
            ));
        }
        if roots
            .insert(entry.key_id.clone(), public_as_array(&public))
            .is_some()
        {
            return Err(format!("duplicate trust root {}", entry.key_id));
        }
    }
    Ok(roots)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevocationsFile {
    schema: String,
    revoked: Vec<Revocation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revocation {
    key_id: String,
    reason: String,
    revoked: String,
}

/// Load the revoked key ids from `meld-revocations/1`.
pub fn load_revocations(path: &Path) -> Result<BTreeSet<String>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: RevocationsFile = serde_json::from_str(&text)
        .map_err(|e| format!("invalid revocations {}: {e}", path.display()))?;
    if file.schema != REVOCATIONS_SCHEMA {
        return Err(format!(
            "unsupported revocations schema in {} (expected {REVOCATIONS_SCHEMA})",
            path.display()
        ));
    }
    let mut revoked = BTreeSet::new();
    for entry in file.revoked {
        if entry.reason.trim().is_empty() {
            return Err("invalid revocation reason".to_string());
        }
        if entry.revoked.len() != 10
            || !entry.revoked.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        {
            return Err(format!("invalid revocation date {:?}", entry.revoked));
        }
        decode_hex(&entry.key_id, 8, "key_id")?;
        if !revoked.insert(entry.key_id.clone()) {
            return Err(format!("duplicate revocation {}", entry.key_id));
        }
    }
    Ok(revoked)
}

/// Verify the registry attestation that binds index, trust roots, revocations
/// and mirrors. Used when a caller deliberately accepts tree-supplied trust
/// metadata; the attestation must be signed by one of those same roots.
pub fn verify_registry_attestation(
    registry_dir: &Path,
    trust_roots: &Path,
    revocations: &Path,
) -> Result<String, String> {
    let read_canonical_json = |name: &str| -> Result<Vec<u8>, String> {
        let path = registry_dir.join(name);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("invalid {}: {e}", path.display()))?;
        if canonical_json(&value)? != text {
            return Err(format!(
                "{} is not canonical (sorted keys, two-space indent, LF)",
                path.display()
            ));
        }
        Ok(text.into_bytes())
    };
    let index = read_canonical_json("index.json")?;
    let mirrors = read_canonical_json("mirrors.json")?;
    let revocations_bytes =
        fs::read(revocations).map_err(|e| format!("{}: {e}", revocations.display()))?;
    let revocations_value: serde_json::Value = serde_json::from_slice(&revocations_bytes)
        .map_err(|e| format!("invalid revocations: {e}"))?;
    if canonical_json(&revocations_value)? != String::from_utf8_lossy(&revocations_bytes) {
        return Err("revocations.json is not canonical".to_string());
    }
    let roots_text =
        fs::read_to_string(trust_roots).map_err(|e| format!("{}: {e}", trust_roots.display()))?;
    let roots_doc: TrustRootsFile = toml::from_str(&roots_text)
        .map_err(|e| format!("invalid trust roots {}: {e}", trust_roots.display()))?;
    if roots_doc.schema != TRUST_ROOTS_SCHEMA
        || canonical_trust_roots_text(&roots_doc) != roots_text
    {
        return Err(format!(
            "{} is not canonical {TRUST_ROOTS_SCHEMA}",
            trust_roots.display()
        ));
    }
    let roots = load_trust_roots(trust_roots)?;
    let revoked = load_revocations(revocations)?;
    let attestation_path = registry_dir.join(ATTESTATION_FILE);
    let text = fs::read_to_string(&attestation_path)
        .map_err(|e| format!("{}: {e}", attestation_path.display()))?;
    let (key_id, signature_bytes) = parse_signature_with_scheme(&text, ATTESTATION_SCHEME)?;
    if revoked.contains(&key_id) {
        return Err(format!("registry attestation signer {key_id} is revoked"));
    }
    let public = roots
        .get(&key_id)
        .ok_or_else(|| format!("registry attestation key-id {key_id} is not in the trust roots"))?;
    let verifying_key = VerifyingKey::from_bytes(public)
        .map_err(|e| format!("invalid attestation trust root public key: {e}"))?;
    verifying_key
        .verify_strict(
            &attestation_message(&index, roots_text.as_bytes(), &revocations_bytes, &mirrors),
            &Signature::from_bytes(&signature_bytes),
        )
        .map_err(|_| {
            "registry attestation mismatch: index, trust roots, revocations or mirrors were modified".to_string()
        })?;
    Ok(key_id)
}

fn canonical_trust_roots_text(doc: &TrustRootsFile) -> String {
    let mut lines = vec![format!("schema = \"{TRUST_ROOTS_SCHEMA}\""), String::new()];
    for entry in &doc.keys {
        lines.push("[[keys]]".to_string());
        lines.push(format!("key_id = \"{}\"", entry.key_id));
        lines.push(format!("public_key = \"{}\"", entry.public_key));
        lines.push(format!(
            "label = {}",
            serde_json::to_string(&entry.label).unwrap_or_else(|_| "\"\"".to_string())
        ));
        lines.push(format!("created = \"{}\"", entry.created));
        lines.push(String::new());
    }
    lines.join("\n")
}

fn attestation_message(
    index: &[u8],
    trust_roots: &[u8],
    revocations: &[u8],
    mirrors: &[u8],
) -> Vec<u8> {
    let mut files = BTreeMap::new();
    for (name, bytes) in [
        ("index.json", index),
        ("mirrors.json", mirrors),
        ("revocations.json", revocations),
        ("trust-roots.toml", trust_roots),
    ] {
        files.insert(
            name.to_string(),
            json!({"bytes": bytes.len(), "sha256": hex(&sha256(bytes))}),
        );
    }
    let payload = json!({"schema": ATTESTATION_SCHEME, "files": files});
    let compact = serde_json::to_string(&payload).unwrap_or_default();
    let mut message = ATTESTATION_SCHEME.as_bytes().to_vec();
    message.push(b'\n');
    message.extend_from_slice(compact.as_bytes());
    message
}

fn decode_hex(text: &str, bytes: usize, field: &str) -> Result<Vec<u8>, String> {
    if text.len() != bytes * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "invalid {field}: expected {bytes} bytes of lowercase hex"
        ));
    }
    if text.to_ascii_lowercase() != text {
        return Err(format!("invalid {field}: hex must be lowercase"));
    }
    (0..bytes)
        .map(|i| {
            u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("invalid {field}: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()
}

fn public_as_array(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    out
}

fn parse_checksums(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("checksums are not UTF-8: {e}"))?;
    // JUDGMENT CALL 53: `packtool` signs the exact bytes of checksums.sha256.
    // On Windows, Python's text-mode write emits CRLF; Git for Windows preserves
    // those signed bytes when `core.autocrlf=false`. Parsing with `split('\n')`
    // left a trailing `\r` on every path (`README.md\r`), so verification
    // reported signed files as missing. Accept uniformly LF or uniformly CRLF
    // line endings and validate the normalized canonical text; reject mixed
    // endings and bare carriage returns instead of guessing at their meaning.
    let lf_count = text.matches('\n').count();
    let crlf_count = text.matches("\r\n").count();
    if crlf_count > 0 && crlf_count != lf_count {
        return Err("checksums.sha256 has mixed line endings".to_string());
    }
    let normalized = if crlf_count > 0 {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    };
    if normalized.contains('\r') {
        return Err("checksums.sha256 has a bare carriage return".to_string());
    }
    let mut entries = BTreeMap::new();
    for (line_no, line) in normalized.split('\n').enumerate() {
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
    if normalized != canonical_checksums(&entries) {
        return Err(
            "checksums.sha256 is not canonical (sorted paths, LF or CRLF, two spaces)".to_string(),
        );
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

fn parse_signature(text: &str) -> Result<(String, [u8; 64]), String> {
    parse_signature_with_scheme(text, SIGNATURE_SCHEME)
}

fn parse_signature_with_scheme(text: &str, scheme: &str) -> Result<(String, [u8; 64]), String> {
    if !text.ends_with('\n') || text.match_indices('\n').count() != 1 {
        return Err(format!(
            "malformed signature file (expected one LF-terminated line with scheme {scheme})"
        ));
    }
    let parts: Vec<_> = text.trim_end_matches('\n').split_whitespace().collect();
    if parts.len() != 3 || parts[0] != scheme {
        return Err(format!(
            "malformed signature file (expected '{scheme} <key-id> <128 hex chars>')"
        ));
    }
    let key_id = decode_hex(parts[1], 8, "key-id")?;
    let key_id = hex(&key_id);
    let sig_bytes = decode_hex(parts[2], 64, "signature")?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&sig_bytes);
    Ok((key_id, signature))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn hmac_is_removed() {
        // HMAC signatures cannot be a public trust root, so the old line is
        // rejected outright instead of being silently accepted as a MAC.
        let old = format!("hmac-sha256 {} {}\n", "a".repeat(16), "b".repeat(64));
        assert!(parse_signature(&old).is_err());
        assert!(parse_signature(&format!(
            "{} {} {}\n",
            SIGNATURE_SCHEME,
            "a".repeat(16),
            "b".repeat(128)
        ))
        .is_ok());
    }

    #[test]
    fn pack_message_is_exact_and_unambiguous() {
        let message = pack_message("demo", "0.1.0", b"digest  file\n");
        assert_eq!(
            message,
            b"meld-pack/ed25519/v1\ndemo\n0.1.0\ndigest  file\n"
        );
    }

    #[test]
    fn malformed_trust_roots_are_rejected() {
        let root = std::env::temp_dir().join(format!("meld-lib-roots-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("trust-roots.toml");
        fs::write(
            &path,
            "[keys]\nkey_id = \"0000000000000000\"\npublic_key = \"00\"\nlabel = \"x\"\ncreated = \"2026-11-20\"\n",
        )
        .unwrap();
        assert!(load_trust_roots(&path).is_err());
        fs::remove_dir_all(root).unwrap();
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

    fn temp(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("meld-lib-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hostile_installed_index_paths_are_rejected() {
        let root = temp("hostile-index");
        let entry = InstalledEntry {
            id: "ruby".to_string(),
            version: "0.1.0".to_string(),
            source: "test".to_string(),
            path: "../../outside".to_string(),
            aliases: vec![],
            tier: "L0".to_string(),
            level: "L0".to_string(),
        };
        assert!(validate_entry_path(&entry).is_err());
        assert!(entry_installed_dir(&root, &entry).is_err());
        assert!(confined_under(&root, &root.join("../../outside")).is_err());

        let mut index = InstalledIndex::empty();
        index.upsert(entry);
        index.save(&root).unwrap();
        assert!(InstalledIndex::load(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packtool_parity_still_excludes_only_known_scratch_dirs() {
        assert!(!SKIP_DIRS.contains(&"target"));
        assert!(SKIP_DIRS.contains(&".git"));
        assert!(SKIP_DIRS.contains(&"__pycache__"));
    }

    #[test]
    fn workspace_root_skips_a_member_manifest() {
        let root = temp("workspace-root");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("member")).unwrap();
        fs::write(
            root.join("member/Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let found = workspace_root(Some(&root.join("member")));
        assert_eq!(found, fs::canonicalize(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_root_honors_an_explicit_plain_directory() {
        let root = temp("explicit-root");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        let plain = root.join("fixtures");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(
            workspace_root(Some(&plain)),
            fs::canonicalize(&plain).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_pin_rejects_ranges_and_partial_versions() {
        assert!(is_exact_version("1.2.3"));
        assert!(is_exact_version("0.1.0-rc.5"));
        assert!(!is_exact_version("1.2"));
        assert!(!is_exact_version(">=1.2.3"));
        assert!(!is_exact_version(" 1.2.3"));
    }

    #[test]
    fn revocations_load_without_any_trust_roots() {
        let root = temp("revocations");
        let path = root.join("revocations.json");
        fs::write(
            &path,
            format!(
                "{{\n  \"revoked\": [\n    {{\n      \"key_id\": \"0123456789abcdef\",\n      \"reason\": \"test\",\n      \"revoked\": \"2026-11-20\"\n    }}\n  ],\n  \"schema\": \"{REVOCATIONS_SCHEMA}\"\n}}\n"
            ),
        )
        .unwrap();
        let revoked = load_revocations(&path).unwrap();
        assert!(revoked.contains("0123456789abcdef"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attestation_message_uses_packtool_field_order() {
        let message = attestation_message(b"index", b"roots", b"revs", b"mirrors");
        let text = String::from_utf8(message).unwrap();
        assert!(text.starts_with(&format!("{ATTESTATION_SCHEME}\n{{")));
        assert!(text.contains("\"files\":{"));
        assert!(text.contains("\"index.json\""));
        assert!(text.contains("\"schema\":\"meld-registry/attestation/ed25519/v1\""));
    }

    #[test]
    fn checksums_accept_uniform_lf_or_crlf_and_reject_mixed_endings() {
        let digest = "a".repeat(64);
        let expected = BTreeMap::from([("README.md".to_string(), digest.clone())]);
        assert_eq!(
            parse_checksums(format!("{digest}  README.md\n").as_bytes()).unwrap(),
            expected
        );
        assert_eq!(
            parse_checksums(format!("{digest}  README.md\r\n").as_bytes()).unwrap(),
            expected
        );
        let mixed = format!("{digest}  README.md\r\n{digest}  worker.sh\n");
        assert_eq!(
            parse_checksums(mixed.as_bytes()).unwrap_err(),
            "checksums.sha256 has mixed line endings"
        );
        let bare = format!("{digest}  README.md\r");
        assert_eq!(
            parse_checksums(bare.as_bytes()).unwrap_err(),
            "checksums.sha256 has a bare carriage return"
        );
    }
}
