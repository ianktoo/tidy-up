//! Configuration: what an installation **permits**, and what it defaults to.
//!
//! These are two different things and the difference is the whole design.
//!
//! A [`Policy`] is a **ceiling**. Flags cannot raise it. It is how an
//! installation says "this machine may only tidy these folders, may never
//! touch a system folder, and may not delete anything", and have that hold
//! whatever the caller types. It is the piece an orchestrator or an MCP host
//! would eventually lean on, and the reason this is not just a presets file.
//!
//! [`Defaults`] are a **floor**. They save typing, and any flag overrides
//! them.
//!
//! # Where configuration may come from
//!
//! A policy is only ever loaded from a path someone named: `--config` or
//! `TIDY_UP_CONFIG`. It is never discovered.
//!
//! Defaults may additionally come from `.tidy-up.json` in the folder being
//! worked on, which is convenient and travels with the folder. That file is
//! **not allowed to contain a policy**, and one found there is ignored with a
//! warning. Otherwise a folder could ship its own permission slip: download an
//! archive, run tidy-up on it, and the archive decides what tidy-up may do.
//! Auto-discovered configuration must never widen what is allowed.
//!
//! # Format
//!
//! JSON. tidy-up takes no dependencies it does not need, and the journals,
//! run logs, plan files and the MCP interface are already JSON, so this needs
//! no parser that is not already here.
//!
//! ```json
//! {
//!   "format": 1,
//!   "policy": { "roots": ["D:\\Media"], "allow_system_folders": false },
//!   "defaults": { "ignore_ext": ["iso"], "depth": 2 },
//!   "profiles": { "photos": { "by": ["year", "month"] } }
//! }
//! ```

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};

use crate::{
    api::{ErrorCode, Refused},
    error::{Error, IoContext, Result},
    plan::ProjectPolicy,
    regroup::GroupBy,
    safety::Platform,
};

/// Bumped when the meaning of anything here changes.
pub const FORMAT_VERSION: u32 = 1;

/// Environment variable naming a policy file.
pub const CONFIG_VAR: &str = "TIDY_UP_CONFIG";

/// Name looked for inside the folder being worked on. Defaults only.
pub const FOLDER_CONFIG: &str = ".tidy-up.json";

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// What this installation permits. A ceiling: flags cannot raise it.
///
/// Every field defaults to "no restriction", so an absent policy behaves
/// exactly as tidy-up did before there was one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Folders that may be worked on. Empty means anywhere.
    ///
    /// A target must be one of these or inside one. Checked on the resolved
    /// path, so `..` and links cannot get around it.
    pub roots: Vec<PathBuf>,
    /// Whether anything may be changed at all. `None` means yes.
    pub allow_writes: Option<bool>,
    /// Whether `--allow-system-folder` works. `None` means yes.
    ///
    /// Set to `false` and the flag becomes an error rather than being quietly
    /// ignored, because a caller who passed it deserves to know it did nothing.
    pub allow_system_folders: Option<bool>,
    /// Whether anything may be deleted outright (`purge`, `compare --action
    /// delete`). `None` means yes.
    pub allow_delete: Option<bool>,
    /// Largest `--depth` that will be honoured. `None` means no cap.
    pub max_depth: Option<u32>,
}

impl Policy {
    /// Whether this policy restricts anything at all.
    pub fn is_open(&self) -> bool {
        *self == Policy::default()
    }

    /// Refuses a target outside the permitted roots.
    ///
    /// `path` must already be resolved: this compares real locations, not the
    /// text the caller typed.
    pub fn check_root(&self, path: &Path) -> Result<()> {
        if self.roots.is_empty() || self.roots.iter().any(|root| under(path, root)) {
            return Ok(());
        }
        Err(Error::Invalid(format!(
            "{} is not one of the folders this installation allows ({})",
            path.display(),
            self.roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// Refuses a change when this installation is read-only.
    pub fn check_write(&self) -> Result<()> {
        match self.allow_writes {
            Some(false) => Err(Error::Invalid(
                "this installation is configured read-only; nothing can be changed".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Refuses a deletion when this installation forbids them.
    pub fn check_delete(&self) -> Result<()> {
        match self.allow_delete {
            Some(false) => Err(Error::Invalid(
                "this installation does not allow deleting; move things aside instead".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Resolves `--allow-system-folder` against the policy.
    ///
    /// Passing the flag where it is forbidden is an error, not a silent no:
    /// the caller thinks they have authorised something and has to be told
    /// they have not.
    pub fn resolve_override(&self, requested: bool) -> Result<bool> {
        match (requested, self.allow_system_folders) {
            (true, Some(false)) => Err(Error::Invalid(
                "--allow-system-folder is disabled by this installation's policy".into(),
            )),
            _ => Ok(requested),
        }
    }

    /// Caps a requested depth.
    pub fn cap_depth(&self, requested: u32) -> u32 {
        match self.max_depth {
            Some(max) => requested.min(max),
            None => requested,
        }
    }

    /// One line describing the restrictions, for the run log and `--verbose`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.roots.is_empty() {
            parts.push(format!("{} allowed folder(s)", self.roots.len()));
        }
        if self.allow_writes == Some(false) {
            parts.push("read-only".into());
        }
        if self.allow_system_folders == Some(false) {
            parts.push("no system-folder override".into());
        }
        if self.allow_delete == Some(false) {
            parts.push("no deleting".into());
        }
        if let Some(max) = self.max_depth {
            parts.push(format!("depth capped at {max}"));
        }
        parts.join(", ")
    }
}

/// Whether `path` is `root` or inside it, using the platform's case rules.
fn under(path: &Path, root: &Path) -> bool {
    crate::safety::under(path, root, Platform::current())
}

// ---------------------------------------------------------------------------
// Defaults and profiles
// ---------------------------------------------------------------------------

/// Values used when a flag does not say otherwise. Any flag wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    /// Extensions to leave alone.
    pub ignore_ext: Vec<String>,
    /// Names or globs to leave alone.
    pub ignore: Vec<String>,
    /// Also process hidden files and folders.
    pub include_hidden: Option<bool>,
    /// Also process shortcuts.
    pub include_shortcuts: Option<bool>,
    /// What to do with detected code projects.
    pub projects: Option<ProjectPolicy>,
    /// Folder levels to look through.
    pub depth: Option<u32>,
    /// Grouping keys for `reorganize`.
    pub by: Vec<GroupBy>,
    /// Record a run log.
    pub log: Option<bool>,
}

impl Defaults {
    /// Overlays `other` on top of `self`; anything set in `other` wins.
    pub fn overlay(&mut self, other: &Defaults) {
        if !other.ignore_ext.is_empty() {
            self.ignore_ext = other.ignore_ext.clone();
        }
        if !other.ignore.is_empty() {
            self.ignore = other.ignore.clone();
        }
        if !other.by.is_empty() {
            self.by = other.by.clone();
        }
        self.include_hidden = other.include_hidden.or(self.include_hidden);
        self.include_shortcuts = other.include_shortcuts.or(self.include_shortcuts);
        self.projects = other.projects.or(self.projects);
        self.depth = other.depth.or(self.depth);
        self.log = other.log.or(self.log);
    }
}

/// A whole configuration file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Format version.
    pub format: u32,
    /// What is permitted. Ignored in a folder config.
    pub policy: Policy,
    /// What to assume when a flag is silent.
    pub defaults: Defaults,
    /// Named sets of defaults, chosen with `--profile`.
    pub profiles: BTreeMap<String, Defaults>,
}

impl Config {
    /// Parses a configuration.
    pub fn parse(text: &str) -> Result<Config> {
        let config: Config = serde_json::from_str(text)?;
        if config.format > FORMAT_VERSION {
            return Err(Error::Invalid(format!(
                "this config was written for a newer tidy-up (format {}, this build understands {FORMAT_VERSION})",
                config.format
            )));
        }
        Ok(config)
    }

    /// Reads a configuration from a path someone named.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).at(path)?;
        Config::parse(&text)
    }

    /// Reads `.tidy-up.json` from a folder, if it has one.
    ///
    /// Any policy in it is dropped, because a folder must not be able to say
    /// what may be done to it. Returns whether one was found and whether it
    /// tried to set a policy, so the caller can warn.
    pub fn load_from_folder(root: &Path) -> (Option<Config>, bool) {
        let path = root.join(FOLDER_CONFIG);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (None, false);
        };
        match Config::parse(&text) {
            Ok(mut config) => {
                let tried = !config.policy.is_open();
                config.policy = Policy::default();
                (Some(config), tried)
            }
            Err(_) => (None, false),
        }
    }

    /// The defaults to use, with a profile overlaid when one is named.
    pub fn resolve(&self, profile: Option<&str>) -> Result<Defaults> {
        let mut defaults = self.defaults.clone();
        if let Some(name) = profile {
            let chosen = self.profiles.get(name).ok_or_else(|| {
                Error::Invalid(format!(
                    "no profile called {name:?}. This config has: {}",
                    if self.profiles.is_empty() {
                        "none".to_string()
                    } else {
                        self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    }
                ))
            })?;
            defaults.overlay(chosen);
        }
        Ok(defaults)
    }
}

// ---------------------------------------------------------------------------
// The policy in force
// ---------------------------------------------------------------------------

static POLICY: OnceLock<Policy> = OnceLock::new();

/// Installs the policy for this run. Called once, before any command.
pub fn set_policy(policy: Policy) {
    let _ = POLICY.set(policy);
}

/// The policy in force. Unrestricted when none was configured.
pub fn policy() -> &'static Policy {
    static OPEN: Policy = Policy {
        roots: Vec::new(),
        allow_writes: None,
        allow_system_folders: None,
        allow_delete: None,
        max_depth: None,
    };
    POLICY.get().unwrap_or(&OPEN)
}

/// Loads the configuration for this run and installs its policy.
///
/// The policy comes only from `--config` or [`CONFIG_VAR`], never from
/// anything discovered. A named config that cannot be read or understood is
/// an error: an installation that believes it is restricted must not quietly
/// turn out not to be.
///
/// Returns the resolved defaults, which callers may apply where a flag was
/// silent.
pub fn install(named: Option<&Path>, profile: Option<&str>) -> anyhow::Result<()> {
    let path = named.map(PathBuf::from).or_else(|| {
        std::env::var_os(CONFIG_VAR)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    let Some(path) = path else {
        // No profile can be honoured without a config to take it from, and
        // silently ignoring one would be the same class of mistake.
        if let Some(name) = profile {
            anyhow::bail!("--profile {name} needs a config; pass --config or set {CONFIG_VAR}");
        }
        return Ok(());
    };
    let config = Config::load(&path)?;
    // Validated now, so a bad profile name fails before any work starts.
    let defaults = config.resolve(profile)?;
    set_defaults(defaults);
    set_policy(config.policy);
    Ok(())
}

static DEFAULTS: OnceLock<Defaults> = OnceLock::new();

/// Installs the defaults for this run.
pub fn set_defaults(defaults: Defaults) {
    let _ = DEFAULTS.set(defaults);
}

/// The defaults in force. Empty when nothing was configured.
pub fn defaults() -> &'static Defaults {
    static NONE: OnceLock<Defaults> = OnceLock::new();
    DEFAULTS
        .get()
        .unwrap_or_else(|| NONE.get_or_init(Defaults::default))
}

/// Turns a policy refusal into the typed error the exit boundary understands.
pub fn refuse(error: Error, path: &Path) -> anyhow::Error {
    Refused::at(ErrorCode::SystemFolder, path, error.to_string()).into()
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
