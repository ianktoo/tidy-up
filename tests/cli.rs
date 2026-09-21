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
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout(&out).contains("Dry run"));
    assert!(dir.path().join("a.png").exists());
    assert!(!dir.path().join("Images").exists());
    assert!(
        !dir.path().join(".tidy-up").exists(),
        "dry run must not create state"
    );
}

#[test]
fn organize_yes_then_restore_yes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.png", "img");
    write(dir.path(), "b.pdf", "doc");
    write(dir.path(), "c.lnk", "shortcut");

    let out = tidy(&["organize", s(dir.path()), "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join("Images/a.png").exists());
    assert!(dir.path().join("Documents/b.pdf").exists());
    assert!(
        dir.path().join("c.lnk").exists(),
        "shortcuts are skipped by default"
    );

    let history = tidy(&["history", s(dir.path())]);
    assert!(stdout(&history).contains("active"));

    let out = tidy(&["restore", s(dir.path()), "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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

    let out = tidy(&[
        "organize",
        s(dir.path()),
        "--yes",
        "-x",
        "pdf",
        "-f",
        s(&ignore),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let out = tidy(&[
        "organize",
        s(dir.path()),
        "-f",
        "/no/such/ignore.txt",
        "--yes",
    ]);
    assert!(!out.status.success());
    assert!(tidy(&["organize", "--depth", "0"]).status.code() == Some(2));
}

#[test]
fn interactive_menu_needs_a_terminal() {
    let out = tidy(&[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("interactive terminal"));
}

// ---------------------------------------------------------------- compare

fn three_folders() -> (tempfile::TempDir, [std::path::PathBuf; 3]) {
    let base = tempfile::tempdir().unwrap();
    let dirs = ["a", "b", "c"].map(|n| base.path().join(n));
    write(&dirs[0], "keep.txt", "same content");
    write(&dirs[0], "only-a.txt", "aaa");
    write(&dirs[1], "copy.txt", "same content");
    write(&dirs[1], "only-b.txt", "bbb");
    write(&dirs[2], "dup.txt", "same content");
    (base, dirs)
}

#[test]
fn compare_without_action_or_terminal_is_a_safe_report() {
    let (_base, [a, b, c]) = three_folders();
    let out = tidy(&["compare", s(&a), s(&b), s(&c)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    assert!(text.contains("Duplicated content"));
    assert!(text.contains("#1") && text.contains("#3"));
    assert!(text.contains("report only"));
    assert!(b.join("copy.txt").exists() && c.join("dup.txt").exists());
    assert!(!a.join(".tidy-up").exists(), "report must not create state");
}

#[test]
fn compare_says_when_folders_are_identical() {
    let base = tempfile::tempdir().unwrap();
    let (x, y) = (base.path().join("x"), base.path().join("y"));
    write(&x, "one.txt", "content one");
    write(&y, "renamed.txt", "content one");
    let out = tidy(&["compare", s(&x), s(&y)]);
    assert!(stdout(&out).contains("exactly the same content"));
}

#[test]
fn compare_move_then_restore_round_trip() {
    let (_base, [a, b, c]) = three_folders();
    let out = tidy(&["compare", s(&a), s(&b), s(&c), "--action", "move", "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(a.join("keep.txt").exists());
    assert!(!b.join("copy.txt").exists() && !c.join("dup.txt").exists());
    assert!(a.join("_Duplicates/Group-001").is_dir());
    assert!(stdout(&out).contains("tidy-up restore"));

    let out = tidy(&["restore", s(&a), "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(b.join("copy.txt").exists() && c.join("dup.txt").exists());
    assert!(!a.join("_Duplicates").exists());
}

#[test]
fn compare_merge_gathers_into_first_folder() {
    let (_base, [a, b, c]) = three_folders();
    let out = tidy(&["compare", s(&a), s(&b), s(&c), "--action", "merge", "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        a.join("only-b.txt").exists(),
        "unique file from B gathered into A"
    );
    assert!(!b.join("only-b.txt").exists());
    assert!(a.join("keep.txt").exists());
}

#[test]
fn compare_delete_removes_extras_only() {
    let (_base, [a, b, c]) = three_folders();
    let out = tidy(&["compare", s(&a), s(&b), s(&c), "-a", "delete", "-y"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(a.join("keep.txt").exists() && a.join("only-a.txt").exists());
    assert!(!b.join("copy.txt").exists() && !c.join("dup.txt").exists());
    assert!(b.join("only-b.txt").exists());
    assert!(!a.join(".tidy-up").exists(), "delete is not journaled");
}

#[test]
fn compare_dry_run_changes_nothing() {
    let (_base, [a, b, c]) = three_folders();
    for action in ["move", "merge", "delete"] {
        let out = tidy(&["compare", s(&a), s(&b), s(&c), "--action", action, "-n"]);
        assert!(
            out.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            b.join("copy.txt").exists() && c.join("dup.txt").exists(),
            "{action}"
        );
        assert!(!a.join(".tidy-up").exists(), "{action}");
    }
}

#[test]
fn compare_rejects_nested_and_missing_folders_and_lone_merge() {
    let (_base, [a, ..]) = three_folders();
    let nested = a.join("inside");
    fs::create_dir_all(&nested).unwrap();
    let out = tidy(&["compare", s(&a), s(&nested)]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("overlap"));

    assert!(
        !tidy(&["compare", s(&a), "/no/such/folder"])
            .status
            .success()
    );
    let out = tidy(&["compare", s(&a), "--action", "merge", "-y"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("at least two"));
    assert_eq!(tidy(&["compare"]).status.code(), Some(2));
}

#[test]
fn compare_honours_ignore_flags() {
    let (_base, [a, b, c]) = three_folders();
    let out = tidy(&[
        "compare",
        s(&a),
        s(&b),
        s(&c),
        "-x",
        "txt",
        "-a",
        "delete",
        "-y",
    ]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("No file content is repeated"));
    assert!(b.join("copy.txt").exists());
}

// ---------------------------------------------------------------- analyze

#[test]
fn analyze_prints_every_section() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "pic.jpg", &"x".repeat(2000));
    write(dir.path(), "docs/report.pdf", &"x".repeat(500));
    write(dir.path(), "app/Cargo.toml", "[package]");
    let out = tidy(&["analyze", s(dir.path())]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    for section in [
        "Analysis of",
        "Partition",
        "By type",
        "Largest files",
        "Largest folders",
        "Code projects",
        "By age",
        "--duplicates",
    ] {
        assert!(text.contains(section), "missing `{section}` in:\n{text}");
    }
    assert!(text.contains("Images") && text.contains("pic.jpg"));
    assert!(
        !dir.path().join(".tidy-up").exists(),
        "analysis must not write anything"
    );
}

#[test]
fn analyze_json_is_valid_and_complete() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.png", "12345");
    write(dir.path(), "sub/b.txt", "123");
    let out = tidy(&["analyze", s(dir.path()), "--json", "--top", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let report = &json[0];
    assert_eq!(report["files"], 2);
    assert_eq!(report["bytes"], 8);
    assert_eq!(
        report["largest_files"].as_array().unwrap().len(),
        1,
        "--top applies"
    );
    assert_eq!(report["by_category"][0]["category"], "Images");
    assert!(report["disk"]["total"].as_u64().unwrap() > 0);
    assert!(report["duplicates"].is_null());
}

#[test]
fn analyze_duplicates_flag_measures_waste() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.bin", &"same".repeat(100));
    write(dir.path(), "copy/b.bin", &"same".repeat(100));
    write(dir.path(), "c.bin", "different");
    let out = tidy(&["analyze", s(dir.path()), "--duplicates", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json[0]["duplicates"]["extra_copies"], 1);
    assert_eq!(json[0]["duplicates"]["reclaimable_bytes"], 400);
    assert!(
        dir.path().join("copy/b.bin").exists(),
        "analysis never moves anything"
    );
}

#[test]
fn analyze_handles_several_folders_and_bad_input() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write(a.path(), "x.txt", "1");
    write(b.path(), "y.txt", "22");
    let out = tidy(&["analyze", s(a.path()), s(b.path()), "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 2);

    let out = tidy(&["analyze", "/definitely/not/here"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error:"));

    let empty = tempfile::tempdir().unwrap();
    assert!(stdout(&tidy(&["analyze", s(empty.path())])).contains("No files found"));
}

// ---------------------------------------------------------------- distribute

fn distribute_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    [std::path::PathBuf; 2],
) {
    let base = tempfile::tempdir().unwrap();
    let src = base.path().join("src");
    for i in 0..10 {
        write(&src, &format!("f{i}.dat"), "0123456789");
    }
    let dests = [base.path().join("d1"), base.path().join("d2")];
    (base, src, dests)
}

fn count_files(dir: &Path) -> usize {
    fn walk(dir: &Path, n: &mut usize) {
        let Ok(read) = fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".tidy-up") {
                continue;
            }
            if path.is_dir() {
                walk(&path, n);
            } else {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

#[test]
fn distribute_dry_run_shows_the_split_and_changes_nothing() {
    let (_base, src, [d1, d2]) = distribute_fixture();
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&d1),
        s(&d2),
        "--ratio",
        "70,30",
        "--max-fill",
        "100",
        "-n",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = stdout(&out);
    assert!(text.contains("Destinations") && text.contains("#1") && text.contains("#2"));
    assert!(text.contains("+70 B") && text.contains("+30 B"), "{text}");
    assert!(text.contains("Sources"));
    assert!(text.contains("Dry run"));
    assert_eq!(count_files(&src), 10);
    assert!(
        !d1.exists() && !d2.exists(),
        "a dry run must not even create the destinations"
    );
}

#[test]
fn distribute_moves_by_ratio_and_restore_undoes_it() {
    let (_base, src, [d1, d2]) = distribute_fixture();
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&d1),
        s(&d2),
        "--ratio",
        "70,30",
        "--max-fill",
        "100",
        "--yes",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!((count_files(&d1), count_files(&d2)), (7, 3));
    assert_eq!(count_files(&src), 0);
    assert!(
        stdout(&out).contains("tidy-up restore"),
        "tells you how to undo"
    );

    let history = tidy(&["history", s(&d1)]);
    assert!(stdout(&history).contains("distribute"));

    let out = tidy(&["restore", s(&d1), "--yes"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_files(&src), 10);
    assert_eq!(count_files(&d1) + count_files(&d2), 0);
}

#[test]
fn distribute_limit_caps_how_much_moves() {
    let (_base, src, [d1, _]) = distribute_fixture();
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&d1),
        "--limit",
        "45",
        "--max-fill",
        "100",
        "--yes",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_files(&d1), 4, "45 bytes fits four 10-byte files");
    assert_eq!(count_files(&src), 6);
    assert!(stdout(&out).contains("left out by --limit"));
}

#[test]
fn distribute_layout_organize_sorts_into_category_folders() {
    let base = tempfile::tempdir().unwrap();
    let src = base.path().join("src");
    write(&src, "a.jpg", "1");
    write(&src, "b.pdf", "2");
    let dest = base.path().join("out");
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&dest),
        "--layout",
        "organize",
        "--max-fill",
        "100",
        "-y",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dest.join("Images/a.jpg").exists() && dest.join("Documents/b.pdf").exists());
}

#[test]
fn distribute_reports_when_nothing_fits() {
    let (_base, src, [d1, _]) = distribute_fixture();
    // a 1-byte minimum-free reserve larger than any real disk is impossible, so use a huge one
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&d1),
        "--min-free",
        "1000000T",
        "--yes",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout(&out).contains("Nothing can be moved"));
    assert_eq!(count_files(&src), 10, "nothing moved");
}

#[test]
fn distribute_rejects_bad_arguments_before_touching_anything() {
    let (_base, src, [d1, d2]) = distribute_fixture();
    let fails = |args: &[&str], needle: &str| {
        let out = tidy(args);
        assert!(!out.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
        assert!(err.contains(needle), "expected `{needle}` in: {err}");
        assert_eq!(count_files(&src), 10, "{args:?} must not move anything");
    };
    // ratio has the wrong number of values
    fails(
        &[
            "distribute",
            "--from",
            s(&src),
            "--to",
            s(&d1),
            s(&d2),
            "--ratio",
            "1,2,3",
            "-y",
        ],
        "ratio",
    );
    // destination inside the source
    let inside = src.join("inner");
    fails(
        &["distribute", "--from", s(&src), "--to", s(&inside), "-y"],
        "overlap",
    );
    // same folder as source and destination
    fails(
        &["distribute", "--from", s(&src), "--to", s(&src), "-y"],
        "overlap",
    );
    // missing source
    fails(
        &["distribute", "--from", "/no/such/dir", "--to", s(&d1), "-y"],
        "",
    );
    // unparsable size and percentage are usage errors (exit code 2)
    assert_eq!(
        tidy(&[
            "distribute",
            "--from",
            s(&src),
            "--to",
            s(&d1),
            "--limit",
            "lots"
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        tidy(&[
            "distribute",
            "--from",
            s(&src),
            "--to",
            s(&d1),
            "--max-fill",
            "0"
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(tidy(&["distribute", "--to", s(&d1)]).status.code(), Some(2));
}

#[test]
fn distribute_needs_confirmation_or_yes() {
    let (_base, src, [d1, _]) = distribute_fixture();
    let out = tidy(&[
        "distribute",
        "--from",
        s(&src),
        "--to",
        s(&d1),
        "--max-fill",
        "100",
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--yes"));
    assert_eq!(count_files(&src), 10);
}

#[test]
fn help_lists_the_new_commands() {
    let text = stdout(&tidy(&["--help"]));
    for word in ["analyze", "distribute", "compare"] {
        assert!(text.contains(word), "help should mention `{word}`");
    }
    let dist = stdout(&tidy(&["distribute", "--help"]));
    for flag in [
        "--ratio",
        "--limit",
        "--max-fill",
        "--min-free",
        "--layout",
        "--strategy",
    ] {
        assert!(
            dist.contains(flag),
            "distribute --help should mention {flag}"
        );
    }
}
