//! CLI-level Phase 5/7 checks: pack-owned formatter, REPL language switching,
//! and end-to-end execution of an installed third-party pack.
//! HMAC was removed; temporary packs are re-signed with Ed25519 keys generated
//! outside the repository after checksums are refreshed.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run_with_env(
    args: &[&str],
    stdin: Option<&str>,
    packs: Option<&Path>,
    env: &[(&str, &str)],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meld"));
    cmd.args(args)
        .current_dir(root())
        .env("MELD_ROOT", root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(packs) = packs {
        cmd.env("MELD_PACKS", packs);
    }
    if let Some(input) = stdin {
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn meld");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for meld")
    } else {
        cmd.output().expect("run meld")
    }
}

fn run(args: &[&str], stdin: Option<&str>, packs: Option<&Path>) -> Output {
    run_with_env(args, stdin, packs, &[])
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from("/mnt/data/opencode/tmp/opencode")
        .join(format!("meld-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn pydeps() -> PathBuf {
    std::fs::canonicalize(root())
        .expect("canonical workspace root")
        .parent()
        .expect("workspace parent")
        .join("pydeps")
}

fn key_pair(base: &Path) -> PathBuf {
    let private = base.join("temporary.private.pem");
    let public = base.join("temporary.public.pem");
    let output = Command::new("python3")
        .arg(root().join("tools/packtool.py"))
        .args([
            "keygen",
            "--private-key",
            private.to_str().unwrap(),
            "--public-key",
            public.to_str().unwrap(),
        ])
        .env("PYTHONPATH", pydeps())
        .output()
        .expect("generate Ed25519 key pair");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    private
}

#[test]
fn fmt_delegates_region_bodies_to_a_pack_owned_formatter() {
    let dir = temp_dir("fmt");
    let file = dir.join("sample.meld");
    std::fs::write(&file, "#meld python\n\n@mockfmt {\n  hello    world\n}\n").unwrap();
    let packs = root().join("tests/fixtures/pack-fmt-mock");

    let formatted = run(&["fmt", file.to_str().unwrap()], None, Some(&packs));
    assert!(
        formatted.status.success(),
        "{}",
        String::from_utf8_lossy(&formatted.stderr)
    );
    let source = std::fs::read_to_string(&file).unwrap();
    assert!(source.contains("hello world"), "{source:?}");
    assert!(!source.contains("hello    world"), "{source:?}");

    let check = run(
        &["fmt", "--check", file.to_str().unwrap()],
        None,
        Some(&packs),
    );
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn repl_switches_languages_and_keeps_running() {
    let output = run(
        &["repl"],
        Some(":lang javascript\nconsole.log(\"from repl\")\n:exit\n"),
        None,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("switched to javascript"), "{stdout}");
    assert!(stdout.contains("from repl"), "{stdout}");
}

#[test]
fn third_party_shell_pack_runs_without_core_changes() {
    let packs = root().join("tests/fixtures/pack-third-party");
    let output = run(&["run", "examples/08-shell-pack.meld"], None, Some(&packs));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from a third-party pack"));
}

#[test]
fn imported_library_overrides_a_same_named_builtin_pack() {
    // JUDGMENT CALL: the project is isolated so this test never writes a lock
    // into the repository. A copy of the built-in bash pack (which supplies the
    // `shell` alias) is placed in the temp project, while `meld.lock` pins an
    // imported `shell` library that prints a unique marker. The marker proves
    // discovery order, not just generic third-party execution.
    let dir = temp_dir("lib-override");
    let project = dir.join("project");
    let libs = dir.join("libs");
    let builtin = project.join("packs/bash");
    let imported = libs.join("shell/0.1.0");
    copy_dir(&root().join("packs/bash"), &builtin);
    copy_dir(&root().join("tests/fixtures/pack-third-party"), &imported);

    let worker = imported.join("worker.sh");
    let original = std::fs::read_to_string(&worker).unwrap();
    let patched = original.replace(
        "sh -c \"$code\" </dev/null >\"$tmpdir/out\" 2>\"$tmpdir/err\"",
        "printf 'imported shell library won\\n' >\"$tmpdir/out\"",
    );
    assert_ne!(original, patched, "fixture worker line changed");
    std::fs::write(&worker, patched).unwrap();

    let private_key = key_pair(&dir);
    let checksums = Command::new("python3")
        .arg("-c")
        .arg("import sys; from pathlib import Path; sys.path.insert(0, sys.argv[1]); import packtool; packtool.write_checksums(Path(sys.argv[2]))")
        .arg(root().join("tools"))
        .arg(&imported)
        .env("PYTHONPATH", pydeps())
        .output()
        .expect("regenerate checksums");
    assert!(
        checksums.status.success(),
        "{}",
        String::from_utf8_lossy(&checksums.stderr)
    );
    let signed = Command::new("python3")
        .args([
            root().join("tools/packtool.py").to_string_lossy().as_ref(),
            "sign",
            imported.to_string_lossy().as_ref(),
            "--private-key",
            private_key.to_string_lossy().as_ref(),
        ])
        .env("PYTHONPATH", pydeps())
        .output()
        .expect("sign library");
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );

    std::fs::write(
        project.join("meld.lock"),
        "{\n  \"libraries\": {\n    \"shell\": \"0.1.0\"\n  },\n  \"schema\": \"meld-project-libraries/1\"\n}\n",
    )
    .unwrap();

    let project_arg = project.to_string_lossy().into_owned();
    let libs_arg = libs.to_string_lossy().into_owned();
    let output = run_with_env(
        &["run", "examples/08-shell-pack.meld"],
        None,
        None,
        &[("MELD_ROOT", &project_arg), ("MELD_LIB_ROOT", &libs_arg)],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("imported shell library won"), "{stdout}");
    assert!(
        !stdout.contains("hello from a third-party pack"),
        "{stdout}"
    );

    std::fs::remove_dir_all(dir).unwrap();
}
