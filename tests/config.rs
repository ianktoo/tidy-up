//! End-to-end tests for the policy file.
//!
//! The thing being tested is that a policy is a *ceiling*: an installation
//! states what it permits and a caller cannot get around it by typing
//! something else. That only means anything if it holds through the real
//! binary, so these drive it.

mod common;

use std::{fs, path::Path};

use common::{all_output, sandbox, tidy};

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Writes a policy file and returns its path.
fn policy(dir: &Path, body: serde_json::Value) -> std::path::PathBuf {
    let path = dir.join("policy.json");
    fs::write(&path, body.to_string()).unwrap();
    path
}

/// A sandbox with `allowed/` and `other/`, each holding one loose file.
fn two_folders() -> tempfile::TempDir {
    let dir = sandbox();
    for name in ["allowed", "other"] {
        fs::create_dir(dir.path().join(name)).unwrap();
        fs::write(dir.path().join(name).join("a.png"), "x").unwrap();
    }
    dir
}

#[test]
fn a_policy_confines_which_folders_may_be_worked_on() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"roots": [allowed]}}),
    );

    let ok = tidy(&["organize", s(&allowed), "--yes", "--config", s(&config)]);
    assert!(ok.status.success(), "{}", all_output(&ok));
    assert!(allowed.join("Images").join("a.png").exists());

    let other = dir.path().join("other");
    let refused = tidy(&["organize", s(&other), "--yes", "--config", s(&config)]);
    assert_eq!(refused.status.code(), Some(3), "{}", all_output(&refused));
    let text = all_output(&refused);
    assert!(text.contains("not one of the folders"), "{text}");
    assert!(
        other.join("a.png").exists(),
        "and the file was not touched:\n{text}"
    );
}

/// A caller who passes a flag the policy forbids is told, rather than having
/// it quietly ignored. They believe they have authorised something.
#[test]
fn a_forbidden_override_is_refused_rather_than_ignored() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"allow_system_folders": false}}),
    );

    let out = tidy(&[
        "organize",
        s(&allowed),
        "--yes",
        "--allow-system-folder",
        "--config",
        s(&config),
    ]);
    assert!(!out.status.success());
    let text = all_output(&out);
    assert!(text.contains("--allow-system-folder"), "{text}");
    assert!(text.contains("policy"), "{text}");
}

#[test]
fn a_read_only_installation_refuses_to_change_anything() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"allow_writes": false}}),
    );

    let out = tidy(&["organize", s(&allowed), "--yes", "--config", s(&config)]);
    assert_eq!(out.status.code(), Some(3), "{}", all_output(&out));
    assert!(allowed.join("a.png").exists(), "nothing moved");

    // Reading is still allowed, which is the point of the setting.
    let reading = tidy(&["history", s(&allowed), "--config", s(&config)]);
    assert!(reading.status.success(), "{}", all_output(&reading));
}

/// The security property. A folder must not be able to say what may be done
/// to it: otherwise unpacking an archive and tidying it lets the archive
/// decide.
#[test]
fn a_folder_cannot_grant_itself_permissions() {
    let dir = two_folders();
    let other = dir.path().join("other");
    let allowed = dir.path().join("allowed");

    // The folder that is *not* allowed ships a config claiming it is.
    fs::write(
        other.join(".tidy-up.json"),
        serde_json::json!({
            "policy": {"roots": [other], "allow_writes": true, "allow_system_folders": true}
        })
        .to_string(),
    )
    .unwrap();

    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"roots": [allowed]}}),
    );
    let out = tidy(&["organize", s(&other), "--yes", "--config", s(&config)]);

    assert_eq!(
        out.status.code(),
        Some(3),
        "the folder's own config must not have widened anything:\n{}",
        all_output(&out)
    );
    assert!(other.join("a.png").exists(), "nothing moved");
}

#[test]
fn an_unreadable_or_nonsense_config_stops_the_run() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");

    let missing = tidy(&[
        "organize",
        s(&allowed),
        "--yes",
        "--config",
        s(&dir.path().join("absent.json")),
    ]);
    assert!(!missing.status.success(), "a named config must exist");

    let broken = dir.path().join("broken.json");
    fs::write(&broken, "{ not json").unwrap();
    let out = tidy(&["organize", s(&allowed), "--yes", "--config", s(&broken)]);
    assert!(!out.status.success());
    assert!(!all_output(&out).contains("panicked"));
    assert!(
        allowed.join("a.png").exists(),
        "an unreadable policy must not mean no policy"
    );
}

/// A typo in a policy would otherwise mean an installation believes it is
/// locked down and is not.
#[test]
fn a_misspelled_policy_field_is_rejected() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"allow_system_folder": false}}),
    );
    let out = tidy(&["organize", s(&allowed), "--yes", "--config", s(&config)]);
    assert!(!out.status.success(), "{}", all_output(&out));
    assert!(all_output(&out).contains("allow_system_folder"));
}

#[test]
fn defaults_apply_when_a_flag_is_silent() {
    let dir = sandbox();
    fs::write(dir.path().join("keep.png"), "x").unwrap();
    fs::write(dir.path().join("skip.iso"), "y").unwrap();
    let config = policy(
        dir.path(),
        serde_json::json!({"defaults": {"ignore_ext": ["iso"]}}),
    );

    assert!(
        tidy(&["organize", s(dir.path()), "--yes", "--config", s(&config)])
            .status
            .success()
    );
    assert!(dir.path().join("Images").join("keep.png").exists());
    assert!(
        dir.path().join("skip.iso").exists(),
        "the configured ignore rule applied"
    );
}

#[test]
fn a_profile_must_exist_and_needs_a_config() {
    let dir = two_folders();
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"profiles": {"photos": {"by": ["year"]}}}),
    );

    let unknown = tidy(&[
        "organize",
        s(&allowed),
        "--yes",
        "--config",
        s(&config),
        "--profile",
        "nope",
    ]);
    assert!(!unknown.status.success());
    let text = all_output(&unknown);
    assert!(text.contains("nope") && text.contains("photos"), "{text}");

    let orphan = tidy(&["organize", s(&allowed), "--yes", "--profile", "photos"]);
    assert!(!orphan.status.success(), "a profile needs a config");
    assert!(all_output(&orphan).contains("needs a config"));
}

#[test]
fn the_environment_variable_names_a_config_too() {
    let dir = two_folders();
    let other = dir.path().join("other");
    let allowed = dir.path().join("allowed");
    let config = policy(
        dir.path(),
        serde_json::json!({"policy": {"roots": [allowed]}}),
    );

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tidy-up"))
        .args(["organize", s(&other), "--yes"])
        .env("NO_COLOR", "1")
        .env("TIDY_UP_CONFIG", s(&config))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("binary runs");
    assert_eq!(out.status.code(), Some(3), "the policy applied");
}

/// With no config at all, nothing changes. This is the regression net for
/// everyone who never writes one.
#[test]
fn no_config_means_no_restrictions() {
    let dir = two_folders();
    for name in ["allowed", "other"] {
        let folder = dir.path().join(name);
        let out = tidy(&["organize", s(&folder), "--yes"]);
        assert!(out.status.success(), "{}", all_output(&out));
        assert!(folder.join("Images").join("a.png").exists());
    }
}
