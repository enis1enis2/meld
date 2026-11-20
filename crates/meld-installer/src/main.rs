//! `meld-pack`: terminal installer for Meld Lang Libraries.
//!
//! A Lang Library is a Lang Pack plus `library.toml` (see `meld-lib`). This
//! tool is deliberately a data installer: it clones a GitHub repository into a
//! temporary directory, discovers library manifests, verifies the selected
//! packs with the same rules as `packtool`, and copies only verified files.
//!
//! JUDGMENT CALL: the installer shells out to `git` instead of linking an HTTP
//! client. Git is already a Meld development dependency, understands GitHub
//! URLs/credentials, and keeps the trust boundary simple: no pack code is
//! imported, evaluated, or executed during install. Toolchain installation is
//! still forbidden (`project.md` §9); this tool installs language *libraries*.
use meld_lib::{
    canonical_json, discover_imported, installed_dir, read_library_manifest, read_pack_facts,
    validate_library_dir, verify_pack, InstalledEntry, InstalledIndex, ProjectLock, LIBRARY_API,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_SOURCE: &str = "enis1enis2/meld";
const DEFAULT_REF: &str = "main";
const DEFAULT_BIN_NAME: &str = "meld-pack";

fn main() {
    let code = match run(env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err((code, message)) => {
            eprintln!("meld-pack: error: {message}");
            code
        }
    };
    process::exit(code);
}

fn usage() -> &'static str {
    "\
usage: meld-pack <command> [names...] [options]

Commands:
  install <lang>...      fetch, verify and install libraries; import into meld.lock
  remove <lang>...       remove installed libraries and their lock entries
  import <lang>...       add already-installed libraries to meld.lock
  unimport <lang>...     remove entries from meld.lock without deleting files
  list                   list installed libraries (active version per id)
  remote-list            list libraries found in the selected GitHub source
  verify <dir>           verify a local Lang Library directory
  self-install           copy this binary to a bin directory on PATH
  self-uninstall         remove the copied binary

Options:
  --source <repo>        GitHub owner/repo, full URL, or local git repo path
                         (default: enis1enis2/meld; env: MELD_PACK_SOURCE)
  --ref <ref>            branch, tag or fetchable commit (default: main; env: MELD_PACK_REF)
  --key <path>           HMAC key for authenticated verification (env: MELD_PACK_KEY)
  --root <dir>           library root (default: ~/.local/share/meld/libraries; env: MELD_LIB_ROOT)
  --project <dir>        project root containing meld.lock (default: MELD_ROOT or cwd ancestor)
                          temporary clones use TMPDIR or MELD_PACK_TMP if set
  --bin-dir <dir>        destination for self-install
  --force                replace a different install at the same coordinates
  --no-import            install files without updating meld.lock
  --keep-import          remove files without updating meld.lock
  --json                 machine-readable output for list
  -h, --help             show this help

Examples:
  meld-pack install bash typescript
  meld-pack install shell --source enis1enis2/meld --ref main --key ./private.key
  meld-pack list
  meld-pack remove ts
  meld-pack self-install
"
}

type AppResult<T = ()> = Result<T, (i32, String)>;

fn fail<T>(message: impl Into<String>) -> AppResult<T> {
    Err((1, message.into()))
}

fn usage_fail<T>(message: impl Into<String>) -> AppResult<T> {
    Err((2, message.into()))
}

#[derive(Default, Debug)]
struct Options {
    root: Option<PathBuf>,
    project: Option<PathBuf>,
    source: Option<String>,
    reference: Option<String>,
    key: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    force: bool,
    no_import: bool,
    keep_import: bool,
    json: bool,
}

fn option_value(
    args: &[String],
    index: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> AppResult<String> {
    if let Some(value) = inline {
        if value.is_empty() {
            return usage_fail(format!("{flag} needs a value"));
        }
        return Ok(value.to_string());
    }
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| (2, format!("{flag} needs a value")))
}

fn parse_options(args: &[String]) -> AppResult<(Vec<String>, Options)> {
    let mut positional = Vec::new();
    let mut opts = Options::default();
    let mut i = 0;
    let mut literal = false;
    while i < args.len() {
        let arg = args[i].as_str();
        if literal {
            positional.push(args[i].clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            literal = true;
            i += 1;
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg, None), |(f, v)| (f, Some(v)));
        match flag {
            "--root" => opts.root = Some(PathBuf::from(option_value(args, &mut i, flag, inline)?)),
            "--project" => {
                opts.project = Some(PathBuf::from(option_value(args, &mut i, flag, inline)?))
            }
            "--source" => opts.source = Some(option_value(args, &mut i, flag, inline)?),
            "--ref" => opts.reference = Some(option_value(args, &mut i, flag, inline)?),
            "--key" => opts.key = Some(PathBuf::from(option_value(args, &mut i, flag, inline)?)),
            "--bin-dir" => {
                opts.bin_dir = Some(PathBuf::from(option_value(args, &mut i, flag, inline)?))
            }
            "--force" => opts.force = true,
            "--no-import" => opts.no_import = true,
            "--keep-import" => opts.keep_import = true,
            "--json" => opts.json = true,
            _ if arg.starts_with('-') => return usage_fail(format!("unknown option {arg}")),
            _ => positional.push(args[i].clone()),
        }
        i += 1;
    }
    Ok((positional, opts))
}

fn run(args: Vec<String>) -> AppResult {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", usage());
        return Ok(());
    }
    let Some(command) = args.first().cloned() else {
        print!("{}", usage());
        return usage_fail("missing command");
    };
    let (names, opts) = parse_options(&args[1..])?;
    match command.as_str() {
        "install" => cmd_install(&names, &opts),
        "remove" => cmd_remove(&names, &opts),
        "import" => cmd_import(&names, &opts),
        "unimport" => cmd_unimport(&names, &opts),
        "list" => cmd_list(&opts),
        "remote-list" => cmd_remote_list(&opts),
        "verify" => cmd_verify(&names, &opts),
        "self-install" => cmd_self_install(&opts),
        "self-uninstall" => cmd_self_uninstall(&opts),
        _ => usage_fail(format!("unknown command {command:?}")),
    }
}

fn library_root(opts: &Options) -> PathBuf {
    opts.root.clone().unwrap_or_else(meld_lib::library_root)
}

fn project_root(opts: &Options) -> PathBuf {
    if let Some(root) = &opts.project {
        return root.clone();
    }
    if let Ok(root) = env::var("MELD_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.ancestors()
        .find(|p| p.join("project.md").is_file() || p.join("Cargo.toml").is_file())
        .unwrap_or(&cwd)
        .to_path_buf()
}

fn key_path(opts: &Options) -> Option<PathBuf> {
    opts.key.clone().or_else(|| {
        env::var("MELD_PACK_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    })
}

#[derive(Clone, Debug)]
struct SourceSpec {
    url: String,
    label: String,
    reference: String,
}

impl SourceSpec {
    fn parse(raw: &str, reference: Option<&str>) -> AppResult<Self> {
        let (base, inline_ref) = match raw.rfind('@') {
            Some(index)
                if index > 0
                    && !(raw.starts_with("git@") && index == 3)
                    && !raw[..index].ends_with('/') =>
            {
                let rest = &raw[index + 1..];
                if rest.is_empty() {
                    return fail("source reference after '@' is empty");
                }
                (&raw[..index], Some(rest))
            }
            _ => (raw, None),
        };
        let reference = reference
            .or(inline_ref)
            .filter(|r| !r.is_empty())
            .unwrap_or(DEFAULT_REF)
            .to_string();
        if !meld_lib::valid_name(&reference) && !reference.starts_with("refs/") {
            return fail(format!("invalid source reference {reference:?}"));
        }

        let base_path = Path::new(base);
        let (url, label) = if base_path.exists() {
            // JUDGMENT CALL: a local path is accepted so tests and offline
            // mirrors use the exact same install path as GitHub. It is still
            // cloned as a git repository; no files are read from the working
            // tree directly.
            let absolute = base_path
                .canonicalize()
                .map_err(|e| (1, format!("cannot resolve local source {base:?}: {e}")))?;
            let text = absolute.to_string_lossy().into_owned();
            (text.clone(), format!("local:{text}"))
        } else if let Some(slug) = base.strip_prefix("github:") {
            (
                format!("https://github.com/{slug}.git"),
                format!("github:{slug}"),
            )
        } else if base.starts_with("http://")
            || base.starts_with("https://")
            || base.starts_with("git@")
            || base.starts_with("file://")
            || base.ends_with(".git")
        {
            (base.to_string(), base.to_string())
        } else {
            let slug = match base.matches('/').count() {
                0 => format!("{DEFAULT_SOURCE_SPLIT}/{base}"),
                1 => base.to_string(),
                _ => {
                    return fail(format!(
                        "source {base:?} must be owner/repo, a GitHub URL, or a local path"
                    ))
                }
            };
            (
                format!("https://github.com/{slug}.git"),
                format!("github:{slug}"),
            )
        };
        Ok(Self {
            url,
            label: format!("{label}@{reference}"),
            reference,
        })
    }
}

const DEFAULT_SOURCE_SPLIT: &str = "enis1enis2";
const TEMP_DIR_ENV: &str = "MELD_PACK_TMP";

/// Base directory for temporary git checkouts.
///
/// JUDGMENT CALL: `env::temp_dir()` is the normal choice, but some sandboxes
/// run `git` in a chroot where the host's `/tmp` is not mounted. In that case
/// git succeeds while the Rust side reads an empty directory, which looks like
/// "no libraries found". `MELD_PACK_TMP` (and the usual `TMPDIR` fallback)
/// lets callers name a directory visible to both processes; the post-checkout
/// visibility check below turns any remaining mismatch into an actionable
/// error instead of a silent empty catalog.
fn temp_base() -> PathBuf {
    if let Ok(dir) = env::var(TEMP_DIR_ENV) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    env::temp_dir()
}

struct TempClone {
    dir: PathBuf,
}

impl Drop for TempClone {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn git(args: &[String]) -> AppResult {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| (1, format!("cannot run git: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err((
        1,
        format!(
            "git {} failed: {}",
            args.first().map(String::as_str).unwrap_or("command"),
            detail
        ),
    ))
}

impl TempClone {
    fn fetch(spec: &SourceSpec) -> AppResult<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = temp_base().join(format!("meld-pack-{}-{nanos}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|e| (1, format!("{}: {e}", dir.display())))?;
        let clone = Self { dir };
        let str_dir = clone.dir.to_string_lossy().into_owned();
        git(&["init".into(), "-q".into(), str_dir.clone()])?;
        git(&[
            "-C".into(),
            str_dir.clone(),
            "remote".into(),
            "add".into(),
            "origin".into(),
            spec.url.clone(),
        ])?;
        let fetch = git(&[
            "-C".into(),
            str_dir.clone(),
            "fetch".into(),
            "-q".into(),
            "--depth".into(),
            "1".into(),
            "origin".into(),
            spec.reference.clone(),
        ]);
        if let Err(err) = fetch {
            let _ = fs::remove_dir_all(&clone.dir);
            return Err(err);
        }
        if let Err(err) = git(&[
            "-C".into(),
            str_dir,
            "checkout".into(),
            "-q".into(),
            "FETCH_HEAD".into(),
        ]) {
            let _ = fs::remove_dir_all(&clone.dir);
            return Err(err);
        }
        if !clone.dir.join(".git").is_dir() {
            let message = format!(
                "temporary clone {} is not visible to meld-pack after git checkout; set TMPDIR or {TEMP_DIR_ENV} to a directory shared with git (for example /mnt/data/opencode/tmp)",
                clone.dir.display()
            );
            let _ = fs::remove_dir_all(&clone.dir);
            return fail(message);
        }
        Ok(clone)
    }
}

#[derive(Clone, Debug)]
struct RemoteLibrary {
    id: String,
    name: String,
    version: String,
    aliases: Vec<String>,
    tier: String,
    level: String,
    dir: PathBuf,
}

fn collect_library_dirs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if matches!(name.as_str(), ".git" | "target" | "__pycache__") {
                continue;
            }
            if path.join("library.toml").is_file() {
                out.push(path);
            } else {
                collect_library_dirs(&path, out);
            }
        }
    }
}

fn catalog_remote(clone: &Path) -> AppResult<Vec<RemoteLibrary>> {
    let mut dirs = Vec::new();
    collect_library_dirs(clone, &mut dirs);
    let mut errors = Vec::new();
    let mut libraries = Vec::new();
    for dir in dirs {
        match read_library_manifest(&dir).and_then(|manifest| {
            let (tier, level, _) = read_pack_facts(&dir)?;
            Ok((manifest, tier, level))
        }) {
            Ok((manifest, tier, level)) => libraries.push(RemoteLibrary {
                id: manifest.id,
                name: manifest.name,
                version: manifest.version,
                aliases: manifest.aliases,
                tier,
                level,
                dir,
            }),
            Err(err) => errors.push(format!("{}: {err}", dir.display())),
        }
    }
    if !errors.is_empty() {
        return fail(format!(
            "invalid remote libraries:\n  {}",
            errors.join("\n  ")
        ));
    }
    libraries.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
    Ok(libraries)
}

fn resolve_remote<'a>(libraries: &'a [RemoteLibrary], name: &str) -> AppResult<&'a RemoteLibrary> {
    if let Some(found) = libraries.iter().find(|lib| lib.id == name) {
        return Ok(found);
    }
    let matches: Vec<_> = libraries
        .iter()
        .filter(|lib| lib.aliases.iter().any(|alias| alias == name))
        .collect();
    match matches.len() {
        0 => fail(format!(
            "unknown library or alias {name:?} in remote source"
        )),
        1 => Ok(matches[0]),
        _ => fail(format!(
            "ambiguous alias {name:?}: {}",
            matches
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn select_remote<'a>(
    libraries: &'a [RemoteLibrary],
    names: &[String],
) -> AppResult<Vec<&'a RemoteLibrary>> {
    let mut selected = Vec::new();
    for name in names {
        let lib = resolve_remote(libraries, name)?;
        if !selected
            .iter()
            .any(|item: &&RemoteLibrary| item.id == lib.id)
        {
            selected.push(lib);
        }
    }
    Ok(selected)
}

fn source_spec(opts: &Options) -> AppResult<SourceSpec> {
    let source = opts
        .source
        .clone()
        .or_else(|| env::var("MELD_PACK_SOURCE").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_SOURCE.to_string());
    SourceSpec::parse(&source, opts.reference.as_deref())
}

fn checksums_bytes(dir: &Path) -> AppResult<Vec<u8>> {
    fs::read(dir.join(meld_lib::CHECKSUM_FILE)).map_err(|e| {
        (
            1,
            format!("{}: {e}", dir.join(meld_lib::CHECKSUM_FILE).display()),
        )
    })
}

fn cmd_install(names: &[String], opts: &Options) -> AppResult {
    if names.is_empty() {
        return usage_fail("install needs at least one library name");
    }
    let spec = source_spec(opts)?;
    let clone = TempClone::fetch(&spec)?;
    let libraries = catalog_remote(&clone.dir)?;
    let selected = select_remote(&libraries, names)?;
    let root = library_root(opts);
    let project = project_root(opts);
    let key = key_path(opts);
    let mut index = InstalledIndex::load(&root).map_err(|e| (1, e))?;
    let mut lock = if opts.no_import {
        None
    } else {
        Some(ProjectLock::load(&project).map_err(|e| (1, e))?)
    };

    for lib in selected {
        // JUDGMENT CALL: every selected pack is verified in the clone *and* in
        // its final destination. The second check catches copy-time corruption
        // and keeps the installed bytes authoritative for later runs.
        let manifest = validate_library_dir(&lib.dir)
            .map_err(|e| (1, format!("{}: {e}", lib.dir.display())))?;
        verify_pack(&lib.dir, key.as_deref())
            .map_err(|e| (1, format!("{}: {e}", lib.dir.display())))?;
        let dest = installed_dir(&root, &manifest.id, &manifest.version);
        if dest.exists() {
            if checksums_bytes(&dest).ok().as_deref() == checksums_bytes(&lib.dir).ok().as_deref() {
                println!("{} {}: already installed", manifest.id, manifest.version);
            } else if opts.force {
                if dest.is_dir() {
                    fs::remove_dir_all(&dest)
                        .map_err(|e| (1, format!("{}: {e}", dest.display())))?;
                } else {
                    fs::remove_file(&dest).map_err(|e| (1, format!("{}: {e}", dest.display())))?;
                }
            } else {
                return fail(format!(
                    "different install already exists at {}; bump the version or pass --force",
                    dest.display()
                ));
            }
        }
        if !dest.exists() {
            meld_lib::copy_dir(&lib.dir, &dest).map_err(|e| (1, e))?;
        }
        verify_pack(&dest, key.as_deref()).map_err(|e| (1, format!("{}: {e}", dest.display())))?;
        let (tier, level, _) = read_pack_facts(&dest).map_err(|e| (1, e))?;
        index.upsert(InstalledEntry {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            source: spec.label.clone(),
            path: format!("{}/{}", manifest.id, manifest.version),
            aliases: manifest.aliases.clone(),
            tier,
            level,
        });
        index.save(&root).map_err(|e| (1, e))?;
        if let Some(lock) = lock.as_mut() {
            lock.libraries
                .insert(manifest.id.clone(), manifest.version.clone());
            lock.save(&project).map_err(|e| (1, e))?;
        }
        println!(
            "installed {} {} ({} -> {})",
            manifest.id,
            manifest.version,
            spec.label,
            dest.display()
        );
    }
    if let Some(lock) = lock.as_ref() {
        println!(
            "imported into {}",
            project.join(meld_lib::LOCK_FILE).display()
        );
        let _ = lock;
    }
    // JUDGMENT CALL: discovering the new imports is a cheap consistency check.
    // Missing files are skipped by design, so this cannot fail a successful
    // install; it is here to document the runtime handoff.
    let _ = discover_imported(&project, &[root]);
    Ok(())
}

fn cmd_remove(names: &[String], opts: &Options) -> AppResult {
    if names.is_empty() {
        return usage_fail("remove needs at least one library name");
    }
    let root = library_root(opts);
    let project = project_root(opts);
    let mut index = InstalledIndex::load(&root).map_err(|e| (1, e))?;
    let mut lock = if opts.keep_import {
        None
    } else {
        Some(ProjectLock::load(&project).map_err(|e| (1, e))?)
    };
    for name in names {
        let entry = index.resolve(name).map_err(|e| (1, e))?.clone();
        let target = root.join(&entry.path);
        if target.is_dir() {
            fs::remove_dir_all(&target).map_err(|e| (1, format!("{}: {e}", target.display())))?;
        } else if target.exists() {
            fs::remove_file(&target).map_err(|e| (1, format!("{}: {e}", target.display())))?;
        }
        let parent = root.join(&entry.id);
        if parent.is_dir()
            && fs::read_dir(&parent)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            let _ = fs::remove_dir(&parent);
        }
        index.remove(&entry.id);
        index.save(&root).map_err(|e| (1, e))?;
        if let Some(lock) = lock.as_mut() {
            lock.libraries.remove(&entry.id);
            lock.libraries.remove(name);
            lock.save(&project).map_err(|e| (1, e))?;
        }
        println!("removed {} {}", entry.id, entry.version);
    }
    Ok(())
}

fn cmd_import(names: &[String], opts: &Options) -> AppResult {
    if names.is_empty() {
        return usage_fail("import needs at least one library name");
    }
    let root = library_root(opts);
    let project = project_root(opts);
    let index = InstalledIndex::load(&root).map_err(|e| (1, e))?;
    let mut lock = ProjectLock::load(&project).map_err(|e| (1, e))?;
    for name in names {
        let entry = index.resolve(name).map_err(|e| (1, e))?;
        lock.libraries
            .insert(entry.id.clone(), entry.version.clone());
        println!("imported {} {}", entry.id, entry.version);
    }
    lock.save(&project).map_err(|e| (1, e))?;
    println!("wrote {}", project.join(meld_lib::LOCK_FILE).display());
    Ok(())
}

fn cmd_unimport(names: &[String], opts: &Options) -> AppResult {
    if names.is_empty() {
        return usage_fail("unimport needs at least one library name");
    }
    let root = library_root(opts);
    let project = project_root(opts);
    let index = InstalledIndex::load(&root).map_err(|e| (1, e))?;
    let mut lock = ProjectLock::load(&project).map_err(|e| (1, e))?;
    for name in names {
        let id = index
            .resolve(name)
            .map(|e| e.id.clone())
            .unwrap_or_else(|_| name.clone());
        lock.libraries.remove(&id);
        lock.libraries.remove(name);
        println!("unimported {id}");
    }
    lock.save(&project).map_err(|e| (1, e))?;
    println!("wrote {}", project.join(meld_lib::LOCK_FILE).display());
    Ok(())
}

fn cmd_list(opts: &Options) -> AppResult {
    let root = library_root(opts);
    let index = InstalledIndex::load(&root).map_err(|e| (1, e))?;
    if opts.json {
        println!("{}", canonical_json(&index).map_err(|e| (1, e))?.trim_end());
        return Ok(());
    }
    println!("library root: {}", root.display());
    if index.libraries.is_empty() {
        println!("(no installed libraries)");
        return Ok(());
    }
    println!(
        "{:<16} {:<10} {:<5} {:<5} {:<8} SOURCE",
        "ID", "VERSION", "TIER", "LEVEL", "STATUS"
    );
    for entry in index.libraries.values() {
        let status = if root.join(&entry.path).is_dir() {
            "ok"
        } else {
            "MISSING"
        };
        println!(
            "{:<16} {:<10} {:<5} {:<5} {:<8} {}",
            entry.id, entry.version, entry.tier, entry.level, status, entry.source
        );
    }
    Ok(())
}

fn cmd_remote_list(opts: &Options) -> AppResult {
    let spec = source_spec(opts)?;
    let clone = TempClone::fetch(&spec)?;
    let libraries = catalog_remote(&clone.dir)?;
    println!("source: {}", spec.label);
    if libraries.is_empty() {
        println!("(no libraries found; every lang library needs {LIBRARY_API} in library.toml)");
        return Ok(());
    }
    println!(
        "{:<16} {:<10} {:<5} {:<5} NAME",
        "ID", "VERSION", "TIER", "LEVEL"
    );
    for lib in libraries {
        println!(
            "{:<16} {:<10} {:<5} {:<5} {}",
            lib.id, lib.version, lib.tier, lib.level, lib.name
        );
    }
    Ok(())
}

fn cmd_verify(names: &[String], opts: &Options) -> AppResult {
    let Some(dir) = names.first() else {
        return usage_fail("verify needs a library directory");
    };
    let path = PathBuf::from(dir);
    let key = key_path(opts);
    let manifest = validate_library_dir(&path).map_err(|e| (1, e))?;
    let result = verify_pack(&path, key.as_deref()).map_err(|e| (1, e))?;
    let (tier, level, _) = read_pack_facts(&path).map_err(|e| (1, e))?;
    println!("library: {} {}", manifest.id, manifest.version);
    println!("name: {}", manifest.name);
    println!("api: {}", manifest.api);
    println!("tier: {}  level: {}", tier, level);
    println!("strict: {}", result.strict);
    println!("checksums: {} file(s) verified", result.file_count);
    match result.signature.as_str() {
        "absent" => println!("signature: absent"),
        "present-unverified" => println!(
            "signature: present (key-id {}) but not verified; pass --key for MAC check",
            result.key_id.unwrap_or_default()
        ),
        _ => println!(
            "signature: verified (key-id {})",
            result.key_id.unwrap_or_default()
        ),
    }
    println!("OK");
    Ok(())
}

fn cmd_self_install(opts: &Options) -> AppResult {
    let bin_dir = opts
        .bin_dir
        .clone()
        .or_else(|| {
            env::var("XDG_BIN_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            env::var("HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|home| PathBuf::from(home).join(".local/bin"))
        })
        .ok_or_else(|| {
            (
                1,
                "cannot determine bin directory; pass --bin-dir".to_string(),
            )
        })?;
    fs::create_dir_all(&bin_dir).map_err(|e| (1, format!("{}: {e}", bin_dir.display())))?;
    let exe = env::current_exe().map_err(|e| (1, format!("cannot locate meld-pack: {e}")))?;
    let dest = bin_dir.join(format!("{DEFAULT_BIN_NAME}{}", env::consts::EXE_SUFFIX));
    if exe != dest {
        fs::copy(&exe, &dest)
            .map_err(|e| (1, format!("{} -> {}: {e}", exe.display(), dest.display())))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
            .map_err(|e| (1, format!("{}: {e}", dest.display())))?;
    }
    println!("installed meld-pack -> {}", dest.display());
    println!(
        "ensure {} is on PATH, then run `meld-pack list`",
        bin_dir.display()
    );
    Ok(())
}

fn cmd_self_uninstall(opts: &Options) -> AppResult {
    let bin_dir = opts
        .bin_dir
        .clone()
        .or_else(|| {
            env::var("XDG_BIN_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    env::var("HOME")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .map(|home| PathBuf::from(home).join(".local/bin"))
                })
        })
        .ok_or_else(|| (1, "cannot determine bin directory".to_string()))?;
    let dest = bin_dir.join(format!("{DEFAULT_BIN_NAME}{}", env::consts::EXE_SUFFIX));
    if dest.exists() {
        fs::remove_file(&dest).map_err(|e| (1, format!("{}: {e}", dest.display())))?;
        println!("removed {}", dest.display());
    } else {
        println!("not installed: {}", dest.display());
    }
    Ok(())
}
