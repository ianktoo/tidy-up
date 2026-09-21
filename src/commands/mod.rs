//! Command implementations: glue between the CLI, the engine and the terminal UI.

pub mod compare;
pub mod dedupe;
pub mod interactive;
pub mod organize;
pub mod restore;

use std::path::Path;

use anyhow::Result;

use crate::{
    cli::{Cli, Command},
    executor::ExecutionReport,
    ui,
};

/// How many failures to list before summarising the rest.
const FAILURE_LIMIT: usize = 10;

/// Runs whichever command the user asked for (interactive menu if none).
pub fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        None => interactive::run(),
        Some(Command::Organize(args)) => organize::run(&args),
        Some(Command::Dedupe(args)) => dedupe::run(&args),
        Some(Command::Compare(args)) => compare::run(&args),
        Some(Command::Restore(args)) => restore::run(&args),
        Some(Command::History(args)) => restore::history(&args),
        Some(Command::Purge(args)) => dedupe::purge(&args),
    }
}

/// Prints the outcome of an executed plan and how to undo it.
pub(crate) fn print_execution(root: &Path, report: &ExecutionReport) {
    ui::heading("Done");
    ui::success(&format!(
        "Moved {} ({})",
        ui::plural(report.moved, "item"),
        ui::format_size(report.bytes)
    ));
    if !report.failed.is_empty() {
        ui::warn(&format!("{} could not be moved:", ui::plural(report.failed.len(), "item")));
        for (path, reason) in report.failed.iter().take(FAILURE_LIMIT) {
            ui::hint(&format!("{}: {reason}", ui::rel(root, path)));
        }
        if report.failed.len() > FAILURE_LIMIT {
            ui::hint(&format!("… and {} more", report.failed.len() - FAILURE_LIMIT));
        }
    }
    if !report.journal_id.is_empty() {
        ui::info(&format!("Recorded as run {}", report.journal_id));
        ui::hint(&format!("Undo with: tidy-up restore \"{}\"", root.display()));
    }
}
