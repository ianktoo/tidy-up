//! `tidy-up reorganize`
//!
//! Where `organize` sorts the loose files in a folder and leaves its own output
//! alone, `reorganize` unpacks an existing tree and re-files all of it. That is
//! what makes it useful on a folder somebody organized badly years ago, and what
//! makes it the most far-reaching command in the tool: it descends into
//! everything and deletes the folders it empties.

use anyhow::Result;

use crate::{
    api::{Outcome, PlanFile},
    cli::ReorganizeArgs,
    commands::{guard::Guard, print_execution},
    executor::execute_and_clean,
    journal::Operation,
    obs,
    regroup::{build_reorganize_plan, describe},
    scan::{ScanOptions, scan},
    timefmt::utc_offset,
    ui,
};

/// Scans, plans, confirms and performs a reorganize run.
pub fn run(args: &ReorganizeArgs) -> Result<Outcome> {
    let root = Guard::write(&args.safety, args.yes, args.dry_run)
        .with_filter(&args.filter)
        .root(&args.path)?;

    let options = ScanOptions {
        // Unlimited by default, and with an empty skip list: the whole point is
        // to look inside the category folders a previous run created.
        max_depth: crate::cli::depth_or(args.depth, u32::MAX) as usize,
        rules: args.filter.to_rules()?,
        skip_root_dirs: Default::default(),
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
        "Scanned {}: {} found",
        root.display(),
        ui::plural(scanned.files.len(), "file")
    ));

    let keys = if args.by == vec![crate::regroup::GroupBy::Type]
        && !crate::config::defaults().by.is_empty()
    {
        crate::config::defaults().by.clone()
    } else {
        args.by.clone()
    };
    let reorganized = build_reorganize_plan(
        &root,
        &scanned,
        &keys,
        crate::cli::projects_or(args.projects),
        utc_offset(),
    );
    let emptied: Vec<_> = if args.keep_empty_dirs {
        Vec::new()
    } else {
        reorganized.emptied.clone()
    };
    let plan = &reorganized.plan;
    obs::plan_built("reorganize", plan);

    let mut outcome = Outcome::at(&root, args.dry_run).with_scan(&scanned, args.verbose);
    if plan.is_empty() && emptied.is_empty() {
        ui::print_skipped(&root, &plan.skipped, args.verbose);
        ui::success(&format!(
            "Nothing to do: this folder is already grouped by {}.",
            describe(&keys)
        ));
        return Ok(outcome);
    }
    outcome.plan = Some(PlanFile::of(plan, Operation::Reorganize));
    ui::print_plan(plan, args.verbose);
    ui::print_skipped(&root, &plan.skipped, args.verbose);

    if args.dry_run {
        crate::out!();
        if !emptied.is_empty() {
            ui::info(&format!(
                "{} would be left empty and deleted",
                ui::plural(emptied.len(), "folder")
            ));
        }
        ui::warn("Dry run: nothing was changed.");
        return Ok(outcome);
    }

    crate::out!();
    // The blast radius is stated plainly: this command moves far more than
    // `organize` does, and deletes folders, which `organize` never does.
    let mut prompt = format!(
        "Move {} and re-file by {}?",
        ui::plural(plan.moves.len(), "item"),
        describe(&keys)
    );
    if !emptied.is_empty() {
        prompt.push_str(&format!(
            " {} will be left empty and deleted.",
            ui::plural(emptied.len(), "folder")
        ));
    }
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(outcome);
    }

    let bar = ui::TransferBar::new("Reorganizing");
    let report = execute_and_clean(plan, Operation::Reorganize, &emptied, |p| bar.update(p));
    bar.finish();
    let report = report?;
    if report.dirs_removed > 0 {
        ui::info(&format!(
            "Removed {} that ended up empty",
            ui::plural(report.dirs_removed, "folder")
        ));
    }
    print_execution(&root, &report);
    outcome.execution = Some(report);
    Ok(outcome)
}
