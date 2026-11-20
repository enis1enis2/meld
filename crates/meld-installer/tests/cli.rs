//! End-to-end coverage for `meld-pack` with Ed25519 public trust roots.
//!
//! JUDGMENT CALL: a local git repository is used instead of a live GitHub call
//! so tests are offline and deterministic. It exercises the same git clone and
//! library-discovery code path as a GitHub source. HMAC was removed: these
//! tests only generate temporary Ed25519 key pairs and verify that tampering,
//! unknown keys, revoked keys and broken trust-root metadata are rejected.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pydeps() -> PathBuf {
    // JUDGMENT CALL: test key generation needs `cryptography`, but this
    // workspace can be checked out anywhere. MELD_PYDEPS lets local and CI
    // environments point at an extracted dependency directory without baking
    // an absolute host path into the test suite.
    if let Ok(value) = std::env::var("MELD_PYDEPS") {
        return PathBuf::from(value);
    }
    fs::canonicalize(root())
        .expect("canonical workspace root")
        .parent()
        .expect("workspace parent")
        .join("pydeps")
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::var("MELD_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join(format!(
        "meld-installer-{name}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// The sandbox git wrapper chroots, so pass `-C` explicitly; relying on the
/// process cwd would initialize a repository at the chroot root instead.
fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn commit(dir: &Path, message: &str) {
    let add = git(dir, &["add", "-A"]);
    assert!(add.status.success(), "{}", text(&add));
    let output = git(
        dir,
        &[
            "-c",
            "user.name=Meld Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
}

fn packtool(args: &[&str]) -> Output {
    Command::new("python3")
        .arg(root().join("tools/packtool.py"))
        .args(args)
        .env("PYTHONPATH", pydeps())
        .output()
        .expect("run packtool")
}

fn installer(tmp: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meld-pack"))
        .args(args)
        .env("MELD_PACK_TMP", tmp)
        .env_remove("MELD_PACK_SOURCE")
        .env_remove("MELD_PACK_REF")
        .env_remove("MELD_PACK_TRUST_ROOTS")
        .env_remove("MELD_PACK_REVOCATIONS")
        .output()
        .expect("run meld-pack")
}

fn key_pair(base: &Path, name: &str) -> (PathBuf, PathBuf) {
    let private = base.join(format!("{name}.private.pem"));
    let public = base.join(format!("{name}.public.pem"));
    let output = packtool(&[
        "keygen",
        "--private-key",
        private.to_str().unwrap(),
        "--public-key",
        public.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output));
    (private, public)
}

fn trust_roots(base: &Path, public: &Path, name: &str, label: &str) -> PathBuf {
    let target = base.join(name);
    let output = packtool(&[
        "trust-roots",
        "init",
        "--trust-roots",
        target.to_str().unwrap(),
        "--public-key",
        public.to_str().unwrap(),
        "--label",
        label,
        "--created",
        "2026-11-20",
    ]);
    assert!(output.status.success(), "{}", text(&output));
    target
}

fn revocations(base: &Path, name: &str) -> PathBuf {
    let target = base.join(name);
    let output = packtool(&[
        "revocations",
        "init",
        "--revocations",
        target.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output));
    target
}

fn revoke(path: &Path, key_id: &str) {
    let output = packtool(&[
        "revocations",
        "revoke",
        "--revocations",
        path.to_str().unwrap(),
        "--key-id",
        key_id,
        "--reason",
        "test revocation",
        "--revoked",
        "2026-11-20",
    ]);
    assert!(output.status.success(), "{}", text(&output));
}

fn signature_key_id(pack: &Path) -> String {
    let text = fs::read_to_string(pack.join("signature.ed25519")).unwrap();
    text.split_whitespace().nth(1).expect("key-id").to_string()
}

fn refresh_checksums(pack: &Path) {
    let script = "import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); import packtool; packtool.write_checksums(Path(sys.argv[2]))";
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(root().join("tools"))
        .arg(pack)
        .env("PYTHONPATH", pydeps())
        .output()
        .expect("refresh checksums");
    assert!(output.status.success(), "{}", text(&output));
}

struct Repo {
    path: PathBuf,
    trust_roots: PathBuf,
    revocations: PathBuf,
}

impl Repo {
    fn source(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    fn pack(&self) -> PathBuf {
        self.path.join("packs/shell")
    }
}

fn make_repo(base: &Path, name: &str, private: &Path, public: &Path) -> Repo {
    let repo = base.join(name);
    let pack = repo.join("packs/shell");
    fs::create_dir_all(repo.join("packs/registry")).unwrap();
    copy_dir(&root().join("tests/fixtures/pack-third-party"), &pack);
    // JUDGMENT CALL 54: the checked-in fixture carries checksums for its own
    // host's line endings. Recompute them over the copied working-tree bytes
    // before signing; `packtool sign` now refuses stale manifests, and Git for
    // Windows can preserve CRLF in both the fixture commit and the clone.
    refresh_checksums(&pack);

    let output = packtool(&[
        "sign",
        pack.to_str().unwrap(),
        "--private-key",
        private.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", text(&output));

    let roots = trust_roots(
        base,
        public,
        &format!("{name}-roots.toml"),
        "Test release key",
    );
    let revs = revocations(base, &format!("{name}-revocations.json"));
    fs::copy(&roots, repo.join("packs/registry/trust-roots.toml")).unwrap();
    fs::copy(&revs, repo.join("packs/registry/revocations.json")).unwrap();

    let init = git(&repo, &["init", "-q", "-b", "main"]);
    assert!(init.status.success(), "{}", text(&init));
    // JUDGMENT CALL 52: packtool signs the working-tree bytes. Git for Windows
    // would otherwise normalize CRLF on `git add`, so the committed payload
    // would no longer match its checksums or signature after a byte-preserving
    // clone. Store exactly what was signed; production clones already set
    // `core.autocrlf=false` (judgment call 51).
    let autocrlf = git(&repo, &["config", "core.autocrlf", "false"]);
    assert!(autocrlf.status.success(), "{}", text(&autocrlf));
    commit(&repo, "fixture");
    Repo {
        path: repo,
        trust_roots: roots,
        revocations: revs,
    }
}

#[test]
fn install_remove_and_self_install_round_trip() {
    let dir = temp_dir("round-trip");
    let (private, public) = key_pair(&dir, "signer");
    let repo = make_repo(&dir, "source", &private, &public);
    let libs = dir.join("libs");
    let project = dir.join("project");
    let bin = dir.join("bin");
    fs::create_dir_all(&project).unwrap();
    let source = repo.source();
    let root_arg = libs.to_string_lossy().into_owned();
    let project_arg = project.to_string_lossy().into_owned();

    let list = installer(&dir, &["remote-list", "--source", &source]);
    assert!(list.status.success(), "{}", text(&list));
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("shell"),
        "{}",
        text(&list)
    );

    // No explicit trust flags: remote installs read both files from the clone.
    let install = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(install.status.success(), "{}", text(&install));
    let installed = libs.join("shell/0.1.0");
    assert!(installed.join("library.toml").is_file());
    let lock = fs::read_to_string(project.join("meld.lock")).unwrap();
    assert!(lock.contains("\"shell\": \"0.1.0\""), "{lock}");

    let verify = installer(
        &dir,
        &[
            "verify",
            installed.to_str().unwrap(),
            "--trust-roots",
            repo.trust_roots.to_str().unwrap(),
            "--revocations",
            repo.revocations.to_str().unwrap(),
        ],
    );
    assert!(verify.status.success(), "{}", text(&verify));
    assert!(
        String::from_utf8_lossy(&verify.stdout).contains("signature: verified"),
        "{}",
        text(&verify)
    );

    let again = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(again.status.success(), "{}", text(&again));
    assert!(String::from_utf8_lossy(&again.stdout).contains("already installed"));

    let list = installer(&dir, &["list", "--root", &root_arg, "--json"]);
    assert!(list.status.success(), "{}", text(&list));
    assert!(String::from_utf8_lossy(&list.stdout).contains("meld-lang-library-index/1"));

    let remove = installer(
        &dir,
        &[
            "remove",
            "sh",
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(remove.status.success(), "{}", text(&remove));
    assert!(!installed.exists());
    let lock = fs::read_to_string(project.join("meld.lock")).unwrap();
    assert!(lock.contains("\"libraries\": {}"), "{lock}");

    let self_install = installer(&dir, &["self-install", "--bin-dir", bin.to_str().unwrap()]);
    assert!(self_install.status.success(), "{}", text(&self_install));
    let binary = bin.join(format!("meld-pack{}", std::env::consts::EXE_SUFFIX));
    assert!(binary.is_file());

    let self_uninstall = installer(
        &dir,
        &["self-uninstall", "--bin-dir", bin.to_str().unwrap()],
    );
    assert!(self_uninstall.status.success(), "{}", text(&self_uninstall));
    assert!(!binary.exists());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tampered_payloads_are_rejected() {
    let dir = temp_dir("tamper");
    let (private, public) = key_pair(&dir, "signer");
    let libs = dir.join("libs");
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();
    let root_arg = libs.to_string_lossy().into_owned();
    let project_arg = project.to_string_lossy().into_owned();

    // Payload edited after checksumming: checksum verification fails first.
    let stale_checksums = make_repo(&dir, "stale-checksums", &private, &public);
    let worker = stale_checksums.pack().join("worker.sh");
    let original = fs::read_to_string(&worker).unwrap();
    fs::write(&worker, format!("{original}# tampered\n")).unwrap();
    commit(&stale_checksums.path, "tampered payload");
    let source = stale_checksums.source();
    let install = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(!install.status.success(), "{}", text(&install));
    assert!(
        String::from_utf8_lossy(&install.stderr).contains("checksum mismatch: worker.sh"),
        "{}",
        text(&install)
    );

    // Checksums refreshed but the signature left behind: Ed25519 fails.
    let stale_signature = make_repo(&dir, "stale-signature", &private, &public);
    let worker = stale_signature.pack().join("worker.sh");
    fs::write(&worker, format!("{original}# tampered\n")).unwrap();
    refresh_checksums(&stale_signature.pack());
    commit(&stale_signature.path, "tampered signature");
    let source = stale_signature.source();
    let install = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(!install.status.success(), "{}", text(&install));
    assert!(
        String::from_utf8_lossy(&install.stderr).contains("signature mismatch"),
        "{}",
        text(&install)
    );

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unknown_and_revoked_signers_are_rejected() {
    let dir = temp_dir("signers");
    let (private, public) = key_pair(&dir, "signer");
    let (_other_private, other_public) = key_pair(&dir, "other");
    let repo = make_repo(&dir, "source", &private, &public);
    let other_roots = trust_roots(&dir, &other_public, "other-roots.toml", "Unknown key");
    let libs = dir.join("libs");
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();
    let source = repo.source();
    let root_arg = libs.to_string_lossy().into_owned();
    let project_arg = project.to_string_lossy().into_owned();

    let unknown = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
            "--trust-roots",
            other_roots.to_str().unwrap(),
        ],
    );
    assert!(!unknown.status.success(), "{}", text(&unknown));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("is not in the trust roots"),
        "{}",
        text(&unknown)
    );

    let revoked = revocations(&dir, "revoked.json");
    revoke(&revoked, &signature_key_id(&repo.pack()));
    let revoked_case = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
            "--revocations",
            revoked.to_str().unwrap(),
        ],
    );
    assert!(!revoked_case.status.success(), "{}", text(&revoked_case));
    assert!(
        String::from_utf8_lossy(&revoked_case.stderr).contains("is revoked"),
        "{}",
        text(&revoked_case)
    );

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_install_requires_valid_trust_root_metadata() {
    let dir = temp_dir("trust-roots");
    let (private, public) = key_pair(&dir, "signer");
    let repo = make_repo(&dir, "source", &private, &public);
    let libs = dir.join("libs");
    let project = dir.join("project");
    fs::create_dir_all(&project).unwrap();
    let source = repo.source();
    let root_arg = libs.to_string_lossy().into_owned();
    let project_arg = project.to_string_lossy().into_owned();
    let clone_roots = repo.path.join("packs/registry/trust-roots.toml");
    let clone_revs = repo.path.join("packs/registry/revocations.json");

    fs::remove_file(&clone_roots).unwrap();
    commit(&repo.path, "remove trust roots");
    let missing = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(!missing.status.success(), "{}", text(&missing));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no trust roots"),
        "{}",
        text(&missing)
    );

    let valid_roots = fs::read_to_string(&repo.trust_roots).unwrap();
    let key_id = signature_key_id(&repo.pack());
    let broken_id = &key_id[..8];
    fs::write(&clone_roots, valid_roots.replacen(broken_id, "00000000", 1)).unwrap();
    commit(&repo.path, "malformed trust roots");
    let malformed = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(!malformed.status.success(), "{}", text(&malformed));
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("does not match public key"),
        "{}",
        text(&malformed)
    );

    fs::copy(&repo.trust_roots, &clone_roots).unwrap();
    fs::remove_file(&clone_revs).unwrap();
    commit(&repo.path, "remove revocations");
    let missing_revs = installer(
        &dir,
        &[
            "install",
            "shell",
            "--source",
            &source,
            "--root",
            &root_arg,
            "--project",
            &project_arg,
        ],
    );
    assert!(!missing_revs.status.success(), "{}", text(&missing_revs));
    assert!(
        String::from_utf8_lossy(&missing_revs.stderr).contains("no revocations"),
        "{}",
        text(&missing_revs)
    );

    fs::remove_dir_all(dir).unwrap();
}
