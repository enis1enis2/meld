//! End-to-end coverage for `meld-pack`: clone a local git source, verify with a
//! shared HMAC key, install, import, list, remove, and self-install.
//!
//! JUDGMENT CALL: a local git repository is used instead of a live GitHub call
//! so tests are offline and deterministic. It exercises the same git clone and
//! library-discovery code path as a GitHub source; a live smoke test is done
//! manually against the published repository.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = PathBuf::from("/mnt/data/opencode/tmp/opencode").join(format!(
        "meld-installer-{name}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
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

fn git(dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.current_dir(dir).args(args);
    command.output().expect("run git")
}

fn installer(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meld-pack"))
        .args(args)
        .output()
        .expect("run meld-pack")
}

fn text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn install_remove_and_self_install_round_trip() {
    let dir = temp_dir("round-trip");
    let repo = dir.join("repo");
    let pack = repo.join("packs/shell");
    let libs = dir.join("libs");
    let project = dir.join("project");
    let bin = dir.join("bin");
    fs::create_dir_all(&project).unwrap();
    copy_dir(&root().join("tests/fixtures/pack-third-party"), &pack);

    let key = dir.join("shared.key");
    fs::write(&key, b"k".repeat(32)).unwrap();
    let signed = Command::new("python3")
        .args([
            root().join("tools/packtool.py").to_string_lossy().as_ref(),
            "sign",
            pack.to_string_lossy().as_ref(),
            "--key",
            key.to_string_lossy().as_ref(),
        ])
        .output()
        .expect("sign pack");
    assert!(signed.status.success(), "{}", text(&signed));

    let init = git(&repo, &["init", "-q", "-b", "main"]);
    assert!(init.status.success(), "{}", text(&init));
    assert!(git(&repo, &["add", "."]).status.success());
    let commit = Command::new("git")
        .current_dir(&repo)
        .args([
            "-c",
            "user.name=Meld Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ])
        .output()
        .expect("commit");
    assert!(commit.status.success(), "{}", text(&commit));

    let source = repo.to_string_lossy().into_owned();
    let list = installer(&["remote-list", "--source", &source]);
    assert!(list.status.success(), "{}", text(&list));
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("shell"),
        "{}",
        text(&list)
    );

    let install = installer(&[
        "install",
        "shell",
        "--source",
        &source,
        "--root",
        libs.to_string_lossy().as_ref(),
        "--project",
        project.to_string_lossy().as_ref(),
        "--key",
        key.to_string_lossy().as_ref(),
    ]);
    assert!(install.status.success(), "{}", text(&install));
    let installed = libs.join("shell/0.1.0");
    assert!(installed.join("library.toml").is_file());
    let lock = fs::read_to_string(project.join("meld.lock")).unwrap();
    assert!(lock.contains("\"shell\": \"0.1.0\""), "{lock}");

    let verify = installer(&[
        "verify",
        installed.to_string_lossy().as_ref(),
        "--key",
        key.to_string_lossy().as_ref(),
    ]);
    assert!(verify.status.success(), "{}", text(&verify));
    assert!(String::from_utf8_lossy(&verify.stdout).contains("signature: verified"));

    let again = installer(&[
        "install",
        "shell",
        "--source",
        &source,
        "--root",
        libs.to_string_lossy().as_ref(),
        "--project",
        project.to_string_lossy().as_ref(),
        "--key",
        key.to_string_lossy().as_ref(),
    ]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(String::from_utf8_lossy(&again.stdout).contains("already installed"));

    let list = installer(&["list", "--root", libs.to_string_lossy().as_ref(), "--json"]);
    assert!(list.status.success(), "{}", text(&list));
    assert!(String::from_utf8_lossy(&list.stdout).contains("meld-lang-library-index/1"));

    let remove = installer(&[
        "remove",
        "sh",
        "--root",
        libs.to_string_lossy().as_ref(),
        "--project",
        project.to_string_lossy().as_ref(),
    ]);
    assert!(remove.status.success(), "{}", text(&remove));
    assert!(!installed.exists());
    let lock = fs::read_to_string(project.join("meld.lock")).unwrap();
    assert!(lock.contains("\"libraries\": {}"), "{lock}");

    let self_install = installer(&["self-install", "--bin-dir", bin.to_string_lossy().as_ref()]);
    assert!(self_install.status.success(), "{}", text(&self_install));
    let binary = bin.join(format!("meld-pack{}", std::env::consts::EXE_SUFFIX));
    assert!(binary.is_file());

    let self_uninstall = installer(&[
        "self-uninstall",
        "--bin-dir",
        bin.to_string_lossy().as_ref(),
    ]);
    assert!(self_uninstall.status.success(), "{}", text(&self_uninstall));
    assert!(!binary.exists());

    fs::remove_dir_all(dir).unwrap();
}
