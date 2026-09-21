//! The undo log.
//!
//! Every run that changes files writes one *journal* under
//! `<root>/.tidy-up/journals/<id>.jsonl`. A journal is JSON Lines: one header
//! record followed by one record per change, appended and flushed as it happens,
//! so a crash mid-run still leaves a complete record of everything moved so far.
//! Paths are stored relative to the root so the folder (or drive) can be
//! relocated or re-lettered without invalidating its history.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, IoContext, Result},
    plan::MoveKind,
    timefmt,
};

/// Name of the hidden state folder placed at the root of every processed folder.
pub const TOOL_DIR: &str = ".tidy-up";
const JOURNAL_DIR: &str = "journals";
const JOURNAL_EXT: &str = "jsonl";
const FORMAT_VERSION: u32 = 1;

/// What kind of run produced a journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Sorting files into category folders.
    Organize,
    /// Moving duplicate files into the duplicates folder.
    Dedupe,
    /// Comparing several folders, then moving or merging their duplicated content.
    Compare,
    /// Spreading files across several destinations.
    Distribute,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Operation::Organize => "organize",
            Operation::Dedupe => "dedupe",
            Operation::Compare => "compare",
            Operation::Distribute => "distribute",
        })
    }
}

/// First line of every journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Journal format version.
    pub version: u32,
    /// Unique, chronologically sortable id (`YYYYMMDD-HHMMSS[-n]`).
    pub id: String,
    /// Kind of run.
    pub operation: Operation,
    /// Absolute root at the time of the run (informational only).
    pub root: PathBuf,
    /// Unix timestamp (seconds) of the run.
    pub created_at: u64,
}

/// One line of a journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    /// Journal header (always first).
    Header(Header),
    /// A directory this run created.
    DirCreated {
        /// Path relative to the root.
        path: PathBuf,
    },
    /// A file or directory that moved.
    Move {
        /// Original location, relative to the root.
        from: PathBuf,
        /// New location, relative to the root.
        to: PathBuf,
        /// Whether a file or a directory moved.
        kind: MoveKind,
    },
    /// The run was undone at this Unix time.
    Restored {
        /// Unix timestamp (seconds).
        at: u64,
    },
}

/// Directory holding all journals for `root`.
pub fn journal_dir(root: &Path) -> PathBuf {
    root.join(TOOL_DIR).join(JOURNAL_DIR)
}

/// Appends records to a brand-new journal as a run progresses.
#[derive(Debug)]
pub struct JournalWriter {
    file: File,
    root: PathBuf,
    id: String,
}

impl JournalWriter {
    /// Creates a new journal for `root` and writes its header.
    pub fn create(root: &Path, operation: Operation) -> Result<Self> {
        let dir = journal_dir(root);
        fs::create_dir_all(&dir).at(&dir)?;
        let now = timefmt::now_secs();
        let base = timefmt::compact_id(now);

        let (file, id) = (0u32..)
            .find_map(|attempt| {
                let id = if attempt == 0 {
                    base.clone()
                } else {
                    format!("{base}-{}", attempt + 1)
                };
                let path = dir.join(format!("{id}.{JOURNAL_EXT}"));
                match OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => Some(Ok((file, id))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(e) => Some(Err(Error::Io { path, source: e })),
                }
            })
            .expect("attempt counter exhausted")?;

        let mut writer = Self {
            file,
            root: root.to_path_buf(),
            id: id.clone(),
        };
        writer.append(&Record::Header(Header {
            version: FORMAT_VERSION,
            id,
            operation,
            root: root.to_path_buf(),
            created_at: now,
        }))?;
        Ok(writer)
    }

    /// The journal's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Records that `path` (absolute, under the root) was created as a directory.
    pub fn record_dir_created(&mut self, path: &Path) -> Result<()> {
        let path = self.relative(path)?;
        self.append(&Record::DirCreated { path })
    }

    /// Records that `from` moved to `to` (both absolute, under the root).
    pub fn record_move(&mut self, from: &Path, to: &Path, kind: MoveKind) -> Result<()> {
        let (from, to) = (self.relative(from)?, self.relative(to)?);
        self.append(&Record::Move { from, to, kind })
    }

    /// Paths under the root are stored relative to it (portable). Paths outside it,
    /// which happens when several folders are compared, are stored absolute; joining
    /// an absolute path onto the root at restore time yields that path unchanged.
    fn relative(&self, path: &Path) -> Result<PathBuf> {
        if !path.is_absolute() {
            return Err(Error::Invalid(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        Ok(path.strip_prefix(&self.root).unwrap_or(path).to_path_buf())
    }

    fn append(&mut self, record: &Record) -> Result<()> {
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        // `File` is unbuffered, so each record reaches the OS immediately.
        self.file.write_all(line.as_bytes()).at(&self.root)
    }
}

/// A journal loaded from disk.
#[derive(Debug, Clone)]
pub struct Journal {
    /// File the journal was read from.
    pub path: PathBuf,
    /// Parsed header.
    pub header: Header,
    /// All records after the header, in write order.
    pub records: Vec<Record>,
}

impl Journal {
    /// Loads and validates one journal file.
    ///
    /// A malformed *final* line is tolerated (it is what an interrupted write looks like).
    pub fn load(path: &Path) -> Result<Journal> {
        let text = fs::read_to_string(path).at(path)?;
        let bad = |reason: String| Error::Journal {
            path: path.to_path_buf(),
            reason,
        };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut parsed = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Record>(line) {
                Ok(record) => parsed.push(record),
                Err(_) if i + 1 == lines.len() && i > 0 => {}
                Err(e) => return Err(bad(format!("line {}: {e}", i + 1))),
            }
        }
        let mut iter = parsed.into_iter();
        let header = match iter.next() {
            Some(Record::Header(h)) => h,
            _ => return Err(bad("first line is not a header".into())),
        };
        if header.version > FORMAT_VERSION {
            return Err(bad(format!(
                "written by a newer tidy-up (format v{})",
                header.version
            )));
        }
        Ok(Journal {
            path: path.to_path_buf(),
            header,
            records: iter.collect(),
        })
    }

    /// Loads every journal under `root`, oldest first. Missing folder → empty list.
    pub fn load_all(root: &Path) -> Result<Vec<Journal>> {
        let dir = journal_dir(root);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let paths: Vec<PathBuf> = fs::read_dir(&dir)
            .at(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == JOURNAL_EXT))
            .collect();
        let mut journals = paths
            .iter()
            .map(|p| Journal::load(p))
            .collect::<Result<Vec<_>>>()?;
        // Sort by creation order, not filename: `<id>-2.jsonl` sorts *before* `<id>.jsonl`.
        // Within one second a longer id is a later collision suffix (`-2`, `-3`, ... `-10`).
        journals.sort_by(|a, b| {
            (a.header.created_at, a.header.id.len(), &a.header.id).cmp(&(
                b.header.created_at,
                b.header.id.len(),
                &b.header.id,
            ))
        });
        Ok(journals)
    }

    /// Finds a journal by exact id or unique id prefix.
    pub fn find(root: &Path, id: &str) -> Result<Journal> {
        let mut matches: Vec<Journal> = Journal::load_all(root)?
            .into_iter()
            .filter(|j| j.header.id.starts_with(id))
            .collect();
        if let Some(pos) = matches.iter().position(|j| j.header.id == id) {
            return Ok(matches.swap_remove(pos));
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(Error::Invalid(format!("no journal matching `{id}`"))),
            n => Err(Error::Invalid(format!(
                "`{id}` is ambiguous ({n} journals match)"
            ))),
        }
    }

    /// Iterates over the file/directory moves in write order.
    pub fn moves(&self) -> impl Iterator<Item = (&Path, &Path, MoveKind)> {
        self.records.iter().filter_map(|r| match r {
            Record::Move { from, to, kind } => Some((from.as_path(), to.as_path(), *kind)),
            _ => None,
        })
    }

    /// Number of recorded moves.
    pub fn move_count(&self) -> usize {
        self.moves().count()
    }

    /// Whether this journal has already been undone.
    pub fn is_restored(&self) -> bool {
        self.records
            .iter()
            .any(|r| matches!(r, Record::Restored { .. }))
    }

    /// Appends a `Restored` marker so the journal is not undone twice.
    pub fn mark_restored(&mut self) -> Result<()> {
        let record = Record::Restored {
            at: timefmt::now_secs(),
        };
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .at(&self.path)?;
        let mut line = serde_json::to_string(&record)?;
        line.push('\n');
        file.write_all(line.as_bytes()).at(&self.path)?;
        self.records.push(record);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn round_trips_records_with_relative_paths() {
        let dir = root();
        let mut w = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        w.record_dir_created(&dir.path().join("Images")).unwrap();
        w.record_move(
            &dir.path().join("a.png"),
            &dir.path().join("Images/a.png"),
            MoveKind::File,
        )
        .unwrap();

        let journal =
            Journal::load(&journal_dir(dir.path()).join(format!("{}.jsonl", w.id()))).unwrap();
        assert_eq!(journal.header.operation, Operation::Organize);
        assert_eq!(journal.move_count(), 1);
        let (from, to, kind) = journal.moves().next().unwrap();
        assert_eq!(from, Path::new("a.png"));
        assert_eq!(to, Path::new("Images/a.png"));
        assert_eq!(kind, MoveKind::File);
        assert!(!journal.is_restored());
    }

    #[test]
    fn paths_outside_root_are_stored_absolute_and_relative_ones_rejected() {
        let dir = root();
        let other = root();
        let mut w = JournalWriter::create(dir.path(), Operation::Compare).unwrap();
        w.record_move(
            &other.path().join("x"),
            &dir.path().join("y"),
            MoveKind::File,
        )
        .unwrap();
        let j = Journal::find(dir.path(), w.id()).unwrap();
        let (from, to, _) = j.moves().next().unwrap();
        assert_eq!(from, other.path().join("x"), "outside paths stay absolute");
        assert_eq!(to, Path::new("y"), "inside paths are relative");
        // joining an absolute path onto the root gives it back unchanged (used by restore)
        assert_eq!(dir.path().join(from), other.path().join("x"));

        assert!(
            w.record_move(Path::new("rel"), &dir.path().join("y"), MoveKind::File)
                .is_err()
        );
    }

    #[test]
    fn ids_never_collide_within_one_second() {
        let dir = root();
        let a = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        let b = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        assert_ne!(a.id(), b.id());
        assert_eq!(Journal::load_all(dir.path()).unwrap().len(), 2);
    }

    #[test]
    fn load_all_returns_creation_order_even_within_one_second() {
        let dir = root();
        // Enough writers to reach a two-digit suffix (`-10`), which also breaks naive sorting.
        let created: Vec<String> = (0..12)
            .map(|_| {
                JournalWriter::create(dir.path(), Operation::Organize)
                    .unwrap()
                    .id()
                    .to_string()
            })
            .collect();
        let loaded: Vec<String> = Journal::load_all(dir.path())
            .unwrap()
            .into_iter()
            .map(|j| j.header.id)
            .collect();
        assert_eq!(loaded, created);
    }

    #[test]
    fn mark_restored_persists() {
        let dir = root();
        let w = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        let mut j = Journal::find(dir.path(), w.id()).unwrap();
        j.mark_restored().unwrap();
        assert!(j.is_restored());
        assert!(Journal::find(dir.path(), w.id()).unwrap().is_restored());
    }

    #[test]
    fn tolerates_torn_final_line_but_not_corruption_elsewhere() {
        let dir = root();
        let w = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        let path = journal_dir(dir.path()).join(format!("{}.jsonl", w.id()));
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"type\":\"move\",\"from\":\"a").unwrap();
        assert!(Journal::load(&path).is_ok());

        f.write_all(b"\n{\"type\":\"restored\",\"at\":1}\n")
            .unwrap();
        assert!(Journal::load(&path).is_err());
    }

    #[test]
    fn find_supports_prefix_and_reports_unknown() {
        let dir = root();
        let w = JournalWriter::create(dir.path(), Operation::Organize).unwrap();
        assert!(Journal::find(dir.path(), &w.id()[..8]).is_ok());
        assert!(Journal::find(dir.path(), "1999").is_err());
    }

    #[test]
    fn load_all_on_untouched_folder_is_empty() {
        assert!(Journal::load_all(root().path()).unwrap().is_empty());
    }

    #[test]
    fn rejects_headerless_files() {
        let dir = root();
        let path = dir.path().join("x.jsonl");
        fs::write(&path, "{\"type\":\"restored\",\"at\":1}\n").unwrap();
        assert!(Journal::load(&path).is_err());
    }
}
