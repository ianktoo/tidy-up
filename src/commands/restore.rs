//! `tidy-up restore` and `tidy-up history`

use std::path::Path;

use anyhow::{Result, bail};
use console::style;

use crate::{
    cli::{HistoryArgs, RestoreArgs},
    fsops::resolve_root,
    journal::Journal,
    restore::{ConflictPolicy, RestoreOptions, RestoreReport, restore},
    timefmt::format_utc,
    ui,
};

/// Lines of each problem category to print before summarising.
const DETAIL_LIMIT: usize = 10;

/// Loads journals that have not been undone yet, oldest first.
pub fn active_journals(root: &Path) -> Result<Vec<Journal>> {
    Ok(Journal::load_all(root)?
        .into_iter()
        .filter(|j| !j.is_restored())
        .collect())
}

/// Restores the most recent run (or `--id`, or `--all`).
pub fn run(args: &RestoreArgs) -> Result<()> {
    let root = resolve_root(&args.path)?;
    let mut active = active_journals(&root)?;

    let selected = match (&args.id, args.all) {
        (Some(id), _) => vec![Journal::find(&root, id)?],
        (None, true) => {
            active.reverse();
            std::mem::take(&mut active)
        }
        (None, false) => active.pop().into_iter().collect(),
    };
    if selected.is_empty() {
        ui::success("Nothing to restore: no active runs recorded for this folder.");
        return Ok(());
    }
    for mut journal in selected {
        restore_one(
            &root,
            &mut journal,
            args.on_conflict,
            args.dry_run,
            args.yes,
        )?;
    }
    if !active.is_empty() {
        ui::hint(&format!(
            "{} older run(s) remain; run `tidy-up restore` again to undo the next one.",
            active.len()
        ));
    }
    Ok(())
}

/// Confirms and undoes a single journal, printing a full account of the result.
pub fn restore_one(
    root: &Path,
    journal: &mut Journal,
    conflict: ConflictPolicy,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    if journal.is_restored() {
        bail!("run {} was already restored", journal.header.id);
    }
    ui::heading(&format!("Restore run {}", journal.header.id));
    ui::info(&format!(
        "{} · {} · {}",
        journal.header.operation,
        format_utc(journal.header.created_at),
        ui::plural(journal.move_count(), "move")
    ));
    if !dry_run && !ui::confirm("Put everything back where it was?", true, yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(());
    }

    let bar = ui::progress_bar(journal.records.len(), "Restoring");
    let options = RestoreOptions { conflict, dry_run };
    let report = restore(root, journal, options, |done, _| {
        bar.set_position(done as u64)
    });
    bar.finish_and_clear();
    print_report(root, &report?, dry_run);
    Ok(())
}

fn print_report(root: &Path, report: &RestoreReport, dry_run: bool) {
    let verb = if dry_run { "Would restore" } else { "Restored" };
    ui::success(&format!("{verb} {}", ui::plural(report.restored, "item")));
    if report.dirs_removed > 0 {
        ui::info(&format!(
            "Removed {} that tidy-up had created",
            ui::plural(report.dirs_removed, "empty folder")
        ));
    }
    if !report.renamed.is_empty() {
        ui::warn(&format!(
            "{} restored under a new name (original name was taken):",
            ui::plural(report.renamed.len(), "item")
        ));
        for (from, to) in report.renamed.iter().take(DETAIL_LIMIT) {
            ui::hint(&format!("{} → {}", ui::rel(root, from), ui::rel(root, to)));
        }
    }
    let problems: [(&str, Vec<String>); 3] = [
        (
            "left in place, original name is taken (use --on-conflict rename)",
            report.conflicts.iter().map(|p| ui::rel(root, p)).collect(),
        ),
        (
            "missing (deleted or purged) and skipped",
            report.missing.iter().map(|p| ui::rel(root, p)).collect(),
        ),
        (
            "failed",
            report
                .failed
                .iter()
                .map(|(p, why)| format!("{}: {why}", ui::rel(root, p)))
                .collect(),
        ),
    ];
    for (label, items) in problems.iter().filter(|(_, items)| !items.is_empty()) {
        ui::warn(&format!("{} {label}:", ui::plural(items.len(), "item")));
        items
            .iter()
            .take(DETAIL_LIMIT)
            .for_each(|line| ui::hint(line));
        if items.len() > DETAIL_LIMIT {
            ui::hint(&format!("… and {} more", items.len() - DETAIL_LIMIT));
        }
    }
    if dry_run {
        ui::warn("Dry run: nothing was changed.");
    }
}

/// Lists every recorded run for a folder.
pub fn history(args: &HistoryArgs) -> Result<()> {
    let root = resolve_root(&args.path)?;
    let journals = Journal::load_all(&root)?;
    if journals.is_empty() {
        ui::info("No runs recorded for this folder yet.");
        return Ok(());
    }
    ui::heading(&format!("History for {}", root.display()));
    for j in journals.iter().rev() {
        let status = if j.is_restored() {
            style("restored").dim()
        } else {
            style("active").green()
        };
        println!(
            "  {}  {:<9} {:<26} {:>10}  {}",
            style(&j.header.id).bold(),
            j.header.operation.to_string(),
            format_utc(j.header.created_at),
            ui::plural(j.move_count(), "move"),
            status
        );
    }
    Ok(())
}
