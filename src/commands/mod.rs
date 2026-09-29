//! Command implementations: glue between the CLI, the engine and the terminal UI.

pub mod analyze;
pub mod apply;
pub mod compare;
pub mod dedupe;
pub mod distribute;
pub(crate) mod guard;
pub mod interactive;
pub mod mcp;
pub mod organize;
pub mod reorganize;
pub mod restore;
pub mod session;
pub mod show;

use std::path::Path;

use anyhow::Result;

use crate::{
    api::Outcome,
    cli::{Cli, Command},
    executor::ExecutionReport,
    obs,
    outcome::Problems,
    scan::ScanResult,
    ui,
};

/// Skipped entries counted by reason, for the run log.
pub(crate) fn skip_counts(scan: &ScanResult) -> std::collections::BTreeMap<&'static str, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for skipped in &scan.skipped {
        *counts.entry(skipped.reason.key()).or_insert(0) += 1;
    }
    counts
}

/// How many individual problems to list before summarising the rest.
const FAILURE_LIMIT: usize = 10;

/// Prints everything that went wrong, grouped by cause.
///
/// Grouping matters: twenty lines of `Permission denied` teach nobody anything,
/// while "18 permission denied, pick a folder you own" is something a person can
/// act on. Individual paths follow, capped unless `--verbose` is given.
pub(crate) fn print_problems(root: &Path, problems: &Problems, verbose: bool) {
    if problems.is_empty() {
        return;
    }
    ui::warn(&format!(
        "{} could not be processed:",
        ui::plural(problems.len(), "item")
    ));
    for (cause, count) in problems.by_cause() {
        let hint = cause.hint().map(|h| format!("  ({h})")).unwrap_or_default();
        ui::hint(&format!("{:<28} {count:>5}{hint}", cause.label()));
    }
    let limit = if verbose { usize::MAX } else { FAILURE_LIMIT };
    for problem in problems.iter().take(limit) {
        ui::hint(&format!(
            "{}: {}",
            ui::rel(root, &problem.path),
            problem.message
        ));
    }
    if problems.len() > limit {
        ui::hint(&format!(
            "… and {} more (use --verbose to list them all)",
            problems.len() - limit
        ));
    }
}

/// Runs whichever command the user asked for (interactive menu if none).
/// Runs whichever command the user asked for, and hands back what it did.
///
/// `--strict` is applied by the caller, from the returned outcome, so that the
/// JSON rendering still describes a run that is about to exit non-zero.
pub fn dispatch(cli: Cli) -> Result<Outcome> {
    match cli.command {
        None => interactive::run(),
        Some(Command::Organize(args)) => organize::run(&args),
        Some(Command::Reorganize(args)) => reorganize::run(&args),
        Some(Command::Dedupe(args)) => dedupe::run(&args),
        Some(Command::Compare(args)) => compare::run(&args),
        Some(Command::Analyze(args)) => analyze::run(&args),
        Some(Command::Distribute(args)) => distribute::run(&args),
        Some(Command::Apply(args)) => apply::run(&args),
        Some(Command::Restore(args)) => restore::run(&args),
        Some(Command::Mcp(args)) => mcp::run(&args),
        Some(Command::Show(args)) => show::run(&args),
        Some(Command::History(args)) => restore::history(&args),
        Some(Command::Purge(args)) => dedupe::purge(&args),
    }
}

/// Prints the outcome of an executed plan and how to undo it.
pub(crate) fn print_execution(root: &Path, report: &ExecutionReport) {
    obs::metrics(|m| m.absorb_execution(report));
    obs::event(obs::Event::Execute {
        journal_id: report.journal_id.clone(),
        moved: report.moved,
        bytes: report.bytes,
        dirs_removed: report.dirs_removed,
        failed: report.problems.len(),
        ms: 0,
    });
    ui::heading("Done");
    ui::success(&format!(
        "Moved {} ({})",
        ui::plural(report.moved, "item"),
        ui::format_size(report.bytes)
    ));
    // Already counted above, so only the printing is left to do here.
    if !report.problems.is_empty() {
        print_problems(root, &report.problems, false);
    }
    if !report.journal_id.is_empty() {
        ui::info(&format!("Recorded as run {}", report.journal_id));
        ui::hint(&format!(
            "Undo with: tidy-up restore \"{}\"",
            root.display()
        ));
    }
}
