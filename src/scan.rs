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
pub fn scan(root: &Path, opts: &ScanOptions) -> Result<ScanResult> {
    let mut out = ScanResult::default();
    walk(root, 1, opts, &mut out)?;
    Ok(out)
}

fn walk(dir: &Path, depth: usize, opts: &ScanOptions, out: &mut ScanResult) -> Result<()> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(_) if depth > 1 => {
            out.skipped.push(Skipped {
                path: dir.to_path_buf(),
                reason: SkipReason::Unreadable,
            });
            return Ok(());
        }
        Err(source) => {
            return Err(Error::Io {
                path: dir.to_path_buf(),
                source,
            });
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
        let Ok(meta) = fs::symlink_metadata(&path) else {
            skip(SkipReason::Unreadable);
            continue;
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
        if opts.rules.skips_hidden() && is_hidden(&name, &meta) {
            skip(SkipReason::Hidden);
            continue;
        }

        if is_dir {
            if is_project_dir(&path) {
                out.projects.push(path);
            } else if depth < opts.max_depth {
                walk(&path, depth + 1, opts, out)?;
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
    Ok(())
}

/// Dot-files everywhere, plus the HIDDEN/SYSTEM attributes on Windows.
pub(crate) fn is_hidden(name: &str, meta: &Metadata) -> bool {
    name.starts_with('.') || has_hidden_attribute(meta)
}

#[cfg(windows)]
fn has_hidden_attribute(meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    meta.file_attributes() & HIDDEN_OR_SYSTEM != 0
}

#[cfg(not(windows))]
fn has_hidden_attribute(_meta: &Metadata) -> bool {
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
        let hidden = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(names(&hidden), ["plain.txt"]);
        assert_eq!(
            hidden
                .skipped
                .iter()
                .filter(|s| s.reason == SkipReason::Hidden)
                .count(),
            2
        );

        let mut options = opts(1);
        options.rules = IgnoreRules::new().include_hidden(true);
        assert_eq!(scan(dir.path(), &options).unwrap().files.len(), 3);
    }

    #[cfg(unix)]
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

    #[test]
    fn reports_file_sizes() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "abc.txt");
        let result = scan(dir.path(), &opts(1)).unwrap();
        assert_eq!(result.files[0].size, 7);
    }
}
