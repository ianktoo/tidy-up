//! Duplicate detection and the plan that isolates duplicates for review.
//!
//! Detection is staged so most files are never fully read:
//! 1. group by size (free — already known from the scan),
//! 2. hash only the first 4 KiB of size-mates,
//! 3. fully hash whatever still collides.
//!
//! Hashing runs on a small thread pool. Nothing is deleted: duplicates are moved
//! into `_Duplicates/Group-NNN/` and the user decides what to purge.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    error::{IoContext, Result},
    fsops::unique_path,
    journal::TOOL_DIR,
    parallel::parallel_map,
    plan::{DUPLICATES_DIR, MoveKind, Plan, PlannedMove},
    scan::FileEntry,
};

/// Bytes hashed in the cheap first pass.
const PARTIAL_BYTES: u64 = 4096;

/// A set of byte-identical files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Size of each file in bytes.
    pub size: u64,
    /// Hex BLAKE3 digest shared by all members.
    pub hash: String,
    /// The copy that stays where it is (shallowest, then oldest, then by path).
    pub keeper: PathBuf,
    /// The redundant copies.
    pub duplicates: Vec<PathBuf>,
}

impl DuplicateGroup {
    /// Bytes freed if every duplicate were deleted.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.size * self.duplicates.len() as u64
    }
}

/// Result of [`find_duplicates`].
#[derive(Debug, Default)]
pub struct DuplicateReport {
    /// Duplicate groups, largest reclaimable space first.
    pub groups: Vec<DuplicateGroup>,
    /// Files that could not be read and were therefore not compared.
    pub unreadable: Vec<PathBuf>,
}

impl DuplicateReport {
    /// Total number of redundant copies across all groups.
    pub fn duplicate_count(&self) -> usize {
        self.groups.iter().map(|g| g.duplicates.len()).sum()
    }

    /// Total bytes reclaimable by deleting every redundant copy.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.groups.iter().map(DuplicateGroup::reclaimable_bytes).sum()
    }
}

/// Hashes at most `limit` bytes of `path` (`None` = whole file).
fn hash_file(path: &Path, limit: Option<u64>) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    match limit {
        Some(n) => hasher.update_reader(file.take(n))?,
        None => hasher.update_reader(file)?,
    };
    Ok(hasher.finalize().to_hex().to_string())
}

/// Hashes `files` in parallel, returning `(file, hash)` for the readable ones
/// and pushing the rest onto `unreadable`.
fn hash_stage<'a>(
    files: Vec<&'a FileEntry>,
    limit: Option<u64>,
    tick: &(dyn Fn() + Sync),
    unreadable: &mut Vec<PathBuf>,
) -> Vec<(&'a FileEntry, String)> {
    let hashes = parallel_map(&files, |f| {
        let result = hash_file(&f.path, limit);
        tick();
        result
    });
    files
        .into_iter()
        .zip(hashes)
        .filter_map(|(file, hash)| match hash {
            Ok(hash) => Some((file, hash)),
            Err(_) => {
                unreadable.push(file.path.clone());
                None
            }
        })
        .collect()
}

/// Keeps only groups with more than one member.
fn collisions<'a, K: Ord>(
    items: impl IntoIterator<Item = (K, &'a FileEntry)>,
) -> Vec<(K, Vec<&'a FileEntry>)> {
    let mut map: BTreeMap<K, Vec<&FileEntry>> = BTreeMap::new();
    for (key, file) in items {
        map.entry(key).or_default().push(file);
    }
    map.into_iter().filter(|(_, g)| g.len() > 1).collect()
}

/// Finds byte-identical files among `files`. Empty files are ignored.
///
/// `tick` is called once per file hashed (from worker threads) so callers can drive a progress bar.
pub fn find_duplicates(files: &[FileEntry], tick: &(dyn Fn() + Sync)) -> DuplicateReport {
    let mut report = DuplicateReport::default();

    let by_size = collisions(files.iter().filter(|f| f.size > 0).map(|f| (f.size, f)));
    let partial_input: Vec<_> = by_size.into_iter().flat_map(|(_, g)| g).collect();
    let partial = hash_stage(partial_input, Some(PARTIAL_BYTES), tick, &mut report.unreadable);

    let by_partial = collisions(partial.into_iter().map(|(f, h)| ((f.size, h), f)));
    let full_input: Vec<_> = by_partial.into_iter().flat_map(|(_, g)| g).collect();
    let full = hash_stage(full_input, None, tick, &mut report.unreadable);

    let mut groups: Vec<DuplicateGroup> = collisions(full.into_iter().map(|(f, h)| ((f.size, h), f)))
        .into_iter()
        .map(|((size, hash), mut members)| {
            members.sort_by_key(|f| keeper_rank(f));
            DuplicateGroup {
                size,
                hash,
                keeper: members[0].path.clone(),
                duplicates: members[1..].iter().map(|f| f.path.clone()).collect(),
            }
        })
        .collect();

    groups.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then_with(|| a.keeper.cmp(&b.keeper))
    });
    report.groups = groups;
    report.unreadable.sort();
    report
}

/// Sort key deciding which copy to keep: shallowest, then oldest, then path.
fn keeper_rank(file: &FileEntry) -> (usize, SystemTime, &Path) {
    (
        file.depth,
        file.modified.unwrap_or(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u32::MAX as u64)),
        &file.path,
    )
}

/// Builds the plan that moves each redundant copy into `_Duplicates/Group-NNN/`.
pub fn build_dedupe_plan(root: &Path, groups: &[DuplicateGroup]) -> Plan {
    let mut moves = Vec::new();
    let mut reserved = HashSet::new();
    for (index, group) in groups.iter().enumerate() {
        let folder = root.join(DUPLICATES_DIR).join(group_name(index));
        for path in &group.duplicates {
            let Some(name) = path.file_name() else { continue };
            let to = unique_path(&folder.join(name), &reserved);
            reserved.insert(to.clone());
            moves.push(PlannedMove {
                from: path.clone(),
                to,
                kind: MoveKind::File,
                size: group.size,
            });
        }
    }
    Plan {
        root: root.to_path_buf(),
        moves,
        skipped: Vec::new(),
    }
}

fn group_name(index: usize) -> String {
    format!("Group-{:03}", index + 1)
}

/// Renders a plain-text report of what was isolated and what was kept, and saves it as
/// `.tidy-up/reports/<journal_id>-duplicates.txt`. Returns the report path.
pub fn write_report(root: &Path, journal_id: &str, groups: &[DuplicateGroup]) -> Result<PathBuf> {
    let dir = root.join(TOOL_DIR).join("reports");
    fs::create_dir_all(&dir).at(&dir)?;
    let path = dir.join(format!("{journal_id}-duplicates.txt"));

    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    let mut text = String::from("tidy-up duplicate report\n========================\n\n");
    text.push_str("Each group lists the copy that was KEPT in place and the copies moved to\n");
    text.push_str(&format!("{DUPLICATES_DIR}/<group>/. Review them, then run `tidy-up purge` to delete, or\n"));
    text.push_str("`tidy-up restore` to put everything back.\n\n");
    for (index, group) in groups.iter().enumerate() {
        text.push_str(&format!(
            "{}  ({} bytes each, blake3 {})\n  kept:  {}\n",
            group_name(index),
            group.size,
            &group.hash[..group.hash.len().min(16)],
            rel(&group.keeper)
        ));
        for dup in &group.duplicates {
            text.push_str(&format!("  moved: {}\n", rel(dup)));
        }
        text.push('\n');
    }
    fs::write(&path, text).at(&path)?;
    Ok(path)
}

/// Sums file count and bytes inside the duplicates folder (0, 0 if absent).
pub fn duplicates_folder_stats(root: &Path) -> (usize, u64) {
    fn walk(dir: &Path, acc: &mut (usize, u64)) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.filter_map(|e| e.ok()) {
            match entry.metadata() {
                Ok(m) if m.is_dir() => walk(&entry.path(), acc),
                Ok(m) => {
                    acc.0 += 1;
                    acc.1 += m.len();
                }
                Err(_) => {}
            }
        }
    }
    let mut acc = (0, 0);
    walk(&root.join(DUPLICATES_DIR), &mut acc);
    acc
}

/// Permanently deletes the duplicates folder. Returns `(files, bytes)` removed.
pub fn purge_duplicates(root: &Path) -> Result<(usize, u64)> {
    let dir = root.join(DUPLICATES_DIR);
    let stats = duplicates_folder_stats(root);
    if dir.is_dir() {
        fs::remove_dir_all(&dir).at(&dir)?;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{
        scan::{ScanOptions, scan},
        rules::IgnoreRules,
    };

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn entries(root: &Path) -> Vec<FileEntry> {
        let opts = ScanOptions {
            max_depth: usize::MAX,
            rules: IgnoreRules::new(),
            skip_root_dirs: Default::default(),
        };
        scan(root, &opts).unwrap().files
    }

    #[test]
    fn finds_identical_files_and_ignores_lookalikes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", b"same content");
        write(dir.path(), "sub/copy.txt", b"same content");
        write(dir.path(), "same-size.txt", b"SAME CONTENT");
        write(dir.path(), "unique.txt", b"different");
        let report = find_duplicates(&entries(dir.path()), &|| {});
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.keeper, dir.path().join("a.txt"));
        assert_eq!(group.duplicates, [dir.path().join("sub/copy.txt")]);
        assert_eq!(report.duplicate_count(), 1);
        assert_eq!(report.reclaimable_bytes(), 12);
    }

    #[test]
    fn same_prefix_but_different_tail_is_not_a_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = vec![7u8; 10_000];
        let mut b = a.clone();
        a.push(1);
        b.push(2);
        write(dir.path(), "a.bin", &a);
        write(dir.path(), "b.bin", &b);
        assert!(find_duplicates(&entries(dir.path()), &|| {}).groups.is_empty());
    }

    #[test]
    fn empty_files_are_never_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", b"");
        write(dir.path(), "b", b"");
        assert!(find_duplicates(&entries(dir.path()), &|| {}).groups.is_empty());
    }

    #[test]
    fn three_copies_yield_one_keeper_two_duplicates_shallowest_kept() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "deep/er/x.txt", b"data!");
        write(dir.path(), "mid/x.txt", b"data!");
        write(dir.path(), "top.txt", b"data!");
        let report = find_duplicates(&entries(dir.path()), &|| {});
        let group = &report.groups[0];
        assert_eq!(group.keeper, dir.path().join("top.txt"));
        assert_eq!(group.duplicates.len(), 2);
    }

    #[test]
    fn groups_sorted_by_reclaimable_space() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "s1.txt", b"ab");
        write(dir.path(), "s2.txt", b"ab");
        write(dir.path(), "b1.txt", &[9u8; 1000]);
        write(dir.path(), "b2.txt", &[9u8; 1000]);
        let report = find_duplicates(&entries(dir.path()), &|| {});
        assert_eq!(report.groups[0].size, 1000);
        assert_eq!(report.groups[1].size, 2);
    }

    #[test]
    fn tick_fires_for_hashed_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", b"same");
        write(dir.path(), "b.txt", b"same");
        write(dir.path(), "c.txt", b"lone-and-long");
        let ticks = AtomicUsize::new(0);
        find_duplicates(&entries(dir.path()), &|| {
            ticks.fetch_add(1, Ordering::Relaxed);
        });
        // two files hashed partially, then the same two hashed fully
        assert_eq!(ticks.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn unreadable_files_are_reported_not_fatal() {
        let ghost = FileEntry {
            path: PathBuf::from("/no/such/file-a"),
            size: 5,
            modified: None,
            depth: 1,
        };
        let ghost2 = FileEntry {
            path: PathBuf::from("/no/such/file-b"),
            ..ghost.clone()
        };
        let report = find_duplicates(&[ghost, ghost2], &|| {});
        assert!(report.groups.is_empty());
        assert_eq!(report.unreadable.len(), 2);
    }

    #[test]
    fn plan_moves_only_duplicates_into_numbered_groups() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", b"same");
        write(dir.path(), "x/a.txt", b"same");
        write(dir.path(), "y/a.txt", b"same");
        let report = find_duplicates(&entries(dir.path()), &|| {});
        let plan = build_dedupe_plan(dir.path(), &report.groups);
        assert_eq!(plan.moves.len(), 2);
        assert!(plan.moves.iter().all(|m| m.to.starts_with(dir.path().join("_Duplicates/Group-001"))));
        assert!(plan.moves.iter().all(|m| m.from != dir.path().join("a.txt")));
        let names: HashSet<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(names.len(), 2, "same-named duplicates must not collide");
    }

    #[test]
    fn report_lists_kept_and_moved_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", b"same");
        write(dir.path(), "b.txt", b"same");
        let report = find_duplicates(&entries(dir.path()), &|| {});
        let path = write_report(dir.path(), "20260101-000000", &report.groups).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("kept:  a.txt"));
        assert!(text.contains("moved: b.txt"));
        assert!(text.contains("Group-001"));
    }

    #[test]
    fn stats_and_purge() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(duplicates_folder_stats(dir.path()), (0, 0));
        write(dir.path(), "_Duplicates/Group-001/a.txt", b"12345");
        write(dir.path(), "_Duplicates/Group-002/b.txt", b"123");
        assert_eq!(duplicates_folder_stats(dir.path()), (2, 8));
        assert_eq!(purge_duplicates(dir.path()).unwrap(), (2, 8));
        assert!(!dir.path().join("_Duplicates").exists());
        assert_eq!(purge_duplicates(dir.path()).unwrap(), (0, 0));
    }
}
