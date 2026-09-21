//! `tidy-up compare`

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use console::style;
use dialoguer::Select;

use crate::{
    cli::CompareArgs,
    commands::print_execution,
    compare::{
        CompareAction, Comparison, Relation, build_merge_plan, compare, scan_folders,
        validate_folders,
    },
    dedupe::{DuplicateGroup, build_dedupe_plan, delete_duplicates, find_duplicates, write_report},
    executor::execute,
    journal::Operation,
    plan::{DUPLICATES_DIR, Plan},
    ui,
};

/// Groups listed when `--verbose` is off.
const GROUP_PREVIEW: usize = 8;

/// Compares the folders, reports how they relate, and applies the chosen action.
pub fn run(args: &CompareArgs) -> Result<()> {
    let folders = validate_folders(&args.paths)?;
    let rules = args.filter.to_rules()?;
    let max_depth = args.depth.map_or(usize::MAX, |d| d as usize);

    let spinner = ui::spinner("Scanning");
    let files = scan_folders(&folders, &rules, max_depth, |i, n| {
        spinner.set_message(format!("Scanning folder {}/{n}", i + 1));
    });
    spinner.finish_and_clear();
    let files = files?;
    ui::info(&format!(
        "Scanned {} in {}",
        ui::plural(files.len(), "file"),
        ui::plural(folders.len(), "folder")
    ));

    let bar = ui::HashBar::new();
    let found = find_duplicates(&files, &bar);
    bar.finish();
    if !found.unreadable.is_empty() {
        ui::warn(&format!(
            "{} could not be read and were not compared",
            ui::plural(found.unreadable.len(), "file")
        ));
    }

    let comparison = compare(&folders, &files, &found.groups);
    print_comparison(&comparison);

    let has_duplicates = !found.groups.is_empty();
    if has_duplicates {
        print_groups(&folders, &found.groups, args.verbose);
        println!(
            "\n  {}",
            style(format!(
                "{} in {} · {} reclaimable",
                ui::plural(found.duplicate_count(), "extra copy"),
                ui::plural(found.groups.len(), "group"),
                ui::format_size(found.reclaimable_bytes())
            ))
            .bold()
        );
    } else {
        println!();
        ui::success("No file content is repeated across these folders.");
    }

    let action = match choose_action(args, &folders[0], has_duplicates, folders.len())? {
        CompareAction::Leave => {
            ui::hint("Left everything as is.");
            return Ok(());
        }
        action => action,
    };
    match action {
        CompareAction::Move => {
            let plan = build_dedupe_plan(&folders[0], &found.groups);
            apply_plan(&folders[0], plan, &found.groups, args)
        }
        CompareAction::Merge => {
            let plan = build_merge_plan(&folders, &files, &found.groups);
            apply_plan(&folders[0], plan, &found.groups, args)
        }
        CompareAction::Delete => delete_flow(&found.groups, args),
        CompareAction::Leave => Ok(()),
    }
}

/// Uses `--action`, else asks (on a terminal), else assumes "leave".
fn choose_action(
    args: &CompareArgs,
    primary: &Path,
    has_duplicates: bool,
    folder_count: usize,
) -> Result<CompareAction> {
    if let Some(action) = args.action {
        if action == CompareAction::Merge && folder_count < 2 {
            bail!("merge needs at least two folders");
        }
        if matches!(action, CompareAction::Move | CompareAction::Delete) && !has_duplicates {
            return Ok(CompareAction::Leave);
        }
        return Ok(action);
    }
    if !ui::is_interactive() {
        ui::hint("No --action given, so this was a report only (see `tidy-up compare --help`).");
        return Ok(CompareAction::Leave);
    }

    let primary = primary.display();
    let mut options = vec![(CompareAction::Leave, "Leave everything as is".to_string())];
    if has_duplicates {
        options.push((
            CompareAction::Move,
            format!("Move extra copies into {DUPLICATES_DIR}/ inside {primary} for review"),
        ));
    }
    if folder_count > 1 {
        options.push((
            CompareAction::Merge,
            format!("Merge everything into {primary} and move extra copies aside"),
        ));
    }
    if has_duplicates {
        options.push((
            CompareAction::Delete,
            "Delete extra copies permanently".into(),
        ));
    }
    println!();
    let labels: Vec<&str> = options.iter().map(|(_, label)| label.as_str()).collect();
    let pick = Select::new()
        .with_prompt("What should happen with the duplicates?")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(options[pick].0)
}

/// Previews, confirms and executes a move/merge plan (undoable).
fn apply_plan(
    primary: &Path,
    plan: Plan,
    groups: &[DuplicateGroup],
    args: &CompareArgs,
) -> Result<()> {
    if plan.is_empty() {
        ui::success("Nothing to move.");
        return Ok(());
    }
    ui::print_plan(&plan, args.verbose);
    if args.dry_run {
        println!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(());
    }
    println!();
    let prompt = format!(
        "Apply these {}? (you can undo this)",
        ui::plural(plan.moves.len(), "move")
    );
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(());
    }

    let bar = ui::TransferBar::new("Moving");
    let executed = execute(&plan, Operation::Compare, |p| bar.update(p));
    bar.finish();
    let executed = executed?;
    print_execution(primary, &executed);

    if !groups.is_empty() && !executed.journal_id.is_empty() {
        let report = write_report(primary, &executed.journal_id, groups)?;
        ui::hint(&format!("Report: {}", report.display()));
        ui::hint(&format!(
            "Review {DUPLICATES_DIR}/, then `tidy-up purge \"{}\"` to delete it for good.",
            primary.display()
        ));
    }
    Ok(())
}

/// Permanently deletes extra copies after re-verifying each one.
fn delete_flow(groups: &[DuplicateGroup], args: &CompareArgs) -> Result<()> {
    let count: usize = groups.iter().map(|g| g.duplicates.len()).sum();
    let bytes: u64 = groups.iter().map(DuplicateGroup::reclaimable_bytes).sum();
    println!();
    ui::warn(&format!(
        "This permanently deletes {} ({}). One copy of each file is kept. It cannot be undone.",
        ui::plural(count, "file"),
        ui::format_size(bytes)
    ));
    if args.dry_run {
        ui::warn("Dry run: nothing was deleted.");
        return Ok(());
    }
    if !ui::confirm("Delete them?", false, args.yes)? {
        ui::info("Cancelled. Nothing was deleted.");
        return Ok(());
    }

    let bar = ui::progress_bar(count, "Deleting");
    let report = delete_duplicates(groups, |done, total| {
        bar.set_length(total as u64);
        bar.set_position(done as u64);
    });
    bar.finish_and_clear();

    ui::heading("Done");
    ui::success(&format!(
        "Deleted {}, freed {}",
        ui::plural(report.deleted, "file"),
        ui::format_size(report.bytes)
    ));
    if !report.skipped.is_empty() {
        ui::warn(&format!(
            "{} kept because they could not be verified or removed:",
            ui::plural(report.skipped.len(), "file")
        ));
        for (path, reason) in report.skipped.iter().take(10) {
            ui::hint(&format!("{}: {reason}", path.display()));
        }
    }
    Ok(())
}

fn tag(index: usize) -> String {
    format!("#{}", index + 1)
}

fn print_comparison(comparison: &Comparison) {
    ui::heading("Folders");
    for (i, f) in comparison.folders.iter().enumerate() {
        println!(
            "  {}  {}\n      {} · {} | {} shared · {} unique · {} repeated inside",
            style(tag(i)).cyan().bold(),
            f.path.display(),
            ui::plural(f.files, "file"),
            ui::format_size(f.bytes),
            f.shared,
            f.unique,
            f.local_copies
        );
    }
    if comparison.folders.len() < 2 {
        return;
    }
    ui::heading("How they compare");
    if comparison.all_identical {
        ui::success("All folders contain exactly the same content.");
        return;
    }
    for &(i, j, relation) in &comparison.relations {
        let (a, b) = (tag(i), tag(j));
        let text = match relation {
            Relation::Identical => format!("{a} and {b} have identical content"),
            Relation::FirstInSecond => format!("everything in {a} is also in {b} ({b} has more)"),
            Relation::SecondInFirst => format!("everything in {b} is also in {a} ({a} has more)"),
            Relation::Overlapping { common } => format!(
                "{a} and {b} share {} but each has files the other lacks",
                ui::plural(common, "item")
            ),
            Relation::Disjoint => format!("{a} and {b} have nothing in common"),
        };
        ui::info(&text);
    }
}

/// `#2  sub/file.txt`: which folder a path belongs to, plus its path inside it.
fn label(folders: &[PathBuf], path: &Path) -> String {
    folders
        .iter()
        .enumerate()
        .find_map(|(i, folder)| {
            path.strip_prefix(folder)
                .ok()
                .map(|rel| format!("{}  {}", tag(i), rel.display()))
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn print_groups(folders: &[PathBuf], groups: &[DuplicateGroup], verbose: bool) {
    ui::heading("Duplicated content");
    let shown = if verbose {
        groups.len()
    } else {
        GROUP_PREVIEW.min(groups.len())
    };
    for (index, group) in groups.iter().take(shown).enumerate() {
        println!(
            "  {} {} x {}",
            style(format!("Group-{:03}", index + 1)).bold(),
            ui::plural(group.duplicates.len() + 1, "copy"),
            style(ui::format_size(group.size)).dim()
        );
        println!(
            "    {} {}",
            style("keep ").green(),
            label(folders, &group.keeper)
        );
        for dup in &group.duplicates {
            println!("    {} {}", style("extra").yellow(), label(folders, dup));
        }
    }
    if shown < groups.len() {
        ui::hint(&format!(
            "... and {} more groups (use --verbose to list everything)",
            groups.len() - shown
        ));
    }
}
