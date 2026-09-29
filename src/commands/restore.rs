//! `tidy-up restore` and `tidy-up history`

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use console::style;

use crate::{
    api::Outcome,
    cli::{HistoryArgs, RestoreArgs},
    commands::guard::Guard,
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
pub fn run(args: &RestoreArgs) -> Result<Outcome> {
    let root = Guard::write(&args.safety, args.yes, args.dry_run).root(&args.path)?;
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
        return Ok(Outcome::default());
    }
    for mut journal in selected {
        restore_one(
            &root,
            &mut journal,
            args.on_conflict,
            args.dry_run,
            args.yes,
            args.allow_outside_root,
        )?;
    }
    if !active.is_empty() {
        ui::hint(&format!(
            "{} older run(s) remain; run `tidy-up restore` again to undo the next one.",
            active.len()
        ));
    }
    Ok(Outcome::default())
}

/// Confirms and undoes a single journal, printing a full account of the result.
/// Whether this journal may put files back outside the folder being restored,
/// asking first if it wants to.
///
/// `compare` and `distribute` legitimately record absolute paths in the other
/// folders they were given, and restoring one has to reach them. But a journal
/// is a file inside the folder, so a folder obtained from somewhere else can
/// carry one claiming a file belongs anywhere on the disk. The destinations are
/// therefore shown and confirmed rather than assumed.
///
/// `--yes` on its own does not reach this: `--allow-outside-root` is the
/// deliberate act, exactly as `--allow-system-folder` is for the guard. With
/// the flag, `--yes` answers the confirmation as it does everywhere else.
fn allow_outside_root(root: &Path, journal: &Journal, permitted: bool, yes: bool) -> Result<bool> {
    let outside: Vec<PathBuf> = journal
        .moves()
        .map(|(from, _, _)| root.join(from))
        .filter(|p| p.is_absolute() && !p.starts_with(root))
        .collect();
    if outside.is_empty() {
        return Ok(false);
    }
    ui::heading("Careful");
    ui::danger(&format!(
        "This run recorded {} outside {}.",
        ui::plural(outside.len(), "item"),
        root.display()
    ));
    ui::hint("That is normal for a `compare` or `distribute` across folders.");
    ui::hint("It is not normal for a folder you got from somewhere else: a journal");
    ui::hint("is just a file, and one can claim a file belongs anywhere on the disk.");
    for path in outside.iter().take(5) {
        ui::hint(&format!("  {}", path.display()));
    }
    if outside.len() > 5 {
        ui::hint(&format!("  … and {} more", outside.len() - 5));
    }
    if !permitted {
        ui::hint("Pass --allow-outside-root to put them back as well.");
        ui::info("Leaving them. Everything inside the folder is still restored.");
        return Ok(false);
    }
    // Same shape as the system-folder guard: the flag is the deliberate act
    // and gets you to the question, `--yes` answers it, and `--yes` on its own
    // is not enough. Agreeing to restore a folder is not agreeing to write
    // outside it; saying so explicitly is.
    if !ui::confirm("Put those back too?", false, yes)? {
        ui::info("Leaving them. Everything inside the folder is still restored.");
        return Ok(false);
    }
    Ok(true)
}

pub fn restore_one(
    root: &Path,
    journal: &mut Journal,
    conflict: ConflictPolicy,
    dry_run: bool,
    yes: bool,
    allow_outside: bool,
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
    let options = RestoreOptions {
        conflict,
        dry_run,
        allow_outside: allow_outside_root(root, journal, allow_outside, yes)?,
    };
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
    let problems: [(&str, Vec<String>); 4] = [
        (
            "left in place, original name is taken (use --on-conflict rename)",
            report.conflicts.iter().map(|p| ui::rel(root, p)).collect(),
        ),
        (
            "left alone because they belong outside this folder",
            report.outside.iter().map(|p| ui::rel(root, p)).collect(),
        ),
        (
            "missing (deleted or purged) and skipped",
            report.missing.iter().map(|p| ui::rel(root, p)).collect(),
        ),
        (
            "failed",
            report
                .problems
                .iter()
                .map(|problem| format!("{}: {}", ui::rel(root, &problem.path), problem.message))
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
pub fn history(args: &HistoryArgs) -> Result<Outcome> {
    // `history` only reads, so it warns and carries on.
    let root = Guard::read().root(&args.path)?;
    let journals = Journal::load_all(&root)?;
    if journals.is_empty() {
        ui::info("No runs recorded for this folder yet.");
        return Ok(Outcome::default());
    }
    ui::heading(&format!("History for {}", root.display()));
    for j in journals.iter().rev() {
        let status = if j.is_restored() {
            style("restored").dim()
        } else {
            style("active").green()
        };
        crate::out!(
            "  {}  {:<9} {:<26} {:>10}  {}",
            style(&j.header.id).bold(),
            j.header.operation.to_string(),
            format_utc(j.header.created_at),
            ui::plural(j.move_count(), "move"),
            status
        );
    }
    Ok(Outcome::default())
}
