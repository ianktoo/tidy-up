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
//! | decide | [`plan`], [`dedupe`], [`compare`] | pure, reviewable lists of moves |
//! | act | [`executor`], [`fsops`] | perform moves safely, never overwriting |
//! | remember | [`journal`] | append-only JSON Lines undo log |
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
pub mod parallel;
pub mod plan;
pub mod projects;
pub mod restore;
pub mod rules;
pub mod scan;
pub mod timefmt;
pub mod ui;

use clap::Parser;

/// Parses the process arguments and runs the requested command.
pub fn run() -> anyhow::Result<()> {
    commands::dispatch(cli::Cli::parse())
}
