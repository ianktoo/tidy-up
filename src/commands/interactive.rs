//! Menu-driven mode, used when `tidy-up` is run with no sub-command.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Select};

use crate::{
    api::Outcome,
    cli::{
        AnalyzeArgs, CompareArgs, DedupeArgs, DistributeArgs, HistoryArgs, OrganizeArgs, PurgeArgs,
        ReorganizeArgs,
    },
    commands::{
        analyze, compare, dedupe, distribute, organize, reorganize, restore,
        session::{Session, split_list},
    },
    disk::{parse_percent, parse_size},
    distribute::{Granularity, Layout, Prefer, StrategyKind},
    fsops::resolve_root,
    journal::Journal,
    regroup::GroupBy,
    restore::ConflictPolicy,
    ui,
};

/// Folder depth used when the user opts to include sub-folders.
const SUBFOLDER_DEPTH: u32 = 4;

const MENU: [&str; 12] = [
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
    "Change options",
    "Quit",
];

/// Runs the interactive menu loop.
pub fn run() -> Result<Outcome> {
    ui::banner();
    if !ui::is_interactive() {
        bail!("no command given and no interactive terminal available; see `tidy-up --help`");
    }
    let mut root = choose_folder()?;
    let mut session = Session::default();
    loop {
        crate::out!();
        session.show();
        let pick = Select::new()
            .with_prompt(format!("What would you like to do in {}?", root.display()))
            .items(&MENU)
            .default(0)
            .interact()?;
        let outcome = match pick {
            0 => organize_flow(&root, &session),
            1 => reorganize_flow(&root, &session),
            2 => dedupe_flow(&root, &session),
            3 => compare_flow(&root, &session),
            4 => analyze::run(&AnalyzeArgs {
                paths: vec![root.clone()],
                top: 10,
                duplicates: false,
            })
            .map(|_| ()),
            5 => distribute_flow(&root, &session),
            6 => restore_flow(&root, &session),
            7 => restore::history(&HistoryArgs { path: root.clone() }).map(|_| ()),
            8 => dedupe::purge(&PurgeArgs {
                path: root.to_path_buf(),
                safety: session.safety.clone(),
                yes: false,
            })
            .map(|_| ()),
            9 => choose_folder().map(|new_root| root = new_root),
            10 => session.edit(),
            _ => return Ok(Outcome::default()),
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

/// Asks only what is specific to this run; everything else comes from the
/// options the user set in the menu.
fn organize_flow(root: &Path, session: &Session) -> Result<()> {
    let mut filter = session.filter.clone();
    if filter.ignore_ext.is_empty() {
        let answer: String = Input::new()
            .with_prompt("File extensions to leave alone (comma-separated, blank for none)")
            .allow_empty(true)
            .interact_text()?;
        filter.ignore_ext = split_list(&answer);
    }
    let depth = match session.depth {
        Some(depth) => depth,
        None => {
            let subfolders = Confirm::new()
                .with_prompt("Also sort files inside sub-folders?")
                .default(false)
                .interact()?;
            if subfolders { SUBFOLDER_DEPTH } else { 1 }
        }
    };

    let _outcome = organize::run(&OrganizeArgs {
        path: root.to_path_buf(),
        safety: session.safety.clone(),
        filter,
        depth,
        projects: session.projects,
        dry_run: session.dry_run,
        yes: false,
        verbose: session.verbose,
    })?;
    Ok(())
}

/// Asks how to group, then re-files the whole folder.
///
/// The menu never sets `--allow-system-folder`. If someone points it at a system
/// folder, the right answer is the refusal that names the flag.
fn reorganize_flow(root: &Path, session: &Session) -> Result<()> {
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
    // This one moves far more than the others, so it offers the preview even
    // when the session has not asked for one.
    let preview = session.dry_run
        || Confirm::new()
            .with_prompt("Show the plan first, without changing anything?")
            .default(true)
            .interact()?;

    let _outcome = reorganize::run(&ReorganizeArgs {
        path: root.to_path_buf(),
        by: CHOICES[pick].1.to_vec(),
        filter: session.filter.clone(),
        safety: session.safety.clone(),
        depth: session.depth,
        keep_empty_dirs: false,
        projects: session.projects,
        dry_run: preview,
        yes: false,
        verbose: session.verbose,
    })?;
    Ok(())
}

fn dedupe_flow(root: &Path, session: &Session) -> Result<()> {
    dedupe::run(&DedupeArgs {
        path: root.to_path_buf(),
        safety: session.safety.clone(),
        filter: session.filter.clone(),
        depth: session.depth,
        dry_run: session.dry_run,
        yes: false,
        verbose: session.verbose,
    })
    .map(|_| ())
}

/// Asks where to move files to and how to split them, then runs `distribute` with `root`
/// as the source.
fn distribute_flow(root: &Path, session: &Session) -> Result<()> {
    crate::out!(
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
        filter: session.filter.clone(),
        safety: session.safety.clone(),
        dry_run: session.dry_run,
        yes: false,
        verbose: session.verbose,
    })
    .map(|_| ())
}

/// Collects extra folders to compare against `root` (which becomes the primary).
fn compare_flow(root: &Path, session: &Session) -> Result<()> {
    crate::out!(
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
        safety: session.safety.clone(),
        filter: session.filter.clone(),
        depth: session.depth,
        action: None,
        dry_run: session.dry_run,
        yes: false,
        verbose: session.verbose,
    })
    .map(|_| ())
}

fn restore_flow(root: &Path, session: &Session) -> Result<()> {
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
    restore::restore_one(
        root,
        &mut journal,
        ConflictPolicy::Rename,
        session.dry_run,
        false,
        // The menu never reaches outside the folder being restored; someone
        // who needs that can say so on the command line.
        false,
    )
}

fn describe(journal: &Journal) -> String {
    format!(
        "{}  {} · {}",
        journal.header.id,
        journal.header.operation,
        ui::plural(journal.move_count(), "move")
    )
}
