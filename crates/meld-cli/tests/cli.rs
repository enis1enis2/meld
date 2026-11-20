//! CLI-level Phase 5/7 checks: pack-owned formatter, REPL language switching,
//! and end-to-end execution of an installed third-party pack.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&str], stdin: Option<&str>, packs: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meld"));
    cmd.args(args)
        .current_dir(root())
        .env("MELD_ROOT", root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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

fn temp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from("/mnt/data/opencode/tmp/opencode")
        .join(format!("meld-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fmt_delegates_region_bodies_to_a_pack_owned_formatter() {
    let dir = temp_dir("fmt");
    let file = dir.join("sample.meld");
    std::fs::write(
        &file,
        "#meld python\n\n@mockfmt {\n  hello    world\n}\n",
    )
    .unwrap();
    let packs = root().join("tests/fixtures/pack-fmt-mock");

    let formatted = run(&["fmt", file.to_str().unwrap()], None, Some(&packs));
    assert!(formatted.status.success(), "{}", String::from_utf8_lossy(&formatted.stderr));
    let source = std::fs::read_to_string(&file).unwrap();
    assert!(source.contains("hello world"), "{source:?}");
    assert!(!source.contains("hello    world"), "{source:?}");

    let check = run(&["fmt", "--check", file.to_str().unwrap()], None, Some(&packs));
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn repl_switches_languages_and_keeps_running() {
    let output = run(
        &["repl"],
        Some(":lang javascript\nconsole.log(\"from repl\")\n:exit\n"),
        None,
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("switched to javascript"), "{stdout}");
    assert!(stdout.contains("from repl"), "{stdout}");
}

#[test]
fn third_party_shell_pack_runs_without_core_changes() {
    let packs = root().join("tests/fixtures/pack-third-party");
    let output = run(&["run", "examples/08-shell-pack.meld"], None, Some(&packs));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("hello from a third-party pack")
    );
}
