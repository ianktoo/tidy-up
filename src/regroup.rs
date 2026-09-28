//! Re-filing an existing tree into a fresh hierarchy.
//!
//! [`crate::plan::build_organize_plan`] sorts loose files into category folders and
//! deliberately refuses to touch its own output. `reorganize` is the opposite
//! operation: it descends into everything, including folders a previous run made,
//! and re-files every file under an ordered list of [`GroupBy`] keys. Folders it
//! empties are deleted.
//!
//! Each key becomes one level of nesting, so `--by year,month,type` produces
//! `2024/03-March/Images/`.
//!
//! Two properties make this safe to point at a folder you care about:
//!
//! * **Every key is total.** A file with no extension, no timestamp or an
//!   unpronounceable name still gets a bucket. Nothing is silently dropped.
//! * **It is idempotent.** A file already in the right place produces no move, so
//!   running the same command twice is a no-op. This is what `organize` gets from
//!   its skip list and what `reorganize`, which has no skip list, has to compute.
//!
//! ```
//! use std::path::Path;
//! use tidy_up::{regroup::{build_reorganize_plan, GroupBy}, rules::IgnoreRules,
//!               plan::ProjectPolicy, scan::{scan, ScanOptions}};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = Path::new("C:/Users/me/OldArchive");
//! let options = ScanOptions {
//!     max_depth: usize::MAX,
//!     rules: IgnoreRules::new(),
//!     skip_root_dirs: Default::default(),  // descend into everything
//! };
//! # if root.exists() {
//! let scanned = scan(root, &options)?;
//! let plan = build_reorganize_plan(root, &scanned, &[GroupBy::Year, GroupBy::Type],
//!                                  ProjectPolicy::Keep, 0);
//! println!("{} moves, {} folders emptied", plan.plan.moves.len(), plan.emptied.len());
//! # }
//! # Ok(()) }
//! ```

use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{
    category::Category,
    fsops::unique_path,
    journal::TOOL_DIR,
    plan::{MoveKind, Plan, PlannedMove, ProjectPolicy},
    scan::{FileEntry, ScanResult, SkipReason, Skipped},
    timefmt::{MONTHS, civil_date},
};

/// Bucket used when a file has no usable timestamp.
const UNKNOWN_DATE: &str = "Unknown date";
/// Bucket used when a name or extension yields nothing usable.
const OTHER: &str = "Other";

/// One level of the folder hierarchy a reorganize run builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum GroupBy {
    /// Broad file type: `Images`, `Documents`, `3D Models`.
    ///
    /// The same categories `organize` uses, so the two commands agree.
    Type,
    /// The file extension, upper-cased: `PDF`, `JPG`, `No extension`.
    ///
    /// Finer than [`GroupBy::Type`]: this is the one to use for sorting a pile of
    /// documents by what kind of document they are.
    Ext,
    /// Four-digit year of the last modification.
    Year,
    /// Month of the last modification, as `03-March`, which sorts correctly and
    /// still reads as a month.
    Month,
    /// Exact day of the last modification, as `2024-03-17`.
    Day,
    /// Size band: `Tiny`, `Small`, `Medium`, `Large`, `Huge`.
    Size,
    /// First character of the name: `A` to `Z`, `0-9`, or `Other`.
    Alpha,
}

/// Size bands, as `(exclusive upper bound in bytes, folder name)`.
const SIZE_BANDS: &[(u64, &str)] = &[
    (1 << 20, "Tiny (under 1 MiB)"),
    (10 << 20, "Small (under 10 MiB)"),
    (100 << 20, "Medium (under 100 MiB)"),
    (1 << 30, "Large (under 1 GiB)"),
];
/// Band for anything above the last bound.
const HUGE: &str = "Huge (1 GiB and up)";

impl GroupBy {
    /// The folder name this key assigns to `file`.
    ///
    /// Always non-empty, and always a name every supported file system accepts.
    /// `offset` shifts timestamps for the date keys; see [`crate::timefmt`].
    pub fn bucket(self, file: &FileEntry, offset: i64) -> String {
        let name = match self {
            GroupBy::Type => Category::from_path(&file.path).folder_name().to_string(),
            GroupBy::Ext => match file.path.extension().and_then(|e| e.to_str()) {
                Some(ext) if !ext.is_empty() => ext.to_uppercase(),
                _ => "No extension".to_string(),
            },
            GroupBy::Year => match self.date(file, offset) {
                Some((year, _, _)) => format!("{year:04}"),
                None => UNKNOWN_DATE.to_string(),
            },
            GroupBy::Month => match self.date(file, offset) {
                Some((_, month, _)) => {
                    format!("{month:02}-{}", MONTHS[month.clamp(1, 12) as usize])
                }
                None => UNKNOWN_DATE.to_string(),
            },
            GroupBy::Day => match self.date(file, offset) {
                Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
                None => UNKNOWN_DATE.to_string(),
            },
            GroupBy::Size => SIZE_BANDS
                .iter()
                .find(|(limit, _)| file.size < *limit)
                .map_or(HUGE, |(_, label)| label)
                .to_string(),
            GroupBy::Alpha => initial(&file.path),
        };
        sanitize(&name)
    }

    fn date(self, file: &FileEntry, offset: i64) -> Option<(i64, u32, u32)> {
        let secs = file.modified?.duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(civil_date(secs, offset))
    }

    /// The name as it appears in `--by`, for messages.
    pub fn key(self) -> &'static str {
        match self {
            GroupBy::Type => "type",
            GroupBy::Ext => "ext",
            GroupBy::Year => "year",
            GroupBy::Month => "month",
            GroupBy::Day => "day",
            GroupBy::Size => "size",
            GroupBy::Alpha => "alpha",
        }
    }
}

/// The bucket a name starts with: a letter, `0-9` for a digit, else `Other`.
fn initial(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.chars().next())
        .map_or_else(
            || OTHER.to_string(),
            |c| {
                if c.is_ascii_alphabetic() {
                    c.to_ascii_uppercase().to_string()
                } else if c.is_ascii_digit() {
                    "0-9".to_string()
                } else if c.is_alphabetic() {
                    // Keep non-ASCII letters as themselves rather than lumping
                    // every accented or non-Latin name into one bucket.
                    c.to_uppercase().to_string()
                } else {
                    OTHER.to_string()
                }
            },
        )
}

/// Names Windows reserves for devices, which cannot be used for a folder at all.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes `name` safe to use as a folder on every supported platform.
///
/// A bucket is derived from file contents (an extension, an initial), so it can
/// contain anything. Windows rejects several characters outright, silently strips
/// trailing dots and spaces, and reserves a handful of device names.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).trim();
    if trimmed.is_empty() {
        return OTHER.to_string();
    }
    if RESERVED
        .iter()
        .any(|reserved| trimmed.eq_ignore_ascii_case(reserved))
    {
        return format!("{trimmed}_");
    }
    trimmed.to_string()
}

/// A [`Plan`] plus the folders the run will leave behind empty.
#[derive(Debug, Clone)]
pub struct ReorganizePlan {
    /// The moves, and everything that was skipped.
    pub plan: Plan,
    /// Folders that will hold nothing afterwards, deepest first so they can be
    /// removed in order. Never includes the root or the tool's own state folder.
    pub emptied: Vec<PathBuf>,
}

impl ReorganizePlan {
    /// `true` when there is nothing to move and nothing to clean up.
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty() && self.emptied.is_empty()
    }
}

/// Builds the plan that flattens `root` and re-files everything under `keys`.
///
/// Pure: nothing is touched on disk beyond the existence checks
/// [`unique_path`] needs to avoid collisions.
pub fn build_reorganize_plan(
    root: &Path,
    scan: &ScanResult,
    keys: &[GroupBy],
    projects: ProjectPolicy,
    offset: i64,
) -> ReorganizePlan {
    let mut reserved = HashSet::new();
    let mut moves = Vec::with_capacity(scan.files.len());
    let mut skipped = scan.skipped.clone();
    // Every folder that currently holds something we are moving out of.
    let mut vacated: BTreeSet<PathBuf> = BTreeSet::new();
    // Folders that keep at least one thing, so must not be removed.
    let mut retained: BTreeSet<PathBuf> = BTreeSet::new();

    for file in &scan.files {
        let Some(name) = file.path.file_name() else {
            continue;
        };
        let mut dest = root.to_path_buf();
        for key in keys {
            dest.push(key.bucket(file, offset));
        }
        let dest = dest.join(name);

        // Already exactly where this run would put it. Recording a move would
        // make `reorganize` non-idempotent and would churn the journal.
        if dest == file.path {
            retain_ancestors(root, &file.path, &mut retained);
            continue;
        }
        let dest = unique_path(&dest, &reserved);
        reserved.insert(dest.clone());
        if let Some(parent) = file.path.parent()
            && parent != root
        {
            vacated.insert(parent.to_path_buf());
        }
        moves.push(PlannedMove {
            from: file.path.clone(),
            to: dest,
            kind: MoveKind::File,
            size: file.size,
        });
    }

    for project in &scan.projects {
        match projects {
            // A project is kept whole and left where it is, so its folder and
            // every folder above it must survive the cleanup.
            ProjectPolicy::Keep => {
                retain_ancestors(root, project, &mut retained);
                skipped.push(Skipped {
                    path: project.clone(),
                    reason: SkipReason::Project,
                });
            }
            ProjectPolicy::Move => {
                let Some(name) = project.file_name() else {
                    continue;
                };
                let dest = unique_path(
                    &root.join(crate::category::PROJECTS_DIR).join(name),
                    &reserved,
                );
                reserved.insert(dest.clone());
                if let Some(parent) = project.parent()
                    && parent != root
                {
                    vacated.insert(parent.to_path_buf());
                }
                moves.push(PlannedMove {
                    from: project.clone(),
                    to: dest,
                    kind: MoveKind::Dir,
                    size: 0,
                });
            }
        }
    }

    // Anything skipped stays where it is, so its folder has to stay too.
    for entry in &skipped {
        if entry.reason != SkipReason::Folder {
            retain_ancestors(root, &entry.path, &mut retained);
        }
    }

    let emptied = emptied_folders(root, &vacated, &retained);
    ReorganizePlan {
        plan: Plan {
            root: root.to_path_buf(),
            moves,
            skipped,
        },
        emptied,
    }
}

/// Marks every folder between `root` and `path` as one that must survive.
fn retain_ancestors(root: &Path, path: &Path, retained: &mut BTreeSet<PathBuf>) {
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == root || !dir.starts_with(root) {
            break;
        }
        retained.insert(dir.to_path_buf());
        current = dir.parent();
    }
}

/// The folders that will be empty afterwards, deepest first.
///
/// A folder is a candidate when everything in it is moving out. Its parents
/// become candidates too, since a folder holding only empty folders ends up empty
/// itself. Anything holding something that stays is excluded, along with all of
/// its ancestors.
fn emptied_folders(
    root: &Path,
    vacated: &BTreeSet<PathBuf>,
    retained: &BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in vacated {
        let mut current = Some(dir.as_path());
        while let Some(dir) = current {
            if dir == root || !dir.starts_with(root) {
                break;
            }
            candidates.insert(dir.to_path_buf());
            current = dir.parent();
        }
    }
    let tool_dir = root.join(TOOL_DIR);
    let mut out: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|dir| !retained.contains(dir))
        .filter(|dir| *dir != tool_dir && !dir.starts_with(&tool_dir))
        .collect();
    // Deepest first, so a child is always removed before its parent.
    out.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    out
}

/// Describes the grouping in words, for a confirmation prompt.
///
/// `["year", "type"]` becomes `year, then type`.
pub fn describe(keys: &[GroupBy]) -> String {
    let names: Vec<&str> = keys.iter().map(|k| k.key()).collect();
    match names.len() {
        0 => "nothing".to_string(),
        1 => names[0].to_string(),
        _ => names.join(", then "),
    }
}

#[cfg(test)]
#[path = "regroup_tests.rs"]
mod tests;
