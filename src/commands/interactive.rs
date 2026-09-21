//! Menu-driven mode, used when `tidy-up` is run with no sub-command.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Select};

use crate::{
    cli::{DedupeArgs, FilterArgs, HistoryArgs, OrganizeArgs, PurgeArgs},
    commands::{dedupe, organize, restore},
    fsops::resolve_root,
    journal::Journal,
    plan::ProjectPolicy,
    restore::ConflictPolicy,
    ui,
};

/// Folder depth used when the user opts to include sub-folders.
const SUBFOLDER_DEPTH: u32 = 4;

const MENU: [&str; 7] = [
    "Organize by file type",
    "Find duplicates and isolate them for review",
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
            1 => dedupe_flow(&root),
            2 => restore_flow(&root),
            3 => restore::history(&HistoryArgs { path: root.clone() }),
            4 => dedupe::purge(&PurgeArgs { path: root.to_path_buf(), yes: false }),
            5 => choose_folder().map(|new_root| root = new_root),
            _ => return Ok(()),
        };
        if let Err(err) = outcome {
            ui::error(&err);
        }
    }
}

/// Offers common folders plus free-form entry and returns a validated absolute path.
fn choose_folder() -> Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let mut options: Vec<(String, PathBuf)> = vec![("This folder (current directory)".into(), PathBuf::from("."))];
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
        None => PathBuf::from(Input::<String>::new().with_prompt("Folder path").interact_text()?),
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
        filter: FilterArgs {
            ignore_ext: ignore_ext.split(',').map(str::to_owned).collect(),
            include_shortcuts,
            ..Default::default()
        },
        depth: if subfolders { SUBFOLDER_DEPTH } else { 1 },
        projects: if move_projects { ProjectPolicy::Move } else { ProjectPolicy::Keep },
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

fn dedupe_flow(root: &Path) -> Result<()> {
    dedupe::run(&DedupeArgs {
        path: root.to_path_buf(),
        filter: FilterArgs::default(),
        depth: None,
        dry_run: false,
        yes: false,
        verbose: false,
    })
}

fn restore_flow(root: &Path) -> Result<()> {
    let mut active = restore::active_journals(root)?;
    if active.is_empty() {
        ui::success("Nothing to restore — no active runs recorded for this folder.");
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
