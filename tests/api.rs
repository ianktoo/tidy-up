//! End-to-end tests for the machine interface: `--json`, exit codes and the
//! propose/review/apply cycle.
//!
//! These drive the compiled binary, because the contract being tested is what
//! a caller sees from outside the process, not what a function returns.

mod common;

use std::{fs, path::Path};

use common::{all_output, home, sandbox, tidy};
use serde_json::Value;

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Runs the binary and parses its stdout as the JSON envelope.
fn json(args: &[&str]) -> (Value, i32) {
    let out = tidy(args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout was not one JSON object ({e}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (value, out.status.code().unwrap_or(-1))
}

fn demo(root: &Path) {
    fs::write(root.join("a.png"), "x").unwrap();
    fs::write(root.join("b.pdf"), "yy").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

/// The point of the envelope: a caller can parse the output without knowing
/// which command ran, or whether it worked.
#[test]
fn every_command_prints_one_parseable_object() {
    let dir = sandbox();
    demo(dir.path());
    let path = s(dir.path());

    for args in [
        vec!["organize", path, "--dry-run"],
        vec!["reorganize", path, "--dry-run"],
        vec!["dedupe", path, "--dry-run"],
        vec!["history", path],
        vec!["analyze", path, "--top", "0"],
    ] {
        let mut with_json = args.clone();
        with_json.push("--json");
        let (value, code) = json(&with_json);
        assert_eq!(value["status"], "ok", "{args:?} -> {value}");
        assert_eq!(value["command"], args[0], "{value}");
        assert_eq!(value["format"], 1);
        assert!(value["tidy_up"].is_string());
        assert_eq!(code, 0, "{args:?}");
    }
}

/// Anything else on stdout would make the output unparseable, so the prose
/// renderer has to be completely silent.
#[test]
fn json_mode_prints_nothing_else_on_stdout() {
    let dir = sandbox();
    demo(dir.path());
    let out = tidy(&["organize", s(dir.path()), "--dry-run", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one line, got:\n{stdout}"
    );
    assert!(stdout.starts_with('{'), "{stdout}");
    for noise in ["Plan", "Scanned", "Dry run", "Left alone"] {
        assert!(
            !stdout.contains(noise),
            "{noise} leaked into stdout:\n{stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// The distinction the codes exist for. A caller that cannot tell a refusal
/// from a crash will either retry something it should not, or give up on
/// something it should retry.
#[test]
fn a_refusal_exits_differently_from_a_failure() {
    let Some(home) = home() else { return };

    let (refused, code) = json(&["organize", s(&home), "--yes", "--json"]);
    assert_eq!(code, 3, "guard refusal: {refused}");
    assert_eq!(refused["status"], "error");
    assert_eq!(refused["error"]["code"], "system_folder");
    assert!(refused["error"]["path"].is_string());

    let missing = if cfg!(windows) {
        r"C:\definitely\not\here"
    } else {
        "/definitely/not/here"
    };
    let (failed, code) = json(&["organize", missing, "--yes", "--json"]);
    assert_eq!(code, 1, "missing folder: {failed}");
    assert_eq!(failed["error"]["code"], "not_found");
}

/// The message is for a person and may be reworded; the code is the contract.
#[test]
fn the_error_message_is_not_repeated_back_to_itself() {
    let missing = if cfg!(windows) {
        r"C:\definitely\not\here"
    } else {
        "/definitely/not/here"
    };
    let (failed, _) = json(&["organize", missing, "--yes", "--json"]);
    let message = failed["error"]["message"].as_str().unwrap();
    assert_eq!(
        message.matches("os error").count(),
        1,
        "the cause is stated once, not chained onto itself: {message}"
    );
}

/// A run that could not do everything still did something, so by default it
/// succeeds. `--strict` is how a caller opts into hearing about it.
#[test]
fn strict_only_changes_the_exit_code_of_a_run_that_skipped_something() {
    let dir = sandbox();
    demo(dir.path());
    let out = tidy(&["organize", s(dir.path()), "--yes", "--strict"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a clean run must not fail under --strict:\n{}",
        all_output(&out)
    );
}

// ---------------------------------------------------------------------------
// Propose, review, apply
// ---------------------------------------------------------------------------

/// The cycle the whole machine interface exists for: one process proposes, a
/// person or another process reviews, a second process carries it out. Nothing
/// is re-decided in between.
#[test]
fn a_plan_can_be_proposed_then_applied_by_a_separate_run() {
    let dir = sandbox();
    demo(dir.path());

    let (proposed, code) = json(&["organize", s(dir.path()), "--dry-run", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(proposed["outcome"]["dry_run"], true);
    assert_eq!(proposed["outcome"]["plan"]["moves"], 2);
    // Proposing changes nothing at all.
    assert!(dir.path().join("a.png").exists());
    assert!(!dir.path().join(".tidy-up").exists());

    let plan_path = dir.path().join("plan.json");
    fs::write(&plan_path, proposed.to_string()).unwrap();

    let (applied, code) = json(&["apply", s(&plan_path), "--yes", "--json"]);
    assert_eq!(code, 0, "{applied}");
    assert_eq!(applied["outcome"]["execution"]["moved"], 2, "{applied}");
    assert!(dir.path().join("Images/a.png").exists());
    assert!(dir.path().join("Documents/b.pdf").exists());

    // And it is undoable like any other run.
    let journal = applied["outcome"]["execution"]["journal_id"]
        .as_str()
        .unwrap();
    assert!(!journal.is_empty());
    assert!(
        tidy(&["restore", s(dir.path()), "--all", "--yes"])
            .status
            .success()
    );
    assert!(dir.path().join("a.png").exists());
}

#[test]
fn apply_can_preview_before_it_commits() {
    let dir = sandbox();
    demo(dir.path());
    let (proposed, _) = json(&["organize", s(dir.path()), "--dry-run", "--json"]);
    let plan_path = dir.path().join("plan.json");
    fs::write(&plan_path, proposed.to_string()).unwrap();

    let (previewed, code) = json(&["apply", s(&plan_path), "--dry-run", "--json"]);
    assert_eq!(code, 0, "{previewed}");
    assert!(dir.path().join("a.png").exists(), "nothing moved");
    assert!(!dir.path().join(".tidy-up").exists(), "no state created");
}

// ---------------------------------------------------------------------------
// The trust boundary
// ---------------------------------------------------------------------------

/// A plan file is JSON on disk. It can be hand-edited, or produced by
/// something acting on instructions from elsewhere. If `apply` carried one out
/// unchecked it would be a way to move any file anywhere, so a plan may only
/// touch paths inside its own root.
#[test]
fn a_plan_that_reaches_outside_its_root_is_refused() {
    let dir = sandbox();
    demo(dir.path());
    let (proposed, _) = json(&["organize", s(dir.path()), "--dry-run", "--json"]);

    let mut plan = proposed["outcome"]["plan"].clone();
    let escape = if cfg!(windows) {
        r"C:\Windows\pwned.png"
    } else {
        "/etc/pwned.png"
    };
    plan["items"][0]["to"] = Value::String(escape.to_string());

    let plan_path = dir.path().join("evil.json");
    fs::write(&plan_path, plan.to_string()).unwrap();

    let (result, code) = json(&["apply", s(&plan_path), "--yes", "--json"]);
    assert_ne!(code, 0, "this must not be carried out: {result}");
    assert_eq!(result["status"], "error");
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("outside its own root"),
        "{result}"
    );
    assert!(!Path::new(escape).exists(), "it must not have been created");
    assert!(dir.path().join("a.png").exists(), "and nothing moved");
}

#[test]
fn a_plan_file_that_is_not_a_plan_is_an_error_not_a_panic() {
    let dir = sandbox();
    for (name, body) in [
        ("garbage.json", "not json at all"),
        ("empty.json", "{}"),
        ("partial.json", r#"{"format":1,"tool":"0.0.0"}"#),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, body).unwrap();
        let out = tidy(&["apply", s(&path), "--yes", "--json"]);
        assert!(!out.status.success(), "{name} should have failed");
        let text = all_output(&out);
        assert!(!text.contains("panicked"), "{name}:\n{text}");
    }
}

/// The guard applies to a plan exactly as it would to the original command;
/// a plan cannot talk its way past it.
#[test]
fn applying_a_plan_rooted_at_a_system_folder_is_refused() {
    let Some(home) = home() else { return };
    let dir = sandbox();
    let plan = serde_json::json!({
        "format": 1,
        "tool": "0.0.0",
        "operation": "organize",
        "created_at": 0,
        "root": home,
        "moves": 1,
        "bytes": 1,
        "items": [{
            "from": home.join("a.png"),
            "to": home.join("Images").join("a.png"),
            "kind": "file",
            "size": 1
        }]
    });
    let path = dir.path().join("system.json");
    fs::write(&path, plan.to_string()).unwrap();

    let (result, code) = json(&["apply", s(&path), "--yes", "--json"]);
    assert_eq!(code, 3, "{result}");
    assert_eq!(result["error"]["code"], "system_folder");
}
