//! Meld CLI: check / run / fmt / repl / doctor / pack new (LSP is meld-lsp).
use meld_core::binding::{self, Import};
use meld_core::compose::{self, Unit};
use meld_core::error::MeldError;
use meld_core::region_end_spec::RegionEndSpec;
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_core::splitter;
use meld_core::syntax::SyntaxLayer;
use meld_sandbox::{
    effective_policy, parse_grant, parse_pack, parse_runner, platform_backend, Access, Capability,
    CommandSpec, Grant, SafeRoot, SandboxError, SandboxPolicy,
};
use meld_value::SourceStyle;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("check") => cmd_check(args.get(1).map(String::as_str)),
        Some("run") => cmd_run(args.get(1).map(String::as_str)),
        Some("doctor") => cmd_doctor(),
        Some("lib") | Some("library") => cmd_library(),
        Some("fmt") => {
            let check = args.iter().any(|a| a == "--check");
            let path = args
                .iter()
                .skip(1)
                .find(|a| a.as_str() != "--check")
                .map(String::as_str);
            cmd_fmt(path, check)
        }
        Some("repl") => cmd_repl(),
        Some("pack") if args.get(1).map(String::as_str) == Some("new") => {
            cmd_pack_new(args.get(2).map(String::as_str))
        }
        _ => {
            eprintln!("usage: meld <check|run|fmt [--check]|repl|doctor|lib|pack> [file|lang]");
            2
        }
    };
    std::process::exit(code);
}

/// A T2 launch profile discovered from a Lang Pack's `runner.toml`.
/// **JUDGMENT CALL:** runtime command discovery is data-driven so a third-party
/// pack can be installed without editing this crate. Only T2/meld-t2/1 packs
/// are auto-launched; other tiers have explicit APIs and are not guessed.
#[derive(Clone, Debug)]
struct RunnerSpec {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
}

#[derive(Clone, Debug)]
struct PackInfo {
    id: String,
    aliases: Vec<String>,
    tier: String,
    level: String,
    dir: PathBuf,
    runner: Option<RunnerSpec>,
    inline_style: Option<SourceStyle>,
    /// Data-only scanner profile; never a command or Wasm module to execute.
    region_end: RegionEndSpec,
    /// Pack maximum and runner constraints. The CLI intersects these with the
    /// file default and any region grant before spawning a worker.
    pack_policy: SandboxPolicy,
    runner_policy: SandboxPolicy,
    /// Unreadable or inconsistent security manifests are retained as an error
    /// and surfaced at launch; they must never be treated as a weaker policy.
    policy_error: Option<String>,
}

impl PackInfo {
    fn syntax_only(&self) -> bool {
        self.tier == "L0" || self.level == "L0" || self.runner.is_none()
    }
}

fn project_root() -> PathBuf {
    // JUDGMENT CALL: manifest paths are validated as declarative paths, so the
    // root itself must be canonical before it is passed to the policy parser.
    // `MELD_ROOT` normally contains `..` in test harnesses because it is
    // derived from CARGO_MANIFEST_DIR plus a relative parent.
    let canonical = |path: PathBuf| std::fs::canonicalize(&path).unwrap_or(path);
    if let Ok(root) = std::env::var("MELD_ROOT") {
        return canonical(PathBuf::from(root));
    }
    std::env::current_dir()
        .ok()
        .and_then(|c| {
            c.ancestors()
                .find(|p| p.join("project.md").is_file() || p.join("Cargo.toml").is_file())
                .map(Path::to_path_buf)
        })
        .map(canonical)
        .unwrap_or_else(|| canonical(PathBuf::from(".")))
}

fn rebase_arg(arg: &str, root: &Path, pack_dir: &Path) -> String {
    let p = Path::new(arg);
    if p.is_absolute() {
        return arg.to_string();
    }
    // JUDGMENT CALL: prefer paths inside the discovered pack, then the
    // pack directory by basename, and only then the project root. The old
    // order could silently run a built-in worker when `meld.lock` pinned an
    // updated installed library with the same id (spec/libraries.md §5).
    let local = pack_dir.join(p);
    if local.exists() {
        return local.to_string_lossy().into_owned();
    }
    if let Some(name) = p.file_name() {
        let local_name = pack_dir.join(name);
        if local_name.exists() {
            return local_name.to_string_lossy().into_owned();
        }
    }
    let rooted = root.join(p);
    if rooted.exists() {
        return rooted.to_string_lossy().into_owned();
    }
    arg.to_string()
}

fn load_pack_dir(root: &Path, pack_dir: &Path) -> Option<PackInfo> {
    let manifest_path = pack_dir.join("pack.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest = toml::from_str::<toml::Value>(&manifest_text).ok()?;
    let id = manifest.get("id")?.as_str()?.to_string();
    let aliases = manifest
        .get("aliases")
        .and_then(toml::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let tier = manifest
        .get("tier")
        .and_then(toml::Value::as_str)
        .unwrap_or("L0")
        .to_string();
    let level = manifest
        .get("level")
        .and_then(toml::Value::as_str)
        .unwrap_or("L0")
        .to_string();

    // JUDGMENT CALL: an unreadable or inconsistent security manifest is kept as
    // an explicit launch error. Falling back to a weaker policy would turn a
    // malformed pack into extra host authority.
    let mut policy_error = None;
    let pack_policy = match parse_pack(&manifest_text, root) {
        Ok(policy) => policy,
        Err(e) => {
            policy_error = Some(format!("{}: {e}", manifest_path.display()));
            SandboxPolicy::deny_all("pack(invalid)")
        }
    };

    let runner_path = pack_dir.join("runner.toml");
    let runner_text = std::fs::read_to_string(&runner_path).ok();
    let (runner, runner_policy) = match runner_text.as_deref() {
        Some(text) => {
            let policy = match parse_runner(text, root) {
                Ok(policy) => policy,
                Err(e) => {
                    policy_error = Some(format!("{}: {e}", runner_path.display()));
                    SandboxPolicy::deny_all("runner(invalid)")
                }
            };
            let spec = toml::from_str::<toml::Value>(text).ok().and_then(|runner| {
                if runner.get("tier").and_then(toml::Value::as_str) != Some("T2")
                    || runner.get("protocol").and_then(toml::Value::as_str) != Some("meld-t2/1")
                {
                    return None;
                }
                let program = runner.get("command")?.as_str()?.to_string();
                let args: Vec<String> = runner
                    .get("args")
                    .and_then(toml::Value::as_array)
                    .map(|xs| {
                        xs.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let cwd_text = runner
                    .get("cwd")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("project");
                let cwd = resolve_runner_cwd(root, cwd_text).ok()?;
                Some(RunnerSpec {
                    program: rebase_arg(&program, root, pack_dir),
                    args: args.iter().map(|a| rebase_arg(a, root, pack_dir)).collect(),
                    cwd,
                })
            });
            (spec, policy)
        }
        None => (None, SandboxPolicy::deny_all("runner(missing)")),
    };

    let inline_style = std::fs::read_to_string(pack_dir.join("inline.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|doc| {
            doc.get("inline")
                .and_then(|v| v.get("style"))
                .and_then(toml::Value::as_str)
                .and_then(SourceStyle::from_id)
        });

    Some(PackInfo {
        id,
        aliases,
        tier,
        level,
        dir: pack_dir.to_path_buf(),
        runner,
        inline_style,
        region_end: RegionEndSpec::from_dir(pack_dir).unwrap_or_default(),
        pack_policy,
        runner_policy,
        policy_error,
    })
}

fn insert_pack(out: &mut HashMap<String, PackInfo>, root: &Path, pack_dir: &Path) {
    let Some(info) = load_pack_dir(root, pack_dir) else {
        return;
    };
    let mut names = vec![info.id.clone()];
    names.extend(info.aliases.clone());
    for name in names.into_iter().filter(|n| !n.is_empty()) {
        out.insert(name, info.clone());
    }
}

fn discover_packs(root: &Path) -> HashMap<String, PackInfo> {
    let mut out = HashMap::new();
    let mut scan_roots = vec![root.join("packs")];
    let mut extra_roots = Vec::new();
    // JUDGMENT CALL 43: MELD_PACKS uses the host path-list separator via
    // `std::env::split_paths`. A literal ':' split loses a Windows drive letter
    // and turns `D:\...` into a drive-relative path, so a pack in %TEMP% on C:
    // was silently ignored and its scanner metadata never registered.
    if let Ok(extra) = std::env::var("MELD_PACKS") {
        for path in std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()) {
            extra_roots.push(path.clone());
            scan_roots.push(path);
        }
    }
    for scan_root in scan_roots {
        let mut dirs = Vec::new();
        if scan_root.join("pack.toml").is_file() {
            dirs.push(scan_root.clone());
        }
        if let Ok(entries) = std::fs::read_dir(&scan_root) {
            dirs.extend(entries.flatten().map(|e| e.path()));
        }
        for pack_dir in dirs {
            insert_pack(&mut out, root, &pack_dir);
        }
    }
    // JUDGMENT CALL: imported libraries are inserted last so a project lock
    // wins over a same-named built-in pack. A malformed lock is reported as a
    // warning instead of breaking check/run; `meld-pack list` gives details.
    match meld_lib::discover_imported(root, &extra_roots) {
        Ok(imported) => {
            for (_, dir) in imported {
                insert_pack(&mut out, root, &dir);
            }
        }
        Err(e) => eprintln!("meld: warning: cannot read {}: {e}", meld_lib::LOCK_FILE),
    }
    out
}

/// Build one deterministic scanner registry from built-ins plus every
/// discovered pack's `region_end.toml`. Packs are visited in id order and
/// aliases are sorted, so MELD_PACKS/import discovery cannot make boundaries
/// depend on directory iteration order.
fn scanner_registry(packs: &HashMap<String, PackInfo>) -> splitter::ScannerRegistry {
    let mut registry = splitter::ScannerRegistry::default_registry();
    let mut infos: Vec<&PackInfo> = packs.values().collect();
    infos.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen: Vec<String> = Vec::new();
    for info in infos {
        if seen.contains(&info.id) {
            continue;
        }
        seen.push(info.id.clone());
        let mut names = vec![info.id.clone()];
        names.extend(info.aliases.iter().cloned());
        names.sort();
        names.dedup();
        info.region_end.register(&mut registry, names);
    }
    registry
}

fn read_file(path: Option<&str>) -> Result<(PathBuf, String), i32> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("meld: missing file argument");
            return Err(2);
        }
    };
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("meld: {e}");
            return Err(1);
        }
    };
    Ok((path, src))
}

fn cmd_check(path: Option<&str>) -> i32 {
    let (p, src) = match read_file(path) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let packs = discover_packs(&project_root());
    let registry = scanner_registry(&packs);
    let doc = match splitter::split(&src, &registry) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    // JUDGMENT CALL: real published tree-sitter grammars live in
    // meld-grammars, not in the core. `meld check` parses every region whose
    // language is bundled and reports ERROR nodes; unknown languages are left
    // to their pack so an L0 pack never hard-fails the CLI.
    let mut syntax = SyntaxLayer::new();
    meld_grammars::register_all(&mut syntax);

    let mut problems = 0;
    for r in &doc.regions {
        if r.confidence < 0.75 {
            eprintln!(
                "{}:{}: low detection confidence ({:.2}) in region {} — pass --assume-detected to run",
                p.display(),
                r.full_start,
                r.confidence,
                r.id
            );
            problems += 1;
        }
        if r.kind != splitter::RegionKind::Header && r.start > r.end {
            eprintln!("{}: invalid region span {}", p.display(), r.id);
            problems += 1;
        }
        // JUDGMENT CALL: untagged regions are prose or partial fragments
        // surrounding inline islands (e.g. `print(` + `)`); they are not
        // standalone source and must not produce syntax errors.
        if r.kind == splitter::RegionKind::Header
            || r.kind == splitter::RegionKind::Untagged
            || r.lang == "text"
        {
            continue;
        }
        match syntax.parse_region(&doc, r) {
            Ok(info) => {
                println!(
                    "  {}: {} root={} named={}",
                    r.id, r.lang, info.root_kind, info.named_nodes
                );
                if info.has_error {
                    eprintln!("{}: syntax ERROR in region {}", p.display(), r.id);
                    problems += 1;
                }
            }
            Err(MeldError::UnknownLang(_)) => {}
            Err(e) => {
                eprintln!("{e}");
                problems += 1;
            }
        }
    }
    match binding::resolve(&doc) {
        Ok(b) => match binding::execution_plan(&doc, &b) {
            Ok(plan) => println!(
                "{}: {} regions, plan: {}",
                p.display(),
                doc.regions.len(),
                plan.join(" → ")
            ),
            Err(e) => {
                eprintln!("{e}");
                problems += 1;
            }
        },
        Err(e) => {
            eprintln!("{e}");
            problems += 1;
        }
    }
    if problems == 0 {
        0
    } else {
        1
    }
}

fn sandbox_error(error: SandboxError) -> MeldError {
    match error {
        SandboxError::Denied(message) | SandboxError::Unsupported(message) => {
            MeldError::SandboxDenied(message)
        }
        other => MeldError::UnknownLang(format!("invalid sandbox policy: {other}")),
    }
}

/// `cwd = "project"` means the discovered project root. Any other value must be
/// a relative path inside it; absolute paths and `..` are hard errors.
fn resolve_runner_cwd(root: &Path, text: &str) -> Result<PathBuf, MeldError> {
    if text == "project" {
        return root
            .canonicalize()
            .map_err(|e| MeldError::UnknownLang(format!("cannot resolve project root: {e}")));
    }
    let candidate = Path::new(text);
    if candidate.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(MeldError::SandboxDenied(format!(
            "runner cwd escapes the project: {text}"
        )));
    }
    root.join(candidate)
        .canonicalize()
        .map_err(|e| MeldError::UnknownLang(format!("cannot resolve runner cwd {text}: {e}")))
}

/// The file default is the project working tree: readable and writable by the
/// worker, never networked. Pack and runner manifests can only narrow it.
fn file_default_policy(root: &Path) -> Result<SandboxPolicy, MeldError> {
    let root = root
        .canonicalize()
        .map_err(|e| MeldError::UnknownLang(format!("cannot resolve project root: {e}")))?;
    let mut policy = SandboxPolicy::deny_all("file-default");
    policy
        .fs
        .add_root(SafeRoot::new("project", &root).map_err(sandbox_error)?)
        .map_err(sandbox_error)?;
    policy
        .fs
        .grant("project", "**", Access::Read)
        .map_err(sandbox_error)?;
    policy
        .fs
        .grant("project", "**", Access::Write)
        .map_err(sandbox_error)?;
    for capability in [Capability::FsRead, Capability::FsWrite, Capability::Stdio] {
        policy.capabilities.allow(capability);
    }
    policy.validate().map_err(sandbox_error)?;
    Ok(policy)
}

/// Extract `grant(...)` calls from header and glue text only. Native region
/// bodies are skipped so code strings cannot be mistaken for policy, and an
/// unterminated or malformed grant is a hard error.
fn grant_calls(text: &str) -> Result<Vec<String>, MeldError> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        if &bytes[i..i + 6] != b"grant(" {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 6;
        let mut depth = 1usize;
        let mut quote: Option<u8> = None;
        let mut escaped = false;
        while j < bytes.len() && depth > 0 {
            let byte = bytes[j];
            match quote {
                Some(q) => {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == q {
                        quote = None;
                    }
                }
                None => match byte {
                    b'"' | b'\'' => quote = Some(byte),
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                },
            }
            j += 1;
        }
        if depth != 0 {
            return Err(MeldError::SandboxDenied(
                "unterminated grant(...) in Meld source".to_string(),
            ));
        }
        out.push(text[start..j].to_string());
        i = j;
    }
    Ok(out)
}

fn source_grants(src: &str, doc: &splitter::SplitDocument) -> Result<Vec<Grant>, MeldError> {
    let mut skipped: Vec<(usize, usize)> = doc
        .regions
        .iter()
        .filter(|r| {
            !matches!(
                r.kind,
                splitter::RegionKind::Header | splitter::RegionKind::Untagged
            )
        })
        .map(|r| (r.full_start, r.full_end))
        .collect();
    skipped.sort_unstable();
    let mut calls = Vec::new();
    let mut cursor = 0;
    for (start, end) in skipped {
        if start > cursor {
            if let Some(segment) = src.get(cursor..start) {
                calls.extend(grant_calls(segment)?);
            }
        }
        cursor = cursor.max(end);
    }
    if cursor < src.len() {
        if let Some(segment) = src.get(cursor..) {
            calls.extend(grant_calls(segment)?);
        }
    }
    calls
        .iter()
        .map(|call| parse_grant(call).map_err(sandbox_error))
        .collect()
}

/// Environment variables passed to a worker are an explicit allowlist. The
/// sandbox clears the rest of the host environment before exec.
fn worker_environment(spec: &mut CommandSpec) {
    // JUDGMENT CALL 44: Windows runtimes locate the crypto provider through
    // SystemRoot and scratch storage through TEMP/TMP. Without these OS
    // locators, Node aborts during startup with `ncrypto::CSPRNG(nullptr, 0)`
    // after the sandbox clears the environment. They are not secrets; every
    // other host variable stays blocked.
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "PYTHONPATH",
        "NODE_PATH",
        "LUA_PATH",
        #[cfg(windows)]
        "SYSTEMROOT",
        #[cfg(windows)]
        "TEMP",
        #[cfg(windows)]
        "TMP",
    ] {
        if let Ok(value) = std::env::var(key) {
            spec.env.insert(key.to_string(), value);
        }
    }
}

#[cfg(debug_assertions)]
fn test_unsandboxing_enabled() -> bool {
    std::env::var("MELD_TEST_UNSANDBOXED").as_deref() == Ok("1")
}

#[cfg(not(debug_assertions))]
fn test_unsandboxing_enabled() -> bool {
    false
}

fn spawn_worker(
    pack: &PackInfo,
    t3: &Arc<meld_t3::T3Backend>,
    grants: &[Grant],
) -> meld_core::error::Result<meld_t2::T2Backend> {
    if let Some(error) = &pack.policy_error {
        return Err(MeldError::SandboxDenied(format!("{}: {error}", pack.id)));
    }
    let runner = pack
        .runner
        .as_ref()
        .ok_or_else(|| MeldError::UnknownLang(format!("{} has no T2 runner.toml", pack.id)))?;
    let root = project_root();
    let file_default = file_default_policy(&root)?;
    let mut effective =
        effective_policy(&pack.pack_policy, &pack.runner_policy, &file_default, None)
            .map_err(sandbox_error)?;
    for grant in grants {
        effective = effective.apply_grant(grant).map_err(sandbox_error)?;
    }
    effective.validate().map_err(sandbox_error)?;

    let mut spec =
        CommandSpec::new(&runner.program, runner.args.iter().cloned()).with_cwd(&runner.cwd);
    worker_environment(&mut spec);

    // JUDGMENT CALL: this switch exists only in debug test binaries so fixtures
    // can run on hosts without namespace/cgroup privileges. It warns loudly,
    // is compiled out of release builds, and never affects production launch.
    let mut backend = if test_unsandboxing_enabled() {
        eprintln!(
            "meld: warning: MELD_TEST_UNSANDBOXED is set; T2 worker for {} is NOT OS-confined",
            pack.id
        );
        meld_t2::T2Backend::spawn_with_spec(&spec)?
    } else {
        let backend = platform_backend();
        let probe = backend.probe();
        probe.require(&effective).map_err(sandbox_error)?;
        meld_t2::T2Backend::spawn_sandboxed(&backend, &spec, &effective)?
    };
    let t3 = Arc::clone(t3);
    backend.set_host_call(move |name, values| t3.call_value(name, values));
    Ok(backend)
}

type WorkerCache = HashMap<String, meld_t2::T2Backend>;

/// Immutable inputs shared by every inline expression in one composite region.
/// Grouping them keeps `render_inline` under the lint's argument limit without
/// changing ownership: the worker cache remains exclusively mutable.
struct InlineContext<'a> {
    packs: &'a HashMap<String, PackInfo>,
    bridge: &'a Bridge,
    t3: &'a Arc<meld_t3::T3Backend>,
    grants: &'a [Grant],
}

fn backend_for<'a>(
    cache: &'a mut WorkerCache,
    packs: &HashMap<String, PackInfo>,
    t3: &Arc<meld_t3::T3Backend>,
    lang: &str,
    grants: &[Grant],
) -> Result<&'a mut meld_t2::T2Backend, i32> {
    if !cache.contains_key(lang) {
        let pack = packs.get(lang).ok_or_else(|| {
            eprintln!("no lang pack for '{lang}'");
            1
        })?;
        let backend = spawn_worker(pack, t3, grants).map_err(|e| {
            eprintln!("failed to spawn worker for {lang}: {e}");
            1
        })?;
        cache.insert(lang.to_string(), backend);
    }
    let backend = cache.get_mut(lang).expect("worker inserted");
    backend.ensure_initialized(lang).map_err(|e| {
        eprintln!("{e}");
        1
    })?;
    Ok(backend)
}

fn should_skip(region: &meld_core::splitter::Region, packs: &HashMap<String, PackInfo>) -> bool {
    if region.kind == splitter::RegionKind::Header || region.lang == "text" {
        return true;
    }
    packs
        .get(&region.lang)
        .map(PackInfo::syntax_only)
        .unwrap_or(false)
}

fn render_inline(
    expr: &compose::InlineExpr,
    style: SourceStyle,
    ctx: &InlineContext<'_>,
    imports: &[Import],
    cache: &mut WorkerCache,
) -> Result<String, i32> {
    let backend = backend_for(cache, ctx.packs, ctx.t3, &expr.lang, ctx.grants)?;
    backend.apply_imports(imports, ctx.bridge).map_err(|e| {
        eprintln!("{e}");
        1
    })?;
    let value = backend.eval(&expr.id, &expr.code).map_err(|e| {
        eprintln!("{e}");
        1
    })?;
    value.to_source_literal(style).map_err(|e| {
        eprintln!("inline value cannot be rendered: {e}");
        1
    })
}

fn cmd_run(path: Option<&str>) -> i32 {
    let (_p, src) = match read_file(path) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let packs = discover_packs(&project_root());
    let registry = scanner_registry(&packs);
    let doc = match splitter::split(&src, &registry) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let grants = match source_grants(&src, &doc) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let bindings = match binding::resolve(&doc) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let plan = match binding::execution_plan(&doc, &bindings) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let units = match compose::build_units(&doc, &plan, &bindings) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // JUDGMENT CALL: one T3 component is loaded for the whole run and shared
    // with every T2 worker through a host callback. This is what lets a Python
    // region call a typed Rust/WIT function without embedding Wasmtime itself.
    let t3 = match meld_t3::T3Backend::demo() {
        Ok(v) => Arc::new(v),
        Err(e) => {
            eprintln!("failed to load T3 demo component: {e}");
            return 1;
        }
    };
    let mut stdout = Vec::new();
    let mut bridge = Bridge::default();
    let mut cache: WorkerCache = HashMap::new();

    for unit in units {
        let (region, code, imports, exports) = match unit {
            Unit::Job(job) => (
                job.region.clone(),
                job.code,
                job.imports.clone(),
                job.exports.clone(),
            ),
            Unit::Composite {
                anchor,
                segments,
                imports,
                exports,
            } => {
                let Some(pack) = packs.get(&anchor.lang) else {
                    eprintln!("no lang pack for containing region '{}'", anchor.lang);
                    return 1;
                };
                let Some(style) = pack.inline_style else {
                    eprintln!("pack '{}' has no inline.toml style", pack.id);
                    return 1;
                };
                let inline_ctx = InlineContext {
                    packs: &packs,
                    bridge: &bridge,
                    t3: &t3,
                    grants: &grants,
                };
                let code = compose::render_segments(&segments, |expr| {
                    let inline_imports = bindings
                        .imports
                        .iter()
                        .filter(|i| i.region_id == expr.id)
                        .cloned()
                        .collect::<Vec<_>>();
                    render_inline(expr, style, &inline_ctx, &inline_imports, &mut cache)
                });
                let code = match code {
                    Ok(v) => v,
                    Err(c) => return c,
                };
                (anchor, code, imports, exports)
            }
        };
        if should_skip(&region, &packs) || code.trim().is_empty() {
            continue;
        }
        let backend = match backend_for(&mut cache, &packs, &t3, &region.lang, &grants) {
            Ok(v) => v,
            Err(c) => return c,
        };
        let job = Job {
            region,
            code,
            exports,
            imports,
        };
        match backend.run(&job, &mut bridge) {
            Ok(lines) => stdout.extend(lines),
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    }
    for line in stdout {
        println!("{line}");
    }
    0
}

/// Canonicalise glue lines without changing line count or region byte layout.
fn format_glue(body: &str) -> String {
    let mut out = String::new();
    for line in body.split_inclusive('\n') {
        let core = line.trim_end_matches('\n').trim_end_matches('\r');
        let trimmed = core.trim_start();
        let indent_len = core.len() - trimmed.len();
        let indent = &core[..indent_len];
        let canonical = if let Some(rest) = trimmed.strip_prefix("export ") {
            let mut parts = rest.split_whitespace();
            let name = parts.next();
            let bridge = match (parts.next(), parts.next()) {
                (Some("as"), Some(b)) => Some(b),
                _ => None,
            };
            name.map(|n| match bridge {
                Some(b) => format!("export {n} as {b}"),
                None => format!("export {n}"),
            })
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            let mut parts = rest.split_whitespace();
            let name = parts.next();
            let from = match (parts.next(), parts.next()) {
                (Some("from"), Some(r)) => Some(r),
                _ => None,
            };
            name.and_then(|n| from.map(|r| format!("import {n} from {r}")))
        } else {
            None
        };
        if let Some(text) = canonical {
            out.push_str(indent);
            out.push_str(&text);
            if line.ends_with("\r\n") {
                out.push_str("\r\n");
            } else if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

// JUDGMENT CALL: formatter commands are pack data. When a pack ships
// `formatter.toml`, Meld writes the region body to a temp file, runs the
// command with {src}/{out} placeholders and reads the formatted bytes back.
// Without that file Meld formats only glue; it never guesses a formatter.
fn format_region_body(pack: Option<&PackInfo>, body: &str) -> String {
    let Some(pack) = pack else {
        return format_glue(body);
    };
    let config = match std::fs::read_to_string(pack.dir.join("formatter.toml")) {
        Ok(text) => text,
        Err(_) => return format_glue(body),
    };
    let doc: toml::Value = match toml::from_str(&config) {
        Ok(v) => v,
        Err(_) => return format_glue(body),
    };
    let Some(command) = doc
        .get("formatter")
        .and_then(|v| v.get("command"))
        .and_then(toml::Value::as_str)
    else {
        return format_glue(body);
    };
    let args = doc
        .get("formatter")
        .and_then(|v| v.get("args"))
        .and_then(toml::Value::as_array)
        .map(|xs| {
            xs.iter()
                .filter_map(|x| x.as_str())
                // JUDGMENT CALL: formatter paths follow the same rebasing rule
                // as runners, so installed packs need no absolute paths.
                .map(|x| rebase_arg(x, &project_root(), &pack.dir))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("meld-fmt-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("region.src");
    let out = dir.join("region.out");
    if std::fs::write(&src, format_glue(body)).is_err() {
        return format_glue(body);
    }
    let status = Command::new(command)
        .args(args.iter().map(|a| {
            a.replace("{src}", &src.to_string_lossy())
                .replace("{out}", &out.to_string_lossy())
        }))
        .status();
    let formatted = std::fs::read_to_string(&out).ok();
    let _ = std::fs::remove_dir_all(&dir);
    match (status, formatted) {
        (Ok(s), Some(text)) if s.success() => text,
        _ => format_glue(body),
    }
}

fn cmd_fmt(path: Option<&str>, check: bool) -> i32 {
    let (p, src) = match read_file(path) {
        Ok(v) => v,
        Err(c) => return c,
    };
    let packs = discover_packs(&project_root());
    let registry = scanner_registry(&packs);
    let doc = match splitter::split(&src, &registry) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut out = String::new();
    for region in &doc.regions {
        out.push_str(&region.open_marker);
        let formatted = format_region_body(packs.get(&region.lang), region.body(&src));
        out.push_str(&formatted);
        out.push_str(&region.close_marker);
    }
    if out == src {
        println!("{}: already formatted", p.display());
        return 0;
    }
    if check {
        eprintln!("{}: needs formatting", p.display());
        return 1;
    }
    match std::fs::write(&p, out) {
        Ok(()) => {
            println!("{}: formatted", p.display());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn cmd_repl() -> i32 {
    let packs = discover_packs(&project_root());
    let t3 = match meld_t3::T3Backend::demo() {
        Ok(v) => Arc::new(v),
        Err(e) => {
            eprintln!("failed to load T3 demo component: {e}");
            return 1;
        }
    };
    let mut lang = if packs.contains_key("python") {
        "python".to_string()
    } else {
        packs
            .values()
            .find(|p| p.runner.is_some())
            .map(|p| p.id.clone())
            .unwrap_or_else(|| "python".to_string())
    };
    let mut cache: WorkerCache = HashMap::new();
    let mut bridge = Bridge::default();
    let mut counter = 0usize;
    let stdin = io::stdin();
    println!("Meld REPL (`:help` for commands)");
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == ":exit" || trimmed == ":quit" {
            break;
        }
        if trimmed == ":help" {
            println!(":lang <id>   switch the active language");
            println!(":bridge      list bridge values");
            println!(":exit        leave the REPL");
            continue;
        }
        if let Some(next) = trimmed.strip_prefix(":lang ") {
            let next = next.trim().to_string();
            if !packs.contains_key(&next) {
                eprintln!("unknown pack '{next}'");
                continue;
            }
            lang = next;
            println!("switched to {lang}");
            continue;
        }
        if trimmed == ":bridge" {
            let mut keys: Vec<_> = bridge.values.keys().collect();
            keys.sort();
            for key in keys {
                println!("{key} = {:?}", bridge.values[key]);
            }
            continue;
        }
        eprint!("meld[{lang}]> ");
        let _ = io::stderr().flush();
        let backend = match backend_for(&mut cache, &packs, &t3, &lang, &[]) {
            Ok(v) => v,
            Err(c) => return c,
        };
        let region = meld_core::splitter::Region {
            id: format!("repl{counter}"),
            lang: lang.clone(),
            kind: splitter::RegionKind::Untagged,
            start: 0,
            end: line.len(),
            full_start: 0,
            full_end: line.len(),
            open_marker: String::new(),
            close_marker: String::new(),
            confidence: 1.0,
        };
        counter += 1;
        let job = Job {
            region,
            code: line,
            imports: vec![],
            exports: vec![],
        };
        match backend.run(&job, &mut bridge) {
            Ok(lines) => {
                for text in lines {
                    println!("{text}");
                }
            }
            Err(e) => eprintln!("{e}"),
        }
    }
    0
}

fn cmd_doctor() -> i32 {
    // JUDGMENT CALL: `sh` is included because the built-in Bash pack executes
    // through it; doctor reports only, never installs (project.md §9).
    for tool in ["rustc", "cargo", "python3", "node", "lua", "sh"] {
        let found = Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {tool}"))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        println!(
            "{tool}: {}",
            if found {
                "ok"
            } else {
                "MISSING (report only; never auto-installed)"
            }
        );
    }
    let backend = platform_backend();
    let probe = backend.probe();
    println!("sandbox backend: {}", probe.kind);
    println!(
        "sandbox available: {}",
        if probe.available { "ok" } else { "NO" }
    );
    for note in &probe.notes {
        println!("sandbox note: {note}");
    }
    match file_default_policy(&project_root()).and_then(|policy| {
        probe
            .require(&policy)
            .map_err(sandbox_error)
            .map(|_| policy.fingerprint())
    }) {
        Ok(fingerprint) => println!("sandbox default policy: {fingerprint}"),
        Err(e) => println!("sandbox default policy: ENFORCEMENT UNAVAILABLE ({e})"),
    }
    let packs = discover_packs(&project_root());
    let mut ids: Vec<_> = packs
        .values()
        .map(|p| (p.id.as_str(), p.tier.as_str()))
        .collect();
    ids.sort();
    ids.dedup();
    for (id, tier) in ids {
        println!("pack {id}: {tier}");
    }
    let root = meld_lib::library_root();
    match meld_lib::InstalledIndex::load(&root) {
        Ok(index) => {
            for entry in index.libraries.values() {
                let status = if root.join(&entry.path).is_dir() {
                    "ok"
                } else {
                    "MISSING"
                };
                println!(
                    "library {}: {} (tier={} level={} status={})",
                    entry.id, entry.version, entry.tier, entry.level, status
                );
            }
        }
        Err(e) => eprintln!("meld: warning: invalid library index: {e}"),
    }
    match meld_lib::ProjectLock::load(&project_root()) {
        Ok(lock) => {
            for (id, version) in &lock.libraries {
                println!("import {id}: {version}");
            }
        }
        Err(e) => eprintln!("meld: warning: invalid {}: {e}", meld_lib::LOCK_FILE),
    }
    0
}

/// Show the installed/imported split. This is intentionally a read-only view;
/// installs and removals live in the `meld-pack` terminal app.
fn cmd_library() -> i32 {
    let root = meld_lib::library_root();
    let project = project_root();
    let index = match meld_lib::InstalledIndex::load(&root) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("meld: {e}");
            return 1;
        }
    };
    let lock = match meld_lib::ProjectLock::load(&project) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("meld: {e}");
            return 1;
        }
    };
    println!("library root: {}", root.display());
    if index.libraries.is_empty() {
        println!("installed: (none; use meld-pack install <lang>)");
    }
    for entry in index.libraries.values() {
        let status = if root.join(&entry.path).is_dir() {
            "ok"
        } else {
            "MISSING"
        };
        println!(
            "installed {} {} tier={} level={} status={} source={}",
            entry.id, entry.version, entry.tier, entry.level, status, entry.source
        );
    }
    if lock.libraries.is_empty() {
        println!("imported: (none; use meld-pack import <lang>)");
    }
    for (id, version) in &lock.libraries {
        println!("imported {id}: {version}");
    }
    0
}

fn cmd_pack_new(lang: Option<&str>) -> i32 {
    let lang = match lang {
        Some(l) => l,
        None => {
            eprintln!("usage: meld pack new <lang>");
            return 2;
        }
    };
    let dir = Path::new("packs").join(lang);
    if dir.exists() {
        eprintln!("pack already exists: {}", dir.display());
        return 1;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{e}");
        return 1;
    }
    let manifest = format!(
        "id = \"{lang}\"\naliases = []\nextensions = []\ntier = \"T2\"\nlevel = \"L1\"\ncapabilities = [\"stdout\"]\n"
    );
    let _ = std::fs::write(dir.join("pack.toml"), manifest);
    println!("created {}", dir.display());
    0
}
