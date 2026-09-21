//! Undoing a journaled run.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    error::{IoContext, Result},
    fsops::{move_path, unique_path},
    journal::{Journal, Record},
};

/// What to do when a file's original location is occupied again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ConflictPolicy {
    /// Restore under a free name such as `name (1).ext`.
    #[default]
    Rename,
    /// Leave the file where it is and report the conflict.
    Skip,
}

/// Options for [`restore`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RestoreOptions {
    /// Behaviour when the original path is taken.
    pub conflict: ConflictPolicy,
    /// Compute and report what would happen without touching anything.
    pub dry_run: bool,
}

/// What a restore did (or, for a dry run, would do).
#[derive(Debug, Default)]
pub struct RestoreReport {
    /// Items moved back to their original path.
    pub restored: usize,
    /// Items that were restored under a different name because the original was taken.
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Items left in place because the original path was taken (policy `Skip`).
    pub conflicts: Vec<PathBuf>,
    /// Items that no longer exist where the journal says they are (deleted or purged).
    pub missing: Vec<PathBuf>,
    /// Items that could not be moved, with the reason.
    pub failed: Vec<(PathBuf, String)>,
    /// Empty folders created by the run that were removed again.
    pub dirs_removed: usize,
}

impl RestoreReport {
    /// `true` if everything that could be restored was.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty() && self.failed.is_empty()
    }
}

/// Reverses `journal` (newest change first) beneath `root`.
///
/// On a real run that ends without conflicts or failures the journal is marked
/// restored so it cannot be applied twice. Missing items do not block that:
/// they are gone for good and are simply reported.
pub fn restore(
    root: &Path,
    journal: &mut Journal,
    options: RestoreOptions,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<RestoreReport> {
    let mut report = RestoreReport::default();
    let total = journal.records.len();
    let no_reservations = HashSet::new();

    for (done, record) in journal.records.iter().enumerate().rev() {
        on_progress(total - 1 - done, total);
        match record {
            Record::Move { from, to, .. } => {
                let current = root.join(to);
                let original = root.join(from);
                if fs::symlink_metadata(&current).is_err() {
                    report.missing.push(to.clone());
                    continue;
                }
                let target = if original.exists() {
                    match options.conflict {
                        ConflictPolicy::Skip => {
                            report.conflicts.push(from.clone());
                            continue;
                        }
                        ConflictPolicy::Rename => {
                            let free = unique_path(&original, &no_reservations);
                            report.renamed.push((from.clone(), free.clone()));
                            free
                        }
                    }
                } else {
                    original
                };
                if options.dry_run {
                    report.restored += 1;
                    continue;
                }
                match move_path(&current, &target) {
                    Ok(()) => report.restored += 1,
                    Err(e) => {
                        report.renamed.retain(|(_, new)| new != &target);
                        report.failed.push((to.clone(), e.to_string()));
                    }
                }
            }
            // `remove_dir` only succeeds on an empty folder, which is exactly what we want.
            Record::DirCreated { path }
                if !options.dry_run && fs::remove_dir(root.join(path)).is_ok() =>
            {
                report.dirs_removed += 1;
            }
            _ => {}
        }
    }
    on_progress(total, total);

    if !options.dry_run && report.is_clean() {
        journal.mark_restored()?;
    }
    Ok(report)
}

/// Deletes the tidy-up state folder's journals for `root` that are already restored.
///
/// Returns how many journal files were removed.
pub fn forget_restored(root: &Path) -> Result<usize> {
    let mut removed = 0;
    for journal in Journal::load_all(root)? {
        if journal.is_restored() {
            fs::remove_file(&journal.path).at(&journal.path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        executor::execute,
        journal::Operation,
        plan::{MoveKind, Plan, PlannedMove},
    };

    /// Builds a folder, organizes it by hand-made plan, returns (dir, journal id).
    fn organized(files: &[(&str, &str)], moves: &[(&str, &str)]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            let path = dir.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        let plan = Plan {
            root: dir.path().to_path_buf(),
            moves: moves
                .iter()
                .map(|(f, t)| PlannedMove {
                    from: dir.path().join(f),
                    to: dir.path().join(t),
                    kind: MoveKind::File,
                    size: 1,
                })
                .collect(),
            skipped: vec![],
        };
        let id = execute(&plan, Operation::Organize, |_| {})
            .unwrap()
            .journal_id;
        (dir, id)
    }

    fn run(dir: &Path, id: &str, options: RestoreOptions) -> RestoreReport {
        let mut journal = Journal::find(dir, id).unwrap();
        restore(dir, &mut journal, options, |_, _| {}).unwrap()
    }

    #[test]
    fn restores_files_and_removes_created_folders() {
        let (dir, id) = organized(
            &[("a.png", "1"), ("sub/b.pdf", "2")],
            &[("a.png", "Images/a.png"), ("sub/b.pdf", "Documents/b.pdf")],
        );
        let report = run(dir.path(), &id, RestoreOptions::default());
        assert_eq!(report.restored, 2);
        assert_eq!(report.dirs_removed, 2);
        assert!(report.is_clean());
        assert_eq!(fs::read_to_string(dir.path().join("a.png")).unwrap(), "1");
        assert_eq!(
            fs::read_to_string(dir.path().join("sub/b.pdf")).unwrap(),
            "2"
        );
        assert!(!dir.path().join("Images").exists());
        assert!(!dir.path().join("Documents").exists());
        assert!(Journal::find(dir.path(), &id).unwrap().is_restored());
    }

    #[test]
    fn same_named_files_return_to_their_own_homes() {
        let (dir, id) = organized(
            &[("x/a.png", "X"), ("y/a.png", "Y")],
            &[("x/a.png", "Images/a.png"), ("y/a.png", "Images/a (1).png")],
        );
        run(dir.path(), &id, RestoreOptions::default());
        assert_eq!(fs::read_to_string(dir.path().join("x/a.png")).unwrap(), "X");
        assert_eq!(fs::read_to_string(dir.path().join("y/a.png")).unwrap(), "Y");
    }

    #[test]
    fn dry_run_changes_nothing() {
        let (dir, id) = organized(&[("a.png", "1")], &[("a.png", "Images/a.png")]);
        let report = run(
            dir.path(),
            &id,
            RestoreOptions {
                dry_run: true,
                ..Default::default()
            },
        );
        assert_eq!(report.restored, 1);
        assert!(dir.path().join("Images/a.png").exists());
        assert!(!Journal::find(dir.path(), &id).unwrap().is_restored());
    }

    #[test]
    fn conflict_rename_keeps_both_files() {
        let (dir, id) = organized(&[("a.png", "moved")], &[("a.png", "Images/a.png")]);
        fs::write(dir.path().join("a.png"), "newcomer").unwrap();
        let report = run(dir.path(), &id, RestoreOptions::default());
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.png")).unwrap(),
            "newcomer"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a (1).png")).unwrap(),
            "moved"
        );
    }

    #[test]
    fn conflict_skip_leaves_file_and_keeps_journal_active() {
        let (dir, id) = organized(&[("a.png", "moved")], &[("a.png", "Images/a.png")]);
        fs::write(dir.path().join("a.png"), "newcomer").unwrap();
        let options = RestoreOptions {
            conflict: ConflictPolicy::Skip,
            dry_run: false,
        };
        let report = run(dir.path(), &id, options);
        assert_eq!(report.conflicts.len(), 1);
        assert!(dir.path().join("Images/a.png").exists());
        assert!(!Journal::find(dir.path(), &id).unwrap().is_restored());
    }

    #[test]
    fn missing_files_are_reported_and_do_not_block_completion() {
        let (dir, id) = organized(
            &[("a.png", "1"), ("b.png", "2")],
            &[("a.png", "Images/a.png"), ("b.png", "Images/b.png")],
        );
        fs::remove_file(dir.path().join("Images/a.png")).unwrap();
        let report = run(dir.path(), &id, RestoreOptions::default());
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.restored, 1);
        assert!(Journal::find(dir.path(), &id).unwrap().is_restored());
    }

    #[test]
    fn keeps_created_folder_that_still_has_other_content() {
        let (dir, id) = organized(&[("a.png", "1")], &[("a.png", "Images/a.png")]);
        fs::write(dir.path().join("Images/user-added.png"), "u").unwrap();
        let report = run(dir.path(), &id, RestoreOptions::default());
        assert_eq!(report.dirs_removed, 0);
        assert!(dir.path().join("Images/user-added.png").exists());
    }

    #[test]
    fn forget_restored_only_removes_finished_journals() {
        let (dir, done) = organized(&[("a.png", "1")], &[("a.png", "Images/a.png")]);
        run(dir.path(), &done, RestoreOptions::default());
        fs::write(dir.path().join("b.png"), "2").unwrap();
        let plan = Plan {
            root: dir.path().to_path_buf(),
            moves: vec![PlannedMove {
                from: dir.path().join("b.png"),
                to: dir.path().join("Images/b.png"),
                kind: MoveKind::File,
                size: 1,
            }],
            skipped: vec![],
        };
        let active = execute(&plan, Operation::Organize, |_| {})
            .unwrap()
            .journal_id;
        assert_eq!(forget_restored(dir.path()).unwrap(), 1);
        let left = Journal::load_all(dir.path()).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].header.id, active);
    }
}
