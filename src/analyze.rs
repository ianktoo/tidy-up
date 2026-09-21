//! Read-only analysis of where the space in a folder or drive goes.
//!
//! One walk over the metadata (file contents are never read) produces totals, a breakdown by
//! file type, the largest files and folders, how stale the data is, and the footprint of code
//! projects. Memory stays bounded however large the tree is: only the top-N entries are kept.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{
    category::Category,
    dedupe::DuplicateReport,
    disk::DiskProbe,
    error::{Error, Result},
    projects::is_marker,
};

const DAY: u64 = 86_400;
const PROGRESS_EVERY: u64 = 1024;

/// Labels of the age buckets, newest first (see [`Analysis::age`]).
pub const AGE_LABELS: [&str; 5] = [
    "Last 30 days",
    "30 days to 1 year",
    "1 to 3 years",
    "Over 3 years",
    "Unknown date",
];

/// Knobs for [`analyze`].
#[derive(Debug, Clone, Copy)]
pub struct AnalyzeOptions {
    /// How many entries to keep in each "largest" list.
    pub top: usize,
    /// The instant ages are measured against (injectable for tests).
    pub now: SystemTime,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            top: 10,
            now: SystemTime::now(),
        }
    }
}

/// Size and count for one file-type category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryStat {
    /// Category folder name, e.g. `Images`.
    pub category: String,
    /// Number of files.
    pub files: u64,
    /// Total size in bytes.
    pub bytes: u64,
}

/// One of the largest files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileInfo {
    /// Path (lossy UTF-8).
    pub path: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Last modified, seconds since the Unix epoch.
    pub modified: Option<u64>,
}

/// One of the largest folders (sizes include everything below them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderInfo {
    /// Path (lossy UTF-8).
    pub path: String,
    /// Total size of all files inside, in bytes.
    pub bytes: u64,
    /// Number of files inside.
    pub files: u64,
}

/// How much data falls in one age range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgeBucket {
    /// Human label, see [`AGE_LABELS`].
    pub label: &'static str,
    /// Number of files.
    pub files: u64,
    /// Total size in bytes.
    pub bytes: u64,
}

/// Usage of the partition the analysed folder lives on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskUsage {
    /// Partition size.
    pub total: u64,
    /// Bytes in use.
    pub used: u64,
    /// Bytes still available.
    pub available: u64,
}

/// Wasted space from duplicated content (filled in when duplicate detection is requested).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateSummary {
    /// Number of groups of identical files.
    pub groups: usize,
    /// Redundant copies across all groups.
    pub extra_copies: usize,
    /// Bytes freed by removing every redundant copy.
    pub reclaimable_bytes: u64,
}

impl DuplicateSummary {
    /// Summarises a duplicate-detection result.
    pub fn from_report(report: &DuplicateReport) -> Self {
        Self {
            groups: report.groups.len(),
            extra_copies: report.duplicate_count(),
            reclaimable_bytes: report.reclaimable_bytes(),
        }
    }
}

/// Everything learned about one folder.
#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    /// The analysed folder.
    pub root: String,
    /// Regular files found.
    pub files: u64,
    /// Sub-folders found (the root itself is not counted).
    pub folders: u64,
    /// Total size of all files (apparent size, not disk allocation).
    pub bytes: u64,
    /// Zero-byte files.
    pub empty_files: u64,
    /// Folders with no entries at all.
    pub empty_folders: u64,
    /// Symbolic links and unreadable folders that were not counted.
    pub skipped: u64,
    /// Partition usage, when it could be determined.
    pub disk: Option<DiskUsage>,
    /// Per-category totals, largest first.
    pub by_category: Vec<CategoryStat>,
    /// The largest files, largest first.
    pub largest_files: Vec<FileInfo>,
    /// The largest folders (cumulative), largest first.
    pub largest_folders: Vec<FolderInfo>,
    /// Data by modification age, newest bucket first ([`AGE_LABELS`]).
    pub age: Vec<AgeBucket>,
    /// Outermost code projects found (nested ones are part of their parent).
    pub project_count: u64,
    /// Total size of those projects.
    pub project_bytes: u64,
    /// The largest projects, largest first.
    pub largest_projects: Vec<FolderInfo>,
    /// Duplicate waste, if it was computed.
    pub duplicates: Option<DuplicateSummary>,
}

/// Keeps the `cap` entries with the largest keys without storing the rest.
struct TopN<T: Ord> {
    cap: usize,
    heap: BinaryHeap<Reverse<(u64, T)>>,
}

impl<T: Ord> TopN<T> {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            heap: BinaryHeap::new(),
        }
    }

    /// Whether an entry with `key` could be kept (lets callers skip building the value).
    /// Ties are accepted so the result never depends on directory iteration order.
    fn accepts(&self, key: u64) -> bool {
        self.cap > 0
            && (self.heap.len() < self.cap
                || self.heap.peek().is_some_and(|smallest| key >= smallest.0.0))
    }

    /// Adds an entry, then drops the smallest `(key, value)` if over capacity.
    fn push(&mut self, key: u64, value: T) {
        self.heap.push(Reverse((key, value)));
        if self.heap.len() > self.cap {
            self.heap.pop();
        }
    }

    fn into_desc(self) -> Vec<(u64, T)> {
        let mut items: Vec<_> = self.heap.into_iter().map(|Reverse(item)| item).collect();
        items.sort_by(|a, b| b.cmp(a));
        items
    }
}

#[derive(Default, Clone, Copy)]
struct Totals {
    bytes: u64,
    files: u64,
}

struct Walker<'a> {
    now: SystemTime,
    files: u64,
    folders: u64,
    empty_files: u64,
    empty_folders: u64,
    skipped: u64,
    by_category: HashMap<Category, (u64, u64)>,
    age: [(u64, u64); 5],
    largest_files: TopN<(String, Option<u64>)>,
    largest_folders: TopN<(String, u64)>,
    largest_projects: TopN<(String, u64)>,
    project_count: u64,
    project_bytes: u64,
    on_progress: &'a mut dyn FnMut(u64),
}

fn age_bucket(now: SystemTime, modified: Option<SystemTime>) -> usize {
    let Some(modified) = modified else { return 4 };
    let age = now
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match age {
        a if a < 30 * DAY => 0,
        a if a < 365 * DAY => 1,
        a if a < 3 * 365 * DAY => 2,
        _ => 3,
    }
}

impl Walker<'_> {
    fn walk(&mut self, dir: &Path, is_root: bool, in_project: bool) -> Totals {
        let entries: Vec<fs::DirEntry> = match fs::read_dir(dir) {
            Ok(read) => read.filter_map(|e| e.ok()).collect(),
            Err(_) => {
                self.skipped += 1;
                return Totals::default();
            }
        };
        let is_project = entries
            .iter()
            .any(|e| e.file_name().to_str().is_some_and(is_marker));
        let counts_as_project = is_project && !in_project;
        if !is_root && entries.is_empty() {
            self.empty_folders += 1;
        }

        let mut totals = Totals::default();
        for entry in &entries {
            // `DirEntry::metadata` does not follow symbolic links.
            let Ok(meta) = entry.metadata() else {
                self.skipped += 1;
                continue;
            };
            if meta.file_type().is_symlink() {
                self.skipped += 1;
            } else if meta.is_dir() {
                self.folders += 1;
                let child = self.walk(&entry.path(), false, in_project || is_project);
                totals.bytes += child.bytes;
                totals.files += child.files;
            } else if meta.is_file() {
                self.record_file(&entry.path(), &meta);
                totals.bytes += meta.len();
                totals.files += 1;
            }
        }

        if !is_root {
            self.offer_folder(dir, totals);
        }
        if counts_as_project {
            self.project_count += 1;
            self.project_bytes += totals.bytes;
            if self.largest_projects.accepts(totals.bytes) {
                self.largest_projects
                    .push(totals.bytes, (path_string(dir), totals.files));
            }
        }
        totals
    }

    fn record_file(&mut self, path: &Path, meta: &fs::Metadata) {
        let size = meta.len();
        self.files += 1;
        if size == 0 {
            self.empty_files += 1;
        }
        let entry = self
            .by_category
            .entry(Category::from_path(path))
            .or_default();
        entry.0 += 1;
        entry.1 += size;

        let modified = meta.modified().ok();
        let bucket = &mut self.age[age_bucket(self.now, modified)];
        bucket.0 += 1;
        bucket.1 += size;

        if self.largest_files.accepts(size) {
            let secs = modified
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            self.largest_files.push(size, (path_string(path), secs));
        }
        if self.files % PROGRESS_EVERY == 0 {
            (self.on_progress)(self.files);
        }
    }

    fn offer_folder(&mut self, dir: &Path, totals: Totals) {
        if self.largest_folders.accepts(totals.bytes) {
            self.largest_folders
                .push(totals.bytes, (path_string(dir), totals.files));
        }
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Walks `root` and summarises it. `on_progress(files_so_far)` is called periodically.
///
/// Symbolic links are never followed. Unreadable folders are counted in
/// [`Analysis::skipped`] rather than aborting the analysis.
pub fn analyze(
    root: &Path,
    options: &AnalyzeOptions,
    disks: &dyn DiskProbe,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Analysis> {
    if !root.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a folder",
            root.display()
        )));
    }
    let top = options.top;
    let mut walker = Walker {
        now: options.now,
        files: 0,
        folders: 0,
        empty_files: 0,
        empty_folders: 0,
        skipped: 0,
        by_category: HashMap::new(),
        age: [(0, 0); 5],
        largest_files: TopN::new(top),
        largest_folders: TopN::new(top),
        largest_projects: TopN::new(top),
        project_count: 0,
        project_bytes: 0,
        on_progress,
    };
    let totals = walker.walk(root, true, false);
    (walker.on_progress)(walker.files);

    let mut by_category: Vec<CategoryStat> = walker
        .by_category
        .into_iter()
        .map(|(category, (files, bytes))| CategoryStat {
            category: category.folder_name().to_string(),
            files,
            bytes,
        })
        .collect();
    by_category.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.category.cmp(&b.category))
    });

    let folder_infos = |top: TopN<(String, u64)>| -> Vec<FolderInfo> {
        top.into_desc()
            .into_iter()
            .map(|(bytes, (path, files))| FolderInfo { path, bytes, files })
            .collect()
    };

    Ok(Analysis {
        root: path_string(root),
        files: walker.files,
        folders: walker.folders,
        bytes: totals.bytes,
        empty_files: walker.empty_files,
        empty_folders: walker.empty_folders,
        skipped: walker.skipped,
        disk: disks.space(root).ok().map(|s| DiskUsage {
            total: s.total,
            used: s.used(),
            available: s.available,
        }),
        by_category,
        largest_files: walker
            .largest_files
            .into_desc()
            .into_iter()
            .map(|(bytes, (path, modified))| FileInfo {
                path,
                bytes,
                modified,
            })
            .collect(),
        largest_folders: folder_infos(walker.largest_folders),
        age: walker
            .age
            .iter()
            .zip(AGE_LABELS)
            .map(|(&(files, bytes), label)| AgeBucket {
                label,
                files,
                bytes,
            })
            .collect(),
        project_count: walker.project_count,
        project_bytes: walker.project_bytes,
        largest_projects: folder_infos(walker.largest_projects),
        duplicates: None,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::disk::{StaticDisks, SystemDisks};

    fn write(root: &Path, rel: &str, bytes: usize) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn age_file(root: &Path, rel: &str, now: SystemTime, days: u64) {
        let file = fs::File::options()
            .write(true)
            .open(root.join(rel))
            .unwrap();
        file.set_modified(now - Duration::from_secs(days * DAY))
            .unwrap();
    }

    fn run(root: &Path, top: usize) -> Analysis {
        let options = AnalyzeOptions {
            top,
            ..Default::default()
        };
        analyze(root, &options, &SystemDisks, &mut |_| {}).unwrap()
    }

    #[test]
    fn totals_and_categories() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.jpg", 100);
        write(dir.path(), "b.png", 50);
        write(dir.path(), "docs/c.pdf", 400);
        write(dir.path(), "docs/deep/d.txt", 10);
        write(dir.path(), "empty.txt", 0);
        let a = run(dir.path(), 10);

        assert_eq!((a.files, a.folders, a.bytes), (5, 2, 560));
        assert_eq!(a.empty_files, 1);
        let cats: Vec<_> = a
            .by_category
            .iter()
            .map(|c| (c.category.as_str(), c.files, c.bytes))
            .collect();
        assert_eq!(
            cats,
            [
                ("Documents", 1, 400),
                ("Images", 2, 150),
                ("Text Files", 2, 10)
            ]
        );
    }

    #[test]
    fn largest_files_are_ordered_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        for (name, size) in [
            ("s.bin", 1),
            ("m.bin", 50),
            ("l.bin", 500),
            ("xl.bin", 5000),
        ] {
            write(dir.path(), name, size);
        }
        let a = run(dir.path(), 2);
        let sizes: Vec<u64> = a.largest_files.iter().map(|f| f.bytes).collect();
        assert_eq!(sizes, [5000, 500]);
        assert!(a.largest_files[0].path.ends_with("xl.bin"));
        assert!(a.largest_files[0].modified.is_some());
    }

    #[test]
    fn folder_sizes_are_cumulative_and_exclude_the_root() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "top.bin", 1);
        write(dir.path(), "outer/a.bin", 100);
        write(dir.path(), "outer/inner/b.bin", 200);
        let a = run(dir.path(), 10);
        let by_name: HashMap<String, (u64, u64)> = a
            .largest_folders
            .iter()
            .map(|f| {
                let name = Path::new(&f.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (name, (f.bytes, f.files))
            })
            .collect();
        assert_eq!(by_name["outer"], (300, 2));
        assert_eq!(by_name["inner"], (200, 1));
        assert_eq!(by_name.len(), 2, "the root is not listed");
        assert_eq!(a.largest_folders[0].bytes, 300, "largest first");
    }

    #[test]
    fn projects_are_counted_once_even_when_nested() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "app/package.json", 10);
        write(dir.path(), "app/src/index.js", 90);
        write(dir.path(), "app/node_modules/dep/package.json", 900);
        write(dir.path(), "tool/Cargo.toml", 40);
        write(dir.path(), "photos/a.jpg", 5);
        let a = run(dir.path(), 10);
        assert_eq!(a.project_count, 2, "node_modules/dep belongs to app");
        assert_eq!(a.project_bytes, 1000 + 40);
        assert_eq!(a.largest_projects[0].bytes, 1000);
        assert!(a.largest_projects[0].path.ends_with("app"));
    }

    #[test]
    fn age_buckets_use_the_injected_clock() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        for (name, days) in [("new", 1), ("half", 200), ("mid", 500), ("old", 2000)] {
            write(dir.path(), name, 10);
            age_file(dir.path(), name, now, days);
        }
        let options = AnalyzeOptions { top: 5, now };
        let a = analyze(dir.path(), &options, &SystemDisks, &mut |_| {}).unwrap();
        let counts: Vec<u64> = a.age.iter().map(|b| b.files).collect();
        assert_eq!(counts, [1, 1, 1, 1, 0]);
        assert_eq!(a.age[0].label, AGE_LABELS[0]);
    }

    #[test]
    fn age_bucket_edges() {
        let now = SystemTime::now();
        let ago = |d: u64| Some(now - Duration::from_secs(d * DAY));
        assert_eq!(age_bucket(now, ago(29)), 0);
        assert_eq!(age_bucket(now, ago(30)), 1);
        assert_eq!(age_bucket(now, ago(364)), 1);
        assert_eq!(age_bucket(now, ago(365)), 2);
        assert_eq!(age_bucket(now, ago(3 * 365)), 3);
        assert_eq!(age_bucket(now, None), 4);
        assert_eq!(
            age_bucket(now, Some(now + Duration::from_secs(1000))),
            0,
            "files dated in the future count as new"
        );
    }

    #[test]
    fn empty_folders_are_counted_but_not_the_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hollow")).unwrap();
        write(dir.path().join("full").as_path(), "x.bin", 1);
        assert_eq!(run(dir.path(), 5).empty_folders, 1);
        let empty_root = tempfile::tempdir().unwrap();
        let a = run(empty_root.path(), 5);
        assert_eq!((a.files, a.empty_folders, a.bytes), (0, 0, 0));
        assert!(a.largest_files.is_empty() && a.by_category.is_empty());
    }

    #[test]
    fn top_zero_keeps_no_lists_but_still_totals() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "sub/a.bin", 7);
        let a = run(dir.path(), 0);
        assert_eq!(a.bytes, 7);
        assert!(a.largest_files.is_empty() && a.largest_folders.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real/a.bin", 100);
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let a = run(dir.path(), 5);
        assert_eq!((a.files, a.bytes, a.skipped), (1, 100, 1));
    }

    #[test]
    fn disk_usage_comes_from_the_probe() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.bin", 1);
        let disks = StaticDisks::new().with(dir.path(), 1000, 250, "v");
        let a = analyze(dir.path(), &AnalyzeOptions::default(), &disks, &mut |_| {}).unwrap();
        assert_eq!(
            a.disk,
            Some(DiskUsage {
                total: 1000,
                used: 750,
                available: 250
            })
        );
        let unknown = analyze(
            dir.path(),
            &AnalyzeOptions::default(),
            &StaticDisks::new(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(unknown.disk, None, "a failing probe is not fatal");
    }

    #[test]
    fn bad_roots_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "f.txt", 1);
        let opts = AnalyzeOptions::default();
        assert!(analyze(&dir.path().join("f.txt"), &opts, &SystemDisks, &mut |_| {}).is_err());
        assert!(analyze(&dir.path().join("nope"), &opts, &SystemDisks, &mut |_| {}).is_err());
    }

    #[test]
    fn progress_is_reported_and_ends_with_the_final_count() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..(PROGRESS_EVERY as usize + 50) {
            write(dir.path(), &format!("f{i}.bin"), 1);
        }
        let mut seen = Vec::new();
        analyze(
            dir.path(),
            &AnalyzeOptions::default(),
            &SystemDisks,
            &mut |n| seen.push(n),
        )
        .unwrap();
        assert!(seen.len() >= 2, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), PROGRESS_EVERY + 50);
    }

    #[test]
    fn duplicate_summary_reflects_the_report() {
        use crate::dedupe::DuplicateGroup;
        let report = DuplicateReport {
            groups: vec![
                DuplicateGroup {
                    size: 10,
                    hash: "h1".into(),
                    keeper: "k1".into(),
                    duplicates: vec!["a".into(), "b".into()],
                },
                DuplicateGroup {
                    size: 5,
                    hash: "h2".into(),
                    keeper: "k2".into(),
                    duplicates: vec!["c".into()],
                },
            ],
            unreadable: vec![],
        };
        assert_eq!(
            DuplicateSummary::from_report(&report),
            DuplicateSummary {
                groups: 2,
                extra_copies: 3,
                reclaimable_bytes: 25
            }
        );
        assert_eq!(
            DuplicateSummary::from_report(&DuplicateReport::default()).groups,
            0
        );
    }

    #[test]
    fn report_serializes_to_json() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.png", 3);
        let json = serde_json::to_value(run(dir.path(), 3)).unwrap();
        assert_eq!(json["files"], 1);
        assert_eq!(json["by_category"][0]["category"], "Images");
        assert!(json["duplicates"].is_null());
        assert_eq!(json["age"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn top_n_prefers_larger_keys_and_breaks_ties_deterministically() {
        let items = [(5u64, "a"), (9, "b"), (5, "c"), (1, "d"), (5, "0")];
        // every insertion order must give the same answer
        for rotation in 0..items.len() {
            let mut top = TopN::new(2);
            for &(key, name) in items.iter().cycle().skip(rotation).take(items.len()) {
                if top.accepts(key) {
                    top.push(key, name);
                }
            }
            assert_eq!(top.into_desc(), [(9, "b"), (5, "c")], "rotation {rotation}");
        }
    }
}
