//! Black-box tests that run the compiled `tidy-up` binary.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn tidy(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tidy-up"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn help_and_version_work() {
    let help = tidy(&["--help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    for word in ["organize", "dedupe", "restore", "history", "purge"] {
        assert!(text.contains(word), "help should mention `{word}`");
    }
    assert!(stdout(&tidy(&["--version"])).contains("tidy-up"));
}

#[test]
fn dry_run_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.png", "x");
    let out = tidy(&["organize", s(dir.path()), "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).contains("Dry run"));
    assert!(dir.path().join("a.png").exists());
    assert!(!dir.path().join("Images").exists());
    assert!(!dir.path().join(".tidy-up").exists(), "dry run must not create state");
}

#[test]
fn organize_yes_then_restore_yes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.png", "img");
    write(dir.path(), "b.pdf", "doc");
    write(dir.path(), "c.lnk", "shortcut");

    let out = tidy(&["organize", s(dir.path()), "--yes"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.path().join("Images/a.png").exists());
    assert!(dir.path().join("Documents/b.pdf").exists());
    assert!(dir.path().join("c.lnk").exists(), "shortcuts are skipped by default");

    let history = tidy(&["history", s(dir.path())]);
    assert!(stdout(&history).contains("active"));

    let out = tidy(&["restore", s(dir.path()), "--yes"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(dir.path().join("a.png").exists());
    assert!(dir.path().join("b.pdf").exists());
    assert!(!dir.path().join("Images").exists());

    assert!(stdout(&tidy(&["history", s(dir.path())])).contains("restored"));
    assert!(stdout(&tidy(&["restore", s(dir.path()), "--yes"])).contains("Nothing to restore"));
}

#[test]
fn ignore_flags_and_ignore_file_are_honoured() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "keep.pdf", "1");
    write(dir.path(), "keep.iso", "2");
    write(dir.path(), "keep.tmp", "3");
    write(dir.path(), "move.png", "4");
    let ignore = dir.path().join("ignore.txt");
    fs::write(&ignore, "# things to keep\n.iso\n*.tmp\n").unwrap();

    let out = tidy(&["organize", s(dir.path()), "--yes", "-x", "pdf", "-f", s(&ignore)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for kept in ["keep.pdf", "keep.iso", "keep.tmp"] {
        assert!(dir.path().join(kept).exists(), "{kept} should stay");
    }
    assert!(dir.path().join("Images/move.png").exists());
}

#[test]
fn include_shortcuts_flag_moves_them() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "App.lnk", "x");
    let out = tidy(&["organize", s(dir.path()), "--yes", "--include-shortcuts"]);
    assert!(out.status.success());
    assert!(dir.path().join("Other/App.lnk").exists());
}

#[test]
fn dedupe_then_purge() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", "same content");
    write(dir.path(), "b.txt", "same content");

    let out = tidy(&["dedupe", s(dir.path()), "--yes"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("Group-001"));
    assert!(text.contains("purge"), "should recommend the next step");
    assert!(dir.path().join("a.txt").exists());
    assert!(!dir.path().join("b.txt").exists());
    assert!(dir.path().join("_Duplicates/Group-001/b.txt").exists());

    let out = tidy(&["purge", s(dir.path()), "--yes"]);
    assert!(out.status.success());
    assert!(!dir.path().join("_Duplicates").exists());
    assert!(dir.path().join("a.txt").exists());
}

#[test]
fn dedupe_with_nothing_to_do_says_so() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.txt", "one");
    write(dir.path(), "b.txt", "two");
    let out = tidy(&["dedupe", s(dir.path()), "--yes"]);
    assert!(stdout(&out).contains("No duplicates found"));
}

#[test]
fn refuses_to_prompt_without_terminal_or_yes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.png", "x");
    let out = tidy(&["organize", s(dir.path())]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--yes"));
    assert!(dir.path().join("a.png").exists());
}

#[test]
fn bad_inputs_fail_cleanly() {
    let out = tidy(&["organize", "/definitely/not/a/real/folder", "--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));

    let dir = tempfile::tempdir().unwrap();
    let out = tidy(&["organize", s(dir.path()), "-f", "/no/such/ignore.txt", "--yes"]);
    assert!(!out.status.success());
    assert!(tidy(&["organize", "--depth", "0"]).status.code() == Some(2));
}

#[test]
fn interactive_menu_needs_a_terminal() {
    let out = tidy(&[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));
}
