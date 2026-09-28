//! # tidy-up
//!
//! Organizes messy folders by file type, isolates duplicate files for review, and
//! journals every change so any run can be undone.
//!
//! The crate is layered so each stage can be reused on its own:
//!
//! | stage | module | responsibility |
//! |-------|--------|----------------|
//! | classify | [`category`], [`rules`], [`projects`] | what a file is, and what to leave alone |
//! | discover | [`scan`] | walk a folder into eligible files + skipped entries |
//! | decide | [`plan`], [`dedupe`], [`compare`], [`regroup`] | pure, reviewable lists of moves |
//! | act | [`executor`], [`fsops`] | perform moves safely, never overwriting |
//! | remember | [`journal`] | append-only JSON Lines undo log |
//! | guard | [`safety`] | refuse to reorganize folders the system manages |
//! | survive | [`outcome`] | collect failures, skip the item, keep going |
//! | observe | [`obs`] | a JSON Lines record of what a run decided |
//! | undo | [`restore`] | reverse a journal |
//! | present | [`cli`], [`commands`], [`ui`] | arguments, flows, terminal output |
//!
//! ```no_run
//! use tidy_up::{
//!     executor::execute, journal::Operation, plan::{build_organize_plan, organize_skip_dirs, ProjectPolicy},
//!     rules::IgnoreRules, scan::{scan, ScanOptions},
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = std::path::Path::new("C:/Users/me/Downloads");
//! let options = ScanOptions { max_depth: 1, rules: IgnoreRules::new(), skip_root_dirs: organize_skip_dirs() };
//! let plan = build_organize_plan(root, &scan(root, &options)?, ProjectPolicy::Keep);
//! let report = execute(&plan, Operation::Organize, |_progress| {})?;
//! println!("journal {}: {} moved", report.journal_id, report.moved);
//! # Ok(()) }
//! ```

pub mod analyze;
pub mod category;
pub mod cli;
pub mod commands;
pub mod compare;
pub mod dedupe;
pub mod disk;
pub mod distribute;
pub mod error;
pub mod executor;
pub mod fsops;
pub mod journal;
pub mod obs;
pub mod outcome;
pub mod parallel;
pub mod plan;
pub mod projects;
pub mod regroup;
pub mod restore;
pub mod rules;
pub mod safety;
pub mod scan;
pub mod timefmt;
pub mod ui;

use clap::Parser;

/// Parses the process arguments and runs the requested command.
///
/// The run log is opened here and closed here, so the closing event is written
/// whatever the command did, including on the error path.
pub fn run() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    obs::start(cli.command_name(), cli.log);
    let result = commands::dispatch(cli);
    let status = match &result {
        Ok(()) => obs::Status::Ok,
        Err(e) if e.to_string().starts_with("Cancelled") => obs::Status::Cancelled,
        Err(e) if e.to_string().starts_with("refusing") => obs::Status::Blocked,
        Err(_) => obs::Status::Error,
    };
    if let Some(path) = obs::finish(status) {
        ui::hint(&format!("Run log: {}", path.display()));
    }
    result
}
