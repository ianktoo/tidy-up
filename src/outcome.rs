//! Survivable failure handling: one item going wrong never stops a run.
//!
//! Every stage that touches many files collects [`Problem`]s instead of returning
//! early, so a locked file, an unreadable folder or a full disk costs you that one
//! item and nothing else. The run finishes, and [`Problems`] is what gets reported.
//!
//! A run is only aborted when continuing would do something the user did not ask
//! for, or would lose the ability to undo it. Everything else is a problem to
//! record and step over.
//!
//! ```
//! use std::path::Path;
//! use tidy_up::outcome::{Op, Problems};
//!
//! let mut problems = Problems::new();
//! let size = problems.try_or_skip(Path::new("ghost.txt"), Op::Metadata, || {
//!     std::fs::metadata("ghost.txt").map(|m| m.len()).map_err(|source| {
//!         tidy_up::error::Error::Io { path: "ghost.txt".into(), source }
//!     })
//! });
//! assert!(size.is_none());
//! assert_eq!(problems.len(), 1);
//! ```

use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::error::Error;

/// What was being attempted when something went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Listing the contents of a folder.
    ReadDir,
    /// Reading a file's size, type or timestamps.
    Metadata,
    /// Reading a file's contents (hashing, comparing).
    Read,
    /// Renaming or moving.
    Move,
    /// Copying across partitions.
    Copy,
    /// Deleting a file or folder.
    Remove,
    /// Creating a folder.
    CreateDir,
    /// Writing the undo journal.
    Journal,
}

impl Op {
    /// Short phrase used in messages: "could not {verb} ...".
    pub fn verb(self) -> &'static str {
        match self {
            Op::ReadDir => "list",
            Op::Metadata => "inspect",
            Op::Read => "read",
            Op::Move => "move",
            Op::Copy => "copy",
            Op::Remove => "delete",
            Op::CreateDir => "create",
            Op::Journal => "record",
        }
    }

    /// Stable machine token for logs and counters.
    pub fn key(self) -> &'static str {
        match self {
            Op::ReadDir => "read_dir",
            Op::Metadata => "metadata",
            Op::Read => "read",
            Op::Move => "move",
            Op::Copy => "copy",
            Op::Remove => "remove",
            Op::CreateDir => "create_dir",
            Op::Journal => "journal",
        }
    }
}

/// Why something went wrong, classified once so every caller counts the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    /// The operating system refused: no permission.
    Denied,
    /// The item is gone (deleted or renamed since the scan).
    NotFound,
    /// Another program is holding the file open.
    InUse,
    /// Something is already there, and tidy-up never overwrites.
    Exists,
    /// A rename cannot cross partitions.
    CrossesDevices,
    /// The destination partition is full.
    NoSpace,
    /// A path or name the file system will not accept.
    Invalid,
    /// The work panicked. Reported, never propagated.
    Panic,
    /// Anything else.
    Other,
}

/// Windows reports a file held open by another process as ERROR_SHARING_VIOLATION,
/// which has no stable [`io::ErrorKind`] of its own.
const WINDOWS_SHARING_VIOLATION: i32 = 32;
/// Windows ERROR_LOCK_VIOLATION.
const WINDOWS_LOCK_VIOLATION: i32 = 33;

impl Cause {
    /// Classifies an I/O error. The single place error kinds are interpreted.
    pub fn of(error: &io::Error) -> Cause {
        match error.kind() {
            io::ErrorKind::PermissionDenied => Cause::Denied,
            io::ErrorKind::NotFound => Cause::NotFound,
            io::ErrorKind::AlreadyExists => Cause::Exists,
            io::ErrorKind::CrossesDevices => Cause::CrossesDevices,
            io::ErrorKind::StorageFull => Cause::NoSpace,
            io::ErrorKind::ReadOnlyFilesystem => Cause::Denied,
            io::ErrorKind::ResourceBusy => Cause::InUse,
            // `ErrorKind::InvalidFilename` would belong here too, but it is
            // still unstable on the minimum supported Rust version. A rejected
            // name usually surfaces as `InvalidInput` anyway.
            io::ErrorKind::InvalidInput => Cause::Invalid,
            _ => match error.raw_os_error() {
                Some(WINDOWS_SHARING_VIOLATION) | Some(WINDOWS_LOCK_VIOLATION) if cfg!(windows) => {
                    Cause::InUse
                }
                _ => Cause::Other,
            },
        }
    }

    /// Classifies a crate error, looking through to the I/O cause when there is one.
    pub fn of_error(error: &Error) -> Cause {
        match error {
            Error::Io { source, .. } => Cause::of(source),
            Error::Denied { .. } => Cause::Denied,
            Error::Invalid(_) | Error::Journal { .. } => Cause::Invalid,
            Error::Json(_) => Cause::Other,
        }
    }

    /// Stable machine token for logs and counters.
    pub fn key(self) -> &'static str {
        match self {
            Cause::Denied => "denied",
            Cause::NotFound => "not_found",
            Cause::InUse => "in_use",
            Cause::Exists => "exists",
            Cause::CrossesDevices => "crosses_devices",
            Cause::NoSpace => "no_space",
            Cause::Invalid => "invalid",
            Cause::Panic => "panic",
            Cause::Other => "other",
        }
    }

    /// Plural heading used when grouping a report by cause.
    pub fn label(self) -> &'static str {
        match self {
            Cause::Denied => "Permission denied",
            Cause::NotFound => "No longer there",
            Cause::InUse => "In use by another program",
            Cause::Exists => "Something is already there",
            Cause::CrossesDevices => "Could not cross partitions",
            Cause::NoSpace => "The disk is full",
            Cause::Invalid => "Not a usable name or path",
            Cause::Panic => "Unexpected internal error",
            Cause::Other => "Other",
        }
    }

    /// What the user can do about it, when there is something.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Cause::Denied => Some("pick a folder you own, or run as an administrator"),
            Cause::NotFound => Some("these were deleted or renamed while tidy-up was running"),
            Cause::InUse => Some("close the programs holding these files and run again"),
            Cause::NoSpace => Some("free some space on the destination and run again"),
            Cause::Invalid => Some("rename these items to something the file system accepts"),
            Cause::Panic => Some("please report this, with the file names if you can share them"),
            Cause::Exists | Cause::CrossesDevices | Cause::Other => None,
        }
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One thing that did not work, and what tidy-up did about it (nothing: it moved on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The item that could not be processed.
    pub path: PathBuf,
    /// What was being attempted.
    pub op: Op,
    /// Why it failed.
    pub cause: Cause,
    /// The underlying message, for the detail lines and the log.
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not {} {}: {}",
            self.op.verb(),
            self.path.display(),
            self.message
        )
    }
}

/// Problems collected during a run, in the order they happened.
///
/// Recording never fails and never short-circuits: that is the whole point.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Problems(Vec<Problem>);

impl Problems {
    /// An empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many problems were recorded.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the run was completely clean.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every problem, in the order they happened.
    pub fn iter(&self) -> std::slice::Iter<'_, Problem> {
        self.0.iter()
    }

    /// Records a crate error against `path`.
    pub fn record(&mut self, path: &Path, op: Op, error: &Error) {
        self.push(Problem {
            path: path.to_path_buf(),
            op,
            cause: Cause::of_error(error),
            message: error.to_string(),
        });
    }

    /// Records a bare I/O error against `path`.
    pub fn record_io(&mut self, path: &Path, op: Op, error: &io::Error) {
        self.push(Problem {
            path: path.to_path_buf(),
            op,
            cause: Cause::of(error),
            message: error.to_string(),
        });
    }

    /// Records an already-classified problem.
    pub fn push(&mut self, problem: Problem) {
        self.0.push(problem);
    }

    /// Absorbs another collection, keeping order.
    pub fn absorb(&mut self, other: Problems) {
        self.0.extend(other.0);
    }

    /// Runs `f`; on failure records the problem and yields `None`.
    ///
    /// This is the survival primitive: call it instead of `?` anywhere a single
    /// item failing should not end the run.
    pub fn try_or_skip<T>(
        &mut self,
        path: &Path,
        op: Op,
        f: impl FnOnce() -> crate::error::Result<T>,
    ) -> Option<T> {
        match f() {
            Ok(value) => Some(value),
            Err(error) => {
                self.record(path, op, &error);
                None
            }
        }
    }

    /// Like [`try_or_skip`](Self::try_or_skip), and also contains a panic.
    ///
    /// A bug processing one file must not take the process down with it, so the
    /// closure runs inside [`std::panic::catch_unwind`] and a panic becomes an
    /// ordinary [`Cause::Panic`] problem.
    pub fn catch_or_skip<T>(
        &mut self,
        path: &Path,
        op: Op,
        f: impl FnOnce() -> crate::error::Result<T> + std::panic::UnwindSafe,
    ) -> Option<T> {
        match std::panic::catch_unwind(f) {
            Ok(Ok(value)) => Some(value),
            Ok(Err(error)) => {
                self.record(path, op, &error);
                None
            }
            Err(payload) => {
                self.push(Problem {
                    path: path.to_path_buf(),
                    op,
                    cause: Cause::Panic,
                    message: panic_message(&payload),
                });
                None
            }
        }
    }

    /// Counts grouped by cause, worst-understood first by the enum's own order.
    pub fn by_cause(&self) -> BTreeMap<Cause, usize> {
        let mut counts = BTreeMap::new();
        for problem in &self.0 {
            *counts.entry(problem.cause).or_insert(0) += 1;
        }
        counts
    }

    /// Whether anything with this cause was recorded.
    pub fn any(&self, cause: Cause) -> bool {
        self.0.iter().any(|p| p.cause == cause)
    }
}

impl<'a> IntoIterator for &'a Problems {
    type Item = &'a Problem;
    type IntoIter = std::slice::Iter<'a, Problem>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<Problem> for Problems {
    fn from_iter<I: IntoIterator<Item = Problem>>(iter: I) -> Self {
        Problems(iter.into_iter().collect())
    }
}

/// Best effort at the text a panic carried.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panicked".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error(kind: io::ErrorKind) -> Error {
        Error::Io {
            path: PathBuf::from("x"),
            source: io::Error::from(kind),
        }
    }

    #[test]
    fn classifies_the_kinds_that_have_a_hint() {
        assert_eq!(
            Cause::of(&io::ErrorKind::PermissionDenied.into()),
            Cause::Denied
        );
        assert_eq!(Cause::of(&io::ErrorKind::NotFound.into()), Cause::NotFound);
        assert_eq!(
            Cause::of(&io::ErrorKind::StorageFull.into()),
            Cause::NoSpace
        );
        assert_eq!(
            Cause::of(&io::ErrorKind::CrossesDevices.into()),
            Cause::CrossesDevices
        );
        assert_eq!(
            Cause::of(&io::ErrorKind::ReadOnlyFilesystem.into()),
            Cause::Denied,
            "a read-only mount is a permission problem as far as the user is concerned"
        );
        assert_eq!(Cause::of(&io::ErrorKind::BrokenPipe.into()), Cause::Other);
    }

    /// Windows has no `ErrorKind` for a file another program is holding open, so it
    /// has to be recognised by its raw code or every locked file reads as "other".
    #[cfg(windows)]
    #[test]
    fn windows_sharing_violations_are_recognised_as_in_use() {
        let error = io::Error::from_raw_os_error(WINDOWS_SHARING_VIOLATION);
        assert_eq!(Cause::of(&error), Cause::InUse);
    }

    #[test]
    fn crate_errors_are_classified_through_their_io_cause() {
        assert_eq!(
            Cause::of_error(&io_error(io::ErrorKind::PermissionDenied)),
            Cause::Denied
        );
        assert_eq!(
            Cause::of_error(&Error::Denied {
                path: PathBuf::from("x")
            }),
            Cause::Denied
        );
        assert_eq!(
            Cause::of_error(&Error::Invalid("nope".into())),
            Cause::Invalid
        );
    }

    #[test]
    fn try_or_skip_records_and_keeps_going() {
        let mut problems = Problems::new();
        let mut seen = Vec::new();
        for name in ["a", "b", "c"] {
            let value = problems.try_or_skip(Path::new(name), Op::Read, || {
                if name == "b" {
                    Err(io_error(io::ErrorKind::PermissionDenied))
                } else {
                    Ok(name.to_uppercase())
                }
            });
            seen.extend(value);
        }
        assert_eq!(seen, ["A", "C"], "one failure does not stop the others");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems.iter().next().unwrap().cause, Cause::Denied);
        assert_eq!(problems.iter().next().unwrap().op, Op::Read);
    }

    /// A bug while processing one file must cost that file, not the whole run.
    #[test]
    fn catch_or_skip_turns_a_panic_into_a_problem() {
        let mut problems = Problems::new();
        let value = problems.catch_or_skip(
            Path::new("boom.txt"),
            Op::Read,
            || -> crate::error::Result<u8> { panic!("classifier exploded") },
        );
        assert!(value.is_none());
        let problem = problems.iter().next().unwrap();
        assert_eq!(problem.cause, Cause::Panic);
        assert!(problem.message.contains("classifier exploded"));

        let ok = problems.catch_or_skip(Path::new("fine.txt"), Op::Read, || Ok(7u8));
        assert_eq!(ok, Some(7), "the collection is still usable afterwards");
    }

    #[test]
    fn groups_by_cause_for_the_summary() {
        let mut problems = Problems::new();
        for name in ["a", "b"] {
            problems.record(
                Path::new(name),
                Op::Move,
                &io_error(io::ErrorKind::PermissionDenied),
            );
        }
        problems.record(Path::new("c"), Op::Move, &io_error(io::ErrorKind::NotFound));

        let counts = problems.by_cause();
        assert_eq!(counts[&Cause::Denied], 2);
        assert_eq!(counts[&Cause::NotFound], 1);
        assert!(problems.any(Cause::Denied));
        assert!(!problems.any(Cause::NoSpace));
    }

    #[test]
    fn every_cause_has_a_label_and_a_stable_key() {
        let all = [
            Cause::Denied,
            Cause::NotFound,
            Cause::InUse,
            Cause::Exists,
            Cause::CrossesDevices,
            Cause::NoSpace,
            Cause::Invalid,
            Cause::Panic,
            Cause::Other,
        ];
        let mut keys: Vec<_> = all.iter().map(|c| c.key()).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "keys must be unique: they go into logs");
        assert!(all.iter().all(|c| !c.label().is_empty()));
    }

    #[test]
    fn problems_display_with_the_verb_and_the_path() {
        let mut problems = Problems::new();
        problems.record(
            Path::new("Downloads/locked.pdf"),
            Op::Move,
            &io_error(io::ErrorKind::PermissionDenied),
        );
        let text = problems.iter().next().unwrap().to_string();
        assert!(text.starts_with("could not move"), "{text}");
        assert!(text.contains("locked.pdf"), "{text}");
    }

    #[test]
    fn absorb_keeps_order() {
        let mut first = Problems::new();
        first.record(Path::new("a"), Op::Read, &Error::Invalid("1".into()));
        let mut second = Problems::new();
        second.record(Path::new("b"), Op::Read, &Error::Invalid("2".into()));
        first.absorb(second);
        let paths: Vec<_> = first.iter().map(|p| p.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("a"), PathBuf::from("b")]);
    }
}
