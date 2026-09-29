//! Tests for [`crate::config`].
//!
//! The distinction being tested throughout is policy versus defaults: a
//! ceiling flags cannot raise, against a floor any flag overrides.

use super::*;

fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Media")
    } else {
        PathBuf::from("/media")
    }
}

fn inside(rel: &str) -> PathBuf {
    root().join(rel)
}

fn elsewhere() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Secrets")
    } else {
        PathBuf::from("/secrets")
    }
}

// ---------------------------------------------------------------------------
// The security property
// ---------------------------------------------------------------------------

/// The one that matters. A folder must not be able to say what may be done to
/// it: download an archive, run tidy-up on it, and otherwise the archive
/// decides. Auto-discovered configuration may lower the bar for convenience,
/// never raise it for permission.
#[test]
fn a_folder_config_cannot_grant_itself_permissions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FOLDER_CONFIG),
        r#"{
            "policy": {"allow_system_folders": true, "allow_writes": true, "max_depth": 99},
            "defaults": {"ignore_ext": ["iso"], "depth": 3}
        }"#,
    )
    .unwrap();

    let (config, tried) = Config::load_from_folder(dir.path());
    let config = config.expect("the file is read");
    assert!(tried, "the caller must be told it tried");
    assert!(
        config.policy.is_open(),
        "the policy must be dropped entirely, got {:?}",
        config.policy
    );
    // The harmless half still applies, which is the point of allowing the file.
    assert_eq!(config.defaults.ignore_ext, ["iso"]);
    assert_eq!(config.defaults.depth, Some(3));
}

#[test]
fn a_folder_with_no_config_or_a_broken_one_is_simply_unconfigured() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Config::load_from_folder(dir.path()).0, None);

    std::fs::write(dir.path().join(FOLDER_CONFIG), "{ not json").unwrap();
    let (config, tried) = Config::load_from_folder(dir.path());
    assert_eq!(config, None, "unreadable is the same as absent");
    assert!(!tried);
}

/// A folder config with no policy in it should not trip the warning.
#[test]
fn a_well_behaved_folder_config_does_not_warn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(FOLDER_CONFIG),
        r#"{"defaults": {"depth": 2}}"#,
    )
    .unwrap();
    let (config, tried) = Config::load_from_folder(dir.path());
    assert!(!tried);
    assert_eq!(config.unwrap().defaults.depth, Some(2));
}

// ---------------------------------------------------------------------------
// Policy as a ceiling
// ---------------------------------------------------------------------------

#[test]
fn an_absent_policy_restricts_nothing() {
    let open = Policy::default();
    assert!(open.is_open());
    assert!(open.check_root(&elsewhere()).is_ok());
    assert!(open.check_write().is_ok());
    assert!(open.check_delete().is_ok());
    assert!(open.resolve_override(true).unwrap());
    assert_eq!(open.cap_depth(99), 99);
    assert_eq!(open.summary(), "");
}

#[test]
fn roots_confine_where_work_may_happen() {
    let policy = Policy {
        roots: vec![root()],
        ..Default::default()
    };
    assert!(policy.check_root(&root()).is_ok(), "the root itself");
    assert!(policy.check_root(&inside("photos/2024")).is_ok());

    let err = policy.check_root(&elsewhere()).unwrap_err().to_string();
    assert!(err.contains("not one of the folders"), "{err}");
    assert!(
        err.contains(&root().display().to_string()),
        "says which: {err}"
    );
}

/// Containment is component-wise, like the guard's, so a folder whose name
/// merely starts with an allowed one is still outside.
#[test]
fn a_name_that_merely_starts_the_same_is_not_inside() {
    let policy = Policy {
        roots: vec![root()],
        ..Default::default()
    };
    let lookalike = PathBuf::from(format!("{}-other", root().display()));
    assert!(policy.check_root(&lookalike).is_err(), "{lookalike:?}");
}

#[test]
fn several_roots_are_all_allowed() {
    let policy = Policy {
        roots: vec![root(), elsewhere()],
        ..Default::default()
    };
    assert!(policy.check_root(&inside("a")).is_ok());
    assert!(policy.check_root(&elsewhere().join("b")).is_ok());
}

#[test]
fn a_read_only_installation_refuses_changes_and_says_so() {
    let policy = Policy {
        allow_writes: Some(false),
        ..Default::default()
    };
    let err = policy.check_write().unwrap_err().to_string();
    assert!(err.contains("read-only"), "{err}");
    assert!(policy.check_delete().is_ok(), "only writes were forbidden");
}

#[test]
fn deleting_can_be_forbidden_on_its_own() {
    let policy = Policy {
        allow_delete: Some(false),
        ..Default::default()
    };
    assert!(policy.check_delete().is_err());
    assert!(policy.check_write().is_ok(), "moving is still allowed");
}

/// Passing a flag that policy forbids is an error rather than a silent no.
/// The caller believes they have authorised something; they have not, and
/// finding out afterwards is worse than being told.
#[test]
fn a_forbidden_override_flag_is_an_error_not_a_quiet_no() {
    let policy = Policy {
        allow_system_folders: Some(false),
        ..Default::default()
    };
    let err = policy.resolve_override(true).unwrap_err().to_string();
    assert!(err.contains("--allow-system-folder"), "{err}");
    assert!(err.contains("policy"), "{err}");
    assert!(
        !policy.resolve_override(false).unwrap(),
        "not passing it is fine"
    );
}

#[test]
fn depth_is_capped_but_never_raised() {
    let policy = Policy {
        max_depth: Some(3),
        ..Default::default()
    };
    assert_eq!(policy.cap_depth(99), 3, "capped");
    assert_eq!(policy.cap_depth(1), 1, "a smaller request is left alone");
}

#[test]
fn the_summary_names_every_restriction_in_force() {
    let policy = Policy {
        roots: vec![root()],
        allow_writes: Some(false),
        allow_system_folders: Some(false),
        allow_delete: Some(false),
        max_depth: Some(2),
    };
    let summary = policy.summary();
    for expected in [
        "1 allowed folder",
        "read-only",
        "no system-folder override",
        "no deleting",
        "depth capped at 2",
    ] {
        assert!(
            summary.contains(expected),
            "{expected} missing from {summary}"
        );
    }
}

// ---------------------------------------------------------------------------
// Defaults as a floor
// ---------------------------------------------------------------------------

#[test]
fn a_profile_overlays_the_defaults_it_mentions_and_no_others() {
    let config = Config::parse(
        r#"{
            "defaults": {"ignore_ext": ["tmp"], "depth": 1, "log": true},
            "profiles": {"photos": {"by": ["year", "month"], "depth": 4}}
        }"#,
    )
    .unwrap();

    let plain = config.resolve(None).unwrap();
    assert_eq!(plain.depth, Some(1));
    assert!(plain.by.is_empty());

    let photos = config.resolve(Some("photos")).unwrap();
    assert_eq!(photos.depth, Some(4), "the profile wins");
    assert_eq!(photos.by, [GroupBy::Year, GroupBy::Month]);
    assert_eq!(photos.ignore_ext, ["tmp"], "and inherits the rest");
    assert_eq!(photos.log, Some(true));
}

#[test]
fn an_unknown_profile_lists_the_ones_that_exist() {
    let config = Config::parse(r#"{"profiles": {"photos": {}, "code": {}}}"#).unwrap();
    let err = config.resolve(Some("videos")).unwrap_err().to_string();
    assert!(err.contains("videos"), "{err}");
    assert!(err.contains("photos") && err.contains("code"), "{err}");

    let empty = Config::parse("{}").unwrap();
    let err = empty.resolve(Some("anything")).unwrap_err().to_string();
    assert!(err.contains("none"), "{err}");
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[test]
fn an_empty_config_is_valid_and_changes_nothing() {
    let config = Config::parse("{}").unwrap();
    assert!(config.policy.is_open());
    assert_eq!(config.defaults, Defaults::default());
    assert!(config.profiles.is_empty());
}

/// A typo in a config is worth failing over. Silently ignoring
/// `"allow_system_folder"` when the field is `allow_system_folders` would mean
/// an installation believes it is locked down and is not.
#[test]
fn an_unrecognised_field_is_rejected_rather_than_ignored() {
    let err = Config::parse(r#"{"policy": {"allow_system_folder": false}}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("allow_system_folder"), "{err}");
    assert!(
        Config::parse(r#"{"polciy": {}}"#).is_err(),
        "a typo at the top level too"
    );
}

#[test]
fn a_config_from_a_newer_tidy_up_says_so() {
    let err = Config::parse(&format!(r#"{{"format": {}}}"#, FORMAT_VERSION + 1))
        .unwrap_err()
        .to_string();
    assert!(err.contains("newer tidy-up"), "{err}");
}

#[test]
fn a_config_round_trips() {
    let config = Config {
        format: FORMAT_VERSION,
        policy: Policy {
            roots: vec![root()],
            allow_writes: Some(true),
            allow_system_folders: Some(false),
            allow_delete: None,
            max_depth: Some(4),
        },
        defaults: Defaults {
            ignore_ext: vec!["iso".into()],
            projects: Some(ProjectPolicy::Move),
            by: vec![GroupBy::Year],
            ..Default::default()
        },
        profiles: BTreeMap::new(),
    };
    let text = serde_json::to_string(&config).unwrap();
    assert_eq!(Config::parse(&text).unwrap(), config);
}

#[test]
fn a_named_config_is_read_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.json");
    std::fs::write(&path, r#"{"policy": {"max_depth": 2}}"#).unwrap();
    assert_eq!(Config::load(&path).unwrap().policy.max_depth, Some(2));
    assert!(Config::load(&dir.path().join("absent.json")).is_err());
}

/// With nothing installed, everything behaves as it did before policies
/// existed.
#[test]
fn the_policy_in_force_defaults_to_unrestricted() {
    let current = policy();
    assert!(current.check_root(&elsewhere()).is_ok());
    assert!(current.check_write().is_ok());
    assert_eq!(current.cap_depth(1000), 1000);
}
