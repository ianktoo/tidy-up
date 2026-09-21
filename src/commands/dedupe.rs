//! `tidy-up dedupe` and `tidy-up purge`

use anyhow::Result;
use console::style;

use crate::{
    cli::{DedupeArgs, PurgeArgs},
    commands::print_execution,
    dedupe::{
        DuplicateReport, build_dedupe_plan, duplicates_folder_stats, find_duplicates,
        purge_duplicates, write_report,
    },
    executor::execute,
    fsops::resolve_root,
    journal::Operation,
    plan::DUPLICATES_DIR,
    scan::{ScanOptions, scan},
    ui,
};

/// Groups shown when `--verbose` is off.
const GROUP_PREVIEW: usize = 10;

/// Finds duplicates, quarantines the extra copies and recommends deletion.
pub fn run(args: &DedupeArgs) -> Result<()> {
    let root = resolve_root(&args.path)?;
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
    if report.groups.is_empty() {
        ui::success("No duplicates found.");
        return Ok(());
    }
    print_groups(&root, &report, args.verbose);

    let plan = build_dedupe_plan(&root, &report.groups);
    if args.dry_run {
        println!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(());
    }
    println!();
    let prompt = format!(
        "Move {} into {DUPLICATES_DIR}/ for review? (originals stay in place)",
        ui::plural(plan.moves.len(), "duplicate")
    );
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(());
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
    Ok(())
}

fn print_groups(root: &std::path::Path, report: &DuplicateReport, verbose: bool) {
    ui::heading("Duplicates");
    let shown = if verbose {
        report.groups.len()
    } else {
        GROUP_PREVIEW.min(report.groups.len())
    };
    for (index, group) in report.groups.iter().take(shown).enumerate() {
        println!(
            "  {} {} × {}",
            style(format!("Group-{:03}", index + 1)).bold(),
            ui::plural(group.duplicates.len() + 1, "copy"),
            style(ui::format_size(group.size)).dim()
        );
        println!(
            "    {} {}",
            style("keep").green(),
            ui::rel(root, &group.keeper)
        );
        for dup in &group.duplicates {
            println!("    {} {}", style("move").yellow(), ui::rel(root, dup));
        }
    }
    if shown < report.groups.len() {
        ui::hint(&format!(
            "… and {} more groups (use --verbose to list everything)",
            report.groups.len() - shown
        ));
    }
    println!(
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
pub fn purge(args: &PurgeArgs) -> Result<()> {
    let root = resolve_root(&args.path)?;
    let (files, bytes) = duplicates_folder_stats(&root);
    if files == 0 {
        ui::success(&format!("No {DUPLICATES_DIR}/ files to delete."));
        return Ok(());
    }
    ui::warn(&format!(
        "This permanently deletes {} ({}). `tidy-up restore` cannot bring them back.",
        ui::plural(files, "file"),
        ui::format_size(bytes)
    ));
    if !ui::confirm("Delete them?", false, args.yes)? {
        ui::info("Cancelled. Nothing was deleted.");
        return Ok(());
    }
    let (files, bytes) = purge_duplicates(&root)?;
    ui::success(&format!(
        "Deleted {}, freed {}",
        ui::plural(files, "file"),
        ui::format_size(bytes)
    ));
    Ok(())
}
