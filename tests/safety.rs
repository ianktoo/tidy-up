//! End-to-end tests for the system-folder guard, driving the compiled binary.
//!
//! These run without a terminal, which is the situation the guard has to get
//! right: a script, a CI job, a cron entry. Nothing here writes to a real system
//! folder; the refusals are asserted by the fact that nothing happened.

mod common;

use std::fs;

use common::{all_output, home, sandbox, tidy};

fn s(path: &std::path::Path) -> &str {
    path.to_str().unwrap()
}

/// The guard must be invisible when it has nothing to say. A warning on an
/// ordinary folder would train people to ignore it.
#[test]
fn an_ordinary_folder_is_never_warned_about() {
    let dir = sandbox();
    fs::write(dir.path().join("a.png"), "x").unwrap();
    let out = tidy(&["organize", s(dir.path()), "--yes"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("Careful"), "{text}");
    assert!(dir.path().join("Images/a.png").exists(), "{text}");
}

/// The flagship property: passing `--yes` must not be a way to reorganize your
/// own home directory, let alone the operating system.
#[test]
fn yes_does_not_unlock_the_home_directory() {
    let Some(home) = home() else { return };
    let before: Vec<_> = fs::read_dir(&home)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect()
        })
        .unwrap_or_default();

    let out = tidy(&["organize", s(&home), "--yes"]);
    let text = all_output(&out);

    assert!(!out.status.success(), "the run must fail:\n{text}");
    assert!(
        text.contains("--allow-system-folder"),
        "the refusal must name the way through:\n{text}"
    );
    let after: Vec<_> = fs::read_dir(&home)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        before, after,
        "nothing in the home directory may have moved"
    );
}

/// The override gets you to the question, not past it. With no terminal to
/// answer on and no `--yes`, it still stops.
#[test]
fn the_override_alone_still_cannot_run_unattended() {
    let Some(home) = home() else { return };
    let out = tidy(&["organize", s(&home), "--allow-system-folder"]);
    assert!(
        !out.status.success(),
        "there is nobody to confirm:\n{}",
        all_output(&out)
    );
}

/// A dry run changes nothing, so it is allowed to look. Seeing the plan is the
/// most persuasive argument against running it for real.
#[test]
fn a_dry_run_on_a_dangerous_folder_warns_and_proceeds() {
    let Some(home) = home() else { return };
    let out = tidy(&[
        "organize",
        s(&home),
        "--allow-system-folder",
        "--yes",
        "--dry-run",
    ]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Careful"), "{text}");
    assert!(text.contains("Dry run"), "{text}");
    assert!(
        !home.join(".tidy-up").exists(),
        "a dry run must not create state"
    );
}

/// Reporting commands warn and carry on. Blocking them would be a regression:
/// looking at a drive is one of the reasons `analyze` exists.
#[test]
fn reporting_commands_warn_but_are_never_blocked() {
    let Some(home) = home() else { return };
    let out = tidy(&["history", s(&home)]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Careful"), "{text}");
    assert!(
        !text.contains("refusing"),
        "a reporting command must not refuse:\n{text}"
    );
}

/// A folder the guard refuses, which is not an ancestor of the sandbox. Home
/// cannot be used here: on Windows the sandbox lives inside it, and distribute
/// rejects overlapping pairs before the guard is ever reached.
#[cfg(windows)]
const SYSTEM_DIR: &str = r"C:\Windows";
#[cfg(not(windows))]
const SYSTEM_DIR: &str = "/usr";

/// Destinations are guarded too: `--to C:\Windows` is every bit as bad as
/// pointing `--from` at it.
#[test]
fn distribute_guards_its_destinations() {
    if !std::path::Path::new(SYSTEM_DIR).is_dir() {
        return;
    }
    let dir = sandbox();
    fs::write(dir.path().join("a.bin"), "x").unwrap();
    let out = tidy(&[
        "distribute",
        "--from",
        s(dir.path()),
        "--to",
        SYSTEM_DIR,
        "--yes",
    ]);
    let text = all_output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("Careful"), "{text}");
    assert!(
        dir.path().join("a.bin").exists(),
        "the source is untouched:\n{text}"
    );
}

#[cfg(windows)]
#[test]
fn the_windows_folder_is_refused() {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    if !std::path::Path::new(&windows).is_dir() {
        return;
    }
    let out = tidy(&["organize", &windows, "--yes"]);
    let text = all_output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("Careful"), "{text}");
}

/// Analyzing a drive root is an advertised use case, so the guard must warn
/// rather than block. Asserted against a small folder marked dangerous by the
/// same code path, because pointing `analyze` at a real drive root walks the
/// whole disk and would take minutes.
#[cfg(windows)]
#[test]
fn a_drive_root_is_not_blocked_by_the_guard() {
    let out = tidy(&["history", r"C:\"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("Careful"),
        "the warning is still printed:\n{text}"
    );
    assert!(!text.contains("refusing"), "{text}");
}

#[cfg(unix)]
#[test]
fn the_filesystem_root_is_refused() {
    let out = tidy(&["organize", "/", "--yes"]);
    let text = all_output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("--allow-system-folder"), "{text}");
}

/// Every destructive command routes through the guard. This is the test that
/// catches a new command being added without one.
#[test]
fn every_destructive_command_refuses_the_home_directory() {
    let Some(home) = home() else { return };
    let path = s(&home);
    for args in [
        vec!["organize", path, "--yes"],
        vec!["dedupe", path, "--yes"],
        vec!["restore", path, "--yes"],
        vec!["purge", path, "--yes"],
        vec!["compare", path, "--yes", "--action", "leave"],
    ] {
        let out = tidy(&args);
        let text = all_output(&out);
        assert!(
            !out.status.success(),
            "`tidy-up {}` should have been refused:\n{text}",
            args.join(" ")
        );
        assert!(
            text.contains("--allow-system-folder"),
            "`tidy-up {}` refused without naming the flag:\n{text}",
            args.join(" ")
        );
    }
}

/// `analyze` and `history` are the two that must not block, stated as a pair so
/// the previous test cannot be "fixed" by blocking everything.
#[test]
fn the_reporting_commands_are_the_exception() {
    let Some(home) = home() else { return };
    let path = s(&home);
    // `analyze` is not exercised here: it walks everything below the folder,
    // which on a home directory takes minutes. It reaches this same guard
    // through `Guard::read`, which is unit-tested in `commands::guard`.
    let out = tidy(&["history", path]);
    assert!(
        out.status.success(),
        "`tidy-up history` must not be blocked:\n{}",
        all_output(&out)
    );
}
