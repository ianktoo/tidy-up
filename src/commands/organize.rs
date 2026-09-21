//! `tidy-up organize`

use anyhow::Result;

use crate::{
    cli::OrganizeArgs,
    commands::print_execution,
    executor::execute,
    fsops::resolve_root,
    journal::Operation,
    plan::{build_organize_plan, organize_skip_dirs},
    scan::{ScanOptions, scan},
    ui,
};

/// Scans, plans, confirms and performs an organize run.
pub fn run(args: &OrganizeArgs) -> Result<()> {
    let root = resolve_root(&args.path)?;
    let options = ScanOptions {
        max_depth: args.depth as usize,
        rules: args.filter.to_rules()?,
        skip_root_dirs: organize_skip_dirs(),
    };

    let spinner = ui::spinner("Scanning");
    let scanned = scan(&root, &options);
    spinner.finish_and_clear();
    let scanned = scanned?;
    ui::info(&format!(
        "Scanned {}: {} eligible",
        root.display(),
        ui::plural(scanned.files.len(), "file")
    ));

    let plan = build_organize_plan(&root, &scanned, args.projects);
    if plan.is_empty() {
        ui::print_skipped(&root, &plan.skipped, args.verbose);
        ui::success("Nothing to organize: this folder is already tidy.");
        return Ok(());
    }
    ui::print_plan(&plan, args.verbose);
    ui::print_skipped(&root, &plan.skipped, args.verbose);

    if args.dry_run {
        println!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(());
    }
    println!();
    let prompt = format!(
        "Move {} into category folders?",
        ui::plural(plan.moves.len(), "item")
    );
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(());
    }

    let bar = ui::TransferBar::new("Organizing");
    let report = execute(&plan, Operation::Organize, |p| bar.update(p));
    bar.finish();
    print_execution(&root, &report?);
    Ok(())
}
