//! End-to-end tests for `tidy-up show`, the review surface.
//!
//! The question this command answers is "what did that run do, and can I still
//! undo it?", which matters most when something other than a person did the
//! work. These drive the compiled binary, because that is how a reviewer meets
//! it.

mod common;

use std::{fs, path::Path};

use common::{all_output, sandbox, tidy};
use serde_json::Value;

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// The review inside a `--json` envelope.
fn review(args: &[&str]) -> Value {
    let out = tidy(args);
    let text = String::from_utf8_lossy(&out.stdout);
    let envelope: Value = serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("not one JSON object ({e}): {text}"));
    assert_eq!(envelope["status"], "ok", "{envelope}");
    envelope["outcome"]["detail"].clone()
}

/// A folder with five loose files, organized, ready to be reviewed.
fn organized() -> tempfile::TempDir {
    let dir = sandbox();
    for name in ["a.png", "b.pdf", "c.txt", "d.mp3", "e.zip"] {
        fs::write(dir.path().join(name), format!("content-{name}")).unwrap();
    }
    assert!(
        tidy(&["organize", s(dir.path()), "--yes"]).status.success(),
        "fixture setup"
    );
    dir
}

#[test]
fn show_explains_the_most_recent_run_without_being_told_which() {
    let dir = organized();
    let out = tidy(&["show", s(dir.path())]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("organize"), "{text}");
    assert!(text.contains("5 moves"), "{text}");
    assert!(text.contains("Images"), "{text}");
}

/// The question a reviewer actually has. The journal cannot answer it, because
/// the folder carries on changing after the run.
#[test]
fn it_says_whether_the_run_can_still_be_undone_cleanly() {
    let dir = organized();

    let clean = review(&["show", s(dir.path()), "--json"]);
    assert_eq!(clean["moves"], 5);
    assert_eq!(clean["in_place"], 5);
    assert_eq!(clean["restored"], false);
    assert!(clean["bytes_in_place"].as_u64().unwrap() > 0);

    // Something else moves one of the files on afterwards.
    fs::remove_file(dir.path().join("Images").join("a.png")).unwrap();

    let partial = review(&["show", s(dir.path()), "--json"]);
    assert_eq!(partial["in_place"], 4, "{partial}");
    let gone: Vec<&Value> = partial["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["state"] == "moved")
        .collect();
    assert_eq!(gone.len(), 1);
    assert!(gone[0]["to"].as_str().unwrap().contains("a.png"));

    let text = all_output(&tidy(&["show", s(dir.path())]));
    assert!(text.contains("Partly"), "the warning must be plain: {text}");
}

#[test]
fn an_undone_run_is_reported_as_undone() {
    let dir = organized();
    assert!(
        tidy(&["restore", s(dir.path()), "--all", "--yes"])
            .status
            .success()
    );
    let reviewed = review(&["show", s(dir.path()), "--json"]);
    assert_eq!(reviewed["restored"], true, "{reviewed}");

    let text = all_output(&tidy(&["show", s(dir.path())]));
    assert!(text.contains("Already undone"), "{text}");
}

#[test]
fn a_run_can_be_named_by_id_or_by_a_unique_prefix() {
    let dir = organized();
    let id = review(&["show", s(dir.path()), "--json"])["id"]
        .as_str()
        .unwrap()
        .to_string();

    let by_id = review(&["show", s(dir.path()), &id, "--json"]);
    assert_eq!(by_id["id"], id);

    let prefix = &id[..11];
    let by_prefix = review(&["show", s(dir.path()), prefix, "--json"]);
    assert_eq!(by_prefix["id"], id, "a prefix resolves to the same run");
}

/// A journal only records changes, so what a run left alone can only come from
/// the run log. Without one the review still works, it just says less.
#[test]
fn what_the_run_left_alone_comes_from_the_run_log() {
    let dir = sandbox();
    fs::write(dir.path().join("keep.png"), "x").unwrap();
    fs::write(dir.path().join("ignore.iso"), "y").unwrap();
    assert!(
        tidy(&["organize", s(dir.path()), "--yes", "--log", "-x", "iso"])
            .status
            .success()
    );

    let reviewed = review(&["show", s(dir.path()), "--json"]);
    assert_eq!(
        reviewed["skipped"]["ignored"], 1,
        "the ignored file should be accounted for: {reviewed}"
    );
    let text = all_output(&tidy(&["show", s(dir.path())]));
    assert!(text.contains("left alone"), "{text}");
    assert!(
        text.contains("ignore rules"),
        "the reason should read as prose, not as a key: {text}"
    );
}

#[test]
fn a_folder_with_no_runs_says_so_rather_than_failing_obscurely() {
    let dir = sandbox();
    let out = tidy(&["show", s(dir.path())]);
    assert!(!out.status.success());
    let text = all_output(&out);
    assert!(text.contains("no runs recorded"), "{text}");
    assert!(!text.contains("panicked"), "{text}");
}

#[test]
fn an_unknown_id_is_an_error_not_a_panic() {
    let dir = organized();
    let out = tidy(&["show", s(dir.path()), "99999999-999999"]);
    assert!(!out.status.success());
    assert!(!all_output(&out).contains("panicked"));
}

/// Reviewing must never change the folder it is reviewing.
#[test]
fn show_changes_nothing() {
    let dir = organized();
    let before = listing(dir.path());
    assert!(tidy(&["show", s(dir.path()), "--verbose"]).status.success());
    assert_eq!(listing(dir.path()), before);
}

fn listing(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path.clone());
            }
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    out.sort();
    out
}
