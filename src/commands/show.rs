//! `tidy-up show`
//!
//! What did that run actually do?
//!
//! `history` lists runs; this explains one. It is the command a person reaches
//! for when something else did the work, which since the machine interface
//! landed is the common case. An undo journal is only worth having if somebody
//! can read it, and until now the only way to see inside one was to open the
//! JSON Lines file.
//!
//! Two things here are not in the journal and have to be worked out now:
//!
//! * whether each moved item is **still where the run put it**, which decides
//!   whether undoing will be clean or will report items as missing,
//! * what the run **left alone**, which the journal never records because it
//!   only logs changes. That comes from the run log when `--log` was used.

use std::{collections::BTreeMap, path::Path};

use anyhow::Result;
use serde::Serialize;

use crate::{
    api::{ErrorCode, Outcome, Refused},
    cli::ShowArgs,
    commands::guard::Guard,
    journal::{Journal, Operation, Record, journal_dir},
    outcome::Cause,
    plan::MoveKind,
    scan::SkipReason,
    timefmt::format_utc,
    ui,
};

/// Where one moved item is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Whereabouts {
    /// Still where the run put it. Undo will move it back cleanly.
    InPlace,
    /// Gone from where the run put it: moved again, renamed or deleted since.
    /// Undo will report it as missing rather than fail.
    Moved,
}

/// One item the run moved, and where it is now.
#[derive(Debug, Clone, Serialize)]
pub struct Moved {
    /// Where it was before the run, relative to the root.
    pub from: String,
    /// Where the run put it, relative to the root.
    pub to: String,
    /// File or folder.
    pub kind: MoveKind,
    /// Whether it is still there.
    pub state: Whereabouts,
}

/// Everything worth knowing about one run.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    /// Journal id.
    pub id: String,
    /// Which command produced it.
    pub operation: Operation,
    /// When it ran.
    pub created_at: u64,
    /// Human-readable timestamp, so a reader does not have to convert one.
    pub when: String,
    /// The folder it acted on.
    pub root: String,
    /// Items it moved.
    pub moves: usize,
    /// How many of those are still where it put them.
    pub in_place: usize,
    /// Bytes currently sitting at the destinations, measured now rather than
    /// recorded at the time: the journal does not store sizes.
    pub bytes_in_place: u64,
    /// Folders it created.
    pub dirs_created: usize,
    /// Folders it emptied and removed.
    pub dirs_removed: usize,
    /// Whether it has already been undone.
    pub restored: bool,
    /// When it was undone, if it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<u64>,
    /// What it left alone, by reason, when a run log recorded it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub skipped: BTreeMap<String, u64>,
    /// Items it could not process, by cause, when a run log recorded them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub problems: BTreeMap<String, u64>,
    /// Every move.
    pub items: Vec<Moved>,
}

impl Review {
    /// Items that are no longer where the run left them.
    pub fn gone(&self) -> usize {
        self.moves - self.in_place
    }
}

/// Reads a journal and reports what its run did.
pub fn run(args: &ShowArgs) -> Result<Outcome> {
    // Read-only, so it warns about a system folder and carries on.
    let root = Guard::read().root(&args.path)?;

    let journal = match &args.id {
        Some(id) => Journal::find(&root, id)?,
        None => Journal::load_all(&root)?.pop().ok_or_else(|| {
            Refused::at(
                ErrorCode::NotFound,
                &root,
                format!("no runs recorded for {}", root.display()),
            )
        })?,
    };

    let review = review(&root, &journal);
    print_review(&review, args.verbose);

    let mut outcome = Outcome::at(&root, true);
    outcome.detail = Some(serde_json::to_value(&review)?);
    Ok(outcome)
}

/// Builds the review, checking the disk for where things are now.
fn review(root: &Path, journal: &Journal) -> Review {
    let mut items = Vec::new();
    let (mut in_place, mut bytes_in_place) = (0usize, 0u64);
    let (mut dirs_created, mut dirs_removed) = (0usize, 0usize);

    for record in &journal.records {
        match record {
            Record::DirCreated { .. } => dirs_created += 1,
            Record::DirRemoved { .. } => dirs_removed += 1,
            Record::Move { from, to, kind } => {
                let landed = root.join(to);
                let there = landed.exists();
                if there {
                    in_place += 1;
                    // Folders are not measured: the journal records the move,
                    // not the tree, and walking it would be a different claim.
                    if *kind == MoveKind::File {
                        bytes_in_place += std::fs::metadata(&landed).map_or(0, |m| m.len());
                    }
                }
                items.push(Moved {
                    from: from.display().to_string(),
                    to: to.display().to_string(),
                    kind: *kind,
                    state: if there {
                        Whereabouts::InPlace
                    } else {
                        Whereabouts::Moved
                    },
                });
            }
            Record::Header(_) | Record::Restored { .. } => {}
        }
    }

    let restored_at = journal.records.iter().find_map(|r| match r {
        Record::Restored { at } => Some(*at),
        _ => None,
    });
    let (skipped, problems) = from_run_log(root, &journal.header.id);

    Review {
        id: journal.header.id.clone(),
        operation: journal.header.operation,
        created_at: journal.header.created_at,
        when: format_utc(journal.header.created_at),
        root: root.display().to_string(),
        moves: items.len(),
        in_place,
        bytes_in_place,
        dirs_created,
        dirs_removed,
        restored: journal.is_restored(),
        restored_at,
        skipped,
        problems,
        items,
    }
}

/// What the run left alone and what went wrong, from the run log if there is one.
///
/// The journal records changes, so it can never answer this. A missing or
/// unreadable log is not an error: most runs are not logged, and the review is
/// still worth having without it.
fn from_run_log(root: &Path, id: &str) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let path = journal_dir(root)
        .parent()
        .map(|state| state.join("logs").join(format!("{id}.jsonl")));
    let (mut skipped, mut problems) = (BTreeMap::new(), BTreeMap::new());
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else {
        return (skipped, problems);
    };
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match event.get("type").and_then(|t| t.as_str()) {
            Some("scan") => {
                if let Some(counts) = event.get("skipped").and_then(|s| s.as_object()) {
                    for (reason, count) in counts {
                        *skipped.entry(reason.clone()).or_insert(0) += count.as_u64().unwrap_or(0);
                    }
                }
            }
            Some("problem") => {
                if let Some(cause) = event.get("cause").and_then(|c| c.as_str()) {
                    *problems.entry(cause.to_string()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }
    (skipped, problems)
}

/// How many moves to list before summarising, unless `--verbose`.
const PREVIEW: usize = 12;

fn print_review(review: &Review, verbose: bool) {
    ui::heading(&format!("Run {}", review.id));
    ui::info(&format!(
        "{} · {} · {}",
        review.operation,
        review.when,
        ui::plural(review.moves, "move")
    ));

    if review.moves > 0 {
        ui::heading("What it moved");
        let limit = if verbose { usize::MAX } else { PREVIEW };
        for item in review.items.iter().take(limit) {
            let mark = match item.state {
                Whereabouts::InPlace => " ",
                Whereabouts::Moved => "?",
            };
            crate::out!("  {mark} {} \u{2192} {}", item.from, item.to);
        }
        if review.moves > limit {
            ui::hint(&format!(
                "… and {} more (use --verbose to list them all)",
                review.moves - limit
            ));
        }
    }
    if review.dirs_created > 0 {
        ui::info(&format!(
            "{} created",
            ui::plural(review.dirs_created, "folder")
        ));
    }
    if review.dirs_removed > 0 {
        ui::info(&format!(
            "{} emptied and removed",
            ui::plural(review.dirs_removed, "folder")
        ));
    }
    if !review.skipped.is_empty() {
        ui::heading("What it left alone");
        for (reason, count) in &review.skipped {
            ui::hint(&format!("{}: {count}", SkipReason::label_for_key(reason)));
        }
    }
    if !review.problems.is_empty() {
        ui::heading("What it could not do");
        for (cause, count) in &review.problems {
            ui::warn(&format!("{}: {count}", Cause::label_for_key(cause)));
        }
    }

    ui::heading("Can it still be undone?");
    if review.restored {
        let when = review.restored_at.map_or_else(String::new, format_utc);
        ui::success(&format!("Already undone{}.", suffix(&when)));
        return;
    }
    if review.gone() == 0 {
        ui::success(&format!(
            "Yes. All {} still where this run put them ({}).",
            ui::plural(review.moves, "item"),
            ui::format_size(review.bytes_in_place)
        ));
    } else {
        // Undo is still worth running; it puts back what it can and says what
        // it could not find, rather than refusing because the tree moved on.
        ui::warn(&format!(
            "Partly. {} of {} have since been moved, renamed or deleted, and will be reported as missing.",
            review.gone(),
            review.moves
        ));
    }
    ui::hint(&format!(
        "tidy-up restore \"{}\" --id {}",
        review.root, review.id
    ));
}

fn suffix(when: &str) -> String {
    if when.is_empty() {
        String::new()
    } else {
        format!(" at {when}")
    }
}

/// Reviews a journal that a caller has already loaded.
///
/// Used by the MCP server, which resolves and confines the folder itself.
pub fn review_of(root: &Path, journal: &Journal) -> Review {
    review(root, journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        executor::execute,
        plan::{Plan, PlannedMove},
    };

    fn run_organize(root: &Path) -> Journal {
        std::fs::write(root.join("a.png"), "xx").unwrap();
        std::fs::write(root.join("b.pdf"), "yyy").unwrap();
        let plan = Plan {
            root: root.to_path_buf(),
            moves: vec![
                PlannedMove {
                    from: root.join("a.png"),
                    to: root.join("Images").join("a.png"),
                    kind: MoveKind::File,
                    size: 2,
                },
                PlannedMove {
                    from: root.join("b.pdf"),
                    to: root.join("Documents").join("b.pdf"),
                    kind: MoveKind::File,
                    size: 3,
                },
            ],
            skipped: Vec::new(),
        };
        let report = execute(&plan, Operation::Organize, |_| {}).unwrap();
        Journal::find(root, &report.journal_id).unwrap()
    }

    #[test]
    fn a_clean_run_reports_everything_still_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let journal = run_organize(dir.path());
        let review = review_of(dir.path(), &journal);

        assert_eq!(review.moves, 2);
        assert_eq!(review.in_place, 2);
        assert_eq!(review.gone(), 0);
        assert_eq!(review.bytes_in_place, 5, "measured from the disk now");
        assert_eq!(review.dirs_created, 2, "Images and Documents");
        assert!(!review.restored);
        assert!(review.items.iter().all(|i| i.state == Whereabouts::InPlace));
    }

    /// The question the reviewer actually has: will undoing this be clean?
    /// The journal cannot answer it, because the tree moves on afterwards.
    #[test]
    fn an_item_that_moved_on_afterwards_is_reported_as_gone() {
        let dir = tempfile::tempdir().unwrap();
        let journal = run_organize(dir.path());
        std::fs::remove_file(dir.path().join("Images").join("a.png")).unwrap();

        let review = review_of(dir.path(), &journal);
        assert_eq!(review.in_place, 1);
        assert_eq!(review.gone(), 1);
        let missing = review
            .items
            .iter()
            .find(|i| i.state == Whereabouts::Moved)
            .expect("one item is gone");
        assert!(missing.to.contains("a.png"));
    }

    #[test]
    fn paths_are_relative_so_the_report_reads_as_the_folder_does() {
        let dir = tempfile::tempdir().unwrap();
        let journal = run_organize(dir.path());
        let review = review_of(dir.path(), &journal);
        for item in &review.items {
            assert!(
                !item.to.contains(dir.path().to_str().unwrap()),
                "absolute path leaked: {}",
                item.to
            );
        }
    }

    #[test]
    fn an_undone_run_says_so_rather_than_offering_to_undo_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = run_organize(dir.path());
        journal.mark_restored().unwrap();
        let journal = Journal::find(dir.path(), &journal.header.id).unwrap();

        let review = review_of(dir.path(), &journal);
        assert!(review.restored);
        assert!(review.restored_at.is_some());
    }

    /// Most runs are not logged, and a review is still worth having without
    /// one, so a missing log is silence rather than an error.
    #[test]
    fn a_run_with_no_log_still_reviews() {
        let dir = tempfile::tempdir().unwrap();
        let journal = run_organize(dir.path());
        let review = review_of(dir.path(), &journal);
        assert!(review.skipped.is_empty());
        assert!(review.problems.is_empty());
    }

    /// What a run left alone is never in the journal, because a journal only
    /// records changes. It comes from the run log when there is one.
    #[test]
    fn what_the_run_left_alone_comes_from_the_run_log() {
        let dir = tempfile::tempdir().unwrap();
        let journal = run_organize(dir.path());
        let logs = dir.path().join(".tidy-up").join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join(format!("{}.jsonl", journal.header.id)),
            concat!(
                r#"{"type":"scan","skipped":{"hidden":3,"ignored":1}}"#,
                "\n",
                r#"{"type":"problem","cause":"denied"}"#,
                "\n",
                r#"{"type":"problem","cause":"denied"}"#,
                "\n",
                "this line is not JSON and must not stop the rest\n",
            ),
        )
        .unwrap();

        let review = review_of(dir.path(), &journal);
        assert_eq!(review.skipped.get("hidden"), Some(&3));
        assert_eq!(review.skipped.get("ignored"), Some(&1));
        assert_eq!(review.problems.get("denied"), Some(&2));
    }
}
