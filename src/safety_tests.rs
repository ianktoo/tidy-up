//! Rule-table tests for [`crate::safety`].
//!
//! Every case runs on every host: the platform is an argument, not the build
//! target, so the Windows rules are checked on Linux and the Linux rules on
//! Windows. Nothing here touches a real filesystem.

use super::*;

/// One row of the rule table.
struct Case {
    platform: Platform,
    path: &'static str,
    home: Option<&'static str>,
    temp: Option<&'static str>,
    want: Risk,
    /// A reason key that must be among the findings, when it matters which rule fired.
    want_reason: Option<&'static str>,
    why: &'static str,
}

const fn case(
    platform: Platform,
    path: &'static str,
    want: Risk,
    want_reason: Option<&'static str>,
    why: &'static str,
) -> Case {
    Case {
        platform,
        path,
        home: None,
        temp: None,
        want,
        want_reason,
        why,
    }
}

const WIN_HOME: &str = r"C:\Users\me";
const MAC_HOME: &str = "/Users/me";
const NIX_HOME: &str = "/home/me";

fn run(cases: &[Case]) {
    for c in cases {
        let mut env = Environment::fake(c.platform);
        let default_home = match c.platform {
            Platform::Windows => Some(WIN_HOME),
            Platform::MacOs => Some(MAC_HOME),
            _ => Some(NIX_HOME),
        };
        if let Some(home) = c.home.or(default_home) {
            env = env.with_home(home);
        }
        if let Some(temp) = c.temp {
            env = env.with_temp(temp);
        }
        let got = classify(Path::new(c.path), &env, &Facts::default());
        assert_eq!(
            got.risk, c.want,
            "{:?} {}: expected {:?}, got {:?} ({:?})\n  {}",
            c.platform, c.path, c.want, got.risk, got.reasons, c.why
        );
        if let Some(reason) = c.want_reason {
            assert!(
                got.reasons.iter().any(|r| r.key() == reason),
                "{:?} {}: expected reason {reason}, got {:?}\n  {}",
                c.platform,
                c.path,
                got.reasons,
                c.why
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[test]
fn windows_system_locations_are_dangerous() {
    use Platform::Windows as W;
    run(&[
        case(
            W,
            r"C:\",
            Risk::Dangerous,
            Some("filesystem_root"),
            "a drive root is not a folder of files",
        ),
        case(
            W,
            r"D:\",
            Risk::Dangerous,
            Some("filesystem_root"),
            "any drive, not just the system one",
        ),
        case(
            W,
            r"\\server\share",
            Risk::Dangerous,
            Some("unc_share_root"),
            "the top of a share",
        ),
        case(
            W,
            r"\\?\UNC\server\share",
            Risk::Dangerous,
            Some("unc_share_root"),
            "the verbatim spelling of the same place",
        ),
        case(
            W,
            r"C:\Windows",
            Risk::Dangerous,
            Some("system_folder"),
            "the operating system",
        ),
        case(
            W,
            r"C:\Windows\System32\drivers",
            Risk::Dangerous,
            Some("system_folder"),
            "deep inside it too",
        ),
        case(
            W,
            r"C:\Program Files",
            Risk::Dangerous,
            Some("system_folder"),
            "installed programs",
        ),
        case(
            W,
            r"C:\Program Files (x86)\App",
            Risk::Dangerous,
            Some("system_folder"),
            "and the 32-bit tree",
        ),
        case(
            W,
            r"C:\Program Files\WindowsApps",
            Risk::Dangerous,
            Some("system_folder"),
            "store apps, via the parent rule",
        ),
        case(
            W,
            r"C:\ProgramData",
            Risk::Dangerous,
            Some("system_folder"),
            "shared application state",
        ),
        case(
            W,
            r"C:\Users",
            Risk::Dangerous,
            Some("user_container"),
            "everybody's homes, not one person's files",
        ),
        case(
            W,
            WIN_HOME,
            Risk::Dangerous,
            Some("home_root"),
            "the home folder itself",
        ),
        case(
            W,
            r"C:\Users\me\AppData\Roaming",
            Risk::Dangerous,
            Some("application_data"),
            "application state",
        ),
        case(
            W,
            r"C:\$Recycle.Bin",
            Risk::Dangerous,
            Some("system_folder"),
            "the recycle bin",
        ),
        case(
            W,
            r"C:\System Volume Information",
            Risk::Dangerous,
            Some("system_folder"),
            "restore points",
        ),
        case(
            W,
            r"D:\Windows",
            Risk::Dangerous,
            Some("system_folder"),
            "a second Windows install on another drive",
        ),
    ]);
}

#[test]
fn windows_ordinary_folders_are_safe() {
    use Platform::Windows as W;
    run(&[
        case(
            W,
            r"D:\Downloads",
            Risk::Safe,
            None,
            "the single most common target there is",
        ),
        case(
            W,
            r"C:\Users\me\Downloads",
            Risk::Safe,
            None,
            "a folder inside the home folder",
        ),
        case(
            W,
            r"C:\Users\me\Documents\Taxes\2024",
            Risk::Safe,
            None,
            "deep inside the home folder",
        ),
        case(
            W,
            r"C:\Program Files Custom",
            Risk::Safe,
            None,
            "component matching, not string prefixes",
        ),
        case(
            W,
            r"C:\Windows Backups",
            Risk::Safe,
            None,
            "same trap, other direction",
        ),
        case(
            W,
            r"D:\Backup\AppData-2019",
            Risk::Safe,
            None,
            "a folder merely named after AppData",
        ),
        case(W, r"E:\Media\Photos", Risk::Safe, None, "an external drive"),
        case(
            W,
            r"\\server\share\Projects",
            Risk::Safe,
            None,
            "inside a share, not its root",
        ),
    ]);
}

/// `%LOCALAPPDATA%\Temp` is inside a protected tree. Without the carve-out the
/// guard would refuse the system temporary folder, and every test in this crate
/// that uses `tempfile` would start failing.
#[test]
fn windows_temp_is_carved_out_of_the_appdata_rule() {
    let env = Environment::fake(Platform::Windows)
        .with_home(WIN_HOME)
        .with_temp(r"C:\Users\me\AppData\Local\Temp");
    let inside = classify(
        Path::new(r"C:\Users\me\AppData\Local\Temp\tidy-up-abc123"),
        &env,
        &Facts::default(),
    );
    assert_eq!(inside.risk, Risk::Safe, "{:?}", inside.reasons);

    let temp_itself = classify(
        Path::new(r"C:\Users\me\AppData\Local\Temp"),
        &env,
        &Facts::default(),
    );
    assert_eq!(
        temp_itself.risk,
        Risk::Dangerous,
        "the temp folder itself is still application data; only things inside it are exempt"
    );
}

/// Variables only ever add coverage. With none set, the name-based rules relative
/// to any drive root must still fire.
#[test]
fn windows_rules_survive_a_scrubbed_environment() {
    let bare = Environment::fake(Platform::Windows);
    for path in [
        r"C:\Windows",
        r"C:\Program Files",
        r"C:\ProgramData",
        r"C:\Users",
    ] {
        let got = classify(Path::new(path), &bare, &Facts::default());
        assert_eq!(got.risk, Risk::Dangerous, "{path} with no variables set");
    }
}

#[test]
fn windows_variables_extend_coverage_to_unusual_installs() {
    let env = Environment::fake(Platform::Windows).with_var("SystemRoot", r"E:\Win");
    let moved = classify(Path::new(r"E:\Win\System32"), &env, &Facts::default());
    assert_eq!(
        moved.risk,
        Risk::Dangerous,
        "%SystemRoot% pointed elsewhere"
    );
    let normal = classify(Path::new(r"C:\Windows"), &env, &Facts::default());
    assert_eq!(
        normal.risk,
        Risk::Dangerous,
        "and the usual place still counts"
    );
}

#[test]
fn windows_paths_are_matched_regardless_of_case_or_separator() {
    let env = Environment::fake(Platform::Windows);
    for path in [
        r"c:\windows",
        r"C:/WINDOWS/system32",
        r"C:\Windows\",
        r"\\?\C:\Windows",
        r"C:\WINDOWS\.\System32",
    ] {
        let got = classify(Path::new(path), &env, &Facts::default());
        assert_eq!(got.risk, Risk::Dangerous, "{path}");
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[test]
fn macos_system_locations_are_dangerous() {
    use Platform::MacOs as M;
    run(&[
        case(
            M,
            "/",
            Risk::Dangerous,
            Some("filesystem_root"),
            "the root of everything",
        ),
        case(
            M,
            "/System",
            Risk::Dangerous,
            Some("system_folder"),
            "macOS itself",
        ),
        case(
            M,
            "/System/Volumes/Data",
            Risk::Dangerous,
            Some("system_folder"),
            "the data volume root",
        ),
        case(
            M,
            "/Library",
            Risk::Dangerous,
            Some("system_folder"),
            "system-wide support files",
        ),
        case(
            M,
            "/Applications",
            Risk::Dangerous,
            Some("system_folder"),
            "installed applications",
        ),
        case(
            M,
            "/usr",
            Risk::Dangerous,
            Some("system_folder"),
            "the operating system",
        ),
        case(
            M,
            "/usr/share",
            Risk::Dangerous,
            Some("system_folder"),
            "inside it",
        ),
        case(
            M,
            "/etc",
            Risk::Dangerous,
            Some("system_folder"),
            "configuration",
        ),
        case(
            M,
            "/var/db",
            Risk::Dangerous,
            Some("system_folder"),
            "system state",
        ),
        case(
            M,
            "/private/etc",
            Risk::Dangerous,
            Some("system_folder"),
            "the real location of /etc",
        ),
        case(
            M,
            "/Volumes",
            Risk::Dangerous,
            Some("system_folder"),
            "the mount point directory itself",
        ),
        case(
            M,
            "/Users",
            Risk::Dangerous,
            Some("user_container"),
            "everybody's homes",
        ),
        case(
            M,
            MAC_HOME,
            Risk::Dangerous,
            Some("home_root"),
            "the home folder itself",
        ),
        case(
            M,
            "/Users/me/Library/Caches",
            Risk::Dangerous,
            Some("application_data"),
            "the macOS AppData",
        ),
    ]);
}

#[test]
fn macos_ordinary_and_downgraded_locations() {
    use Platform::MacOs as M;
    run(&[
        case(
            M,
            "/Volumes/Backup/Photos",
            Risk::Safe,
            None,
            "an external disk, the normal case",
        ),
        case(
            M,
            "/Users/me/Downloads",
            Risk::Safe,
            None,
            "a folder in the home folder",
        ),
        case(
            M,
            "/System/Volumes/Data/Users/me/Downloads",
            Risk::Safe,
            None,
            "the same home folder seen through the data volume",
        ),
        case(
            M,
            "/private/tmp/work",
            Risk::Safe,
            None,
            "the real location of /tmp",
        ),
        case(
            M,
            "/usr/local/share/mine",
            Risk::Caution,
            Some("system_folder"),
            "the writable firmlink",
        ),
        case(
            M,
            "/opt/homebrew",
            Risk::Caution,
            Some("system_folder"),
            "optional software, often user-installed",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[test]
fn linux_system_locations_are_dangerous() {
    use Platform::Linux as L;
    run(&[
        case(
            L,
            "/",
            Risk::Dangerous,
            Some("filesystem_root"),
            "the root of everything",
        ),
        case(
            L,
            "/usr/lib",
            Risk::Dangerous,
            Some("system_folder"),
            "system libraries",
        ),
        case(
            L,
            "/etc",
            Risk::Dangerous,
            Some("system_folder"),
            "configuration",
        ),
        case(
            L,
            "/boot",
            Risk::Dangerous,
            Some("system_folder"),
            "the boot files",
        ),
        case(
            L,
            "/dev",
            Risk::Dangerous,
            Some("system_folder"),
            "device nodes",
        ),
        case(
            L,
            "/proc/1",
            Risk::Dangerous,
            Some("system_folder"),
            "kernel process information",
        ),
        case(
            L,
            "/sys/class",
            Risk::Dangerous,
            Some("system_folder"),
            "kernel settings",
        ),
        case(
            L,
            "/run/systemd",
            Risk::Dangerous,
            Some("system_folder"),
            "runtime state",
        ),
        case(
            L,
            "/home",
            Risk::Dangerous,
            Some("user_container"),
            "everybody's homes",
        ),
        case(
            L,
            "/root",
            Risk::Dangerous,
            Some("system_folder"),
            "the root account's home",
        ),
        case(
            L,
            NIX_HOME,
            Risk::Dangerous,
            Some("home_root"),
            "the home folder itself",
        ),
        case(
            L,
            "/snap/core",
            Risk::Dangerous,
            Some("system_folder"),
            "installed snaps",
        ),
        case(
            L,
            "/var/log",
            Risk::Dangerous,
            Some("system_folder"),
            "system logs",
        ),
    ]);
}

#[test]
fn linux_ordinary_and_downgraded_locations() {
    use Platform::Linux as L;
    run(&[
        case(
            L,
            "/home/me/Downloads",
            Risk::Safe,
            None,
            "a folder in the home folder",
        ),
        case(
            L,
            "/mnt/data/photos",
            Risk::Safe,
            None,
            "a manually mounted disk",
        ),
        case(
            L,
            "/media/me/USB/pics",
            Risk::Safe,
            None,
            "a desktop-mounted USB stick",
        ),
        case(
            L,
            "/run/media/me/USB/pics",
            Risk::Safe,
            None,
            "the same stick under the newer udisks location, carved out of the /run rule",
        ),
        case(
            L,
            "/tmp/work",
            Risk::Safe,
            None,
            "a perfectly reasonable folder to tidy",
        ),
        case(
            L,
            "/var/tmp/work",
            Risk::Safe,
            None,
            "and its persistent cousin",
        ),
        case(
            L,
            "/data",
            Risk::Safe,
            None,
            "a shallow folder somebody made themselves",
        ),
        case(
            L,
            "/srv/media",
            Risk::Caution,
            Some("system_folder"),
            "served data is often user data",
        ),
        case(
            L,
            "/opt/data",
            Risk::Caution,
            Some("system_folder"),
            "optional software, sometimes user data",
        ),
        case(
            L,
            "/usr/local/share/mine",
            Risk::Caution,
            Some("system_folder"),
            "user-installed software",
        ),
    ]);
}

// ---------------------------------------------------------------------------
// The anti-false-positive net
// ---------------------------------------------------------------------------

/// A guard that fires on ordinary folders teaches people to pass the override flag
/// by reflex, which protects nobody. This is the regression net for that.
#[test]
fn ordinary_folders_are_never_flagged() {
    use Platform::{Linux as L, MacOs as M, Windows as W};
    let ordinary: &[(Platform, &str)] = &[
        (W, r"C:\Users\me\Downloads"),
        (W, r"C:\Users\me\Desktop"),
        (W, r"C:\Users\me\Pictures\2024"),
        (W, r"D:\Downloads"),
        (W, r"D:\Media\Movies"),
        (W, r"E:\Backup"),
        (W, r"C:\Dev\projects"),
        (W, r"\\nas\media\Photos"),
        (M, "/Users/me/Downloads"),
        (M, "/Users/me/Desktop"),
        (M, "/Users/me/Documents/Work"),
        (M, "/Volumes/Backup"),
        (M, "/Volumes/Backup/Photos"),
        (M, "/Users/me/Projects/app"),
        (L, "/home/me/Downloads"),
        (L, "/home/me/Desktop"),
        (L, "/home/me/Pictures/2024"),
        (L, "/mnt/storage"),
        (L, "/mnt/storage/media"),
        (L, "/media/me/Elements"),
        (L, "/data"),
        (L, "/data/archive"),
        (L, "/tmp/scratch"),
        (L, "/home/me/code/tidy-up"),
    ];
    for (platform, path) in ordinary {
        let mut env = Environment::fake(*platform);
        env = env.with_home(match platform {
            Platform::Windows => WIN_HOME,
            Platform::MacOs => MAC_HOME,
            _ => NIX_HOME,
        });
        let got = classify(Path::new(path), &env, &Facts::default());
        assert_eq!(
            got.risk,
            Risk::Safe,
            "{path} is an ordinary folder but was flagged {:?}",
            got.reasons
        );
    }
}

// ---------------------------------------------------------------------------
// Fact-driven rules
// ---------------------------------------------------------------------------

#[test]
fn the_windows_system_attribute_alone_is_enough() {
    let env = Environment::fake(Platform::Windows).with_home(WIN_HOME);
    let facts = Facts {
        system_attribute: Some(true),
        ..Default::default()
    };
    let got = classify(Path::new(r"D:\Something"), &env, &facts);
    assert_eq!(got.risk, Risk::Dangerous);
    assert!(got.reasons.iter().any(|r| r.key() == "system_attribute"));
}

#[test]
fn a_pseudo_filesystem_is_dangerous_but_tmpfs_is_not() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let pseudo = Facts {
        fs_type: Some("proc".into()),
        ..Default::default()
    };
    assert_eq!(
        classify(Path::new("/data"), &env, &pseudo).risk,
        Risk::Dangerous
    );

    let tmpfs = Facts {
        fs_type: Some("tmpfs".into()),
        ..Default::default()
    };
    assert_eq!(
        classify(Path::new("/data"), &env, &tmpfs).risk,
        Risk::Safe,
        "tmpfs is where /tmp lives and is a reasonable thing to tidy"
    );
}

/// No flag can grant permission the operating system refused, so this one verdict
/// must not be overridable.
#[test]
fn an_unwritable_folder_is_dangerous_and_cannot_be_overridden() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let facts = Facts {
        writable: Some(false),
        ..Default::default()
    };
    let got = classify(Path::new("/data"), &env, &facts);
    assert_eq!(got.risk, Risk::Dangerous);
    assert!(!got.overridable());
}

#[test]
fn an_ordinary_dangerous_folder_is_overridable() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let got = classify(Path::new("/usr"), &env, &Facts::default());
    assert_eq!(got.risk, Risk::Dangerous);
    assert!(got.overridable(), "the user must have a way through");
}

/// Every external drive is a mount point, so on its own it can never be more than
/// a caution, and it must stay quiet when something stronger already fired.
#[test]
fn a_mount_point_is_only_a_caution_and_only_when_alone() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let facts = Facts {
        mount_point: Some(true),
        ..Default::default()
    };
    let alone = classify(Path::new("/mnt/data"), &env, &facts);
    assert_eq!(alone.risk, Risk::Caution);
    assert!(alone.reasons.iter().any(|r| r.key() == "mount_point"));

    let with_more = classify(Path::new("/usr"), &env, &facts);
    assert_eq!(with_more.risk, Risk::Dangerous);
    assert!(
        !with_more.reasons.iter().any(|r| r.key() == "mount_point"),
        "no point saying it is a mount point when it is also /usr"
    );
}

#[test]
fn an_unowned_folder_is_a_caution() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let facts = Facts {
        owned_by_me: Some(false),
        ..Default::default()
    };
    assert_eq!(
        classify(Path::new("/data"), &env, &facts).risk,
        Risk::Caution
    );
}

#[test]
fn unknown_facts_never_accuse() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let got = classify(Path::new("/data"), &env, &Facts::default());
    assert!(got.is_safe(), "{:?}", got.reasons);
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[test]
fn reasons_are_deduplicated_and_sorted_worst_first() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let facts = Facts {
        mount_point: Some(true),
        owned_by_me: Some(false),
        writable: Some(false),
        ..Default::default()
    };
    let got = classify(Path::new("/usr/lib"), &env, &facts);
    assert_eq!(got.risk, Risk::Dangerous);
    let risks: Vec<Risk> = got.reasons.iter().map(Reason::risk).collect();
    let mut sorted = risks.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(risks, sorted, "worst reason first: it is the headline");
    let keys: Vec<&str> = got.reasons.iter().map(Reason::key).collect();
    let mut unique = keys.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(keys.len(), unique.len(), "no reason is reported twice");
}

#[test]
fn a_safe_assessment_has_no_reasons_and_a_usable_headline() {
    let safe = Assessment::safe("/home/me/Downloads");
    assert!(safe.is_safe());
    assert!(safe.reasons.is_empty());
    assert!(safe.overridable());

    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let danger = classify(Path::new("/usr"), &env, &Facts::default());
    assert!(danger.headline().contains("/usr"));
    assert!(!danger.headline().is_empty());
    assert!(danger.reasons.iter().all(|r| !r.explain().is_empty()));
}

/// Widening the scope to hidden items widens the blast radius, so a folder that
/// merely warranted a prompt becomes one that needs the override flag.
#[test]
fn include_hidden_escalates_caution_but_not_safe() {
    let caution = Assessment {
        path: PathBuf::from("/opt/data"),
        risk: Risk::Caution,
        reasons: vec![Reason::MountPoint],
    };
    assert_eq!(
        escalate_for_hidden(caution.clone(), true).risk,
        Risk::Dangerous
    );
    assert_eq!(escalate_for_hidden(caution, false).risk, Risk::Caution);

    let safe = Assessment::safe("/home/me/Downloads");
    assert_eq!(
        escalate_for_hidden(safe, true).risk,
        Risk::Safe,
        "a safe folder stays safe; hidden files in your own Downloads are not a hazard"
    );
}

// ---------------------------------------------------------------------------
// The splitter
// ---------------------------------------------------------------------------

#[test]
fn splitting_handles_every_windows_spelling() {
    let w = Platform::Windows;
    let drive = parts(Path::new(r"C:\Windows\System32"), w);
    assert_eq!(drive.prefix.as_deref(), Some("C:"));
    assert_eq!(drive.comps, ["windows", "system32"]);

    assert_eq!(parts(Path::new(r"\\?\C:\Windows"), w), drive_parts());
    assert_eq!(parts(Path::new(r"c:/WINDOWS/"), w).comps, ["windows"]);
    assert_eq!(parts(Path::new(r"C:\"), w).comps, Vec::<String>::new());
    assert!(parts(Path::new(r"C:\"), w).is_root());

    let unc = parts(Path::new(r"\\Server\Share\Data"), w);
    assert_eq!(unc.prefix.as_deref(), Some(r"\\server\share"));
    assert_eq!(unc.comps, ["data"]);
    assert!(!unc.is_root());
    assert!(unc.is_unc());

    let verbatim_unc = parts(Path::new(r"\\?\UNC\Server\Share\Data"), w);
    assert_eq!(
        verbatim_unc, unc,
        "the verbatim spelling must reach the same verdict"
    );

    assert!(parts(Path::new(r"\\Server\Share"), w).is_root());
    assert!(parts(Path::new("relative\\thing"), w).prefix.is_none());
}

fn drive_parts() -> Parts {
    Parts {
        prefix: Some("C:".into()),
        comps: vec!["windows".into()],
    }
}

#[test]
fn splitting_resolves_dot_segments_and_trailing_punctuation() {
    let w = Platform::Windows;
    assert_eq!(parts(Path::new(r"C:\a\.\b"), w).comps, ["a", "b"]);
    assert_eq!(parts(Path::new(r"C:\a\b\..\c"), w).comps, ["a", "c"]);
    // Windows silently strips trailing dots and spaces from names, so a rule must
    // match the name the file system will actually use.
    assert_eq!(parts(Path::new(r"C:\Windows. "), w).comps, ["windows"]);
}

#[test]
fn unix_splitting_is_case_sensitive_on_linux_and_not_on_macos() {
    assert_eq!(
        parts(Path::new("/USR/Lib"), Platform::Linux).comps,
        ["USR", "Lib"]
    );
    assert_eq!(
        parts(Path::new("/USR/Lib"), Platform::MacOs).comps,
        ["usr", "lib"]
    );
    assert!(parts(Path::new("/"), Platform::Linux).is_root());
    assert!(
        parts(Path::new("relative/thing"), Platform::Linux)
            .prefix
            .is_none()
    );
}

/// String prefixes are the classic way to get this wrong.
#[test]
fn matching_is_component_wise_not_textual() {
    let linux = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    assert_eq!(
        classify(Path::new("/usrlocal"), &linux, &Facts::default()).risk,
        Risk::Safe,
        "/usrlocal is not inside /usr"
    );
    let windows = Environment::fake(Platform::Windows).with_home(WIN_HOME);
    assert_eq!(
        classify(
            Path::new(r"C:\Program Files Custom"),
            &windows,
            &Facts::default()
        )
        .risk,
        Risk::Safe,
        "C:\\Program Files Custom is not inside C:\\Program Files"
    );
}

#[test]
fn a_relative_path_is_never_judged_by_the_path_rules() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    assert!(classify(Path::new("usr"), &env, &Facts::default()).is_safe());
    assert!(classify(Path::new("."), &env, &Facts::default()).is_safe());
}

#[test]
fn an_unknown_platform_applies_only_the_universal_rules() {
    let env = Environment::fake(Platform::Other).with_home("/home/me");
    assert_eq!(
        classify(Path::new("/usr"), &env, &Facts::default()).risk,
        Risk::Safe,
        "no rule table"
    );
    assert_eq!(
        classify(Path::new("/"), &env, &Facts::default()).risk,
        Risk::Dangerous,
        "but a filesystem root is a filesystem root anywhere"
    );
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/// A trimmed but realistic `/proc/self/mountinfo`. Note the varying number of
/// optional fields before the `-`, which is the part that trips up naive parsers.
const MOUNTINFO: &str = "\
25 30 0:23 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
26 30 0:24 / /sys rw,nosuid,nodev,noexec,relatime shared:2 - sysfs sysfs rw
30 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
31 30 0:26 / /tmp rw,nosuid,nodev shared:5 - tmpfs tmpfs rw
40 30 8:17 / /mnt/my\\040disk rw,relatime - vfat /dev/sdb1 rw
41 30 0:45 / /home rw,relatime shared:7 master:3 - xfs /dev/sdc1 rw
42 41 0:46 / /home/me/vault rw,relatime - fuse.gocryptfs gocryptfs rw
";

#[test]
fn mountinfo_picks_the_longest_matching_mount() {
    let fs = |p: &str| fs_type_from_mountinfo(MOUNTINFO, Path::new(p));
    assert_eq!(fs("/proc/1/status").as_deref(), Some("proc"));
    assert_eq!(fs("/sys").as_deref(), Some("sysfs"));
    assert_eq!(fs("/tmp/work").as_deref(), Some("tmpfs"));
    assert_eq!(fs("/etc/hosts").as_deref(), Some("ext4"), "falls back to /");
    assert_eq!(
        fs("/home/me/vault/secret").as_deref(),
        Some("fuse.gocryptfs"),
        "the nested mount wins over /home and over /"
    );
    assert_eq!(fs("/home/me/Downloads").as_deref(), Some("xfs"));
}

/// Mount points are octal-escaped, so a disk with a space in its name must still
/// be recognised rather than silently parsed as two fields.
#[test]
fn mountinfo_decodes_escaped_mount_points() {
    assert_eq!(
        fs_type_from_mountinfo(MOUNTINFO, Path::new("/mnt/my disk/photos")).as_deref(),
        Some("vfat")
    );
    assert_eq!(unescape_octal(r"/mnt/my\040disk"), "/mnt/my disk");
    assert_eq!(unescape_octal(r"a\011b\012c\134d"), "a\tb\nc\\d");
    assert_eq!(unescape_octal("/plain/path"), "/plain/path");
    assert_eq!(
        unescape_octal(r"trailing\04"),
        r"trailing\04",
        "a truncated escape is left alone rather than swallowing the text"
    );
}

#[test]
fn mountinfo_survives_nonsense() {
    assert_eq!(fs_type_from_mountinfo("", Path::new("/usr")), None);
    assert_eq!(
        fs_type_from_mountinfo("not a mountinfo line at all", Path::new("/usr")),
        None
    );
    assert_eq!(
        fs_type_from_mountinfo("25 30 0:23 / /proc rw shared:12", Path::new("/proc")),
        None,
        "no separator, so no filesystem type to read"
    );
    assert_eq!(
        fs_type_from_mountinfo("25 30 0:23 / /proc rw -", Path::new("/proc")),
        None,
        "separator but nothing after it"
    );
}

/// The whole point of the pseudo-filesystem rule, end to end through `classify`.
#[test]
fn a_kernel_filesystem_found_through_mountinfo_is_refused() {
    let env = Environment::fake(Platform::Linux).with_home(NIX_HOME);
    let facts = Facts {
        fs_type: fs_type_from_mountinfo(MOUNTINFO, Path::new("/proc/1")),
        ..Default::default()
    };
    let got = classify(Path::new("/proc/1"), &env, &facts);
    assert_eq!(got.risk, Risk::Dangerous);
    assert!(
        got.reasons
            .iter()
            .any(|r| matches!(r, Reason::PseudoFilesystem { fs_type } if fs_type == "proc"))
    );
}

#[test]
fn the_write_probe_says_yes_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(probe_writable(dir.path()), Some(true));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "the probe must clean up after itself, found {leftovers:?}"
    );
}

#[test]
fn the_write_probe_is_unknown_rather_than_false_for_a_missing_folder() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("not-there");
    assert_eq!(
        probe_writable(&missing),
        None,
        "a folder that is not there is unknown, not unwritable: an unknown must never accuse"
    );
}

/// Only meaningful when not running as root, which ignores mode bits entirely.
#[cfg(unix)]
#[test]
fn the_write_probe_detects_a_folder_it_cannot_write_to() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

    if probe_writable(&locked) == Some(true) {
        // Running as root: mode bits do not apply, so there is nothing to assert.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    assert_eq!(probe_writable(&locked), Some(false));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// A read-only probe must not create anything, because `analyze` and `--dry-run`
/// promise to leave the disk alone.
#[test]
fn a_read_only_probe_never_touches_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let facts = SystemProbe::read_only().facts(dir.path());
    assert_eq!(facts.writable, None);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

    let writing = SystemProbe::writing().facts(dir.path());
    assert_eq!(writing.writable, Some(true));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "and the writing probe cleans up too"
    );
}

#[cfg(unix)]
#[test]
fn an_ordinary_subfolder_is_not_a_mount_point() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    assert_eq!(
        SystemProbe::read_only().facts(&sub).mount_point,
        Some(false)
    );
}

#[cfg(windows)]
#[test]
fn an_ordinary_folder_has_neither_windows_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let facts = SystemProbe::read_only().facts(dir.path());
    assert_eq!(facts.system_attribute, Some(false));
    assert_eq!(facts.reparse_point, Some(false));
}

/// The real Windows folder, judged through the real probe and the real
/// environment. The single most important thing this feature must get right.
#[cfg(windows)]
#[test]
fn the_real_windows_folder_is_refused() {
    let windows = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    if !windows.is_dir() {
        return;
    }
    let got = assess(&windows);
    assert_eq!(got.risk, Risk::Dangerous, "{:?}", got.reasons);
}

/// The system temporary folder must stay usable, or every `tempfile` test in this
/// crate would start tripping the guard.
#[test]
fn a_folder_inside_the_system_temp_directory_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let got = assess(dir.path());
    assert!(
        got.is_safe(),
        "{} was flagged {:?}",
        dir.path().display(),
        got.reasons
    );
}

#[test]
fn static_facts_answer_for_the_paths_they_know() {
    let table = StaticFacts::new().with(
        "/data",
        Facts {
            writable: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(table.facts(Path::new("/data")).writable, Some(false));
    assert_eq!(table.facts(Path::new("/elsewhere")), Facts::default());
}

/// `C:\Windows` trips the %SystemRoot% rule, the %windir% rule and the name
/// rule. The person reading the warning should be told once.
#[test]
fn a_folder_that_trips_several_rules_is_explained_once() {
    let env = Environment::fake(Platform::Windows)
        .with_home(WIN_HOME)
        .with_var("SystemRoot", r"C:\Windows")
        .with_var("windir", r"C:\Windows");
    let got = classify(Path::new(r"C:\Windows"), &env, &Facts::default());
    assert_eq!(got.risk, Risk::Dangerous);
    let lines: Vec<String> = got.reasons.iter().map(Reason::explain).collect();
    let mut unique = lines.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        lines.len(),
        unique.len(),
        "the same sentence must not be printed twice: {lines:#?}"
    );
    assert_eq!(lines.len(), 1, "one folder, one explanation: {lines:#?}");
}

/// On macOS `/var`, `/etc` and `/tmp` are symlinks into `/private`, so the same
/// directory has two names. A rule or a carve-out written for one spelling has
/// to match the other, or a verdict depends on how the caller typed the path.
#[test]
fn macos_private_aliases_fold_onto_their_short_names() {
    let m = Platform::MacOs;
    assert_eq!(parts(Path::new("/private/var/db"), m).comps, ["var", "db"]);
    assert_eq!(parts(Path::new("/private/etc"), m).comps, ["etc"]);
    assert_eq!(parts(Path::new("/private/tmp/x"), m).comps, ["tmp", "x"]);
    // Only those three are aliases; anything else under /private stays put.
    assert_eq!(
        parts(Path::new("/private/other"), m).comps,
        ["private", "other"]
    );
    assert_eq!(
        parts(Path::new("/private/var/db"), m),
        parts(Path::new("/var/db"), m),
        "both spellings must reach the same verdict"
    );
}

/// The failure this fixes: `tempfile` hands back `/var/folders/...` while
/// `std::env::temp_dir()` canonicalizes to `/private/var/folders/...`, so the
/// carve-out missed and every temporary directory on macOS was refused.
#[test]
fn a_macos_temp_directory_is_safe_under_either_spelling() {
    let canonical = "/private/var/folders/36/abc/T";
    let env = Environment::fake(Platform::MacOs)
        .with_home(MAC_HOME)
        .with_temp(canonical);
    for path in [
        "/private/var/folders/36/abc/T/.tmpVI2dTP",
        "/var/folders/36/abc/T/.tmpVI2dTP",
    ] {
        let got = classify(Path::new(path), &env, &Facts::default());
        assert!(got.is_safe(), "{path} was flagged {:?}", got.reasons);
    }
}

/// Windows can report the temporary directory with an 8.3 short name while the
/// canonical form is long. Recording both spellings is what keeps the carve-out
/// working for a caller who has not canonicalized the path.
#[test]
fn several_spellings_of_the_temp_directory_are_all_carved_out() {
    let env = Environment::fake(Platform::Windows)
        .with_home(WIN_HOME)
        .with_temp(r"C:\Users\runneradmin\AppData\Local\Temp")
        .with_temp(r"C:\Users\RUNNER~1\AppData\Local\Temp");
    for path in [
        r"C:\Users\runneradmin\AppData\Local\Temp\tidy-abc",
        r"C:\Users\RUNNER~1\AppData\Local\Temp\tidy-abc",
    ] {
        let got = classify(Path::new(path), &env, &Facts::default());
        assert!(got.is_safe(), "{path} was flagged {:?}", got.reasons);
    }
}

/// The guard must not write into a folder it has already decided to refuse.
/// The writability probe creates a file, so it runs only after the path rules
/// have failed to produce a refusal.
#[cfg(unix)]
#[test]
fn a_folder_refused_by_name_is_never_probed_for_writability() {
    // /tmp is carved out, so build a fixture the path rules refuse on sight.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();

    // Judged as a home root, which is refused by name alone.
    let env = Environment::fake(Platform::Linux).with_home(&home);
    let facts = SystemProbe::read_only().facts(&home);
    let verdict = classify(&home, &env, &facts);
    assert_eq!(verdict.risk, Risk::Dangerous);
    assert!(
        std::fs::read_dir(&home).unwrap().next().is_none(),
        "reaching a refusal must not have created anything"
    );
}

/// The probe still runs when the path rules found nothing, because that is the
/// case where being told up front is worth a file created and removed.
#[test]
fn an_ordinary_folder_is_still_checked_for_writability() {
    let dir = tempfile::tempdir().unwrap();
    let got = assess_for_writing(dir.path());
    assert!(got.is_safe(), "{:?}", got.reasons);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "and the probe cleans up after itself"
    );
}
