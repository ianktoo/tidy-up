//! Directory scanning: turns a folder into files to process plus a record of what was skipped.

use std::{
    collections::BTreeSet,
    fmt, fs,
    fs::Metadata,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    error::{Error, Result},
    journal::TOOL_DIR,
    projects::is_project_dir,
    rules::IgnoreRules,
};

/// Why an entry was left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Matched a user rule (extension or glob); carries the rule text.
    Ignored(String),
    /// OS bookkeeping file such as `desktop.ini`.
    SystemFile,
    /// A desktop shortcut.
    Shortcut,
    /// A hidden file or folder.
    Hidden,
    /// A code/git project directory.
    Project,
    /// A folder beyond the configured scan depth.
    Folder,
    /// A symbolic link (never followed or moved).
    Symlink,
    /// The tool's own `.tidy-up` state folder.
    ToolFolder,
    /// A top-level folder created by a previous run (category, Projects, _Duplicates).
    AlreadyOrganized,
    /// A name that is not valid UTF-8 and cannot be journaled safely.
    UnsupportedName,
    /// Metadata or directory contents could not be read.
    Unreadable,
    /// The operating system refused to let this user read it.
    ///
    /// Separate from [`SkipReason::Unreadable`] because it is the one a user can
    /// usually do something about, and saying so is the difference between
    /// "tidy-up is broken" and "you do not own that folder".
    Denied,
    /// Marked by the operating system as one of its own (the Windows SYSTEM
    /// attribute).
    ///
    /// Never lifted by `--include-hidden`: a file Windows owns is not merely hidden.
    SystemAttribute,
}

impl SkipReason {
    /// Short plural label used when summarising skipped entries.
    pub fn label(&self) -> &'static str {
        match self {
            SkipReason::Ignored(_) => "matched your ignore rules",
            SkipReason::SystemFile => "system files",
            SkipReason::Shortcut => "shortcuts",
            SkipReason::Hidden => "hidden items",
            SkipReason::Project => "code projects",
            SkipReason::Folder => "folders beyond scan depth",
            SkipReason::Symlink => "symbolic links",
            SkipReason::ToolFolder => "tidy-up state folders",
            SkipReason::AlreadyOrganized => "already-organized folders",
            SkipReason::UnsupportedName => "non-UTF-8 names",
            SkipReason::Unreadable => "unreadable items",
            SkipReason::Denied => "items you do not have permission to read",
            SkipReason::SystemAttribute => "items the operating system marks as its own",
        }
    }

    /// Stable machine token for logs and counters.
    ///
    /// [`label`](Self::label) is prose meant for people and must never be used as
    /// a key: rewording it would silently break every log already written.
    pub fn key(&self) -> &'static str {
        match self {
            SkipReason::Ignored(_) => "ignored",
            SkipReason::SystemFile => "system_file",
            SkipReason::Shortcut => "shortcut",
            SkipReason::Hidden => "hidden",
            SkipReason::Project => "project",
            SkipReason::Folder => "folder",
            SkipReason::Symlink => "symlink",
            SkipReason::ToolFolder => "tool_folder",
            SkipReason::AlreadyOrganized => "already_organized",
            SkipReason::UnsupportedName => "unsupported_name",
            SkipReason::Unreadable => "unreadable",
            SkipReason::Denied => "denied",
            SkipReason::SystemAttribute => "system_attribute",
        }
    }

    /// Classifies a failed read: a refusal is worth saying out loud, anything
    /// else is merely unreadable.
    pub fn from_io(error: &std::io::Error) -> SkipReason {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => SkipReason::Denied,
            _ => SkipReason::Unreadable,
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::Ignored(rule) => write!(f, "ignored by rule `{rule}`"),
            other => f.write_str(other.label()),
        }
    }
}

/// A regular file found by the scan.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Absolute path.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time, when the platform reports it.
    pub modified: Option<SystemTime>,
    /// Depth below the scan root (1 = direct child).
    pub depth: usize,
    /// Index of the scanned folder this file came from (0 unless several folders are
    /// compared). Lower indexes win when choosing which duplicate to keep.
    pub source: usize,
}

/// An entry that was deliberately not processed.
#[derive(Debug, Clone)]
pub struct Skipped {
    /// Absolute path.
    pub path: PathBuf,
    /// Reason it was skipped.
    pub reason: SkipReason,
}

/// Everything a scan discovered.
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Files eligible for processing, in deterministic (sorted) order.
    pub files: Vec<FileEntry>,
    /// Detected project directories (not descended into).
    pub projects: Vec<PathBuf>,
    /// Entries that were skipped, with reasons.
    pub skipped: Vec<Skipped>,
}

/// Knobs controlling a scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Maximum depth to descend; `1` scans only direct children.
    pub max_depth: usize,
    /// Ignore rules to apply.
    pub rules: IgnoreRules,
    /// Lower-cased names of top-level folders to skip entirely.
    pub skip_root_dirs: BTreeSet<String>,
}

/// Scans `root` according to `opts`.
///
/// Only the root can fail the scan. Everything below it survives: a folder that
/// cannot be listed, an entry whose metadata is refused, a name that is not valid
/// UTF-8, each becomes a [`Skipped`] entry and the walk carries on. A scan of a
/// large tree with one bad corner still returns every other file.
pub fn scan(root: &Path, opts: &ScanOptions) -> Result<ScanResult> {
    if let Err(source) = fs::read_dir(root) {
        return Err(match source.kind() {
            std::io::ErrorKind::PermissionDenied => Error::Denied {
                path: root.to_path_buf(),
            },
            _ => Error::Io {
                path: root.to_path_buf(),
                source,
            },
        });
    }
    let mut out = ScanResult::default();
    walk(root, 1, opts, &mut out);
    Ok(out)
}

fn walk(dir: &Path, depth: usize, opts: &ScanOptions, out: &mut ScanResult) {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(source) => {
            out.skipped.push(Skipped {
                path: dir.to_path_buf(),
                reason: SkipReason::from_io(&source),
            });
            return;
        }
    };
    let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let mut skip = |reason| {
            out.skipped.push(Skipped {
                path: path.clone(),
                reason,
            })
        };

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            skip(SkipReason::UnsupportedName);
            continue;
        };
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(source) => {
                skip(SkipReason::from_io(&source));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            skip(SkipReason::Symlink);
            continue;
        }
        let is_dir = meta.is_dir();

        if is_dir && depth == 1 {
            if name == TOOL_DIR {
                skip(SkipReason::ToolFolder);
                continue;
            }
            if opts.skip_root_dirs.contains(&name.to_lowercase()) {
                skip(SkipReason::AlreadyOrganized);
                continue;
            }
        }
        if let Some(reason) = opts.rules.check(&name, is_dir) {
            skip(reason);
            continue;
        }
        // Not governed by `--include-hidden`: asking to see dot-files is not
        // asking to move the operating system.
        if has_system_attribute(&meta) {
            skip(SkipReason::SystemAttribute);
            continue;
        }
        if opts.rules.skips_hidden() && is_hidden(&name, &meta) {
            skip(SkipReason::Hidden);
            continue;
        }

        if is_dir {
            if is_project_dir(&path) {
                out.projects.push(path);
            } else if depth < opts.max_depth {
                walk(&path, depth + 1, opts, out);
            } else {
                skip(SkipReason::Folder);
            }
        } else if meta.is_file() {
            out.files.push(FileEntry {
                path,
                size: meta.len(),
                modified: meta.modified().ok(),
                depth,
                source: 0,
            });
        }
    }
}

/// Dot-files everywhere, plus the HIDDEN attribute on Windows and the `UF_HIDDEN`
/// flag on macOS.
pub(crate) fn is_hidden(name: &str, meta: &Metadata) -> bool {
    name.starts_with('.') || has_hidden_attribute(meta)
}

#[cfg(windows)]
fn has_hidden_attribute(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

/// `chflags hidden` sets a flag no dot-file check would ever notice.
#[cfg(target_os = "macos")]
fn has_hidden_attribute(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x8000;
    meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
fn has_hidden_attribute(_meta: &Metadata) -> bool {
    false
}

/// Whether Windows marks this as one of its own files.
///
/// Kept apart from [`is_hidden`] deliberately. The two attributes used to be
/// tested together, which meant `--include-hidden` also un-skipped everything
/// Windows owns; asking to see dot-files is not asking to move `pagefile.sys`.
#[cfg(windows)]
pub(crate) fn has_system_attribute(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    meta.file_attributes() & FILE_ATTRIBUTE_SYSTEM != 0
}

#[cfg(not(windows))]
pub(crate) fn has_system_attribute(_meta: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, rel).unwrap();
    }

    fn opts(depth: usize) -> ScanOptions {
        ScanOptions {
            max_depth: depth,
            rules: IgnoreRules::new(),
            skip_root_dirs: ["images".to_string()].into(),
        }
    }

    fn names(result: &ScanResult) -> Vec<String> {
        result
            .files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn depth_one_only_sees_direct_children() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.txt");
        touch(dir.path(), "sub/b.txt");
        let result = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(names(&result), ["a.txt"]);
        assert!(
            result
                .skipped
                .iter()
                .any(|s| s.reason == SkipReason::Folder)
        );
    }

    #[test]
    fn deeper_scans_recurse_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "b.txt");
        touch(dir.path(), "a.txt");
        touch(dir.path(), "sub/c.txt");
        let result = scan(dir.path(), &opts(usize::MAX)).unwrap();
        assert_eq!(names(&result), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(result.files[2].depth, 2);
    }

    #[test]
    fn skips_shortcuts_hidden_and_organized_folders() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "keep.txt");
        touch(dir.path(), "App.lnk");
        touch(dir.path(), ".secret");
        touch(dir.path(), "Images/old.png");
        touch(dir.path(), ".tidy-up/journals/x.jsonl");
        let result = scan(dir.path(), &opts(usize::MAX)).unwrap();
        assert_eq!(names(&result), ["keep.txt"]);
        let reasons: Vec<_> = result.skipped.iter().map(|s| s.reason.clone()).collect();
        assert!(reasons.contains(&SkipReason::Shortcut));
        assert!(reasons.contains(&SkipReason::Hidden));
        assert!(reasons.contains(&SkipReason::AlreadyOrganized));
        assert!(reasons.contains(&SkipReason::ToolFolder));
    }

    #[test]
    fn projects_are_reported_not_descended() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "repo/Cargo.toml");
        touch(dir.path(), "repo/src/main.rs");
        touch(dir.path(), "loose.txt");
        let result = scan(dir.path(), &opts(usize::MAX)).unwrap();
        assert_eq!(names(&result), ["loose.txt"]);
        assert_eq!(result.projects, [dir.path().join("repo")]);
    }

    #[test]
    fn user_rules_apply() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "a.iso");
        touch(dir.path(), "b.txt");
        let mut options = opts(1);
        options.rules.add_extension("iso");
        let result = scan(dir.path(), &options).unwrap();
        assert_eq!(names(&result), ["b.txt"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_hidden_and_system_attributes_are_honoured() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "plain.txt");
        touch(dir.path(), "attr-hidden.txt");
        touch(dir.path(), "attr-system.txt");
        for (file, flag) in [("attr-hidden.txt", "+h"), ("attr-system.txt", "+s")] {
            let status = std::process::Command::new("attrib")
                .arg(flag)
                .arg(dir.path().join(file))
                .status()
                .expect("attrib is part of Windows");
            assert!(status.success());
        }
        let scanned = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(names(&scanned), ["plain.txt"]);
        let reasons =
            |want: SkipReason| scanned.skipped.iter().filter(|s| s.reason == want).count();
        assert_eq!(reasons(SkipReason::Hidden), 1, "only the HIDDEN one");
        assert_eq!(
            reasons(SkipReason::SystemAttribute),
            1,
            "the SYSTEM one is reported as what it is, not as merely hidden"
        );

        // Asking for hidden files is not asking for the operating system: the
        // HIDDEN file comes back, the SYSTEM file stays put.
        let mut options = opts(1);
        options.rules = IgnoreRules::new().include_hidden(true);
        let with_hidden = scan(dir.path(), &options).unwrap();
        assert_eq!(names(&with_hidden), ["attr-hidden.txt", "plain.txt"]);
        assert!(
            with_hidden
                .skipped
                .iter()
                .any(|s| s.reason == SkipReason::SystemAttribute)
        );
    }

    /// Linux and friends allow any bytes in a file name. macOS (APFS and HFS+) rejects names that
    /// are not valid UTF-8 outright, so this situation cannot arise there and the test cannot run.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn names_that_are_not_valid_utf8_are_skipped_not_mangled() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "fine.txt");
        let odd = dir.path().join(OsStr::from_bytes(b"caf\xe9.txt"));
        fs::write(&odd, "latin-1 name").unwrap();
        let result = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(names(&result), ["fine.txt"]);
        assert!(
            result
                .skipped
                .iter()
                .any(|s| s.reason == SkipReason::UnsupportedName)
        );
        assert!(odd.exists(), "the file itself is untouched");
    }

    #[cfg(unix)]
    #[test]
    fn unix_dotfiles_and_dot_directories_count_as_hidden() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), ".bashrc");
        touch(dir.path(), ".config/app/settings.toml");
        touch(dir.path(), "visible.txt");
        let result = scan(dir.path(), &opts(usize::MAX)).unwrap();
        assert_eq!(names(&result), ["visible.txt"]);
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(scan(Path::new("/no/such/dir/anywhere"), &opts(1)).is_err());
    }

    /// One bad corner of a tree must cost that corner and nothing else.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subfolder_is_reported_and_the_rest_still_scans() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "keep.txt");
        touch(dir.path(), "readable/fine.txt");
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden-away.txt"), "x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let result = scan(dir.path(), &opts(usize::MAX)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            names(&result).contains(&"keep.txt".to_string()),
            "the rest of the tree is still returned"
        );
        assert!(names(&result).contains(&"fine.txt".to_string()));
        if names(&result).contains(&"hidden-away.txt".to_string()) {
            return; // running as root: nothing is unreadable
        }
        assert!(
            result
                .skipped
                .iter()
                .any(|s| s.reason == SkipReason::Denied && s.path == locked),
            "and the folder that failed is named, with the reason: {:?}",
            result.skipped
        );
    }

    /// The root is the one place a failure is worth stopping for, and a refusal
    /// there must say so rather than printing an error number.
    #[cfg(unix)]
    #[test]
    fn a_root_this_user_cannot_read_is_a_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let result = scan(&locked, &opts(1));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

        match result {
            Err(Error::Denied { path }) => assert_eq!(path, locked),
            Ok(_) => {} // running as root
            Err(other) => panic!("expected a permission error, got {other}"),
        }
    }

    #[test]
    fn every_skip_reason_has_a_unique_stable_key() {
        let all = [
            SkipReason::Ignored("x".into()),
            SkipReason::SystemFile,
            SkipReason::Shortcut,
            SkipReason::Hidden,
            SkipReason::Project,
            SkipReason::Folder,
            SkipReason::Symlink,
            SkipReason::ToolFolder,
            SkipReason::AlreadyOrganized,
            SkipReason::UnsupportedName,
            SkipReason::Unreadable,
            SkipReason::Denied,
            SkipReason::SystemAttribute,
        ];
        let mut keys: Vec<_> = all.iter().map(SkipReason::key).collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "keys go into logs and must not collide");
        assert!(all.iter().all(|r| !r.label().is_empty()));
    }

    #[test]
    fn failed_reads_are_classified_by_whether_the_user_could_fix_them() {
        use std::io::ErrorKind;
        assert_eq!(
            SkipReason::from_io(&ErrorKind::PermissionDenied.into()),
            SkipReason::Denied
        );
        assert_eq!(
            SkipReason::from_io(&ErrorKind::InvalidData.into()),
            SkipReason::Unreadable
        );
    }

    #[test]
    fn reports_file_sizes() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "abc.txt");
        let result = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(result.files[0].size, 7);
    }
}
