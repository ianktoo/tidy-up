//! Turning scan results into an explicit, reviewable list of moves.
//!
//! A [`Plan`] is pure data: building one touches nothing on disk (apart from
//! existence checks to pick collision-free names), which is what makes
//! `--dry-run` and confirmation prompts trivial.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    category::{Category, PROJECTS_DIR},
    fsops::unique_path,
    scan::{ScanResult, SkipReason, Skipped},
};

/// Name of the folder that receives duplicate files.
pub const DUPLICATES_DIR: &str = "_Duplicates";

/// Whether a move relocates a file or a whole directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveKind {
    /// A single file.
    File,
    /// A directory (used for projects).
    Dir,
}

/// What to do with detected code/git project folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ProjectPolicy {
    /// Leave projects exactly where they are (safe default; moving can break tooling).
    #[default]
    Keep,
    /// Gather projects into a `Projects` folder.
    Move,
}

/// One planned relocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMove {
    /// Current absolute location.
    pub from: PathBuf,
    /// Intended absolute destination.
    pub to: PathBuf,
    /// File or directory.
    pub kind: MoveKind,
    /// Size in bytes (0 for directories).
    pub size: u64,
}

/// A complete set of moves plus everything that was deliberately skipped.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Folder being tidied; every path in the plan lives beneath it.
    pub root: PathBuf,
    /// Moves to perform, in order.
    pub moves: Vec<PlannedMove>,
    /// Entries left alone, with reasons.
    pub skipped: Vec<Skipped>,
}

impl Plan {
    /// `true` when there is nothing to move.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// Total bytes that would be relocated.
    pub fn total_bytes(&self) -> u64 {
        self.moves.iter().map(|m| m.size).sum()
    }

    /// Move counts and byte totals grouped by top-level destination folder.
    pub fn by_folder(&self) -> BTreeMap<String, (usize, u64)> {
        let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for m in &self.moves {
            let folder = m
                .to
                .strip_prefix(&self.root)
                .ok()
                .and_then(|rel| rel.components().next())
                .map_or_else(String::new, |c| c.as_os_str().to_string_lossy().into_owned());
            let entry = groups.entry(folder).or_default();
            entry.0 += 1;
            entry.1 += m.size;
        }
        groups
    }
}

/// Lower-cased names of top-level folders this tool creates; scans skip them
/// so running twice never re-shuffles already organized files.
pub fn organize_skip_dirs() -> BTreeSet<String> {
    Category::ALL
        .iter()
        .map(|c| c.folder_name())
        .chain([PROJECTS_DIR, DUPLICATES_DIR])
        .map(str::to_lowercase)
        .collect()
}

/// Builds the plan that sorts every scanned file into its category folder.
pub fn build_organize_plan(root: &Path, scan: &ScanResult, projects: ProjectPolicy) -> Plan {
    let mut reserved = HashSet::new();
    let mut moves = Vec::with_capacity(scan.files.len());
    let mut skipped = scan.skipped.clone();

    let mut push = |from: &Path, folder: &str, kind: MoveKind, size: u64| {
        let Some(name) = from.file_name() else { return };
        let to = unique_path(&root.join(folder).join(name), &reserved);
        reserved.insert(to.clone());
        moves.push(PlannedMove {
            from: from.to_path_buf(),
            to,
            kind,
            size,
        });
    };

    for file in &scan.files {
        push(
            &file.path,
            Category::from_path(&file.path).folder_name(),
            MoveKind::File,
            file.size,
        );
    }
    for project in &scan.projects {
        match projects {
            ProjectPolicy::Move => push(project, PROJECTS_DIR, MoveKind::Dir, 0),
            ProjectPolicy::Keep => skipped.push(Skipped {
                path: project.clone(),
                reason: SkipReason::Project,
            }),
        }
    }
    Plan {
        root: root.to_path_buf(),
        moves,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::FileEntry;

    fn file(root: &Path, rel: &str, size: u64) -> FileEntry {
        FileEntry {
            path: root.join(rel),
            size,
            modified: None,
            depth: 1,
        }
    }

    fn scan_of(root: &Path, files: &[(&str, u64)], projects: &[&str]) -> ScanResult {
        ScanResult {
            files: files.iter().map(|(p, s)| file(root, p, *s)).collect(),
            projects: projects.iter().map(|p| root.join(p)).collect(),
            skipped: vec![],
        }
    }

    #[test]
    fn sorts_files_into_category_folders() {
        let root = Path::new("/r");
        let scan = scan_of(root, &[("a.png", 10), ("b.pdf", 20), ("c.unknown", 5)], &[]);
        let plan = build_organize_plan(root, &scan, ProjectPolicy::Keep);
        let dests: Vec<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(
            dests,
            [
                root.join("Images/a.png"),
                root.join("Documents/b.pdf"),
                root.join("Other/c.unknown")
            ]
        );
        assert_eq!(plan.total_bytes(), 35);
    }

    #[test]
    fn same_name_files_get_distinct_destinations() {
        let root = Path::new("/r-no-such-dir");
        let scan = scan_of(root, &[("x/a.png", 1), ("y/a.png", 1), ("z/a.png", 1)], &[]);
        let plan = build_organize_plan(root, &scan, ProjectPolicy::Keep);
        let dests: HashSet<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(dests.len(), 3);
        assert!(dests.contains(&root.join("Images/a (2).png")));
    }

    #[test]
    fn projects_kept_or_moved_by_policy() {
        let root = Path::new("/r");
        let scan = scan_of(root, &[], &["repo"]);
        let keep = build_organize_plan(root, &scan, ProjectPolicy::Keep);
        assert!(keep.is_empty());
        assert_eq!(keep.skipped[0].reason, SkipReason::Project);

        let moved = build_organize_plan(root, &scan, ProjectPolicy::Move);
        assert_eq!(moved.moves[0].to, root.join("Projects/repo"));
        assert_eq!(moved.moves[0].kind, MoveKind::Dir);
    }

    #[test]
    fn groups_by_destination_folder() {
        let root = Path::new("/r");
        let scan = scan_of(root, &[("a.png", 10), ("b.jpg", 30), ("c.pdf", 5)], &[]);
        let groups = build_organize_plan(root, &scan, ProjectPolicy::Keep).by_folder();
        assert_eq!(groups["Images"], (2, 40));
        assert_eq!(groups["Documents"], (1, 5));
    }

    #[test]
    fn skip_dirs_cover_every_folder_we_create() {
        let dirs = organize_skip_dirs();
        assert!(dirs.contains("3d models"));
        assert!(dirs.contains("projects"));
        assert!(dirs.contains("_duplicates"));
        assert_eq!(dirs.len(), Category::ALL.len() + 2);
    }
}
