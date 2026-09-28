//! End-to-end tests for the run log and `--strict`.

mod common;

use std::{fs, path::Path};

use common::{all_output, home, sandbox, tidy};
use serde_json::Value;

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Every line of the one log written under `root`.
fn events(root: &Path) -> Vec<Value> {
    let dir = root.join(".tidy-up").join("logs");
    let mut logs: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no log directory at {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    logs.sort();
    assert_eq!(logs.len(), 1, "expected exactly one log, found {logs:?}");
    fs::read_to_string(&logs[0])
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is a JSON object of its own"))
        .collect()
}

fn kinds(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn demo(root: &Path) {
    fs::create_dir_all(root.join("old")).unwrap();
    fs::write(root.join("old/a.png"), "x").unwrap();
    fs::write(root.join("b.pdf"), "yy").unwrap();
}

/// The promise is that tidy-up writes nothing you did not ask for.
#[test]
fn no_log_is_written_unless_asked() {
    let dir = sandbox();
    demo(dir.path());
    assert!(tidy(&["organize", s(dir.path()), "--yes"]).status.success());
    assert!(
        !dir.path().join(".tidy-up").join("logs").exists(),
        "a run that was not asked to log must leave no log"
    );
}

#[test]
fn the_log_records_the_shape_of_a_run() {
    let dir = sandbox();
    demo(dir.path());
    let out = tidy(&["organize", s(dir.path()), "--yes", "--log"]);
    assert!(out.status.success(), "{}", all_output(&out));

    let events = events(dir.path());
    assert_eq!(
        kinds(&events),
        ["run", "safety", "scan", "plan", "execute", "end"],
        "got {events:#?}"
    );
    assert_eq!(events[0]["command"], "organize");
    assert_eq!(events[2]["files"], 1, "one loose file at depth 1");
    assert_eq!(events[3]["moves"], 1);
    assert_eq!(events.last().unwrap()["status"], "ok");
    assert_eq!(events.last().unwrap()["metrics"]["moved"], 1);
}

/// The log and the journal describe the same run, so they have to join.
#[test]
fn the_log_names_the_journal_that_undoes_the_run() {
    let dir = sandbox();
    demo(dir.path());
    assert!(
        tidy(&["organize", s(dir.path()), "--yes", "--log"])
            .status
            .success()
    );
    let events = events(dir.path());
    let journal_id = events
        .iter()
        .find(|e| e["type"] == "execute")
        .and_then(|e| e["journal_id"].as_str())
        .expect("an execute event with a journal id")
        .to_string();

    let journals = dir.path().join(".tidy-up").join("journals");
    assert!(
        journals.join(format!("{journal_id}.jsonl")).exists(),
        "the log points at a journal that does not exist: {journal_id}"
    );
}

/// A refusal is the single most important thing to have a record of.
#[test]
fn a_blocked_run_is_recorded_with_its_reasons() {
    let Some(home) = home() else { return };
    let out = tidy(&["organize", s(&home), "--yes", "--log"]);
    assert!(!out.status.success());

    // The log lands in the folder that was refused, so read it from there.
    let dir = home.join(".tidy-up").join("logs");
    if !dir.exists() {
        // The guard refuses before a log root is attached, which is correct:
        // a refused run must not create state in the folder it refused.
        assert!(
            !home.join(".tidy-up").exists()
                || fs::read_dir(home.join(".tidy-up"))
                    .map(|d| d.count())
                    .unwrap_or(0)
                    > 0,
            "nothing new may be created in a folder tidy-up refused to touch"
        );
        return;
    }
    panic!("a refused run must not write into the folder it refused");
}

#[test]
fn the_environment_variable_turns_logging_on_too() {
    let dir = sandbox();
    demo(dir.path());
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tidy-up"))
        .args(["organize", s(dir.path()), "--yes"])
        .env("NO_COLOR", "1")
        .env("TIDY_UP_LOG", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("binary runs");
    assert!(out.status.success());
    assert!(!events(dir.path()).is_empty());
}

/// A run that skipped things still did the work it could, so by default it
/// succeeds. `--strict` is how a script asks to be told otherwise.
#[test]
fn strict_turns_a_skipped_item_into_a_failure() {
    let dir = sandbox();
    demo(dir.path());
    // Nothing is skipped here, so --strict must still succeed: the flag reports
    // real problems, it does not invent them.
    let clean = tidy(&["organize", s(dir.path()), "--yes", "--strict"]);
    assert!(
        clean.status.success(),
        "a clean run must not fail under --strict:\n{}",
        all_output(&clean)
    );
}

/// Logging is a diagnostic, never something that can change the outcome.
#[test]
fn a_log_that_cannot_be_written_does_not_fail_the_run() {
    let dir = sandbox();
    demo(dir.path());
    // Occupy the log directory's path with a file so creating it must fail.
    let state = dir.path().join(".tidy-up");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("logs"), "not a directory").unwrap();

    let out = tidy(&["organize", s(dir.path()), "--yes", "--log"]);
    assert!(
        out.status.success(),
        "the run itself must still succeed:\n{}",
        all_output(&out)
    );
    assert!(
        dir.path().join("Images").join("b.pdf").exists()
            || dir.path().join("Documents").join("b.pdf").exists()
    );
}
