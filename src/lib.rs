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
//! | report | [`api`] | the same values as JSON, plus exit codes and plan files |
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
pub mod api;
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

/// Parses the process arguments, runs the requested command, and reports.
///
/// Everything that decides how the process ends lives here: which renderer
/// gets the result, what the run log records, and what the exit code is. The
/// commands themselves return an [`api::Outcome`] and print prose along the
/// way; they do not decide any of that.
pub fn run() -> api::Exit {
    let cli = cli::Cli::parse();
    let command = cli.command_name();
    let (json, strict) = (cli.json, cli.strict);
    // Decided before anything runs, because a half-silenced run would emit
    // prose and JSON on the same stream.
    ui::set_quiet(json);

    obs::start(command, cli.log);
    let result = commands::dispatch(cli);

    let (status, exit) = match &result {
        Ok(outcome) if strict && outcome.had_problems() => (obs::Status::Ok, api::Exit::Skipped),
        Ok(_) => (obs::Status::Ok, api::Exit::Ok),
        Err(error) => {
            let code = api::classify(error).code;
            let status = match code {
                api::ErrorCode::Cancelled => obs::Status::Cancelled,
                api::ErrorCode::SystemFolder | api::ErrorCode::NotWritable => obs::Status::Blocked,
                _ => obs::Status::Error,
            };
            (status, code.exit())
        }
    };

    let log_path = obs::finish(status);

    match (&result, json) {
        (Ok(outcome), true) => api::Envelope::ok(command, outcome.clone()).print(),
        (Err(error), true) => api::Envelope::from_error(command, error).print(),
        (Ok(_), false) => {
            if let Some(path) = log_path {
                ui::hint(&format!("Run log: {}", path.display()));
            }
        }
        (Err(error), false) => {
            // A cancelled run is the user getting what they asked for, and the
            // command has already said so in its own words.
            if api::classify(error).code != api::ErrorCode::Cancelled {
                ui::error(error);
            }
        }
    }
    exit
}
