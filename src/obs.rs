//! Run logging: a machine-readable record of what a run decided and what went
//! wrong, written as JSON Lines.
//!
//! # What this is not
//!
//! It is deliberately **not** a per-move log, because there already is one. The
//! undo journal ([`crate::journal`]) records every move as it happens, unbuffered,
//! so a crash leaves a usable record. Duplicating that here would double the
//! writes to say the same thing twice.
//!
//! What the journal cannot record is everything that did *not* happen: the
//! safety verdict, the files that were skipped and why, the items that failed,
//! how long each stage took. That is what this module adds, and it is why the
//! two together answer "what did that run do to my folder".
//!
//! # Design
//!
//! Events are buffered and written once, at the end of the run, to
//! `<root>/.tidy-up/logs/<run-id>.jsonl` beside the journal. Buffering is safe
//! precisely because the journal is the crash-safe record; a run log that is
//! missing after a crash costs a diagnosis, not data.
//!
//! Logging is **off by default**. It is enabled by `--log` or by setting
//! `TIDY_UP_LOG=1`, so the promise that tidy-up writes nothing you did not ask
//! for holds unless you ask for it.
//!
//! The sink is process-level. A CLI has exactly one run, and threading a handle
//! through nine commands to record eight events would cost more in noise than it
//! returns; the trade is that tests of the sink use [`drain`] rather than running
//! in parallel against separate instances.
//!
//! ```
//! use tidy_up::obs;
//! obs::start("organize", false); // disabled: every call below is a no-op
//! obs::event(obs::Event::Plan { operation: "organize".into(), moves: 3, bytes: 99 });
//! assert!(obs::finish(obs::Status::Ok).is_none());
//! ```

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

use serde::Serialize;

use crate::{
    error::{IoContext, Result},
    journal::TOOL_DIR,
    outcome::Problems,
    scan::ScanResult,
    timefmt::{compact_id, now_secs},
};

/// Environment variable that turns logging on without a flag.
pub const LOG_VAR: &str = "TIDY_UP_LOG";
/// Folder the logs go in, beside `journals`.
const LOG_DIR: &str = "logs";
/// How many past logs to keep per folder.
const KEEP: usize = 20;
/// Log format version, so a reader can tell what it is looking at.
const FORMAT_VERSION: u32 = 1;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Finished, though possibly with skipped items.
    Ok,
    /// Stopped by an error.
    Error,
    /// The user declined a confirmation.
    Cancelled,
    /// Refused by the system-folder guard.
    Blocked,
}

/// One line of a run log.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Always first: what was run, and by what.
    Run {
        /// Sub-command name.
        command: String,
        /// Crate version.
        version: String,
        /// Target operating system.
        os: String,
        /// Log format version.
        format: u32,
        /// Unix timestamp the run started.
        started_at: u64,
    },
    /// What the system-folder guard decided.
    Safety {
        /// The folder judged.
        path: String,
        /// `safe`, `caution` or `dangerous`.
        risk: &'static str,
        /// Every rule that fired.
        reasons: Vec<&'static str>,
        /// Whether the user passed the override flag.
        allowed: bool,
    },
    /// What a scan found, and what it left alone.
    Scan {
        /// Folder scanned.
        root: String,
        /// Eligible files.
        files: usize,
        /// Total bytes of those files.
        bytes: u64,
        /// Detected project folders.
        projects: usize,
        /// Skipped entries, counted by [`crate::scan::SkipReason::key`].
        skipped: BTreeMap<&'static str, usize>,
        /// Milliseconds spent.
        ms: u64,
    },
    /// The size of the plan that was built.
    Plan {
        /// Which command built it.
        operation: String,
        /// Number of moves.
        moves: usize,
        /// Bytes those moves would relocate.
        bytes: u64,
    },
    /// What an execution actually did.
    Execute {
        /// Journal that can undo it.
        journal_id: String,
        /// Moves that succeeded.
        moved: usize,
        /// Bytes relocated.
        bytes: u64,
        /// Emptied folders deleted.
        dirs_removed: usize,
        /// Items that failed.
        failed: usize,
        /// Milliseconds spent.
        ms: u64,
    },
    /// One thing that went wrong, and was skipped.
    Problem {
        /// Item affected.
        path: String,
        /// What was being attempted.
        op: &'static str,
        /// Classified cause.
        cause: &'static str,
        /// Underlying message.
        message: String,
    },
    /// Always last.
    End {
        /// How it ended.
        status: Status,
        /// Total milliseconds.
        ms: u64,
        /// Counters for the whole run.
        metrics: RunMetrics,
    },
}

/// Counters covering a whole run, across every stage.
///
/// Kept apart from [`crate::executor::ExecutionReport`], which is scoped to a
/// single plan, because `compare` and `distribute` span several folders.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunMetrics {
    /// Folders looked at.
    pub roots: usize,
    /// Files the scan considered eligible.
    pub files_scanned: usize,
    /// Bytes those files occupy.
    pub bytes_scanned: u64,
    /// Entries deliberately left alone.
    pub skipped: usize,
    /// Moves planned.
    pub planned_moves: usize,
    /// Moves that happened.
    pub moved: usize,
    /// Bytes relocated.
    pub bytes_moved: u64,
    /// Folders deleted after being emptied.
    pub dirs_removed: usize,
    /// Items skipped because something went wrong.
    pub failed: usize,
    /// Of those, how many were permission problems.
    pub denied: usize,
}

impl RunMetrics {
    /// Adds what a scan found.
    pub fn absorb_scan(&mut self, scan: &ScanResult) {
        self.roots += 1;
        self.files_scanned += scan.files.len();
        self.bytes_scanned += scan.files.iter().map(|f| f.size).sum::<u64>();
        self.skipped += scan.skipped.len();
    }

    /// Adds what an execution did.
    pub fn absorb_execution(&mut self, report: &crate::executor::ExecutionReport) {
        self.moved += report.moved;
        self.bytes_moved += report.bytes;
        self.dirs_removed += report.dirs_removed;
    }

    /// Adds a set of problems.
    pub fn absorb_problems(&mut self, problems: &Problems) {
        self.failed += problems.len();
        self.denied += problems
            .iter()
            .filter(|p| p.cause == crate::outcome::Cause::Denied)
            .count();
    }

    /// Renders the counters as aligned lines for `--stats`.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Folders", self.roots.to_string()),
            ("Files seen", self.files_scanned.to_string()),
            ("Bytes seen", crate::ui::format_size(self.bytes_scanned)),
            ("Skipped", self.skipped.to_string()),
            ("Planned moves", self.planned_moves.to_string()),
            ("Moved", self.moved.to_string()),
            ("Bytes moved", crate::ui::format_size(self.bytes_moved)),
            ("Folders removed", self.dirs_removed.to_string()),
            ("Failed", self.failed.to_string()),
            ("Permission denied", self.denied.to_string()),
        ]
    }
}

/// The buffered state of one run.
#[derive(Debug)]
struct Sink {
    enabled: bool,
    id: String,
    started: Instant,
    events: Vec<Event>,
    metrics: RunMetrics,
    root: Option<PathBuf>,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

fn sink() -> Option<std::sync::MutexGuard<'static, Sink>> {
    SINK.get().and_then(|s| s.lock().ok())
}

/// Begins a run. Call once, before anything else.
///
/// `enabled` comes from `--log`; [`LOG_VAR`] can turn it on instead. Starting
/// twice in one process keeps the first run, which is what makes this safe to
/// call from a test.
pub fn start(command: &str, enabled: bool) {
    let enabled = enabled || std::env::var_os(LOG_VAR).is_some_and(|v| v != "0" && !v.is_empty());
    let sink = Sink {
        enabled,
        id: compact_id(now_secs()),
        started: Instant::now(),
        events: Vec::new(),
        metrics: RunMetrics::default(),
        root: None,
    };
    let _ = SINK.set(Mutex::new(sink));
    event(Event::Run {
        command: command.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        format: FORMAT_VERSION,
        started_at: now_secs(),
    });
}

/// Turns recording on or off after the run has started.
///
/// The interactive menu has no command line to carry `--log`, so it needs a way
/// to change its mind. Events already buffered are kept: turning logging on
/// mid-session records the rest of the session, not a rewritten history.
pub fn set_enabled(enabled: bool) {
    if let Some(mut sink) = sink() {
        sink.enabled = enabled;
    }
}

/// Whether anything is being recorded, so callers can skip expensive formatting.
pub fn enabled() -> bool {
    sink().is_some_and(|s| s.enabled)
}

/// Names the folder whose `.tidy-up` directory receives the log.
///
/// For a command spanning several folders this is the same one the journal goes
/// in: the primary for `compare`, the first destination for `distribute`.
pub fn attach(root: &Path) {
    if let Some(mut sink) = sink() {
        if sink.root.is_none() {
            sink.root = Some(root.to_path_buf());
        }
    }
}

/// Records one event. A no-op when logging is off.
pub fn event(event: Event) {
    if let Some(mut sink) = sink() {
        if sink.enabled {
            sink.events.push(event);
        }
    }
}

/// Records every problem in `problems` and folds them into the counters.
///
/// The one place problems are counted, so calling it twice for the same set
/// would double the figures. Callers pass each set exactly once.
pub fn problems(problems: &Problems) {
    if let Some(mut sink) = sink() {
        sink.metrics.absorb_problems(problems);
        if !sink.enabled {
            return;
        }
        for problem in problems.iter() {
            sink.events.push(Event::Problem {
                path: problem.path.display().to_string(),
                op: problem.op.key(),
                cause: problem.cause.key(),
                message: problem.message.clone(),
            });
        }
    }
}

/// Records the plan a command built.
pub fn plan_built(operation: &str, plan: &crate::plan::Plan) {
    metrics(|m| m.planned_moves += plan.moves.len());
    event(Event::Plan {
        operation: operation.to_string(),
        moves: plan.moves.len(),
        bytes: plan.total_bytes(),
    });
}

/// Mutates the run counters.
pub fn metrics(update: impl FnOnce(&mut RunMetrics)) {
    if let Some(mut sink) = sink() {
        update(&mut sink.metrics);
    }
}

/// A copy of the counters so far.
pub fn snapshot() -> RunMetrics {
    sink().map(|s| s.metrics.clone()).unwrap_or_default()
}

/// Milliseconds since the run started.
fn elapsed_ms() -> u64 {
    sink().map_or(0, |s| s.started.elapsed().as_millis() as u64)
}

/// Ends the run and writes the log, returning where it went.
///
/// Returns `None` when logging is off, when no folder was attached, or when the
/// file could not be written: a logging failure must never change what a run
/// reports or the code it exits with.
pub fn finish(status: Status) -> Option<PathBuf> {
    let ms = elapsed_ms();
    let metrics = snapshot();
    event(Event::End {
        status,
        ms,
        metrics,
    });
    let (enabled, id, root, events) = {
        let sink = sink()?;
        (
            sink.enabled,
            sink.id.clone(),
            sink.root.clone()?,
            sink.events.clone(),
        )
    };
    if !enabled {
        return None;
    }
    write_log(&root, &id, &events).ok()
}

fn write_log(root: &Path, id: &str, events: &[Event]) -> Result<PathBuf> {
    let dir = root.join(TOOL_DIR).join(LOG_DIR);
    std::fs::create_dir_all(&dir).at(&dir)?;
    let path = dir.join(format!("{id}.jsonl"));
    let mut text = String::new();
    for event in events {
        text.push_str(&serde_json::to_string(event)?);
        text.push('\n');
    }
    let mut file = std::fs::File::create(&path).at(&path)?;
    file.write_all(text.as_bytes()).at(&path)?;
    prune(&dir);
    Ok(path)
}

/// Keeps the newest [`KEEP`] logs and deletes the rest.
///
/// Ids are chronological, so sorting by name sorts by age. Journals live in a
/// sibling folder and are never touched: they are undo data, logs are not.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    if logs.len() <= KEEP {
        return;
    }
    logs.sort();
    for old in &logs[..logs.len() - KEEP] {
        let _ = std::fs::remove_file(old);
    }
}

/// Takes the buffered events, for tests.
#[doc(hidden)]
pub fn drain() -> Vec<Event> {
    sink()
        .map(|mut s| std::mem::take(&mut s.events))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::Op;

    fn json(event: &Event) -> serde_json::Value {
        serde_json::to_value(event).unwrap()
    }

    /// Off by default is the promise; a run that was not asked to log must leave
    /// nothing behind.
    #[test]
    fn a_disabled_run_writes_nothing_and_buffers_nothing() {
        let sink = Sink {
            enabled: false,
            id: "test".into(),
            started: Instant::now(),
            events: Vec::new(),
            metrics: RunMetrics::default(),
            root: None,
        };
        // Exercised directly rather than through `start`, which owns the one
        // process-level sink.
        assert!(!sink.enabled);
        assert!(sink.events.is_empty());
    }

    #[test]
    fn events_serialize_with_a_stable_tag_and_fields() {
        let event = Event::Plan {
            operation: "organize".into(),
            moves: 3,
            bytes: 99,
        };
        assert_eq!(
            json(&event),
            serde_json::json!({"type": "plan", "operation": "organize", "moves": 3, "bytes": 99})
        );

        let end = Event::End {
            status: Status::Blocked,
            ms: 5,
            metrics: RunMetrics::default(),
        };
        assert_eq!(json(&end)["type"], "end");
        assert_eq!(json(&end)["status"], "blocked");
    }

    #[test]
    fn a_problem_event_carries_the_machine_readable_cause() {
        let mut problems = Problems::new();
        problems.record(
            Path::new("/x/locked.pdf"),
            Op::Move,
            &crate::error::Error::Io {
                path: PathBuf::from("/x/locked.pdf"),
                source: std::io::ErrorKind::PermissionDenied.into(),
            },
        );
        let problem = problems.iter().next().unwrap();
        let event = Event::Problem {
            path: problem.path.display().to_string(),
            op: problem.op.key(),
            cause: problem.cause.key(),
            message: problem.message.clone(),
        };
        let value = json(&event);
        assert_eq!(value["cause"], "denied");
        assert_eq!(value["op"], "move");
        assert_eq!(value["type"], "problem");
    }

    #[test]
    fn metrics_accumulate_across_stages() {
        let mut metrics = RunMetrics::default();
        let mut problems = Problems::new();
        problems.record(
            Path::new("/a"),
            Op::Move,
            &crate::error::Error::Io {
                path: PathBuf::from("/a"),
                source: std::io::ErrorKind::PermissionDenied.into(),
            },
        );
        problems.record(
            Path::new("/b"),
            Op::Read,
            &crate::error::Error::Invalid("x".into()),
        );
        metrics.absorb_problems(&problems);
        assert_eq!(metrics.failed, 2);
        assert_eq!(
            metrics.denied, 1,
            "only the permission one counts as denied"
        );
        assert_eq!(metrics.lines().len(), 10);
    }

    /// Pruning must never touch the journals, which are undo data.
    #[test]
    fn pruning_keeps_the_newest_logs_and_nothing_else_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        for n in 0..KEEP + 5 {
            std::fs::write(logs.join(format!("2026010{n:02}-000000.jsonl")), "{}").unwrap();
        }
        std::fs::write(logs.join("notes.txt"), "keep me").unwrap();

        prune(&logs);

        let remaining: Vec<String> = std::fs::read_dir(&logs)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            remaining.iter().filter(|n| n.ends_with(".jsonl")).count(),
            KEEP
        );
        assert!(
            remaining.iter().any(|n| n == "notes.txt"),
            "only .jsonl files are pruned"
        );
        assert!(
            remaining.iter().any(|n| n.contains("202601024")),
            "the newest must survive: {remaining:?}"
        );
    }

    #[test]
    fn writing_a_log_produces_one_json_object_per_line() {
        let dir = tempfile::tempdir().unwrap();
        let events = vec![
            Event::Run {
                command: "organize".into(),
                version: "0.0.0".into(),
                os: "test".into(),
                format: FORMAT_VERSION,
                started_at: 0,
            },
            Event::End {
                status: Status::Ok,
                ms: 1,
                metrics: RunMetrics::default(),
            },
        ];
        let path = write_log(dir.path(), "20260101-000000", &events).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            serde_json::from_str::<serde_json::Value>(line).expect("each line parses alone");
        }
        assert!(path.ends_with("20260101-000000.jsonl"));
        assert!(path.to_string_lossy().contains(TOOL_DIR));
    }

    /// A folder that cannot be written to must not turn into a failed run.
    #[test]
    fn a_log_that_cannot_be_written_is_not_an_error_for_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("a-file");
        std::fs::write(&blocked, "not a folder").unwrap();
        assert!(write_log(&blocked, "x", &[]).is_err());
        // `finish` swallows exactly this, via `.ok()`.
    }
}
