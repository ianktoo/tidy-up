//! `tidy-up dedupe` and `tidy-up purge`

use anyhow::Result;
use console::style;

use crate::{
    api::{Outcome, PlanFile},
    cli::{DedupeArgs, PurgeArgs},
    commands::guard::Guard,
    commands::print_execution,
    dedupe::{
        DuplicateReport, build_dedupe_plan, duplicates_folder_stats, find_duplicates,
        purge_duplicates, write_report,
    },
    executor::execute,
    journal::Operation,
    plan::DUPLICATES_DIR,
    scan::{ScanOptions, scan},
    ui,
};

/// Groups shown when `--verbose` is off.
const GROUP_PREVIEW: usize = 10;

/// Finds duplicates, quarantines the extra copies and recommends deletion.
pub fn run(args: &DedupeArgs) -> Result<Outcome> {
    let root = Guard::write(&args.safety, args.yes, args.dry_run)
        .with_filter(&args.filter)
        .root(&args.path)?;
    let options = ScanOptions {
        max_depth: args.depth.map_or(usize::MAX, |d| d as usize),
        rules: args.filter.to_rules()?,
        skip_root_dirs: [DUPLICATES_DIR.to_lowercase()].into(),
    };

    let spinner = ui::spinner("Scanning");
    let scanned = scan(&root, &options);
    spinner.finish_and_clear();
    let scanned = scanned?;
    ui::info(&format!(
        "Scanned {}: {} to compare",
        root.display(),
        ui::plural(scanned.files.len(), "file")
    ));

    let bar = ui::HashBar::new();
    let report = find_duplicates(&scanned.files, &bar);
    bar.finish();

    if !report.unreadable.is_empty() {
        ui::warn(&format!(
            "{} could not be read and were not compared",
            ui::plural(report.unreadable.len(), "file")
        ));
    }
    // The same values the prose output above is built from.
    let mut outcome = Outcome::at(&root, args.dry_run).with_scan(&scanned, args.verbose);
    outcome.detail = Some(serde_json::json!({
        "groups": report.groups.len(),
        "extra_copies": report.groups.iter().map(|g| g.duplicates.len()).sum::<usize>(),
        "wasted_bytes": report
            .groups
            .iter()
            .map(|g| g.size * g.duplicates.len() as u64)
            .sum::<u64>(),
        "unreadable": report.unreadable.len(),
    }));
    if report.groups.is_empty() {
        ui::success("No duplicates found.");
        return Ok(outcome);
    }
    print_groups(&root, &report, args.verbose);

    let plan = build_dedupe_plan(&root, &report.groups);
    outcome.plan = Some(PlanFile::of(&plan, Operation::Dedupe));
    if args.dry_run {
        crate::out!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(outcome);
    }
    crate::out!();
    let prompt = format!(
        "Move {} into {DUPLICATES_DIR}/ for review? (originals stay in place)",
        ui::plural(plan.moves.len(), "duplicate")
    );
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(outcome);
    }

    let bar = ui::TransferBar::new("Isolating");
    let executed = execute(&plan, Operation::Dedupe, |p| bar.update(p));
    bar.finish();
    let executed = executed?;
    print_execution(&root, &executed);

    let report_path = write_report(&root, &executed.journal_id, &report.groups)?;
    ui::heading("Recommendation");
    ui::info(&format!(
        "Review {DUPLICATES_DIR}/. Deleting it would free {}.",
        ui::format_size(report.reclaimable_bytes())
    ));
    ui::hint(&format!("Full report: {}", report_path.display()));
    ui::hint(&format!(
        "Delete for good: tidy-up purge \"{}\"",
        root.display()
    ));
    Ok(Outcome::default())
}

fn print_groups(root: &std::path::Path, report: &DuplicateReport, verbose: bool) {
    ui::heading("Duplicates");
    let shown = if verbose {
        report.groups.len()
    } else {
        GROUP_PREVIEW.min(report.groups.len())
    };
    for (index, group) in report.groups.iter().take(shown).enumerate() {
        crate::out!(
            "  {} {} × {}",
            style(format!("Group-{:03}", index + 1)).bold(),
            ui::plural(group.duplicates.len() + 1, "copy"),
            style(ui::format_size(group.size)).dim()
        );
        crate::out!(
            "    {} {}",
            style("keep").green(),
            ui::rel(root, &group.keeper)
        );
        for dup in &group.duplicates {
            crate::out!("    {} {}", style("move").yellow(), ui::rel(root, dup));
        }
    }
    if shown < report.groups.len() {
        ui::hint(&format!(
            "… and {} more groups (use --verbose to list everything)",
            report.groups.len() - shown
        ));
    }
    crate::out!(
        "\n  {}",
        style(format!(
            "{} in {} · {} reclaimable",
            ui::plural(report.duplicate_count(), "redundant copy"),
            ui::plural(report.groups.len(), "group"),
            ui::format_size(report.reclaimable_bytes())
        ))
        .bold()
    );
}

/// Permanently deletes the duplicates folder after confirmation.
pub fn purge(args: &PurgeArgs) -> Result<Outcome> {
    // Purge deletes, and a delete cannot be undone, so it gets the strictest
    // reading of the guard: never treated as a dry run.
    let root = Guard::write(&args.safety, args.yes, false).root(&args.path)?;
    let (files, bytes) = duplicates_folder_stats(&root);
    if files == 0 {
        ui::success(&format!("No {DUPLICATES_DIR}/ files to delete."));
        return Ok(Outcome::default());
    }
    ui::warn(&format!(
        "This permanently deletes {} ({}). `tidy-up restore` cannot bring them back.",
        ui::plural(files, "file"),
        ui::format_size(bytes)
    ));
    if !ui::confirm("Delete them?", false, args.yes)? {
        ui::info("Cancelled. Nothing was deleted.");
        return Ok(Outcome::default());
    }
    let (files, bytes) = purge_duplicates(&root)?;
    ui::success(&format!(
        "Deleted {}, freed {}",
        ui::plural(files, "file"),
        ui::format_size(bytes)
    ));
    Ok(Outcome::default())
}
