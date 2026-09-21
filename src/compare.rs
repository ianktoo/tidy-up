//! Comparing the contents of several folders and planning what to do about the overlap.
//!
//! Folders are compared by file *content* (never by name). The first folder given is the
//! **primary**: when a file exists in several places, the copy in the earliest-listed
//! folder is kept, and a merge gathers everything into the primary.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    dedupe::{DuplicateGroup, build_dedupe_plan},
    error::{Error, Result},
    fsops::{resolve_root, unique_path},
    plan::{DUPLICATES_DIR, MoveKind, Plan, PlannedMove},
    rules::IgnoreRules,
    scan::{FileEntry, ScanOptions, scan},
};

/// What to do with duplicated content once it has been reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CompareAction {
    /// Report only; change nothing.
    Leave,
    /// Move extra copies into `<primary>/_Duplicates/` for review (undoable).
    Move,
    /// Gather every unique file into the primary folder and move extra copies aside (undoable).
    Merge,
    /// Permanently delete extra copies (cannot be undone).
    Delete,
}

/// Resolves and validates the folders to compare.
///
/// Every folder must exist, and no folder may be inside another (or listed twice), since
/// that would make files count against themselves.
pub fn validate_folders(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Err(Error::Invalid("give at least one folder".into()));
    }
    let mut resolved: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        let root = resolve_root(path)?;
        if let Some(other) = resolved
            .iter()
            .find(|o| root.starts_with(o) || o.starts_with(&root))
        {
            return Err(Error::Invalid(format!(
                "{} and {} overlap; pick folders that are not inside one another",
                root.display(),
                other.display()
            )));
        }
        resolved.push(root);
    }
    Ok(resolved)
}

/// Scans every folder and tags each file with the index of the folder it came from.
///
/// `on_folder(index, total)` is called before each folder is scanned.
pub fn scan_folders(
    folders: &[PathBuf],
    rules: &IgnoreRules,
    max_depth: usize,
    mut on_folder: impl FnMut(usize, usize),
) -> Result<Vec<FileEntry>> {
    let options = ScanOptions {
        max_depth,
        rules: rules.clone(),
        skip_root_dirs: [DUPLICATES_DIR.to_lowercase()].into(),
    };
    let mut files = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        on_folder(index, folders.len());
        let mut found = scan(folder, &options)?.files;
        found.iter_mut().for_each(|f| f.source = index);
        files.append(&mut found);
    }
    Ok(files)
}

/// How two folders' content relates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// Exactly the same set of contents.
    Identical,
    /// Everything in the first is also in the second (which has more).
    FirstInSecond,
    /// Everything in the second is also in the first (which has more).
    SecondInFirst,
    /// Some content in common, and each has content the other lacks.
    Overlapping {
        /// Number of distinct contents present in both.
        common: usize,
    },
    /// Nothing in common.
    Disjoint,
}

/// Per-folder numbers. `unique + shared + local_copies == files` (empty files count as unique).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderStats {
    /// The folder.
    pub path: PathBuf,
    /// Files scanned.
    pub files: usize,
    /// Total size of those files.
    pub bytes: u64,
    /// Files whose content exists nowhere else.
    pub unique: usize,
    /// Files whose content also exists in another compared folder.
    pub shared: usize,
    /// Files whose content is repeated only inside this same folder.
    pub local_copies: usize,
}

/// The full picture of how the compared folders relate.
#[derive(Debug, Clone)]
pub struct Comparison {
    /// One entry per folder, in the order given.
    pub folders: Vec<FolderStats>,
    /// Pairwise relations as `(i, j, relation)` with `i < j`.
    pub relations: Vec<(usize, usize, Relation)>,
    /// `true` when there are two or more folders and all hold exactly the same contents.
    pub all_identical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ContentId {
    Empty,
    Group(usize),
    Unique(PathBuf),
}

/// Builds a [`Comparison`] from scanned `files` (tagged by `source`) and the duplicate `groups`
/// found among them.
pub fn compare(folders: &[PathBuf], files: &[FileEntry], groups: &[DuplicateGroup]) -> Comparison {
    let mut group_of: HashMap<&Path, usize> = HashMap::new();
    for (i, group) in groups.iter().enumerate() {
        group_of.insert(group.keeper.as_path(), i);
        for dup in &group.duplicates {
            group_of.insert(dup.as_path(), i);
        }
    }
    let mut spans: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); groups.len()];
    for file in files {
        if let Some(&g) = group_of.get(file.path.as_path()) {
            spans[g].insert(file.source);
        }
    }

    let mut stats: Vec<FolderStats> = folders
        .iter()
        .map(|path| FolderStats {
            path: path.clone(),
            ..Default::default()
        })
        .collect();
    let mut sets: Vec<BTreeSet<ContentId>> = vec![BTreeSet::new(); folders.len()];

    for file in files {
        let stat = &mut stats[file.source];
        stat.files += 1;
        stat.bytes += file.size;
        let id = match group_of.get(file.path.as_path()) {
            _ if file.size == 0 => {
                stat.unique += 1;
                ContentId::Empty
            }
            Some(&g) => {
                if spans[g].len() > 1 {
                    stat.shared += 1;
                } else {
                    stat.local_copies += 1;
                }
                ContentId::Group(g)
            }
            None => {
                stat.unique += 1;
                ContentId::Unique(file.path.clone())
            }
        };
        sets[file.source].insert(id);
    }

    let mut relations = Vec::new();
    for i in 0..folders.len() {
        for j in i + 1..folders.len() {
            let (a, b) = (&sets[i], &sets[j]);
            let common = a.intersection(b).count();
            let relation = if a == b {
                Relation::Identical
            } else if a.is_subset(b) {
                Relation::FirstInSecond
            } else if b.is_subset(a) {
                Relation::SecondInFirst
            } else if common == 0 {
                Relation::Disjoint
            } else {
                Relation::Overlapping { common }
            };
            relations.push((i, j, relation));
        }
    }
    let all_identical = folders.len() > 1 && sets.windows(2).all(|w| w[0] == w[1]);
    Comparison {
        folders: stats,
        relations,
        all_identical,
    }
}

/// Plan for [`CompareAction::Merge`].
///
/// * Extra copies (in any folder, including the primary) move to `<primary>/_Duplicates/`.
/// * Every file that exists only outside the primary, and every kept copy that lives outside
///   it, moves into the primary at the same relative path (renamed `name (1).ext` on clashes).
///
/// After a merge the primary holds one copy of everything.
pub fn build_merge_plan(
    folders: &[PathBuf],
    files: &[FileEntry],
    groups: &[DuplicateGroup],
) -> Plan {
    let primary = &folders[0];
    let mut plan = build_dedupe_plan(primary, groups);

    let redundant: HashSet<&Path> = groups
        .iter()
        .flat_map(|g| g.duplicates.iter().map(PathBuf::as_path))
        .collect();
    let mut reserved = HashSet::new();
    for file in files.iter().filter(|f| f.source != 0) {
        if redundant.contains(file.path.as_path()) {
            continue;
        }
        let Ok(relative) = file.path.strip_prefix(&folders[file.source]) else {
            continue;
        };
        let to = unique_path(&primary.join(relative), &reserved);
        reserved.insert(to.clone());
        plan.moves.push(PlannedMove {
            from: file.path.clone(),
            to,
            kind: MoveKind::File,
            size: file.size,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::dedupe::{NoProgress, find_duplicates};

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    struct Setup {
        _dirs: Vec<tempfile::TempDir>,
        folders: Vec<PathBuf>,
        files: Vec<FileEntry>,
        groups: Vec<DuplicateGroup>,
    }

    /// One temp folder per entry; each entry is a list of `(relative path, content)`.
    fn setup(spec: &[&[(&str, &str)]]) -> Setup {
        let dirs: Vec<_> = spec.iter().map(|_| tempfile::tempdir().unwrap()).collect();
        for (dir, entries) in dirs.iter().zip(spec) {
            for (rel, content) in *entries {
                write(dir.path(), rel, content);
            }
        }
        let folders = validate_folders(
            &dirs
                .iter()
                .map(|d| d.path().to_path_buf())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let files = scan_folders(&folders, &IgnoreRules::new(), usize::MAX, |_, _| {}).unwrap();
        let groups = find_duplicates(&files, &NoProgress).groups;
        Setup {
            _dirs: dirs,
            folders,
            files,
            groups,
        }
    }

    #[test]
    fn validation_rejects_overlap_duplicates_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("inner")).unwrap();
        let outer = dir.path().to_path_buf();
        let inner = dir.path().join("inner");
        assert!(validate_folders(&[outer.clone(), inner.clone()]).is_err());
        assert!(validate_folders(&[inner, outer.clone()]).is_err());
        assert!(validate_folders(&[outer.clone(), outer]).is_err());
        assert!(validate_folders(&[PathBuf::from("/no/such/folder")]).is_err());
        assert!(validate_folders(&[]).is_err());
    }

    #[test]
    fn sibling_folders_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::create_dir_all(dir.path().join("b")).unwrap();
        let resolved = validate_folders(&[dir.path().join("a"), dir.path().join("b")]).unwrap();
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn scan_tags_files_with_their_folder_and_skips_duplicates_dir() {
        let s = setup(&[
            &[("x.txt", "1"), ("_Duplicates/g/old.txt", "old")],
            &[("y.txt", "2")],
        ]);
        assert_eq!(s.files.len(), 2);
        let y = s.files.iter().find(|f| f.path.ends_with("y.txt")).unwrap();
        assert_eq!(y.source, 1);
    }

    #[test]
    fn identical_folders_are_detected() {
        let both: &[(&str, &str)] = &[("a.txt", "AAAA"), ("sub/b.txt", "BBBB")];
        // same content, different names and layout
        let s = setup(&[
            both,
            &[("renamed.txt", "AAAA"), ("other/place.txt", "BBBB")],
            both,
        ]);
        let c = compare(&s.folders, &s.files, &s.groups);
        assert!(c.all_identical);
        assert!(
            c.relations
                .iter()
                .all(|(_, _, r)| *r == Relation::Identical)
        );
        assert_eq!(c.relations.len(), 3);
        assert!(c.folders.iter().all(|f| f.shared == 2 && f.unique == 0));
    }

    #[test]
    fn subset_overlap_and_disjoint_relations() {
        let s = setup(&[
            &[("x", "XXXX"), ("y", "YYYY")], // A
            &[("x", "XXXX")],                // B: subset of A
            &[("y", "YYYY"), ("z", "ZZZZ")], // C: overlaps A
            &[("q", "QQQQ")],                // D: disjoint from all
        ]);
        let c = compare(&s.folders, &s.files, &s.groups);
        let rel = |i, j| c.relations.iter().find(|r| (r.0, r.1) == (i, j)).unwrap().2;
        assert_eq!(rel(0, 1), Relation::SecondInFirst);
        assert_eq!(rel(0, 2), Relation::Overlapping { common: 1 });
        assert_eq!(rel(1, 2), Relation::Disjoint);
        assert_eq!(rel(0, 3), Relation::Disjoint);
        assert!(!c.all_identical);
    }

    #[test]
    fn stats_partition_files_into_unique_shared_and_local() {
        let s = setup(&[
            &[
                ("a", "SHARED"),
                ("b", "LOCAL!"),
                ("c", "LOCAL!"),
                ("d", "only-a"),
            ],
            &[("e", "SHARED")],
        ]);
        let c = compare(&s.folders, &s.files, &s.groups);
        let a = &c.folders[0];
        assert_eq!((a.files, a.shared, a.local_copies, a.unique), (4, 1, 2, 1));
        assert_eq!(a.files, a.shared + a.local_copies + a.unique);
        let b = &c.folders[1];
        assert_eq!((b.files, b.shared, b.unique), (1, 1, 0));
    }

    #[test]
    fn empty_folders_compare_as_identical() {
        let s = setup(&[&[], &[]]);
        assert!(compare(&s.folders, &s.files, &s.groups).all_identical);
    }

    #[test]
    fn keeper_prefers_the_earliest_listed_folder() {
        let s = setup(&[&[("deep/er/x.txt", "SAME")], &[("x.txt", "SAME")]]);
        assert_eq!(s.groups.len(), 1);
        // folder 0 wins even though its copy is deeper
        assert!(s.groups[0].keeper.starts_with(&s.folders[0]));
        assert!(s.groups[0].duplicates[0].starts_with(&s.folders[1]));
    }

    #[test]
    fn merge_gathers_unique_files_and_moves_copies_aside() {
        let s = setup(&[
            &[("keep.txt", "SAME"), ("only-a.txt", "AAAA")],
            &[("dup.txt", "SAME"), ("docs/only-b.txt", "BBBB")],
            &[
                ("docs/only-b.txt", "different-c"),
                ("copy-of-b.txt", "BBBB"),
            ],
        ]);
        let plan = build_merge_plan(&s.folders, &s.files, &s.groups);
        let a = &s.folders[0];
        let dest = |name: &str| {
            plan.moves
                .iter()
                .find(|m| m.from.ends_with(name))
                .map(|m| m.to.clone())
        };
        // extra copies go to _Duplicates
        assert!(dest("dup.txt").unwrap().starts_with(a.join("_Duplicates")));
        assert!(
            dest("copy-of-b.txt")
                .unwrap()
                .starts_with(a.join("_Duplicates"))
        );
        // the kept copy of the B/C-only content moves into the primary at its relative path
        assert_eq!(dest("only-b.txt"), Some(a.join("docs").join("only-b.txt")));
        // files already in the primary never move
        assert!(dest("keep.txt").is_none() && dest("only-a.txt").is_none());
        // the differently-contented docs/only-b.txt in C clashes and is renamed, not overwritten
        let landed: Vec<_> = plan
            .moves
            .iter()
            .filter(|m| !m.to.starts_with(a.join("_Duplicates")))
            .map(|m| m.to.clone())
            .collect();
        let unique: HashSet<_> = landed.iter().collect();
        assert_eq!(unique.len(), landed.len(), "destinations must be distinct");
        assert!(landed.iter().any(|p| p.ends_with("only-b (1).txt")));
    }

    #[test]
    fn merge_brings_in_a_keeper_that_is_not_in_the_primary() {
        let s = setup(&[&[("a.txt", "A")], &[("x.txt", "SAMEX"), ("y.txt", "SAMEX")]]);
        let plan = build_merge_plan(&s.folders, &s.files, &s.groups);
        let aside = s.folders[0].join("_Duplicates");
        let (extra, gathered): (Vec<_>, Vec<_>) =
            plan.moves.iter().partition(|m| m.to.starts_with(&aside));
        assert_eq!(
            gathered.len(),
            1,
            "one copy of the shared content is gathered"
        );
        assert_eq!(extra.len(), 1, "the other copy goes aside");
        assert_ne!(gathered[0].from, extra[0].from);
        assert!(gathered[0].to.starts_with(&s.folders[0]));
    }
}
