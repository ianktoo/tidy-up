//! Carries out a [`Plan`], journaling every change as it happens.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{
    error::Result,
    fsops::{create_dirs_tracked, move_path, unique_path},
    journal::{JournalWriter, Operation},
    plan::Plan,
};

/// Snapshot passed to the progress callback of [`execute`].
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    /// Moves completed or attempted so far.
    pub done: usize,
    /// Total moves in the plan.
    pub total: usize,
    /// Bytes successfully moved so far.
    pub bytes_done: u64,
    /// Bytes in the whole plan.
    pub bytes_total: u64,
    /// The item about to be moved (the plan root once finished).
    pub current: &'a Path,
}

/// Outcome of [`execute`].
#[derive(Debug, Default)]
pub struct ExecutionReport {
    /// Id of the journal describing this run (empty if nothing was planned).
    pub journal_id: String,
    /// Number of moves that succeeded.
    pub moved: usize,
    /// Bytes relocated.
    pub bytes: u64,
    /// Moves that failed, with the reason. Failures never abort the run.
    pub failed: Vec<(PathBuf, String)>,
}

/// Executes `plan`, calling `on_progress` before each move and once at the end.
///
/// Individual move failures are collected in the report. A *journal* failure is
/// fatal: the move in flight is rolled back and the error returned, because a
/// change that cannot be recorded cannot be undone.
pub fn execute(
    plan: &Plan,
    operation: Operation,
    mut on_progress: impl FnMut(&Progress),
) -> Result<ExecutionReport> {
    if plan.is_empty() {
        return Ok(ExecutionReport::default());
    }
    let mut journal = JournalWriter::create(&plan.root, operation)?;
    let mut report = ExecutionReport {
        journal_id: journal.id().to_string(),
        ..Default::default()
    };
    let total = plan.moves.len();
    let bytes_total = plan.total_bytes();
    let no_reservations = HashSet::new();

    for (done, planned) in plan.moves.iter().enumerate() {
        on_progress(&Progress {
            done,
            total,
            bytes_done: report.bytes,
            bytes_total,
            current: &planned.from,
        });

        // The destination may have appeared since planning; never overwrite.
        let dest = unique_path(&planned.to, &no_reservations);
        let Some(parent) = dest.parent() else {
            continue;
        };

        let created = match create_dirs_tracked(parent) {
            Ok(created) => created,
            Err(e) => {
                report.failed.push((planned.from.clone(), e.to_string()));
                continue;
            }
        };
        for dir in &created {
            journal.record_dir_created(dir)?;
        }
        if let Err(e) = move_path(&planned.from, &dest) {
            report.failed.push((planned.from.clone(), e.to_string()));
            continue;
        }
        if let Err(e) = journal.record_move(&planned.from, &dest, planned.kind) {
            let _ = move_path(&dest, &planned.from);
            return Err(e);
        }
        report.moved += 1;
        report.bytes += planned.size;
    }
    on_progress(&Progress {
        done: total,
        total,
        bytes_done: report.bytes,
        bytes_total,
        current: &plan.root,
    });
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        journal::Journal,
        plan::{MoveKind, PlannedMove},
    };

    fn plan_for(root: &std::path::Path, moves: &[(&str, &str)]) -> Plan {
        Plan {
            root: root.to_path_buf(),
            moves: moves
                .iter()
                .map(|(from, to)| PlannedMove {
                    from: root.join(from),
                    to: root.join(to),
                    kind: MoveKind::File,
                    size: 3,
                })
                .collect(),
            skipped: vec![],
        }
    }

    #[test]
    fn moves_files_and_journals_them() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.png"), "abc").unwrap();
        fs::write(dir.path().join("b.pdf"), "abc").unwrap();
        let plan = plan_for(
            dir.path(),
            &[("a.png", "Images/a.png"), ("b.pdf", "Documents/b.pdf")],
        );

        let mut ticks = Vec::new();
        let report = execute(&plan, Operation::Organize, |p| {
            ticks.push((p.done, p.total))
        })
        .unwrap();

        assert_eq!((report.moved, report.bytes), (2, 6));
        assert!(report.failed.is_empty());
        assert!(dir.path().join("Images/a.png").exists());
        assert!(!dir.path().join("a.png").exists());
        assert_eq!(ticks.first(), Some(&(0, 2)));
        assert_eq!(ticks.last(), Some(&(2, 2)));

        let journal = Journal::find(dir.path(), &report.journal_id).unwrap();
        assert_eq!(journal.move_count(), 2);
    }

    #[test]
    fn empty_plan_creates_no_journal() {
        let dir = tempfile::tempdir().unwrap();
        let report = execute(&plan_for(dir.path(), &[]), Operation::Organize, |_| {}).unwrap();
        assert!(report.journal_id.is_empty());
        assert!(Journal::load_all(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn failures_are_collected_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real.txt"), "abc").unwrap();
        let plan = plan_for(
            dir.path(),
            &[
                ("ghost.txt", "Text Files/ghost.txt"),
                ("real.txt", "Text Files/real.txt"),
            ],
        );
        let report = execute(&plan, Operation::Organize, |_| {}).unwrap();
        assert_eq!(report.moved, 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].0.ends_with("ghost.txt"));
    }

    #[test]
    fn never_overwrites_a_destination_that_appeared_after_planning() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Images")).unwrap();
        fs::write(dir.path().join("Images/a.png"), "OLD").unwrap();
        fs::write(dir.path().join("a.png"), "NEW").unwrap();
        let plan = plan_for(dir.path(), &[("a.png", "Images/a.png")]);
        execute(&plan, Operation::Organize, |_| {}).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("Images/a.png")).unwrap(),
            "OLD"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("Images/a (1).png")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn journals_created_directories_before_moves() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.png"), "abc").unwrap();
        let plan = plan_for(dir.path(), &[("a.png", "Images/a.png")]);
        let report = execute(&plan, Operation::Organize, |_| {}).unwrap();
        let journal = Journal::find(dir.path(), &report.journal_id).unwrap();
        use crate::journal::Record;
        assert!(matches!(journal.records[0], Record::DirCreated { .. }));
        assert!(matches!(journal.records[1], Record::Move { .. }));
    }
}
