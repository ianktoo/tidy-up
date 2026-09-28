//! Menu-driven mode, used when `tidy-up` is run with no sub-command.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Select};

use crate::{
    cli::{
        AnalyzeArgs, CompareArgs, DedupeArgs, DistributeArgs, FilterArgs, HistoryArgs,
        OrganizeArgs, PurgeArgs, ReorganizeArgs, SafetyArgs,
    },
    commands::{analyze, compare, dedupe, distribute, organize, reorganize, restore},
    disk::{parse_percent, parse_size},
    distribute::{Granularity, Layout, Prefer, StrategyKind},
    fsops::resolve_root,
    journal::Journal,
    plan::ProjectPolicy,
    regroup::GroupBy,
    restore::ConflictPolicy,
    ui,
};

/// Folder depth used when the user opts to include sub-folders.
const SUBFOLDER_DEPTH: u32 = 4;

const MENU: [&str; 11] = [
    "Organize by file type",
    "Re-file a badly organized folder (reorganize)",
    "Find duplicates and isolate them for review",
    "Compare folders (find overlap, merge or clean up)",
    "Analyze space usage",
    "Spread files across partitions (distribute)",
    "Restore a previous run (undo)",
    "Show history",
    "Delete isolated duplicates for good",
    "Choose a different folder",
    "Quit",
];

/// Runs the interactive menu loop.
pub fn run() -> Result<()> {
    ui::banner();
    if !ui::is_interactive() {
        bail!("no command given and no interactive terminal available; see `tidy-up --help`");
    }
    let mut root = choose_folder()?;
    loop {
        println!();
        let pick = Select::new()
            .with_prompt(format!("What would you like to do in {}?", root.display()))
            .items(&MENU)
            .default(0)
            .interact()?;
        let outcome = match pick {
            0 => organize_flow(&root),
            1 => reorganize_flow(&root),
            2 => dedupe_flow(&root),
            3 => compare_flow(&root),
            4 => analyze::run(&AnalyzeArgs {
                paths: vec![root.clone()],
                top: 10,
                json: false,
                duplicates: false,
            }),
            5 => distribute_flow(&root),
            6 => restore_flow(&root),
            7 => restore::history(&HistoryArgs { path: root.clone() }),
            8 => dedupe::purge(&PurgeArgs {
                path: root.to_path_buf(),
                safety: SafetyArgs::default(),
                yes: false,
            }),
            9 => choose_folder().map(|new_root| root = new_root),
            _ => return Ok(()),
        };
        if let Err(err) = outcome {
            ui::error(&err);
        }
    }
}

/// Offers common folders plus free-form entry and returns a validated absolute path.
fn choose_folder() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let mut options: Vec<(String, PathBuf)> =
        vec![("This folder (current directory)".into(), PathBuf::from("."))];
    if let Some(home) = home {
        for name in ["Downloads", "Desktop", "Documents"] {
            let path = home.join(name);
            if path.is_dir() {
                options.push((path.display().to_string(), path));
            }
        }
    }
    let mut labels: Vec<String> = options.iter().map(|(label, _)| label.clone()).collect();
    labels.push("Type a path…".into());

    let pick = Select::new()
        .with_prompt("Which folder should I work on?")
        .items(&labels)
        .default(0)
        .interact()?;
    let path = match options.get(pick) {
        Some((_, path)) => path.clone(),
        None => PathBuf::from(
            Input::<String>::new()
                .with_prompt("Folder path")
                .interact_text()?,
        ),
    };
    Ok(resolve_root(&path)?)
}

fn organize_flow(root: &Path) -> Result<()> {
    let ignore_ext: String = Input::new()
        .with_prompt("File extensions to leave alone (comma-separated, blank for none)")
        .allow_empty(true)
        .interact_text()?;
    let include_shortcuts = Confirm::new()
        .with_prompt("Also organize shortcuts (.lnk, .url)?")
        .default(false)
        .interact()?;
    let subfolders = Confirm::new()
        .with_prompt("Also sort files inside sub-folders?")
        .default(false)
        .interact()?;
    let move_projects = Confirm::new()
        .with_prompt("Gather detected code/git projects into a Projects folder?")
        .default(false)
        .interact()?;

    organize::run(&OrganizeArgs {
        path: root.to_path_buf(),
        safety: SafetyArgs::default(),
        filter: FilterArgs {
            ignore_ext: ignore_ext.split(',').map(str::to_owned).collect(),
            include_shortcuts,
            ..Default::default()
        },
        depth: if subfolders { SUBFOLDER_DEPTH } else { 1 },
        projects: if move_projects {
            ProjectPolicy::Move
        } else {
            ProjectPolicy::Keep
        },
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

/// Asks how to group, then re-files the whole folder.
///
/// The menu never sets `--allow-system-folder`. If someone points it at a system
/// folder, the right answer is the refusal that names the flag.
fn reorganize_flow(root: &Path) -> Result<()> {
    const CHOICES: [(&str, &[GroupBy]); 6] = [
        ("By file type", &[GroupBy::Type]),
        ("By year", &[GroupBy::Year]),
        ("By year, then file type", &[GroupBy::Year, GroupBy::Type]),
        ("By year, then month", &[GroupBy::Year, GroupBy::Month]),
        (
            "By file type, then extension",
            &[GroupBy::Type, GroupBy::Ext],
        ),
        ("By first letter of the name", &[GroupBy::Alpha]),
    ];
    let labels: Vec<&str> = CHOICES.iter().map(|(label, _)| *label).collect();
    let pick = Select::new()
        .with_prompt("How should the files be grouped?")
        .items(&labels)
        .default(0)
        .interact()?;
    let preview = Confirm::new()
        .with_prompt("Show the plan first, without changing anything?")
        .default(true)
        .interact()?;

    reorganize::run(&ReorganizeArgs {
        path: root.to_path_buf(),
        by: CHOICES[pick].1.to_vec(),
        filter: FilterArgs::default(),
        safety: SafetyArgs::default(),
        depth: None,
        keep_empty_dirs: false,
        projects: ProjectPolicy::Keep,
        dry_run: preview,
        yes: false,
        verbose: false,
    })
}

fn dedupe_flow(root: &Path) -> Result<()> {
    dedupe::run(&DedupeArgs {
        path: root.to_path_buf(),
        safety: SafetyArgs::default(),
        filter: FilterArgs::default(),
        depth: None,
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

/// Asks where to move files to and how to split them, then runs `distribute` with `root`
/// as the source.
fn distribute_flow(root: &Path) -> Result<()> {
    println!(
        "Files will be moved OUT of {} into the folders you name.",
        root.display()
    );
    let mut to = Vec::new();
    loop {
        let prompt = if to.is_empty() {
            "Destination folder (e.g. a folder on another drive)"
        } else {
            "Another destination (blank to continue)"
        };
        let entry: String = Input::new()
            .with_prompt(prompt)
            .allow_empty(!to.is_empty())
            .interact_text()?;
        if entry.trim().is_empty() {
            break;
        }
        to.push(PathBuf::from(entry.trim().trim_matches('"')));
    }

    let mut ratio = Vec::new();
    let mut strategy = StrategyKind::Fill;
    if to.len() > 1 {
        let choices = [
            "Equalise how full the destinations end up (recommended)",
            "In proportion to their free space",
            "Equal shares",
            "A ratio I choose",
        ];
        match Select::new()
            .with_prompt("How should the files be split?")
            .items(&choices)
            .default(0)
            .interact()?
        {
            1 => strategy = StrategyKind::Free,
            2 => strategy = StrategyKind::Even,
            3 => {
                let text: String = Input::new()
                    .with_prompt(format!("Ratio, {} numbers such as 50,30,20", to.len()))
                    .interact_text()?;
                for part in text.split(',') {
                    ratio.push(
                        part.trim()
                            .parse::<f64>()
                            .map_err(|_| anyhow::anyhow!("`{}` is not a number", part.trim()))?,
                    );
                }
            }
            _ => {}
        }
    }

    let max_fill: String = Input::new()
        .with_prompt("Never fill a destination beyond what percent?")
        .default("90".into())
        .interact_text()?;
    let limit: String = Input::new()
        .with_prompt("Move at most how much? (e.g. 200GiB, blank for no limit)")
        .allow_empty(true)
        .interact_text()?;
    let organize = Confirm::new()
        .with_prompt("Sort files into category folders at the destination?")
        .default(false)
        .interact()?;

    distribute::run(&DistributeArgs {
        safety: SafetyArgs::default(),
        from: vec![root.to_path_buf()],
        to,
        ratio,
        strategy,
        limit: if limit.trim().is_empty() {
            None
        } else {
            Some(parse_size(&limit)?)
        },
        max_fill: parse_percent(&max_fill)?,
        min_free: 0,
        layout: if organize {
            Layout::Organize
        } else {
            Layout::Keep
        },
        granularity: Granularity::Item,
        prefer: Prefer::Largest,
        filter: FilterArgs::default(),
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

/// Collects extra folders to compare against `root` (which becomes the primary).
fn compare_flow(root: &Path) -> Result<()> {
    println!(
        "{} is the primary folder: its copies are kept.",
        root.display()
    );
    let mut paths = vec![root.to_path_buf()];
    loop {
        let extra: String = Input::new()
            .with_prompt("Another folder to compare (blank to start)")
            .allow_empty(true)
            .interact_text()?;
        if extra.trim().is_empty() {
            break;
        }
        paths.push(PathBuf::from(extra.trim().trim_matches('"')));
    }
    compare::run(&CompareArgs {
        paths,
        safety: SafetyArgs::default(),
        filter: FilterArgs::default(),
        depth: None,
        action: None,
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

fn restore_flow(root: &Path) -> Result<()> {
    let mut active = restore::active_journals(root)?;
    if active.is_empty() {
        ui::success("Nothing to restore: no active runs recorded for this folder.");
        return Ok(());
    }
    active.reverse(); // newest first
    let labels: Vec<String> = active.iter().map(describe).collect();
    let pick = Select::new()
        .with_prompt("Which run should be undone?")
        .items(&labels)
        .default(0)
        .interact()?;
    let mut journal = active.swap_remove(pick);
    restore::restore_one(root, &mut journal, ConflictPolicy::Rename, false, false)
}

fn describe(journal: &Journal) -> String {
    format!(
        "{}  {} · {}",
        journal.header.id,
        journal.header.operation,
        ui::plural(journal.move_count(), "move")
    )
}
