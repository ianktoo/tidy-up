//! Command-line interface definition (parsing only; behaviour lives in `commands`).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::{
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
    #[arg(short = 'x', long = "ignore-ext", value_name = "EXT", value_delimiter = ',')]
    pub ignore_ext: Vec<String>,
    /// Names or globs to ignore, comma-separated or repeated (e.g. `-i "*.bak,notes.txt"`).
    #[arg(short = 'i', long = "ignore", value_name = "PATTERN", value_delimiter = ',')]
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
            "organize", "D:/Downloads", "-x", "iso,tmp", "-i", "*.bak", "--depth", "3",
            "--projects", "move", "-n", "--include-shortcuts",
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
        let Some(Command::Organize(args)) = parse(&["o"]).command else { panic!() };
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
        assert!(rules.check("a.lnk", false).is_some(), "shortcuts skipped by default");
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
