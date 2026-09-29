//! The machine-facing contract: `--json` output, exit codes and plan files.
//!
//! The human output and this are two renderings of the same values, not two
//! implementations. A command produces an [`Outcome`]; the terminal renders it
//! as prose and progress bars, `--json` renders it as one object on stdout.
//! Keeping a single model is what stops the two drifting apart.
//!
//! Three things here are a contract that outside code may depend on, so none of
//! them may change meaning without a [`FORMAT_VERSION`] bump:
//!
//! * the JSON envelope and the field names inside it,
//! * the [`ErrorCode`] strings,
//! * the [`Exit`] codes.
//!
//! ```
//! use tidy_up::api::{Envelope, Exit, ErrorCode};
//!
//! let envelope = Envelope::failed("organize", ErrorCode::SystemFolder, "refused");
//! assert_eq!(envelope.exit(), Exit::Refused);
//! assert_eq!(serde_json::to_value(&envelope).unwrap()["status"], "error");
//! ```

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    executor::ExecutionReport,
    journal::Operation,
    outcome::Problems,
    plan::{Plan, PlannedMove},
    restore::RestoreReport,
    scan::ScanResult,
    timefmt::now_secs,
};

/// Bumped when the shape or meaning of anything in this module changes.
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// What the process exits with.
///
/// A caller needs to tell "I refused" apart from "I broke", because the first
/// is an answer and the second is worth retrying or reporting. Before these
/// existed every failure was `1`, so a script could only guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// Finished. Includes "nothing to do", a cancelled confirmation, and a run
    /// that skipped items it could not process.
    Ok = 0,
    /// Something went wrong that stopped the run.
    Error = 1,
    /// The arguments did not make sense. Clap uses this one itself.
    Usage = 2,
    /// Refused deliberately: the system-folder guard, or a permission wall.
    Refused = 3,
    /// Finished, but items were skipped and `--strict` was given.
    Skipped = 4,
}

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        std::process::ExitCode::from(exit as u8)
    }
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

/// A stable, machine-readable reason a command failed.
///
/// Prose messages are for people and will be reworded. These will not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Refused by the system-folder guard.
    SystemFolder,
    /// Refused because tidy-up cannot write there. No flag overrides this.
    NotWritable,
    /// The user declined a confirmation.
    Cancelled,
    /// The path does not exist.
    NotFound,
    /// The path exists but is not a folder.
    NotAFolder,
    /// Permission denied reading the target.
    Denied,
    /// Folders overlap when they must be separate.
    Overlap,
    /// A plan file could not be read, or describes something unsafe.
    InvalidPlan,
    /// A journal could not be read.
    InvalidJournal,
    /// The arguments did not make sense.
    Invalid,
    /// An I/O failure that is none of the above.
    Io,
    /// A bug. If you see this, it is worth reporting.
    Internal,
}

impl ErrorCode {
    /// The process exit code this maps to.
    pub fn exit(self) -> Exit {
        match self {
            // A refusal is an answer, not a malfunction.
            ErrorCode::SystemFolder | ErrorCode::NotWritable => Exit::Refused,
            // Cancelling is the user getting what they asked for.
            ErrorCode::Cancelled => Exit::Ok,
            ErrorCode::Invalid | ErrorCode::NotAFolder => Exit::Usage,
            _ => Exit::Error,
        }
    }

    /// Classifies a library error that reached the top without a better label.
    pub fn of(error: &Error) -> ErrorCode {
        match error {
            Error::Denied { .. } => ErrorCode::Denied,
            Error::Journal { .. } => ErrorCode::InvalidJournal,
            Error::Invalid(_) => ErrorCode::Invalid,
            Error::Json(_) => ErrorCode::InvalidPlan,
            Error::Io { source, .. } => match source.kind() {
                std::io::ErrorKind::NotFound => ErrorCode::NotFound,
                std::io::ErrorKind::PermissionDenied => ErrorCode::Denied,
                _ => ErrorCode::Io,
            },
        }
    }
}

/// A command refusing to act, carried as an error so the exit boundary can
/// classify it by type rather than by matching on the message text.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Refused {
    /// Why, machine-readably.
    pub code: ErrorCode,
    /// Why, for a person.
    pub message: String,
    /// The folder involved.
    pub path: Option<PathBuf>,
}

impl Refused {
    /// A refusal with a path.
    pub fn at(code: ErrorCode, path: &Path, message: impl Into<String>) -> Self {
        Refused {
            code,
            message: message.into(),
            path: Some(path.to_path_buf()),
        }
    }

    /// A refusal with no particular path.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Refused {
            code,
            message: message.into(),
            path: None,
        }
    }
}

/// The error half of an [`Envelope`].
#[derive(Debug, Clone, Serialize)]
pub struct ErrorInfo {
    /// Stable machine-readable reason.
    pub code: ErrorCode,
    /// The message a person would have seen.
    pub message: String,
    /// The folder or file involved, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Turns any error that reached the exit boundary into a code and a message.
///
/// A typed [`Refused`] is honoured first; anything else is classified from the
/// library error, and failing that is reported as internal.
pub fn classify(error: &anyhow::Error) -> ErrorInfo {
    if let Some(refused) = error.downcast_ref::<Refused>() {
        return ErrorInfo {
            code: refused.code,
            message: refused.message.clone(),
            path: refused.path.as_ref().map(|p| p.display().to_string()),
        };
    }
    if let Some(inner) = error.downcast_ref::<Error>() {
        let path = match inner {
            Error::Io { path, .. } | Error::Journal { path, .. } | Error::Denied { path } => {
                Some(path.display().to_string())
            }
            _ => None,
        };
        return ErrorInfo {
            // The library error already names the path and the cause, so the
            // anyhow chain would repeat both.
            code: ErrorCode::of(inner),
            message: inner.to_string(),
            path,
        };
    }
    ErrorInfo {
        code: ErrorCode::Internal,
        message: format!("{error:#}"),
        path: None,
    }
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// One skipped entry, with a stable reason key rather than the prose label.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedItem {
    /// Path, relative to the root when it is under it.
    pub path: String,
    /// Stable token from [`crate::scan::SkipReason::key`].
    pub reason: &'static str,
    /// The ignore rule that matched, when the reason is `ignored`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}

/// What a command did, in a form both renderers can use.
///
/// Every field is optional because commands differ; a reader should look for
/// what it needs rather than assume a shape.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcome {
    /// The folder acted on, once resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Further folders, for `compare` and `distribute`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Whether anything was actually changed.
    pub dry_run: bool,
    /// The plan, present on a dry run and on anything that built one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanFile>,
    /// What an execution did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionReport>,
    /// What a restore did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreReport>,
    /// Command-specific detail that has its own shape, such as an analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    /// Counts of skipped entries by reason.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub skipped: BTreeMap<&'static str, usize>,
    /// Skipped entries in full, only when `--verbose` asked for them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_items: Vec<SkippedItem>,
    /// Everything that went wrong and was stepped over.
    #[serde(skip_serializing_if = "Problems::is_empty")]
    pub problems: Problems,
}

impl Outcome {
    /// An outcome for a command that acted on one folder.
    pub fn at(root: &Path, dry_run: bool) -> Self {
        Outcome {
            root: Some(root.display().to_string()),
            dry_run,
            ..Default::default()
        }
    }

    /// Records what a scan left alone.
    pub fn with_scan(mut self, scan: &ScanResult, verbose: bool) -> Self {
        for skipped in &scan.skipped {
            *self.skipped.entry(skipped.reason.key()).or_insert(0) += 1;
        }
        if verbose {
            self.skipped_items = scan
                .skipped
                .iter()
                .map(|s| SkippedItem {
                    path: s.path.display().to_string(),
                    reason: s.reason.key(),
                    rule: match &s.reason {
                        crate::scan::SkipReason::Ignored(rule) => Some(rule.clone()),
                        _ => None,
                    },
                })
                .collect();
        }
        self
    }

    /// Whether anything had to be stepped over, which `--strict` turns into a
    /// non-zero exit.
    pub fn had_problems(&self) -> bool {
        !self.problems.is_empty()
            || self
                .execution
                .as_ref()
                .is_some_and(|e| !e.problems.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Whether the command succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// It did what was asked, or decided there was nothing to do.
    Ok,
    /// It did not.
    Error,
}

/// The single JSON object a `--json` run prints on stdout.
///
/// One object, one line, whatever happened. A caller can parse the output of
/// any command without knowing which command it ran.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    /// Version of the tool that produced this.
    pub tidy_up: &'static str,
    /// Version of this format.
    pub format: u32,
    /// Sub-command name.
    pub command: String,
    /// Whether it worked.
    pub status: Status,
    /// What it did, when it worked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// Why it did not, when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

impl Envelope {
    /// A successful run.
    pub fn ok(command: &str, outcome: Outcome) -> Self {
        Envelope {
            tidy_up: env!("CARGO_PKG_VERSION"),
            format: FORMAT_VERSION,
            command: command.to_string(),
            status: Status::Ok,
            outcome: Some(outcome),
            error: None,
        }
    }

    /// A failed run, from an already-classified error.
    pub fn failed(command: &str, code: ErrorCode, message: impl Into<String>) -> Self {
        Envelope {
            tidy_up: env!("CARGO_PKG_VERSION"),
            format: FORMAT_VERSION,
            command: command.to_string(),
            status: Status::Error,
            outcome: None,
            error: Some(ErrorInfo {
                code,
                message: message.into(),
                path: None,
            }),
        }
    }

    /// A failed run, from whatever reached the exit boundary.
    pub fn from_error(command: &str, error: &anyhow::Error) -> Self {
        Envelope {
            tidy_up: env!("CARGO_PKG_VERSION"),
            format: FORMAT_VERSION,
            command: command.to_string(),
            status: Status::Error,
            outcome: None,
            error: Some(classify(error)),
        }
    }

    /// The exit code this envelope implies, before `--strict` is considered.
    pub fn exit(&self) -> Exit {
        match &self.error {
            Some(info) => info.code.exit(),
            None => Exit::Ok,
        }
    }

    /// Prints the envelope as one line on stdout.
    pub fn print(&self) {
        match serde_json::to_string(self) {
            Ok(line) => println!("{line}"),
            // Serializing cannot realistically fail, but a machine reader must
            // get valid JSON even then, or it has nothing to act on.
            Err(e) => println!(
                r#"{{"tidy_up":"{}","format":{FORMAT_VERSION},"command":"{}","status":"error","error":{{"code":"internal","message":"could not serialize the result: {e}"}}}}"#,
                env!("CARGO_PKG_VERSION"),
                self.command
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Plan files
// ---------------------------------------------------------------------------

/// A plan, written out so it can be reviewed and applied later.
///
/// This is what makes "propose, review, apply" possible: a dry run emits one,
/// a person or an agent reads it, and `tidy-up apply` carries it out without
/// re-deciding anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanFile {
    /// Version of this format.
    pub format: u32,
    /// Version of the tool that produced it.
    pub tool: String,
    /// Which command built it, so applying it journals under the right name.
    pub operation: Operation,
    /// Unix timestamp it was produced.
    pub created_at: u64,
    /// The folder every path below lives under.
    pub root: PathBuf,
    /// Number of moves, so a reader can check before parsing them all.
    pub moves: usize,
    /// Total bytes the moves would relocate.
    pub bytes: u64,
    /// The moves themselves.
    pub items: Vec<PlannedMove>,
}

impl PlanFile {
    /// Captures a plan.
    pub fn of(plan: &Plan, operation: Operation) -> Self {
        PlanFile {
            format: FORMAT_VERSION,
            tool: env!("CARGO_PKG_VERSION").to_string(),
            operation,
            created_at: now_secs(),
            root: plan.root.clone(),
            moves: plan.moves.len(),
            bytes: plan.total_bytes(),
            items: plan.moves.clone(),
        }
    }

    /// Turns a plan file back into a [`Plan`], refusing anything unsafe.
    ///
    /// This is a trust boundary. A plan file is just JSON on disk: it may have
    /// been hand-edited, generated by something else, or supplied by an agent
    /// acting on instructions from a web page. Applying it without checking
    /// would make `apply` a way to move any file anywhere. Every path must
    /// therefore be absolute and under the plan's own root, and the root is
    /// still put through the system-folder guard by the caller.
    pub fn into_plan(self) -> Result<Plan> {
        if self.format > FORMAT_VERSION {
            return Err(Error::Invalid(format!(
                "this plan was written by a newer tidy-up (format {}, this build understands {FORMAT_VERSION})",
                self.format
            )));
        }
        if !self.root.is_absolute() {
            return Err(Error::Invalid(format!(
                "plan root {} is not an absolute path",
                self.root.display()
            )));
        }
        if self.items.len() != self.moves {
            return Err(Error::Invalid(format!(
                "plan says {} moves but lists {}",
                self.moves,
                self.items.len()
            )));
        }
        for item in &self.items {
            for path in [&item.from, &item.to] {
                if !path.is_absolute() {
                    return Err(Error::Invalid(format!(
                        "plan contains a relative path: {}",
                        path.display()
                    )));
                }
                if !path.starts_with(&self.root) {
                    return Err(Error::Invalid(format!(
                        "plan would touch {}, which is outside its own root {}",
                        path.display(),
                        self.root.display()
                    )));
                }
            }
        }
        Ok(Plan {
            root: self.root,
            moves: self.items,
            skipped: Vec::new(),
        })
    }

    /// Reads a plan file, accepting either a bare plan or a `--json` envelope
    /// with one inside, because both are things a caller will reasonably have.
    pub fn parse(text: &str) -> Result<PlanFile> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let plan = value
            .get("outcome")
            .and_then(|outcome| outcome.get("plan"))
            .cloned()
            .unwrap_or(value);
        Ok(serde_json::from_value(plan)?)
    }
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
