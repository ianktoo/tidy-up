//! Options the interactive menu applies to every action.
//!
//! Every option here has a command-line flag, but a person who launched
//! `tidy-up` by double-clicking it, or who just ran it with no sub-command, has
//! no command line to put flags on. Without somewhere to set them, the menu is
//! stuck with defaults: it cannot preview, cannot include hidden files, cannot
//! ignore anything, and cannot reach a folder the guard refuses.
//!
//! These are held for the session only. tidy-up keeps no configuration file
//! outside the folders it processes, and this does not change that: closing the
//! program forgets them.

use anyhow::Result;
use dialoguer::{Confirm, Input, Select};

use crate::{
    cli::{FilterArgs, SafetyArgs},
    obs,
    plan::ProjectPolicy,
    ui,
};

/// How many levels the deepest option scans. Large enough to mean "all of them".
const ALL_LEVELS: u32 = u32::MAX;

/// The current options, shown above the menu and applied to every action.
#[derive(Debug, Clone, Default)]
pub struct Session {
    /// What to leave alone: extensions, globs, shortcuts, hidden items.
    pub filter: FilterArgs,
    /// Whether the system-folder guard may be overridden.
    pub safety: SafetyArgs,
    /// Levels to scan. `None` means each command decides, which is one level for
    /// `organize` and all of them for the rest.
    pub depth: Option<u32>,
    /// What to do with detected code projects.
    pub projects: ProjectPolicy,
    /// Show the plan without changing anything.
    pub dry_run: bool,
    /// List every move and every skipped item rather than a sample.
    pub verbose: bool,
    /// Record a machine-readable log of each run.
    pub log: bool,
}

impl Session {
    /// The depth to pass a command whose own default is one level.
    pub fn depth_or(&self, default: u32) -> u32 {
        self.depth.unwrap_or(default)
    }

    /// One line describing the options that are not at their default, for the
    /// menu header. Empty when everything is default, so the common case says
    /// nothing at all.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.dry_run {
            parts.push("preview only".into());
        }
        match self.depth {
            Some(ALL_LEVELS) => parts.push("all sub-folders".into()),
            Some(n) => parts.push(format!("{n} level(s) deep")),
            None => {}
        }
        if self.filter.include_hidden {
            parts.push("including hidden".into());
        }
        if self.filter.include_shortcuts {
            parts.push("including shortcuts".into());
        }
        if !self.filter.ignore_ext.is_empty() {
            parts.push(format!("ignoring .{}", self.filter.ignore_ext.join(", .")));
        }
        if !self.filter.ignore.is_empty() {
            parts.push(format!("ignoring {}", self.filter.ignore.join(", ")));
        }
        if !self.filter.ignore_file.is_empty() {
            parts.push(format!("{} ignore file(s)", self.filter.ignore_file.len()));
        }
        if self.projects == ProjectPolicy::Move {
            parts.push("gathering projects".into());
        }
        if self.verbose {
            parts.push("verbose".into());
        }
        if self.log {
            parts.push("logging".into());
        }
        if self.safety.allow_system_folder {
            parts.push("SYSTEM FOLDERS ALLOWED".into());
        }
        parts.join(" · ")
    }

    /// Prints the options above the menu, when there are any worth printing.
    pub fn show(&self) {
        let summary = self.summary();
        if !summary.is_empty() {
            ui::hint(&format!("Options: {summary}"));
        }
    }

    /// The options menu. Loops until the user chooses to go back.
    pub fn edit(&mut self) -> Result<()> {
        loop {
            let items = [
                onoff("Preview only, change nothing", self.dry_run),
                format!("Folders to look through .......... {}", self.depth_label()),
                onoff(
                    "Include hidden files and folders",
                    self.filter.include_hidden,
                ),
                onoff("Include shortcuts", self.filter.include_shortcuts),
                format!(
                    "Extensions to leave alone ........ {}",
                    list_label(&self.filter.ignore_ext)
                ),
                format!(
                    "Names or patterns to leave alone . {}",
                    list_label(&self.filter.ignore)
                ),
                format!(
                    "Code projects .................... {}",
                    self.projects_label()
                ),
                onoff("List every item, not a sample", self.verbose),
                onoff("Write a run log", self.log),
                onoff(
                    "Allow folders the system manages",
                    self.safety.allow_system_folder,
                ),
                "Back to the menu".to_string(),
            ];
            let pick = Select::new()
                .with_prompt("Which option?")
                .items(&items)
                .default(items.len() - 1)
                .interact()?;

            match pick {
                0 => self.dry_run = !self.dry_run,
                1 => self.ask_depth()?,
                2 => self.filter.include_hidden = !self.filter.include_hidden,
                3 => self.filter.include_shortcuts = !self.filter.include_shortcuts,
                4 => {
                    self.filter.ignore_ext =
                        ask_list("Extensions to leave alone, comma-separated (e.g. iso,tmp)")?;
                }
                5 => {
                    self.filter.ignore =
                        ask_list("Names or patterns, comma-separated (e.g. *.bak,notes.txt)")?;
                }
                6 => self.projects = toggle_projects(self.projects),
                7 => self.verbose = !self.verbose,
                8 => {
                    self.log = !self.log;
                    obs::set_enabled(self.log);
                }
                9 => self.ask_allow_system()?,
                _ => return Ok(()),
            }
        }
    }

    fn depth_label(&self) -> String {
        match self.depth {
            None => "each command decides".into(),
            Some(ALL_LEVELS) => "all of them".into(),
            Some(n) => format!("{n}"),
        }
    }

    fn projects_label(&self) -> &'static str {
        match self.projects {
            ProjectPolicy::Keep => "leave where they are",
            ProjectPolicy::Move => "gather into Projects",
        }
    }

    fn ask_depth(&mut self) -> Result<()> {
        let choices = [
            "Each command decides (recommended)",
            "Only files directly inside",
            "Two levels",
            "Four levels",
            "All of them",
        ];
        let pick = Select::new()
            .with_prompt("How many folder levels should be looked through?")
            .items(&choices)
            .default(0)
            .interact()?;
        self.depth = match pick {
            1 => Some(1),
            2 => Some(2),
            3 => Some(4),
            4 => Some(ALL_LEVELS),
            _ => None,
        };
        Ok(())
    }

    /// Turning the override on is itself a decision worth confirming, and the
    /// guard will still ask again before it does anything.
    fn ask_allow_system(&mut self) -> Result<()> {
        if self.safety.allow_system_folder {
            self.safety.allow_system_folder = false;
            return Ok(());
        }
        ui::warn("This lets tidy-up work on folders your operating system manages.");
        ui::hint("Reorganizing one of those can stop programs, or the computer, from working.");
        ui::hint("You will still be asked to confirm before anything is changed.");
        self.safety.allow_system_folder = Confirm::new()
            .with_prompt("Turn this on?")
            .default(false)
            .interact()?;
        Ok(())
    }
}

fn onoff(label: &str, value: bool) -> String {
    let dots = ".".repeat(34usize.saturating_sub(label.len()));
    format!("{label} {dots} {}", if value { "on" } else { "off" })
}

fn list_label(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn toggle_projects(current: ProjectPolicy) -> ProjectPolicy {
    match current {
        ProjectPolicy::Keep => ProjectPolicy::Move,
        ProjectPolicy::Move => ProjectPolicy::Keep,
    }
}

fn ask_list(prompt: &str) -> Result<Vec<String>> {
    let text: String = Input::new()
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()?;
    Ok(split_list(&text))
}

/// Splits a comma-separated answer, dropping blanks so a stray comma or an
/// empty answer cannot turn into a rule that matches everything.
pub(crate) fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_session_says_nothing() {
        assert_eq!(Session::default().summary(), "");
    }

    #[test]
    fn the_summary_names_every_option_that_is_not_default() {
        let mut session = Session {
            dry_run: true,
            verbose: true,
            log: true,
            depth: Some(2),
            projects: ProjectPolicy::Move,
            ..Default::default()
        };
        session.filter.include_hidden = true;
        session.filter.include_shortcuts = true;
        session.filter.ignore_ext = vec!["iso".into()];
        session.filter.ignore = vec!["*.bak".into()];

        let summary = session.summary();
        for expected in [
            "preview only",
            "2 level(s) deep",
            "including hidden",
            "including shortcuts",
            ".iso",
            "*.bak",
            "gathering projects",
            "verbose",
            "logging",
        ] {
            assert!(
                summary.contains(expected),
                "{expected} missing from {summary}"
            );
        }
    }

    /// The one option that changes what tidy-up is willing to destroy should be
    /// impossible to miss in the header.
    #[test]
    fn allowing_system_folders_is_shouted_about() {
        let mut session = Session::default();
        session.safety.allow_system_folder = true;
        assert!(session.summary().contains("SYSTEM FOLDERS ALLOWED"));
    }

    #[test]
    fn all_levels_reads_as_words_rather_than_a_huge_number() {
        let session = Session {
            depth: Some(ALL_LEVELS),
            ..Default::default()
        };
        assert_eq!(session.depth_label(), "all of them");
        assert!(session.summary().contains("all sub-folders"));
        assert!(!session.summary().contains(&ALL_LEVELS.to_string()));
    }

    /// `None` means "whatever this command normally does", which differs per
    /// command, so it must not collapse to one number too early.
    #[test]
    fn depth_falls_back_to_each_commands_own_default() {
        let unset = Session::default();
        assert_eq!(unset.depth_or(1), 1, "organize scans one level");
        assert_eq!(unset.depth_or(9), 9);

        let chosen = Session {
            depth: Some(3),
            ..Default::default()
        };
        assert_eq!(chosen.depth_or(1), 3, "an explicit choice wins");
    }

    /// A blank answer, or a stray comma, must not become an empty rule: an empty
    /// ignore pattern would match everything.
    #[test]
    fn a_blank_or_ragged_answer_produces_no_rules() {
        assert!(split_list("").is_empty());
        assert!(split_list("   ").is_empty());
        assert!(split_list(",,,").is_empty());
        assert_eq!(split_list("iso, tmp ,"), ["iso", "tmp"]);
        assert_eq!(split_list("  one  "), ["one"]);
    }

    #[test]
    fn toggling_projects_round_trips() {
        assert_eq!(toggle_projects(ProjectPolicy::Keep), ProjectPolicy::Move);
        assert_eq!(toggle_projects(ProjectPolicy::Move), ProjectPolicy::Keep);
    }

    #[test]
    fn option_labels_line_up_and_show_their_state() {
        let on = onoff("Preview only, change nothing", true);
        let off = onoff("Write a run log", false);
        assert!(on.ends_with(" on"), "{on}");
        assert!(off.ends_with(" off"), "{off}");
        // `rfind`, not `find`: " on" also occurs inside "Preview only".
        assert_eq!(
            on.rfind(' '),
            off.rfind(' '),
            "the state column should be aligned:
{on}
{off}"
        );
        assert_eq!(list_label(&[]), "none");
        assert_eq!(list_label(&["a".to_string(), "b".to_string()]), "a, b");
    }
}
