//! `tidy-up organize`

use anyhow::Result;

use crate::{
    cli::OrganizeArgs,
    commands::{guard::Guard, print_execution},
    executor::execute,
    journal::Operation,
    obs,
    plan::{build_organize_plan, organize_skip_dirs},
    scan::{ScanOptions, scan},
    ui,
};

/// Scans, plans, confirms and performs an organize run.
pub fn run(args: &OrganizeArgs) -> Result<()> {
    let root = Guard::write(&args.safety, args.yes, args.dry_run)
        .with_filter(&args.filter)
        .root(&args.path)?;
    let options = ScanOptions {
        max_depth: args.depth as usize,
        rules: args.filter.to_rules()?,
        skip_root_dirs: organize_skip_dirs(),
    };

    obs::attach(&root);
    let started = std::time::Instant::now();
    let spinner = ui::spinner("Scanning");
    let scanned = scan(&root, &options);
    spinner.finish_and_clear();
    let scanned = scanned?;
    obs::metrics(|m| m.absorb_scan(&scanned));
    obs::event(obs::Event::Scan {
        root: root.display().to_string(),
        files: scanned.files.len(),
        bytes: scanned.files.iter().map(|f| f.size).sum(),
        projects: scanned.projects.len(),
        skipped: crate::commands::skip_counts(&scanned),
        ms: started.elapsed().as_millis() as u64,
    });
    ui::info(&format!(
        "Scanned {}: {} eligible",
        root.display(),
        ui::plural(scanned.files.len(), "file")
    ));

    let plan = build_organize_plan(&root, &scanned, args.projects);
    obs::plan_built("organize", &plan);
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
