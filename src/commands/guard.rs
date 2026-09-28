//! The system-folder guard: policy for what to do about a risky target folder.
//!
//! Detection lives in [`crate::safety`] and is pure. The decision lives here,
//! because it needs a terminal to warn on and a person to ask.
//!
//! The policy in one table:
//!
//! | risk | reporting command | destructive command | with `--allow-system-folder` |
//! |---|---|---|---|
//! | safe | silent | silent | silent |
//! | caution | warn | warn, then confirm | same |
//! | dangerous | warn, proceeds | **refused** | warn, then confirm |
//! | not writable | warn, proceeds | **refused** | **still refused** |
//!
//! `--yes` answers a confirmation. It never substitutes for the override flag, so
//! an existing script that passes `--yes` keeps working on ordinary folders and
//! starts failing loudly on system ones.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{
    cli::{FilterArgs, SafetyArgs},
    fsops::resolve_root,
    obs,
    safety::{Assessment, Reason, Risk, SystemProbe, assess_with, escalate_for_hidden},
    ui,
};

/// The flag a user is told about when a folder is refused.
pub(crate) const OVERRIDE_FLAG: &str = "--allow-system-folder";

/// What the command is about to do with the folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Only look at it. Never blocks, and never touches the disk to find out
    /// whether it could write.
    Read,
    /// Move, delete or create things in it.
    Write,
}

/// The guard for one command invocation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Guard {
    access: Access,
    allow: bool,
    assume_yes: bool,
    dry_run: bool,
    include_hidden: bool,
}

impl Guard {
    /// A guard for a reporting command such as `analyze` or `history`.
    ///
    /// These warn and carry on. Blocking them would be a regression: looking at
    /// `C:\` is one of the reasons `analyze` exists.
    pub(crate) fn read() -> Self {
        Guard {
            access: Access::Read,
            allow: false,
            assume_yes: false,
            dry_run: true,
            include_hidden: false,
        }
    }

    /// A guard for a command that is about to change the folder.
    pub(crate) fn write(safety: &SafetyArgs, yes: bool, dry_run: bool) -> Self {
        Guard {
            access: Access::Write,
            allow: safety.allow_system_folder,
            assume_yes: yes,
            dry_run,
            include_hidden: false,
        }
    }

    /// Notes that hidden items are in scope, which widens the blast radius and so
    /// escalates a caution into a refusal.
    pub(crate) fn with_filter(mut self, filter: &FilterArgs) -> Self {
        self.include_hidden = filter.include_hidden;
        self
    }

    /// Resolves `path` and applies the policy to it.
    ///
    /// Use this instead of [`resolve_root`] in any command that has a target
    /// folder, so there is one place the guard can be forgotten rather than nine.
    pub(crate) fn root(&self, path: &Path) -> Result<PathBuf> {
        let root = resolve_root(path)?;
        self.check(path, &root)?;
        Ok(root)
    }

    /// Applies the policy to an already-resolved folder, for `compare` and
    /// `distribute`, which canonicalize several paths of their own first.
    pub(crate) fn check(&self, original: &Path, root: &Path) -> Result<()> {
        // A dry run and a reporting command must leave the disk alone, so neither
        // gets the write probe. The cost is that a dry run cannot tell you the
        // folder is unwritable, which is the right trade: it was not going to
        // write anything anyway.
        let probe = if self.access == Access::Write && !self.dry_run {
            SystemProbe::writing()
        } else {
            SystemProbe::read_only()
        };
        let assessment = escalate_for_hidden(assess_with(root, &probe), self.include_hidden);
        self.decide(original, &assessment)
    }

    fn decide(&self, original: &Path, assessment: &Assessment) -> Result<()> {
        obs::event(obs::Event::Safety {
            path: assessment.path.display().to_string(),
            risk: assessment.risk.key(),
            reasons: assessment.reasons.iter().map(Reason::key).collect(),
            allowed: self.allow,
        });
        if assessment.is_safe() {
            return Ok(());
        }
        report(original, assessment);

        if self.access == Access::Read {
            // Said out loud, then out of the way.
            return Ok(());
        }

        if assessment.risk == Risk::Dangerous {
            if !assessment.overridable() {
                bail!(
                    "refusing to change {}: tidy-up cannot write there, and {OVERRIDE_FLAG} \
                     cannot grant permission the system refused",
                    original.display()
                );
            }
            if !self.allow {
                bail!(
                    "refusing to change {}: it looks like a folder the system manages. \
                     If you are certain, re-run with {OVERRIDE_FLAG}",
                    original.display()
                );
            }
        }

        let prompt = match assessment.risk {
            Risk::Dangerous => format!("Change {} anyway?", original.display()),
            _ => format!("Continue with {}?", original.display()),
        };
        // Default no: a prompt this dangerous must never be answered by habit.
        if !ui::confirm(&prompt, false, self.assume_yes)? {
            bail!("Cancelled. Nothing was changed.");
        }
        Ok(())
    }
}

/// Prints the headline, one line per reason, and what to do about it.
fn report(original: &Path, assessment: &Assessment) {
    ui::heading("Careful");
    let headline = if original == assessment.path {
        assessment.headline()
    } else {
        // The user typed one thing and it resolved to another (a symlink, a
        // junction, a relative path). Say both, or the warning looks wrong.
        format!(
            "{} is {}",
            original.display(),
            assessment
                .headline()
                .trim_start_matches(&format!("{} is ", assessment.path.display()))
        )
    };
    match assessment.risk {
        Risk::Dangerous => ui::danger(&headline),
        _ => ui::warn(&headline),
    }
    for reason in &assessment.reasons {
        ui::hint(&reason.explain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(allow: bool, yes: bool) -> Guard {
        Guard {
            access: Access::Write,
            allow,
            assume_yes: yes,
            dry_run: false,
            include_hidden: false,
        }
    }

    fn assessment(risk: Risk, reasons: Vec<Reason>) -> Assessment {
        Assessment {
            path: PathBuf::from("/usr"),
            risk,
            reasons,
        }
    }

    #[test]
    fn a_safe_folder_passes_silently() {
        assert!(
            guard(false, false)
                .decide(Path::new("/x"), &Assessment::safe("/x"))
                .is_ok()
        );
    }

    /// The single most important property: a script that passes `--yes` must not
    /// be able to reorganize the operating system.
    #[test]
    fn yes_alone_never_unlocks_a_dangerous_folder() {
        let danger = assessment(Risk::Dangerous, vec![Reason::FilesystemRoot]);
        let err = guard(false, true)
            .decide(Path::new("/"), &danger)
            .unwrap_err()
            .to_string();
        assert!(err.contains(OVERRIDE_FLAG), "{err}");
        assert!(err.contains("refusing"), "{err}");
    }

    /// The flag gets you as far as the question, not past it.
    #[test]
    fn the_override_flag_still_asks() {
        let danger = assessment(Risk::Dangerous, vec![Reason::FilesystemRoot]);
        assert!(
            guard(true, true).decide(Path::new("/"), &danger).is_ok(),
            "flag plus an answered confirmation goes through"
        );
        // Without `--yes` and without a terminal there is nobody to answer, so it
        // stops rather than guessing.
        if !ui::is_interactive() {
            assert!(guard(true, false).decide(Path::new("/"), &danger).is_err());
        }
    }

    /// No flag can grant permission the operating system refused.
    #[test]
    fn an_unwritable_folder_is_refused_even_with_the_flag() {
        let wall = assessment(Risk::Dangerous, vec![Reason::NotWritable]);
        let err = guard(true, true)
            .decide(Path::new("/usr"), &wall)
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot write"), "{err}");
        assert!(
            err.contains("cannot grant permission"),
            "the message must explain why the flag will not help: {err}"
        );
    }

    #[test]
    fn a_caution_folder_only_needs_a_confirmation() {
        let caution = assessment(Risk::Caution, vec![Reason::MountPoint]);
        assert!(
            guard(false, true)
                .decide(Path::new("/mnt/x"), &caution)
                .is_ok(),
            "no override flag needed, just an answer"
        );
    }

    /// Reporting commands say their piece and get out of the way.
    #[test]
    fn a_reporting_command_is_never_blocked() {
        for risk in [Risk::Caution, Risk::Dangerous] {
            let found = assessment(risk, vec![Reason::FilesystemRoot]);
            assert!(
                Guard::read().decide(Path::new("C:/"), &found).is_ok(),
                "analyze on a drive root is a headline use case"
            );
        }
        let wall = assessment(Risk::Dangerous, vec![Reason::NotWritable]);
        assert!(
            Guard::read().decide(Path::new("/usr"), &wall).is_ok(),
            "not being able to write is not a reason to refuse to look"
        );
    }

    #[test]
    fn a_reporting_guard_never_probes_for_writability() {
        let dir = tempfile::tempdir().unwrap();
        Guard::read().check(dir.path(), dir.path()).unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "looking at a folder must not create anything in it"
        );
    }

    #[test]
    fn a_dry_run_never_probes_for_writability_either() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = guard(false, true);
        g.dry_run = true;
        g.check(dir.path(), dir.path()).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Hidden items in scope turn a prompt into a refusal, because that is where
    /// configuration and system state live.
    #[test]
    fn include_hidden_escalates_a_caution_into_a_refusal() {
        let caution = assessment(Risk::Caution, vec![Reason::MountPoint]);
        let escalated = escalate_for_hidden(caution.clone(), true);
        assert_eq!(escalated.risk, Risk::Dangerous);
        let err = guard(false, true).decide(Path::new("/mnt/x"), &escalated);
        assert!(err.is_err(), "now it needs the flag");
        assert!(
            guard(false, true)
                .decide(Path::new("/mnt/x"), &caution)
                .is_ok(),
            "and without --include-hidden it did not"
        );
    }

    #[test]
    fn an_ordinary_folder_resolves_normally_through_the_guard() {
        let dir = tempfile::tempdir().unwrap();
        let root = guard(false, true).root(dir.path()).unwrap();
        assert!(root.is_dir());
        assert_eq!(root, crate::fsops::resolve_root(dir.path()).unwrap());
    }
}
