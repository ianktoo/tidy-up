//! Spreading files across several destinations (typically separate partitions).
//!
//! The job: some folders are too full, and you want to move data out of them into other
//! places, controlling *how much* moves, *how it is split*, and *how full* each destination
//! may become. A single destination gives a limited merge; several give a distribution.
//!
//! The work is split so the interesting part stays pure and testable:
//!
//! 1. [`collect_units`] turns the sources into **units**, the smallest things that are kept
//!    together (a top-level folder with everything in it, or a single file).
//! 2. [`allocate`] decides which unit goes to which destination, honouring capacity, the
//!    byte limit and the chosen [`Strategy`]. It performs no I/O.
//! 3. [`build_plan`] turns the allocation into a [`Plan`] that the normal executor runs,
//!    journals and can undo.

use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    category::Category,
    disk::{DiskProbe, DiskSpace},
    error::{Error, IoContext, Result},
    fsops::{resolve_maybe_new, resolve_root, unique_path},
    journal::TOOL_DIR,
    plan::{DUPLICATES_DIR, MoveKind, Plan, PlannedMove},
    rules::IgnoreRules,
    scan::{ScanOptions, SkipReason, Skipped, is_hidden, scan},
};

/// How the moved data is divided among the destinations.
#[derive(Debug, Clone, PartialEq)]
pub enum Strategy {
    /// Equal shares.
    Even,
    /// Shares proportional to each destination's usable free space.
    FreeSpace,
    /// Equalise how full the destinations end up (by percentage), filling the emptiest first.
    Fill,
    /// Explicit weights, one per destination, e.g. `[50.0, 30.0, 20.0]`.
    Ratio(Vec<f64>),
}

/// The strategies selectable by name on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StrategyKind {
    /// Equal shares.
    Even,
    /// Proportional to free space.
    Free,
    /// Equalise how full the destinations end up.
    Fill,
}

impl From<StrategyKind> for Strategy {
    fn from(kind: StrategyKind) -> Self {
        match kind {
            StrategyKind::Even => Strategy::Even,
            StrategyKind::Free => Strategy::FreeSpace,
            StrategyKind::Fill => Strategy::Fill,
        }
    }
}

/// Which units go first when not everything fits or a byte limit applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Prefer {
    /// Biggest units first: frees the most space with the fewest moves.
    Largest,
    /// Units not touched for the longest time first (good for archiving).
    Oldest,
}

/// Where files land inside a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Layout {
    /// Keep the original folder structure.
    Keep,
    /// Sort into category folders (Images, Documents, ...); implies file-level units.
    Organize,
}

/// The size of the things that are kept together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Granularity {
    /// Each top-level file or folder of a source is one unit, so a folder is never split up.
    Item,
    /// Every file is its own unit (folders may be split across destinations).
    File,
}

/// Constraints that protect the destinations (and cap the effort).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// A destination is never filled beyond this fraction of its partition, `(0, 1]`.
    pub max_fill: f64,
    /// A destination always keeps at least this many bytes free.
    pub min_free: u64,
    /// Move at most this many bytes in total.
    pub max_bytes: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_fill: 0.90,
            min_free: 0,
            max_bytes: None,
        }
    }
}

/// A place files can be moved to, with the state of its partition.
#[derive(Debug, Clone)]
pub struct Destination {
    /// The folder (may not exist yet).
    pub path: PathBuf,
    /// Its partition.
    pub space: DiskSpace,
}

/// One file inside a unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFile {
    /// Current location.
    pub from: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Where it goes, relative to the destination folder.
    pub dest_rel: PathBuf,
}

/// The smallest thing that is moved as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Display name (the top-level entry, or the relative path for a file).
    pub name: String,
    /// The files that make up the unit.
    pub files: Vec<UnitFile>,
    /// Total size in bytes.
    pub size: u64,
    /// The newest modification time of any file inside (`None` if unknown).
    pub modified: Option<SystemTime>,
}

/// The result of [`allocate`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allocation {
    /// For each destination, the indexes (into the unit list) assigned to it.
    pub per_dest: Vec<Vec<usize>>,
    /// Bytes assigned to each destination.
    pub bytes: Vec<u64>,
    /// Usable space each destination had after its reserve was set aside.
    pub capacity: Vec<u64>,
    /// Units that fit nowhere, given the capacity limits.
    pub unplaced: Vec<usize>,
    /// Units left out because the byte limit was reached.
    pub over_limit: Vec<usize>,
}

impl Allocation {
    /// Total bytes that will move.
    pub fn total_bytes(&self) -> u64 {
        self.bytes.iter().sum()
    }

    /// Total number of units that will move.
    pub fn unit_count(&self) -> usize {
        self.per_dest.iter().map(Vec::len).sum()
    }
}

/// Bytes a partition must keep free, given the limits.
fn reserve(space: &DiskSpace, limits: &Limits) -> u64 {
    let by_fill = ((1.0 - limits.max_fill) * space.total as f64).ceil() as u64;
    by_fill.max(limits.min_free)
}

/// Splits `total` bytes across destinations in proportion to `weights`, never exceeding a
/// destination's capacity; whatever a capped destination cannot take is shared by the rest.
fn weighted_targets(total: f64, weights: &[f64], capacity: &[u64]) -> Vec<f64> {
    let mut target = vec![0.0; weights.len()];
    let mut active: Vec<usize> = (0..weights.len())
        .filter(|&i| weights[i] > 0.0 && capacity[i] > 0)
        .collect();
    let mut remaining = total;
    while !active.is_empty() && remaining > 0.5 {
        let weight_sum: f64 = active.iter().map(|&i| weights[i]).sum();
        let saturated: Vec<usize> = active
            .iter()
            .copied()
            .filter(|&i| remaining * weights[i] / weight_sum >= capacity[i] as f64)
            .collect();
        if saturated.is_empty() {
            for &i in &active {
                target[i] = remaining * weights[i] / weight_sum;
            }
            break;
        }
        for i in saturated {
            target[i] = capacity[i] as f64;
            remaining -= capacity[i] as f64;
            active.retain(|&x| x != i);
        }
    }
    target
}

/// Finds the common fill level `L` such that filling every destination up to `L` of its
/// partition absorbs exactly `total` bytes (bisection), respecting capacities.
fn fill_targets(total: u64, dests: &[Destination], capacity: &[u64]) -> Vec<f64> {
    let give = |level: f64| -> Vec<f64> {
        dests
            .iter()
            .zip(capacity)
            .map(|(d, &cap)| {
                (level * d.space.total as f64 - d.space.used() as f64).clamp(0.0, cap as f64)
            })
            .collect()
    };
    let want = total as f64;
    if give(1.0).iter().sum::<f64>() <= want {
        return give(1.0);
    }
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..64 {
        let mid = (low + high) / 2.0;
        if give(mid).iter().sum::<f64>() < want {
            low = mid;
        } else {
            high = mid;
        }
    }
    give(high)
}

fn targets(total: u64, dests: &[Destination], capacity: &[u64], strategy: &Strategy) -> Vec<f64> {
    let weights: Vec<f64> = match strategy {
        Strategy::Fill => return fill_targets(total, dests, capacity),
        Strategy::Even => vec![1.0; dests.len()],
        Strategy::FreeSpace => capacity.iter().map(|&c| c as f64).collect(),
        Strategy::Ratio(weights) => weights.clone(),
    };
    weighted_targets(total as f64, &weights, capacity)
}

/// Decides which units go to which destination. Pure: performs no I/O.
///
/// 1. Each destination's usable capacity is its free space minus a reserve (the larger of
///    `min_free` and the space needed to stay under `max_fill`). Destinations on the same
///    partition share one pool of capacity.
/// 2. Units are picked in `prefer` order until `max_bytes` would be exceeded.
/// 3. A target share per destination is computed from the `strategy`, capped by capacity.
/// 4. Units are placed largest first, each on the destination furthest below its target that
///    can still hold it. Units that fit nowhere are reported in [`Allocation::unplaced`].
pub fn allocate(
    units: &[Unit],
    dests: &[Destination],
    strategy: &Strategy,
    limits: &Limits,
    prefer: Prefer,
) -> Result<Allocation> {
    let n = dests.len();
    if n == 0 {
        return Err(Error::Invalid("at least one destination is needed".into()));
    }
    if !(limits.max_fill > 0.0 && limits.max_fill <= 1.0) {
        return Err(Error::Invalid(
            "the maximum fill level must be between 1% and 100%".into(),
        ));
    }
    if let Strategy::Ratio(weights) = strategy {
        if weights.len() != n {
            return Err(Error::Invalid(format!(
                "the ratio has {} values but there are {n} destinations",
                weights.len()
            )));
        }
        if weights.iter().any(|w| !w.is_finite() || *w < 0.0) || weights.iter().sum::<f64>() <= 0.0
        {
            return Err(Error::Invalid(
                "ratio values must be zero or more, and not all zero".into(),
            ));
        }
    }

    let capacity: Vec<u64> = dests
        .iter()
        .map(|d| d.space.available.saturating_sub(reserve(&d.space, limits)))
        .collect();
    let mut pool: HashMap<&str, u64> = HashMap::new();
    for (dest, &cap) in dests.iter().zip(&capacity) {
        pool.entry(dest.space.volume.as_str())
            .and_modify(|p| *p = (*p).min(cap))
            .or_insert(cap);
    }

    let mut order: Vec<usize> = (0..units.len()).collect();
    match prefer {
        Prefer::Largest => {
            order.sort_by(|&a, &b| units[b].size.cmp(&units[a].size).then_with(|| a.cmp(&b)))
        }
        Prefer::Oldest => order.sort_by(|&a, &b| {
            units[a]
                .modified
                .cmp(&units[b].modified)
                .then_with(|| units[b].size.cmp(&units[a].size))
                .then_with(|| a.cmp(&b))
        }),
    }
    let mut selected = Vec::new();
    let mut over_limit = Vec::new();
    let mut total = 0u64;
    for index in order {
        let size = units[index].size;
        if limits
            .max_bytes
            .is_some_and(|max| total.saturating_add(size) > max)
        {
            over_limit.push(index);
        } else {
            total += size;
            selected.push(index);
        }
    }
    selected.sort_by(|&a, &b| units[b].size.cmp(&units[a].size).then_with(|| a.cmp(&b)));

    let target = targets(total, dests, &capacity, strategy);
    let eligible = |d: usize| !matches!(strategy, Strategy::Ratio(w) if w[d] == 0.0);
    let mut assigned = vec![0u64; n];
    let mut room = capacity.clone();
    let mut per_dest = vec![Vec::new(); n];
    let mut unplaced = Vec::new();

    for index in selected {
        let size = units[index].size;
        let best = (0..n)
            .filter(|&d| {
                eligible(d) && room[d] >= size && pool[dests[d].space.volume.as_str()] >= size
            })
            .max_by(|&a, &b| {
                let deficit_a = target[a] - assigned[a] as f64;
                let deficit_b = target[b] - assigned[b] as f64;
                deficit_a
                    .partial_cmp(&deficit_b)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| b.cmp(&a))
            });
        match best {
            Some(d) => {
                assigned[d] += size;
                room[d] -= size;
                if let Some(p) = pool.get_mut(dests[d].space.volume.as_str()) {
                    *p -= size;
                }
                per_dest[d].push(index);
            }
            None => unplaced.push(index),
        }
    }
    over_limit.sort_unstable();
    unplaced.sort_unstable();
    Ok(Allocation {
        per_dest,
        bytes: assigned,
        capacity,
        unplaced,
        over_limit,
    })
}

/// How full each destination's partition will be after the allocation is applied.
/// Destinations on the same partition report that partition's combined result.
pub fn projected_fill(dests: &[Destination], allocation: &Allocation) -> Vec<f64> {
    let mut added: HashMap<&str, u64> = HashMap::new();
    for (dest, bytes) in dests.iter().zip(&allocation.bytes) {
        *added.entry(dest.space.volume.as_str()).or_default() += bytes;
    }
    dests
        .iter()
        .map(|d| {
            let space = &d.space;
            if space.total == 0 {
                return 0.0;
            }
            let after = space.used() + added[space.volume.as_str()];
            (after as f64 / space.total as f64).min(1.0)
        })
        .collect()
}

/// Resolves and validates the folders involved. Sources must exist; destinations may be new.
/// No folder may be inside (or equal to) another, or files would be moved onto themselves.
pub fn validate_locations(
    sources: &[PathBuf],
    dests: &[PathBuf],
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    if sources.is_empty() || dests.is_empty() {
        return Err(Error::Invalid(
            "give at least one source (--from) and one destination (--to)".into(),
        ));
    }
    let sources: Vec<PathBuf> = sources
        .iter()
        .map(|p| resolve_root(p))
        .collect::<Result<_>>()?;
    let dests: Vec<PathBuf> = dests
        .iter()
        .map(|p| resolve_maybe_new(p))
        .collect::<Result<_>>()?;
    let all: Vec<&PathBuf> = sources.iter().chain(dests.iter()).collect();
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            if a.starts_with(b) || b.starts_with(a) {
                return Err(Error::Invalid(format!(
                    "{} and {} overlap (or are the same folder); sources and destinations must be separate",
                    a.display(),
                    b.display()
                )));
            }
        }
    }
    Ok((sources, dests))
}

/// Reads the partition state of each destination.
pub fn probe_destinations(dests: &[PathBuf], disks: &dyn DiskProbe) -> Result<Vec<Destination>> {
    dests
        .iter()
        .map(|path| {
            Ok(Destination {
                path: path.clone(),
                space: disks.space(path)?,
            })
        })
        .collect()
}

/// How units are formed from the sources.
pub struct CollectOptions<'a> {
    /// Where files land in the destination.
    pub layout: Layout,
    /// Unit size.
    pub granularity: Granularity,
    /// Entries to leave alone.
    pub rules: &'a IgnoreRules,
}

/// The units found, plus everything that was left alone and why.
#[derive(Debug, Default)]
pub struct Collected {
    /// Movable units.
    pub units: Vec<Unit>,
    /// Entries that were skipped.
    pub skipped: Vec<Skipped>,
}

fn skip(out: &mut Collected, path: &Path, reason: SkipReason) {
    out.skipped.push(Skipped {
        path: path.to_path_buf(),
        reason,
    });
}

/// Builds the units for every source.
///
/// With [`Granularity::Item`] and [`Layout::Keep`], each top-level entry is a unit and a folder
/// moves whole (hidden files and `.git` included, so projects survive intact). A folder that
/// contains a symbolic link or an unreadable entry is skipped entirely rather than half-moved.
/// With [`Granularity::File`], or [`Layout::Organize`], every file is a unit and code projects
/// are left alone.
pub fn collect_units(sources: &[PathBuf], options: &CollectOptions) -> Result<Collected> {
    let file_level = options.granularity == Granularity::File || options.layout == Layout::Organize;
    let mut out = Collected::default();
    for source in sources {
        if file_level {
            collect_files(source, options, &mut out)?;
        } else {
            collect_items(source, options, &mut out)?;
        }
    }
    Ok(out)
}

fn collect_files(source: &Path, options: &CollectOptions, out: &mut Collected) -> Result<()> {
    let scan_options = ScanOptions {
        max_depth: usize::MAX,
        rules: options.rules.clone(),
        skip_root_dirs: [DUPLICATES_DIR.to_lowercase()].into(),
    };
    let result = scan(source, &scan_options)?;
    out.skipped.extend(result.skipped);
    for project in result.projects {
        skip(out, &project, SkipReason::Project);
    }
    for file in result.files {
        let relative = file
            .path
            .strip_prefix(source)
            .unwrap_or(&file.path)
            .to_path_buf();
        let dest_rel = match options.layout {
            Layout::Keep => relative.clone(),
            Layout::Organize => PathBuf::from(Category::from_path(&file.path).folder_name())
                .join(file.path.file_name().unwrap_or_default()),
        };
        out.units.push(Unit {
            name: relative.display().to_string(),
            size: file.size,
            modified: file.modified,
            files: vec![UnitFile {
                from: file.path,
                size: file.size,
                dest_rel,
            }],
        });
    }
    Ok(())
}

fn collect_items(source: &Path, options: &CollectOptions, out: &mut Collected) -> Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(source)
        .at(source)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            skip(out, &path, SkipReason::UnsupportedName);
            continue;
        };
        let Ok(meta) = fs::symlink_metadata(&path) else {
            skip(out, &path, SkipReason::Unreadable);
            continue;
        };
        if meta.file_type().is_symlink() {
            skip(out, &path, SkipReason::Symlink);
        } else if name == TOOL_DIR {
            skip(out, &path, SkipReason::ToolFolder);
        } else if let Some(reason) = options.rules.check(&name, meta.is_dir()) {
            skip(out, &path, reason);
        } else if options.rules.skips_hidden() && is_hidden(&name, &meta) {
            skip(out, &path, SkipReason::Hidden);
        } else if meta.is_file() {
            out.units.push(Unit {
                name: name.clone(),
                size: meta.len(),
                modified: meta.modified().ok(),
                files: vec![UnitFile {
                    from: path,
                    size: meta.len(),
                    dest_rel: PathBuf::from(name),
                }],
            });
        } else if meta.is_dir() {
            let mut found = Vec::new();
            match collect_tree(&path, Path::new(&name), &mut found) {
                Err(reason) => skip(out, &path, reason),
                Ok(()) if found.is_empty() => {}
                Ok(()) => {
                    let size = found.iter().map(|(f, _)| f.size).sum();
                    let modified = found.iter().filter_map(|(_, m)| *m).max();
                    out.units.push(Unit {
                        name,
                        size,
                        modified,
                        files: found.into_iter().map(|(f, _)| f).collect(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Every file under `dir` (hidden ones included), or the reason the folder cannot be moved whole.
fn collect_tree(
    dir: &Path,
    relative: &Path,
    files: &mut Vec<(UnitFile, Option<SystemTime>)>,
) -> std::result::Result<(), SkipReason> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(|_| SkipReason::Unreadable)?
        .collect::<std::io::Result<_>>()
        .map_err(|_| SkipReason::Unreadable)?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(SkipReason::UnsupportedName);
        };
        let meta = entry.metadata().map_err(|_| SkipReason::Unreadable)?;
        let child = relative.join(&name);
        if meta.file_type().is_symlink() {
            return Err(SkipReason::Symlink);
        } else if meta.is_dir() {
            collect_tree(&entry.path(), &child, files)?;
        } else if meta.is_file() {
            files.push((
                UnitFile {
                    from: entry.path(),
                    size: meta.len(),
                    dest_rel: child,
                },
                meta.modified().ok(),
            ));
        }
    }
    Ok(())
}

/// Turns an allocation into a plan of per-file moves. `journal_root` is the folder that will
/// hold the undo journal (normally the first destination).
pub fn build_plan(
    journal_root: &Path,
    units: &[Unit],
    dests: &[Destination],
    allocation: &Allocation,
) -> Plan {
    let mut reserved = std::collections::HashSet::new();
    let mut moves = Vec::new();
    for (dest, indexes) in dests.iter().zip(&allocation.per_dest) {
        for &index in indexes {
            for file in &units[index].files {
                let to = unique_path(&dest.path.join(&file.dest_rel), &reserved);
                reserved.insert(to.clone());
                moves.push(PlannedMove {
                    from: file.from.clone(),
                    to,
                    kind: MoveKind::File,
                    size: file.size,
                });
            }
        }
    }
    Plan {
        root: journal_root.to_path_buf(),
        moves,
        skipped: Vec::new(),
    }
}

/// What a plan does to a source's partition.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceImpact {
    /// The source folder.
    pub path: PathBuf,
    /// Fill fraction of its partition before the move.
    pub before: f64,
    /// Fill fraction after (only data that leaves the partition frees space).
    pub after: f64,
    /// Bytes that leave the source's partition.
    pub freed: u64,
}

/// Predicts how much space each source's partition regains. Moves that stay on the same
/// partition (a rename) free nothing.
pub fn source_impact(
    sources: &[PathBuf],
    plan: &Plan,
    dests: &[Destination],
    disks: &dyn DiskProbe,
) -> Vec<SourceImpact> {
    sources
        .iter()
        .filter_map(|source| {
            let space = disks.space(source).ok()?;
            let freed: u64 = plan
                .moves
                .iter()
                .filter(|m| m.from.starts_with(source))
                .filter(|m| {
                    dests
                        .iter()
                        .find(|d| m.to.starts_with(&d.path))
                        .is_some_and(|d| d.space.volume != space.volume)
                })
                .map(|m| m.size)
                .sum();
            let used_after = space.used().saturating_sub(freed);
            Some(SourceImpact {
                path: source.clone(),
                before: space.used_fraction(),
                after: if space.total == 0 {
                    0.0
                } else {
                    used_after as f64 / space.total as f64
                },
                freed,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::disk::StaticDisks;

    const GIB: u64 = 1 << 30;

    fn unit(name: &str, size: u64) -> Unit {
        Unit {
            name: name.into(),
            size,
            modified: None,
            files: vec![UnitFile {
                from: PathBuf::from("/src").join(name),
                size,
                dest_rel: PathBuf::from(name),
            }],
        }
    }

    fn aged_unit(name: &str, size: u64, days_ago: u64) -> Unit {
        Unit {
            modified: Some(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000 - days_ago * 86_400),
            ),
            ..unit(name, size)
        }
    }

    fn dest(name: &str, total: u64, available: u64, volume: &str) -> Destination {
        Destination {
            path: PathBuf::from("/dest").join(name),
            space: DiskSpace {
                total,
                available,
                volume: volume.into(),
            },
        }
    }

    fn equal_units(count: usize, size: u64) -> Vec<Unit> {
        (0..count)
            .map(|i| unit(&format!("u{i:02}"), size))
            .collect()
    }

    fn no_limits() -> Limits {
        Limits {
            max_fill: 1.0,
            ..Default::default()
        }
    }

    fn counts(a: &Allocation) -> Vec<usize> {
        a.per_dest.iter().map(Vec::len).collect()
    }

    #[test]
    fn ratio_splits_proportionally() {
        let dests = [
            dest("a", 1000 * GIB, 900 * GIB, "a"),
            dest("b", 1000 * GIB, 900 * GIB, "b"),
            dest("c", 1000 * GIB, 900 * GIB, "c"),
        ];
        let alloc = allocate(
            &equal_units(10, GIB),
            &dests,
            &Strategy::Ratio(vec![50.0, 30.0, 20.0]),
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&alloc), [5, 3, 2]);
        assert_eq!(alloc.total_bytes(), 10 * GIB);
        assert!(alloc.unplaced.is_empty() && alloc.over_limit.is_empty());
    }

    #[test]
    fn even_splits_equally_and_zero_weight_gets_nothing() {
        let dests = [
            dest("a", 100 * GIB, 90 * GIB, "a"),
            dest("b", 100 * GIB, 90 * GIB, "b"),
        ];
        let even = allocate(
            &equal_units(8, GIB),
            &dests,
            &Strategy::Even,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&even), [4, 4]);

        let zero = allocate(
            &equal_units(8, GIB),
            &dests,
            &Strategy::Ratio(vec![1.0, 0.0]),
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&zero), [8, 0]);
    }

    #[test]
    fn free_space_strategy_follows_available_space() {
        let dests = [
            dest("small", 100 * GIB, 20 * GIB, "s"),
            dest("big", 100 * GIB, 60 * GIB, "b"),
        ];
        let alloc = allocate(
            &equal_units(8, GIB),
            &dests,
            &Strategy::FreeSpace,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        // 20:60 = 1:3
        assert_eq!(counts(&alloc), [2, 6]);
    }

    #[test]
    fn a_full_destination_is_capped_and_the_rest_take_the_overflow() {
        // "tiny" can only take 2 GiB no matter what the ratio says
        let dests = [
            dest("tiny", 100 * GIB, 2 * GIB, "t"),
            dest("roomy", 100 * GIB, 90 * GIB, "r"),
        ];
        let alloc = allocate(
            &equal_units(10, GIB),
            &dests,
            &Strategy::Ratio(vec![50.0, 50.0]),
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&alloc), [2, 8]);
        assert!(alloc.unplaced.is_empty());
    }

    #[test]
    fn fill_strategy_equalises_utilisation() {
        // A is 50% full, B is 10% full; moving 800 GiB should leave both at 70%.
        let dests = [
            dest("a", 1000 * GIB, 500 * GIB, "a"),
            dest("b", 1000 * GIB, 900 * GIB, "b"),
        ];
        let alloc = allocate(
            &equal_units(8, 100 * GIB),
            &dests,
            &Strategy::Fill,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&alloc), [2, 6]);
        let fill = projected_fill(&dests, &alloc);
        assert!(
            (fill[0] - 0.7).abs() < 0.001 && (fill[1] - 0.7).abs() < 0.001,
            "{fill:?}"
        );
    }

    #[test]
    fn fill_strategy_sends_everything_to_the_emptier_when_that_is_enough() {
        let dests = [
            dest("full", 1000 * GIB, 100 * GIB, "a"),
            dest("empty", 1000 * GIB, 900 * GIB, "b"),
        ];
        let alloc = allocate(
            &equal_units(4, 100 * GIB),
            &dests,
            &Strategy::Fill,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(counts(&alloc), [0, 4]);
    }

    #[test]
    fn max_fill_and_min_free_set_the_reserve() {
        // 100 GiB partition, 15 GiB free (85% full); max fill 90% leaves 5 GiB usable
        let d = dest("a", 100 * GIB, 15 * GIB, "a");
        let limits = Limits {
            max_fill: 0.9,
            ..Default::default()
        };
        let alloc = allocate(
            &equal_units(10, GIB),
            std::slice::from_ref(&d),
            &Strategy::Even,
            &limits,
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.capacity, [5 * GIB]);
        assert_eq!(alloc.unit_count(), 5);
        assert_eq!(alloc.unplaced.len(), 5);

        // min_free larger than the fill reserve wins
        let limits = Limits {
            max_fill: 0.9,
            min_free: 12 * GIB,
            ..Default::default()
        };
        let alloc = allocate(
            &equal_units(10, GIB),
            std::slice::from_ref(&d),
            &Strategy::Even,
            &limits,
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.capacity, [3 * GIB]);
    }

    #[test]
    fn a_destination_already_past_the_limit_takes_nothing() {
        let d = dest("a", 100 * GIB, 5 * GIB, "a"); // 95% full, limit 90%
        let alloc = allocate(
            &equal_units(3, GIB),
            &[d],
            &Strategy::Even,
            &Limits::default(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.capacity, [0]);
        assert_eq!(alloc.unit_count(), 0);
        assert_eq!(alloc.unplaced.len(), 3);
    }

    #[test]
    fn the_byte_limit_caps_the_total_and_preference_decides_what_goes() {
        let units = vec![
            aged_unit("old-small", 100, 900),
            aged_unit("new-big", 300, 1),
            aged_unit("mid", 200, 400),
        ];
        let dests = [dest("a", 10_000, 9_000, "a")];
        let limits = Limits {
            max_bytes: Some(350),
            max_fill: 1.0,
            ..Default::default()
        };

        let largest = allocate(&units, &dests, &Strategy::Even, &limits, Prefer::Largest).unwrap();
        assert_eq!(largest.per_dest[0], [1]); // new-big (300); 200 would exceed 350
        assert_eq!(largest.over_limit, [0, 2]);

        let oldest = allocate(&units, &dests, &Strategy::Even, &limits, Prefer::Oldest).unwrap();
        assert_eq!(oldest.bytes, [300]); // old-small (100) + mid (200)
        let mut moved = oldest.per_dest[0].clone();
        moved.sort_unstable();
        assert_eq!(moved, [0, 2]);
        assert_eq!(oldest.over_limit, [1]);
    }

    #[test]
    fn a_unit_bigger_than_every_destination_is_reported_unplaced() {
        let dests = [dest("a", 100, 50, "a"), dest("b", 100, 60, "b")];
        let units = vec![unit("huge", 500), unit("fits", 40)];
        let alloc = allocate(
            &units,
            &dests,
            &Strategy::Even,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.unplaced, [0]);
        assert_eq!(alloc.unit_count(), 1);
    }

    #[test]
    fn destinations_on_one_partition_share_its_free_space() {
        let dests = [dest("a", 100, 10, "same"), dest("b", 100, 10, "same")];
        let alloc = allocate(
            &equal_units(10, 2),
            &dests,
            &Strategy::Even,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.total_bytes(), 10, "the pool is 10 bytes, not 20");
        assert_eq!(alloc.unplaced.len(), 5);
        let fill = projected_fill(&dests, &alloc);
        assert_eq!(fill[0], fill[1]);
        assert_eq!(fill[0], 1.0);
    }

    #[test]
    fn invalid_input_is_rejected() {
        let d = dest("a", 100, 50, "a");
        let units = equal_units(1, 1);
        let go = |dests: &[Destination], strategy: &Strategy, limits: &Limits| {
            allocate(&units, dests, strategy, limits, Prefer::Largest)
        };
        assert!(go(&[], &Strategy::Even, &no_limits()).is_err());
        assert!(
            go(
                std::slice::from_ref(&d),
                &Strategy::Ratio(vec![1.0, 2.0]),
                &no_limits()
            )
            .is_err()
        );
        assert!(
            go(
                std::slice::from_ref(&d),
                &Strategy::Ratio(vec![-1.0]),
                &no_limits()
            )
            .is_err()
        );
        assert!(
            go(
                std::slice::from_ref(&d),
                &Strategy::Ratio(vec![0.0]),
                &no_limits()
            )
            .is_err()
        );
        assert!(
            go(
                std::slice::from_ref(&d),
                &Strategy::Ratio(vec![f64::NAN]),
                &no_limits()
            )
            .is_err()
        );
        for bad in [0.0, -0.5, 1.5] {
            let limits = Limits {
                max_fill: bad,
                ..Default::default()
            };
            assert!(
                go(std::slice::from_ref(&d), &Strategy::Even, &limits).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn allocation_is_deterministic() {
        let dests = [
            dest("a", 500 * GIB, 300 * GIB, "a"),
            dest("b", 500 * GIB, 400 * GIB, "b"),
        ];
        let mut units = equal_units(6, GIB);
        units.extend((0..4).map(|i| unit(&format!("odd{i}"), 3 * GIB)));
        let first = allocate(
            &units,
            &dests,
            &Strategy::Fill,
            &Limits::default(),
            Prefer::Largest,
        )
        .unwrap();
        let second = allocate(
            &units,
            &dests,
            &Strategy::Fill,
            &Limits::default(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn no_units_is_a_valid_empty_allocation() {
        let alloc = allocate(
            &[],
            &[dest("a", 10, 5, "a")],
            &Strategy::Even,
            &no_limits(),
            Prefer::Largest,
        )
        .unwrap();
        assert_eq!(alloc.unit_count(), 0);
        assert_eq!(alloc.total_bytes(), 0);
    }

    #[test]
    fn projected_fill_adds_the_assigned_bytes() {
        let dests = [dest("a", 1000, 800, "a")];
        let alloc = Allocation {
            bytes: vec![300],
            per_dest: vec![vec![]],
            ..Default::default()
        };
        assert_eq!(projected_fill(&dests, &alloc), [0.5]);
    }

    // ------------------------------------------------------------ filesystem-backed pieces

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn collect(sources: &[PathBuf], layout: Layout, granularity: Granularity) -> Collected {
        let rules = IgnoreRules::new();
        collect_units(
            sources,
            &CollectOptions {
                layout,
                granularity,
                rules: &rules,
            },
        )
        .unwrap()
    }

    fn unit_names(c: &Collected) -> Vec<String> {
        let mut names: Vec<String> = c.units.iter().map(|u| u.name.clone()).collect();
        names.sort();
        names
    }

    #[test]
    fn item_units_keep_folders_together_including_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "loose.txt", "abc");
        write(dir.path(), "Photos/a.jpg", "1234");
        write(dir.path(), "Photos/sub/b.jpg", "12");
        write(dir.path(), "repo/.git/HEAD", "ref");
        write(dir.path(), "repo/Cargo.toml", "[package]");
        write(dir.path(), ".hidden-top", "x");
        let c = collect(&[dir.path().to_path_buf()], Layout::Keep, Granularity::Item);

        assert_eq!(unit_names(&c), ["Photos", "loose.txt", "repo"]);
        let photos = c.units.iter().find(|u| u.name == "Photos").unwrap();
        assert_eq!(photos.size, 6);
        assert_eq!(photos.files.len(), 2);
        let mut rels: Vec<_> = photos.files.iter().map(|f| f.dest_rel.clone()).collect();
        rels.sort();
        assert_eq!(
            rels,
            [Path::new("Photos/a.jpg"), Path::new("Photos/sub/b.jpg")]
        );
        let repo = c.units.iter().find(|u| u.name == "repo").unwrap();
        assert!(
            repo.files.iter().any(|f| f.dest_rel.ends_with("HEAD")),
            ".git travels with the project"
        );
        assert!(c.skipped.iter().any(|s| s.reason == SkipReason::Hidden));
    }

    #[test]
    fn file_granularity_splits_folders_and_leaves_projects_alone() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Photos/a.jpg", "1");
        write(dir.path(), "Photos/b.jpg", "22");
        write(dir.path(), "repo/Cargo.toml", "x");
        let c = collect(&[dir.path().to_path_buf()], Layout::Keep, Granularity::File);
        assert_eq!(c.units.len(), 2);
        assert!(c.units.iter().all(|u| u.files.len() == 1));
        assert!(c.skipped.iter().any(|s| s.reason == SkipReason::Project));
        let a = c.units.iter().find(|u| u.name.ends_with("a.jpg")).unwrap();
        assert_eq!(a.files[0].dest_rel, Path::new("Photos/a.jpg"));
    }

    #[test]
    fn organize_layout_sorts_into_category_folders_at_file_level() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "trip/a.jpg", "1");
        write(dir.path(), "report.pdf", "2");
        write(dir.path(), "part.stl", "3");
        // even though item granularity was asked for, organize needs per-file units
        let c = collect(
            &[dir.path().to_path_buf()],
            Layout::Organize,
            Granularity::Item,
        );
        let mut rels: Vec<PathBuf> = c
            .units
            .iter()
            .map(|u| u.files[0].dest_rel.clone())
            .collect();
        rels.sort();
        assert_eq!(
            rels,
            [
                Path::new("3D Models/part.stl"),
                Path::new("Documents/report.pdf"),
                Path::new("Images/a.jpg")
            ]
        );
    }

    #[test]
    fn ignore_rules_apply_to_top_level_entries() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "keep.iso", "1");
        write(dir.path(), "move.txt", "2");
        write(dir.path(), "App.lnk", "3");
        let mut rules = IgnoreRules::new();
        rules.add_extension("iso");
        let c = collect_units(
            &[dir.path().to_path_buf()],
            &CollectOptions {
                layout: Layout::Keep,
                granularity: Granularity::Item,
                rules: &rules,
            },
        )
        .unwrap();
        assert_eq!(unit_names(&c), ["move.txt"]);
    }

    #[test]
    fn the_tool_folder_and_empty_folders_are_never_units() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".tidy-up/journals/x.jsonl", "x");
        fs::create_dir_all(dir.path().join("hollow/deeper")).unwrap();
        write(dir.path(), "real.txt", "x");
        let rules = IgnoreRules::new().include_hidden(true);
        let c = collect_units(
            &[dir.path().to_path_buf()],
            &CollectOptions {
                layout: Layout::Keep,
                granularity: Granularity::Item,
                rules: &rules,
            },
        )
        .unwrap();
        assert_eq!(unit_names(&c), ["real.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_containing_a_symlink_is_skipped_whole() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "album/a.jpg", "1");
        std::os::unix::fs::symlink(
            dir.path().join("album/a.jpg"),
            dir.path().join("album/link.jpg"),
        )
        .unwrap();
        write(dir.path(), "plain/b.jpg", "2");
        let c = collect(&[dir.path().to_path_buf()], Layout::Keep, Granularity::Item);
        assert_eq!(unit_names(&c), ["plain"]);
        assert!(c.skipped.iter().any(|s| s.reason == SkipReason::Symlink));
    }

    #[test]
    fn unit_modified_time_is_the_newest_file_inside() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "album/old.jpg", "1");
        write(dir.path(), "album/new.jpg", "2");
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(dir.path().join("album/old.jpg"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let c = collect(&[dir.path().to_path_buf()], Layout::Keep, Granularity::Item);
        assert!(c.units[0].modified.unwrap() > old);
    }

    #[test]
    fn several_sources_are_all_collected() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(a.path(), "one.txt", "1");
        write(b.path(), "two.txt", "22");
        let c = collect(
            &[a.path().to_path_buf(), b.path().to_path_buf()],
            Layout::Keep,
            Granularity::Item,
        );
        assert_eq!(unit_names(&c), ["one.txt", "two.txt"]);
    }

    #[test]
    fn locations_must_be_separate_and_sources_must_exist() {
        let base = tempfile::tempdir().unwrap();
        for name in ["src", "dst", "src/inner"] {
            fs::create_dir_all(base.path().join(name)).unwrap();
        }
        let p = |n: &str| base.path().join(n);
        assert!(validate_locations(&[p("src")], &[p("dst")]).is_ok());
        assert!(
            validate_locations(&[p("src")], &[p("src/inner")]).is_err(),
            "destination inside source"
        );
        assert!(
            validate_locations(&[p("src/inner")], &[p("src")]).is_err(),
            "source inside destination"
        );
        assert!(
            validate_locations(&[p("src")], &[p("src")]).is_err(),
            "same folder"
        );
        assert!(
            validate_locations(&[p("src")], &[p("dst"), p("dst")]).is_err(),
            "duplicate destination"
        );
        assert!(
            validate_locations(&[p("missing")], &[p("dst")]).is_err(),
            "source must exist"
        );
        assert!(validate_locations(&[], &[p("dst")]).is_err());
        assert!(validate_locations(&[p("src")], &[]).is_err());
        let (_, dests) = validate_locations(&[p("src")], &[p("brand/new")]).unwrap();
        assert!(
            dests[0].ends_with("new") && !dests[0].exists(),
            "new destinations are allowed"
        );
    }

    #[test]
    fn the_plan_places_files_under_their_destination_without_clashes() {
        let dests = [dest("a", 1000, 900, "a"), dest("b", 1000, 900, "b")];
        let mut units = vec![unit("x.txt", 1), unit("y.txt", 1)];
        // a second unit that wants the same relative name as the first
        units.push(Unit {
            files: vec![UnitFile {
                dest_rel: PathBuf::from("x.txt"),
                ..units[0].files[0].clone()
            }],
            name: "other-x.txt".into(),
            ..units[0].clone()
        });
        let alloc = Allocation {
            per_dest: vec![vec![0, 2], vec![1]],
            bytes: vec![2, 1],
            ..Default::default()
        };
        let plan = build_plan(Path::new("/dest/a"), &units, &dests, &alloc);
        assert_eq!(plan.root, Path::new("/dest/a"));
        let tos: Vec<_> = plan.moves.iter().map(|m| m.to.clone()).collect();
        assert_eq!(tos.len(), 3);
        assert!(tos.iter().all(|t| t.starts_with("/dest")));
        let distinct: std::collections::HashSet<_> = tos.iter().collect();
        assert_eq!(distinct.len(), 3, "clashes get a numbered name");
        assert!(tos[2].starts_with("/dest/b"));
    }

    #[test]
    fn source_impact_counts_only_data_that_leaves_the_partition() {
        let dests = [
            dest("far", 1000, 900, "other"),
            dest("near", 1000, 900, "src-vol"),
        ];
        let plan = Plan {
            root: PathBuf::from("/dest/far"),
            moves: vec![
                PlannedMove {
                    from: PathBuf::from("/src/a"),
                    to: PathBuf::from("/dest/far/a"),
                    kind: MoveKind::File,
                    size: 300,
                },
                PlannedMove {
                    from: PathBuf::from("/src/b"),
                    to: PathBuf::from("/dest/near/b"),
                    kind: MoveKind::File,
                    size: 200,
                },
            ],
            skipped: vec![],
        };
        let disks = StaticDisks::new().with("/src", 1000, 100, "src-vol");
        let impact = source_impact(&[PathBuf::from("/src")], &plan, &dests, &disks);
        assert_eq!(impact.len(), 1);
        assert_eq!(
            impact[0].freed, 300,
            "the move to the same partition frees nothing"
        );
        assert_eq!(impact[0].before, 0.9);
        assert!((impact[0].after - 0.6).abs() < 1e-9);
    }

    #[test]
    fn probing_destinations_reads_each_partition() {
        let disks = StaticDisks::new()
            .with("/d1", 100, 40, "v1")
            .with("/d2", 200, 90, "v2");
        let probed =
            probe_destinations(&[PathBuf::from("/d1/x"), PathBuf::from("/d2")], &disks).unwrap();
        assert_eq!(probed[0].space.volume, "v1");
        assert_eq!(probed[1].space.available, 90);
        assert!(probe_destinations(&[PathBuf::from("/unknown")], &disks).is_err());
    }
}
