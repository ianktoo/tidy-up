//! `tidy-up apply`
//!
//! Carries out a plan that an earlier `--dry-run --json` produced. This is the
//! half of "propose, review, apply" that the rest of the tool was already built
//! for: a [`crate::plan::Plan`] is pure data, so it can be written out, read by
//! a person or an agent, and executed later without anything being re-decided.
//!
//! The plan file is a trust boundary. It is JSON on disk, so it may have been
//! hand-edited or produced by something acting on instructions from elsewhere.
//! Three things are checked before anything moves:
//!
//! 1. every path in it is absolute and inside the plan's own root
//!    ([`PlanFile::into_plan`]),
//! 2. the root passes the system-folder guard, exactly as it would have on the
//!    original run,
//! 3. the root still exists and still resolves to the same place.
//!
//! Without the first, `apply` would be a way to move any file anywhere.

use anyhow::Result;

use crate::{
    api::{ErrorCode, Outcome, PlanFile, Refused},
    cli::ApplyArgs,
    commands::{guard::Guard, print_execution},
    executor::execute,
    ui,
};

/// Reads a plan, checks it, confirms it and carries it out.
pub fn run(args: &ApplyArgs) -> Result<Outcome> {
    let text = std::fs::read_to_string(&args.plan).map_err(|source| crate::error::Error::Io {
        path: args.plan.clone(),
        source,
    })?;
    let file = PlanFile::parse(&text)?;
    let operation = file.operation;
    let created_at = file.created_at;
    let tool = file.tool.clone();
    let plan = file.into_plan()?;

    // The guard runs on the plan's root, not on anything the file claims about
    // itself: a plan cannot talk its way past it.
    let root = Guard::write(&args.safety, args.yes, args.dry_run).root(&plan.root)?;
    if root != plan.root {
        return Err(Refused::at(
            ErrorCode::InvalidPlan,
            &plan.root,
            format!(
                "the plan is for {}, which now resolves to {}",
                plan.root.display(),
                root.display()
            ),
        )
        .into());
    }

    ui::info(&format!(
        "Plan from tidy-up {tool}, {}, {} for {}",
        crate::timefmt::format_utc(created_at),
        ui::plural(plan.moves.len(), "move"),
        root.display()
    ));
    ui::print_plan(&plan, args.verbose);

    let mut outcome = Outcome::at(&root, args.dry_run);
    outcome.plan = Some(PlanFile::of(&plan, operation));

    if args.dry_run {
        crate::out!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(outcome);
    }

    // Some of what the plan describes may have moved, been deleted or been
    // created since it was written. Nothing here assumes otherwise: the
    // executor re-checks each destination and records whatever it could not do.
    crate::out!();
    let prompt = format!("Carry out {}?", ui::plural(plan.moves.len(), "move"));
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(outcome);
    }

    let bar = ui::TransferBar::new("Applying");
    let report = execute(&plan, operation, |p| bar.update(p));
    bar.finish();
    let report = report?;
    print_execution(&root, &report);
    outcome.execution = Some(report);
    Ok(outcome)
}
