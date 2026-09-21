//! Command-line interface definition (parsing only; behaviour lives in `commands`).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::{
    compare::CompareAction,
    distribute::{Granularity, Layout, Prefer, StrategyKind},
    plan::ProjectPolicy,
    restore::ConflictPolicy,
    rules::IgnoreRules,
};

/// Top-level parser for the `tidy-up` command.
#[derive(Debug, Parser)]
#[command(
    name = "tidy-up",
    version,
    about = "Organize messy folders by file type, isolate duplicates, and undo it all.",
    long_about = "tidy-up sorts the files in a folder (Downloads, Desktop, a whole drive) into \
                  category folders, finds duplicate files and quarantines them for review, and \
                  journals every move so any run can be restored later.\n\n\
                  Run with no command for an interactive menu.",
    after_help = "Ignore-file syntax (one entry per line):\n  \
                  .iso        ignore an extension\n  \
                  *.tmp       ignore by glob\n  \
                  notes.txt   ignore an exact name\n  \
                  # comment"
)]
pub struct Cli {
    /// What to do; omit for the interactive menu.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available sub-commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sort files into category folders (Images, Documents, 3D Models, ...).
    #[command(visible_alias = "o")]
    Organize(OrganizeArgs),
    /// Find duplicate files and move the extra copies into `_Duplicates/` for review.
    #[command(visible_alias = "d")]
    Dedupe(DedupeArgs),
    /// Compare two or more folders by content, then leave, move, merge or delete the overlap.
    #[command(visible_alias = "c")]
    Compare(CompareArgs),
    /// Show where the space goes: partition usage, biggest files and folders, file types, age.
    #[command(visible_alias = "a")]
    Analyze(AnalyzeArgs),
    /// Move files from full folders into other places (partitions) by ratio, free space or fill level.
    #[command(visible_alias = "x")]
    Distribute(DistributeArgs),
    /// Undo a previous run and put files back where they were.
    #[command(visible_alias = "r")]
    Restore(RestoreArgs),
    /// List previous runs recorded for a folder.
    #[command(visible_alias = "h")]
    History(HistoryArgs),
    /// Permanently delete the `_Duplicates/` folder (asks first).
    Purge(PurgeArgs),
}

/// Flags that decide which entries are left untouched.
#[derive(Debug, Args, Clone, Default)]
pub struct FilterArgs {
    /// Extensions to ignore, comma-separated or repeated (e.g. `-x iso,tmp`).
    #[arg(
        short = 'x',
        long = "ignore-ext",
        value_name = "EXT",
        value_delimiter = ','
    )]
    pub ignore_ext: Vec<String>,
    /// Names or globs to ignore, comma-separated or repeated (e.g. `-i "*.bak,notes.txt"`).
    #[arg(
        short = 'i',
        long = "ignore",
        value_name = "PATTERN",
        value_delimiter = ','
    )]
    pub ignore: Vec<String>,
    /// Text file with one ignore entry per line (may be repeated).
    #[arg(short = 'f', long = "ignore-file", value_name = "FILE")]
    pub ignore_file: Vec<PathBuf>,
    /// Also process shortcuts (.lnk, .url, .webloc, .desktop); skipped by default.
    #[arg(long)]
    pub include_shortcuts: bool,
    /// Also process hidden files and folders; skipped by default.
    #[arg(long)]
    pub include_hidden: bool,
}

impl FilterArgs {
    /// Builds the [`IgnoreRules`] these flags describe.
    pub fn to_rules(&self) -> crate::error::Result<IgnoreRules> {
        let mut rules = IgnoreRules::new()
            .include_shortcuts(self.include_shortcuts)
            .include_hidden(self.include_hidden);
        self.ignore_ext.iter().for_each(|e| rules.add_extension(e));
        self.ignore.iter().for_each(|p| rules.add_entry(p));
        for file in &self.ignore_file {
            rules.load_file(file)?;
        }
        Ok(rules)
    }
}

/// Arguments for `organize`.
#[derive(Debug, Args, Clone)]
pub struct OrganizeArgs {
    /// Folder to organize.
    #[arg(default_value = ".")]
    pub path: PathBuf,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// How many folder levels to look through (1 = only files directly inside).
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub depth: u32,
    /// What to do with detected code/git projects.
    #[arg(long, value_enum, default_value_t = ProjectPolicy::Keep)]
    pub projects: ProjectPolicy,
    /// Show the plan without changing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
    /// List every planned move and every skipped item.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Arguments for `dedupe`.
#[derive(Debug, Args, Clone)]
pub struct DedupeArgs {
    /// Folder to search for duplicates.
    #[arg(default_value = ".")]
    pub path: PathBuf,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// How many folder levels to search (default: all).
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..))]
    pub depth: Option<u32>,
    /// Show what was found without moving anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
    /// List every duplicate.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Arguments for `compare`.
#[derive(Debug, Args, Clone)]
pub struct CompareArgs {
    /// Folders to compare. The FIRST one is the primary: its copy of a duplicated file is
    /// kept, moves go to its `_Duplicates/`, and merges gather everything into it.
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// How many folder levels to search inside each folder (default: all).
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..))]
    pub depth: Option<u32>,
    /// What to do with duplicated content; asked interactively if omitted
    /// (without a terminal, `leave` is assumed).
    #[arg(short, long, value_enum)]
    pub action: Option<CompareAction>,
    /// Show the plan without changing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
    /// List every duplicate group.
    #[arg(short, long)]
    pub verbose: bool,
}

fn size_arg(text: &str) -> Result<u64, String> {
    crate::disk::parse_size(text).map_err(|e| e.to_string())
}

fn percent_arg(text: &str) -> Result<f64, String> {
    crate::disk::parse_percent(text).map_err(|e| e.to_string())
}

/// Arguments for `analyze`.
#[derive(Debug, Args, Clone)]
pub struct AnalyzeArgs {
    /// Folders or drives to analyze.
    #[arg(default_value = ".")]
    pub paths: Vec<PathBuf>,
    /// How many entries to list in each "largest" section.
    #[arg(short, long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(0..=1000))]
    pub top: u32,
    /// Print machine-readable JSON instead of a report.
    #[arg(long)]
    pub json: bool,
    /// Also measure space wasted by duplicate files (reads file contents, so it is slower).
    #[arg(long)]
    pub duplicates: bool,
}

/// Arguments for `distribute`.
#[derive(Debug, Args, Clone)]
pub struct DistributeArgs {
    /// Folders to move files out of.
    #[arg(long, required = true, num_args = 1.., value_name = "FOLDER")]
    pub from: Vec<PathBuf>,
    /// Folders to move files into, typically on different partitions (created if missing).
    /// The undo journal is kept in the first one.
    #[arg(long, required = true, num_args = 1.., value_name = "FOLDER")]
    pub to: Vec<PathBuf>,
    /// Split the moved data in these proportions, one number per destination (e.g. 50,30,20).
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "N,N,...",
        conflicts_with = "strategy"
    )]
    pub ratio: Vec<f64>,
    /// How to split when no ratio is given: fill = equalise how full the destinations end up,
    /// free = proportional to free space, even = equal shares.
    #[arg(long, value_enum, default_value_t = StrategyKind::Fill)]
    pub strategy: StrategyKind,
    /// Move at most this much in total, e.g. 200GiB or 500M (units are binary).
    #[arg(long, value_name = "SIZE", value_parser = size_arg)]
    pub limit: Option<u64>,
    /// Never fill a destination's partition beyond this percentage.
    #[arg(long, default_value = "90", value_name = "PERCENT", value_parser = percent_arg)]
    pub max_fill: f64,
    /// Always keep at least this much free on every destination, e.g. 20GiB.
    #[arg(long, default_value = "0", value_name = "SIZE", value_parser = size_arg)]
    pub min_free: u64,
    /// Where files land: keep the folder structure, or sort into category folders.
    #[arg(long, value_enum, default_value_t = Layout::Keep)]
    pub layout: Layout,
    /// Unit of moving: whole top-level items (folders stay together) or individual files.
    #[arg(long, value_enum, default_value_t = Granularity::Item)]
    pub granularity: Granularity,
    /// Which items to move first when a limit applies or space is short.
    #[arg(long, value_enum, default_value_t = Prefer::Largest)]
    pub prefer: Prefer,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// Show the plan without changing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
    /// List every item and every skipped entry.
    #[arg(short, long)]
    pub verbose: bool,
}

/// Arguments for `restore`.
#[derive(Debug, Args, Clone)]
pub struct RestoreArgs {
    /// Folder that was organized.
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Journal id (or unique prefix) to restore; default is the most recent active one.
    #[arg(long, conflicts_with = "all")]
    pub id: Option<String>,
    /// Restore every active journal, newest first.
    #[arg(long)]
    pub all: bool,
    /// What to do if a file's original name is taken again.
    #[arg(long, value_enum, default_value_t = ConflictPolicy::Rename)]
    pub on_conflict: ConflictPolicy,
    /// Show what would be restored without changing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
}

/// Arguments for `history`.
#[derive(Debug, Args, Clone)]
pub struct HistoryArgs {
    /// Folder to inspect.
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

/// Arguments for `purge`.
#[derive(Debug, Args, Clone)]
pub struct PurgeArgs {
    /// Folder whose `_Duplicates/` should be deleted.
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Do not ask for confirmation.
    #[arg(short, long)]
    pub yes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("tidy-up").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_command_means_interactive() {
        assert!(parse(&[]).command.is_none());
    }

    #[test]
    fn organize_flags_parse() {
        let Some(Command::Organize(args)) = parse(&[
            "organize",
            "D:/Downloads",
            "-x",
            "iso,tmp",
            "-i",
            "*.bak",
            "--depth",
            "3",
            "--projects",
            "move",
            "-n",
            "--include-shortcuts",
        ])
        .command
        else {
            panic!("expected organize");
        };
        assert_eq!(args.path, PathBuf::from("D:/Downloads"));
        assert_eq!(args.filter.ignore_ext, ["iso", "tmp"]);
        assert_eq!(args.filter.ignore, ["*.bak"]);
        assert_eq!(args.depth, 3);
        assert_eq!(args.projects, ProjectPolicy::Move);
        assert!(args.dry_run && args.filter.include_shortcuts && !args.yes);
    }

    #[test]
    fn defaults_are_safe() {
        let Some(Command::Organize(args)) = parse(&["o"]).command else {
            panic!()
        };
        assert_eq!(args.path, PathBuf::from("."));
        assert_eq!(args.depth, 1);
        assert_eq!(args.projects, ProjectPolicy::Keep);
        assert!(!args.yes && !args.dry_run);
    }

    #[test]
    fn depth_zero_is_rejected() {
        assert!(Cli::try_parse_from(["tidy-up", "organize", "--depth", "0"]).is_err());
    }

    #[test]
    fn compare_takes_many_folders_and_an_action() {
        let Some(Command::Compare(args)) = parse(&[
            "compare", "a", "b", "c", "--action", "merge", "-n", "-x", "tmp",
        ])
        .command
        else {
            panic!("expected compare");
        };
        assert_eq!(
            args.paths,
            [PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]
        );
        assert_eq!(args.action, Some(CompareAction::Merge));
        assert!(args.dry_run && !args.yes);
        assert_eq!(args.filter.ignore_ext, ["tmp"]);
        assert!(
            Cli::try_parse_from(["tidy-up", "compare"]).is_err(),
            "needs a folder"
        );
        assert!(Cli::try_parse_from(["tidy-up", "compare", "a", "--action", "shred"]).is_err());
    }

    #[test]
    fn analyze_defaults_and_flags() {
        let Some(Command::Analyze(a)) = parse(&["analyze"]).command else {
            panic!("expected analyze");
        };
        assert_eq!(a.paths, [PathBuf::from(".")]);
        assert_eq!((a.top, a.json, a.duplicates), (10, false, false));
        let Some(Command::Analyze(a)) =
            parse(&["a", "C:/", "D:/", "--top", "3", "--json", "--duplicates"]).command
        else {
            panic!("expected analyze");
        };
        assert_eq!(a.paths.len(), 2);
        assert_eq!((a.top, a.json, a.duplicates), (3, true, true));
    }

    #[test]
    fn distribute_parses_sizes_ratios_and_multiple_folders() {
        let Some(Command::Distribute(d)) = parse(&[
            "distribute",
            "--from",
            "a",
            "b",
            "--to",
            "x",
            "y",
            "z",
            "--ratio",
            "50,30,20",
            "--limit",
            "200GiB",
            "--max-fill",
            "85%",
            "--min-free",
            "20G",
            "--layout",
            "organize",
            "--prefer",
            "oldest",
            "-n",
        ])
        .command
        else {
            panic!("expected distribute");
        };
        assert_eq!(d.from, [PathBuf::from("a"), PathBuf::from("b")]);
        assert_eq!(d.to.len(), 3);
        assert_eq!(d.ratio, [50.0, 30.0, 20.0]);
        assert_eq!(d.limit, Some(200 * 1024u64.pow(3)));
        assert_eq!(d.max_fill, 0.85);
        assert_eq!(d.min_free, 20 * 1024u64.pow(3));
        assert_eq!((d.layout, d.prefer), (Layout::Organize, Prefer::Oldest));
        assert!(d.dry_run);
    }

    #[test]
    fn distribute_defaults_are_conservative() {
        let Some(Command::Distribute(d)) =
            parse(&["distribute", "--from", "a", "--to", "b"]).command
        else {
            panic!("expected distribute");
        };
        assert_eq!(d.max_fill, 0.9);
        assert_eq!((d.min_free, d.limit), (0, None));
        assert_eq!(d.strategy, StrategyKind::Fill);
        assert_eq!((d.layout, d.granularity), (Layout::Keep, Granularity::Item));
        assert!(d.ratio.is_empty() && !d.yes && !d.dry_run);
    }

    #[test]
    fn distribute_rejects_bad_input() {
        let bad = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("tidy-up").chain(args.iter().copied())).is_err()
        };
        assert!(bad(&["distribute", "--to", "b"]), "needs --from");
        assert!(bad(&["distribute", "--from", "a"]), "needs --to");
        assert!(bad(&[
            "distribute",
            "--from",
            "a",
            "--to",
            "b",
            "--limit",
            "lots"
        ]));
        assert!(bad(&[
            "distribute",
            "--from",
            "a",
            "--to",
            "b",
            "--max-fill",
            "150"
        ]));
        assert!(bad(&[
            "distribute",
            "--from",
            "a",
            "--to",
            "b",
            "--ratio",
            "1,2",
            "--strategy",
            "even"
        ]));
    }

    #[test]
    fn restore_id_and_all_conflict() {
        assert!(Cli::try_parse_from(["tidy-up", "restore", "--id", "x", "--all"]).is_err());
    }

    #[test]
    fn filter_args_build_rules() {
        let filter = FilterArgs {
            ignore_ext: vec!["ISO".into()],
            ignore: vec!["*.bak".into(), ".tar.gz".into()],
            ..Default::default()
        };
        let rules = filter.to_rules().unwrap();
        assert!(rules.check("a.iso", false).is_some());
        assert!(rules.check("a.bak", false).is_some());
        assert!(rules.check("a.tar.gz", false).is_some());
        assert!(
            rules.check("a.lnk", false).is_some(),
            "shortcuts skipped by default"
        );
        assert!(rules.check("a.txt", false).is_none());
    }

    #[test]
    fn missing_ignore_file_is_an_error() {
        let filter = FilterArgs {
            ignore_file: vec![PathBuf::from("/no/such/ignore.txt")],
            ..Default::default()
        };
        assert!(filter.to_rules().is_err());
    }
}
